use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use windows::core::GUID;

use crate::document::BackupDocument;
use crate::error::{Error, Result};
use crate::guid::Guid;
use crate::options::{Consistency, SnapshotOptions};
use crate::sys;
use crate::sys::com::Apartment;
use crate::sys::vss::{find_component, to_guid, Backup, Selected};
use crate::volume::{normalize_volume_name, system_time_from_filetime, SplitPath, VolumeSnapshot};
use crate::writer::{check_writers, ComponentId, WriterStatus};

/// VSS creates one snapshot set at a time per machine. Creations in this process take
/// turns, so they don't fail each other with `SnapshotInProgress`.
static CREATING: Mutex<()> = Mutex::new(());

/// Snapshots of one or more volumes, all taken at the same moment.
///
/// Read files through [`path_in_snapshot`](SnapshotSet::path_in_snapshot) or each volume's
/// [`device_path`](VolumeSnapshot::device_path). [`close`](SnapshotSet::close) removes the
/// snapshots and reports any failure; dropping the set removes them too, ignoring failures.
/// They're non-persistent, so if the process dies, VSS removes them itself.
///
/// Every method can be called from any thread, whatever its COM apartment. The set keeps its
/// COM objects on a private thread of its own.
///
/// ```no_run
/// use std::path::Path;
/// use vss_snapshot::{SnapshotOptions, SnapshotSet};
///
/// # fn main() -> vss_snapshot::Result<()> {
/// let snapshots = SnapshotSet::create(["C:"], &SnapshotOptions::new())?;
/// for writer in snapshots.writers().iter().filter(|w| w.is_failed()) {
///     eprintln!("{} wasn't ready: {}", writer.name, writer.state);
/// }
/// if let Some(path) = snapshots.path_in_snapshot(Path::new(r"C:\data\report.xlsx")) {
///     let bytes = std::fs::read(path)?;
/// #   let _ = bytes;
/// }
/// snapshots.close()?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct SnapshotSet {
    id: Guid,
    volumes: Vec<VolumeSnapshot>,
    writers: Vec<WriterStatus>,
    has_components: bool,
    worker: Option<Worker>,
}

/// The private thread that holds the set's COM objects.
#[derive(Debug)]
struct Worker {
    end: Sender<Ending>,
    thread: JoinHandle<Result<Option<BackupDocument>>>,
}

/// How the set ends. If the sender is dropped without sending, it closes.
#[derive(Debug)]
enum Ending {
    /// The backup is stored: tell the selected components' writers it succeeded.
    Complete,
    Close,
}

/// A volume to snapshot, resolved before VSS is involved.
struct Target {
    /// The name the caller used, for errors.
    given: String,
    /// That name normalised, such as `C:\`.
    name: String,
    guid_path: String,
    mount_points: Vec<PathBuf>,
}

/// What the worker reports once the snapshots exist.
struct Created {
    id: Guid,
    volumes: Vec<VolumeSnapshot>,
    writers: Vec<WriterStatus>,
}

impl SnapshotSet {
    /// Takes a snapshot of each volume, all at the same moment.
    ///
    /// A volume is named by drive (`C`, `C:` or `C:\`), by GUID path (`\\?\Volume{...}\`),
    /// or by a folder it's mounted on (`C:\mnt\data`). Naming one volume more than once
    /// takes one snapshot of it.
    ///
    /// Needs an elevated process, SYSTEM, or a member of Backup Operators. Blocks until the
    /// snapshots exist, which with writers can take several seconds. Volume names and
    /// options are checked before VSS is involved.
    ///
    /// With [`SnapshotOptions::with_components`], fails with [`Error::ComponentNotFound`]
    /// if a component isn't on this machine, and with [`Error::WriterFailed`] if one of
    /// their writers failed, which usually means a volume holding their files was left out.
    ///
    /// For an incremental backup that uses the USN journal, read each volume's journal
    /// position before calling this, so every change you're told about is in the snapshot.
    pub fn create<I, S>(volumes: I, options: &SnapshotOptions) -> Result<SnapshotSet>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        options.validate()?;
        let mut names = Vec::new();
        for given in volumes {
            let given = given.as_ref();
            names.push((given.to_owned(), normalize_volume_name(given)?));
        }
        if names.is_empty() {
            return Err(Error::NoVolumes);
        }
        if cfg!(target_pointer_width = "32") && sys::process::is_wow64()? {
            return Err(Error::Wow64NotSupported);
        }
        let targets = resolve(names)?;

        let _creating = CREATING.lock().unwrap_or_else(PoisonError::into_inner);
        let (created_tx, created_rx) = channel();
        let (end_tx, end_rx) = channel();
        let has_components = !options.components().is_empty();
        let options = options.clone();
        let thread = thread::Builder::new()
            .name("vss-snapshot".to_owned())
            .spawn(move || run(&targets, &options, &created_tx, &end_rx))?;

        match created_rx.recv() {
            Ok(Ok(created)) => Ok(SnapshotSet {
                id: created.id,
                volumes: created.volumes,
                writers: created.writers,
                has_components,
                worker: Some(Worker {
                    end: end_tx,
                    thread,
                }),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::Internal {
                    reason: "the snapshot thread stopped while creating the snapshots",
                })
            }
        }
    }

    /// The snapshot set's VSS id.
    pub fn id(&self) -> Guid {
        self.id
    }

    /// One entry for each volume, in the order they were first named.
    pub fn volumes(&self) -> &[VolumeSnapshot] {
        &self.volumes
    }

    /// The entry for a volume, by any name [`create`](SnapshotSet::create) accepts for it:
    /// its drive, a folder it's mounted on, or its GUID path.
    pub fn volume(&self, name: &str) -> Option<&VolumeSnapshot> {
        self.volumes.iter().find(|volume| volume.is_named(name))
    }

    /// Turns a path on a live volume into the same path inside its snapshot, as
    /// `C:\docs\a.txt` into `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy7\docs\a.txt`.
    ///
    /// Picks the volume whose mount point matches the most of the path, so a path through a
    /// folder that another volume in the set is mounted on maps into that volume's snapshot.
    /// Returns `None` if no volume in the set holds the path, or the path is relative or
    /// contains `..`. See [`VolumeSnapshot::path_in_snapshot`].
    pub fn path_in_snapshot(&self, path: &Path) -> Option<PathBuf> {
        let path = SplitPath::parse(path)?;
        let (volume, matched) = self
            .volumes
            .iter()
            .filter_map(|volume| volume.longest_match(&path).map(|m| (volume, m)))
            .max_by_key(|(_, matched)| *matched)?;
        Some(volume.map(&path, matched))
    }

    /// What each writer reported once the snapshots were taken. Empty for
    /// [`Consistency::Crash`].
    ///
    /// A failed writer with no selected component doesn't stop the snapshot being taken.
    /// Its files are only crash-consistent, so check [`WriterStatus::is_failed`] and decide
    /// what to do.
    pub fn writers(&self) -> &[WriterStatus] {
        &self.writers
    }

    /// Removes the snapshots, and reports any failure, which dropping the set can't. Paths
    /// inside the snapshots stop working. If components were selected, their writers are
    /// told the backup didn't succeed.
    ///
    /// Every cleanup step is attempted even if an earlier one fails. If removing the
    /// snapshots itself fails, VSS removes them when the process exits.
    pub fn close(mut self) -> Result<()> {
        self.end(Ending::Close).map(|_| ())
    }

    /// Tells the selected components' writers that the backup succeeded, removes the
    /// snapshots, and returns the document a restore through the writers needs. Call this
    /// only once the backup is safely stored, and store the document with it.
    ///
    /// Fails with [`Error::WriterFailed`] if one of those writers has failed since the
    /// snapshot was taken, so a backup is never recorded as good when it isn't. Fails with
    /// [`Error::InvalidOptions`] if no components were selected; use
    /// [`close`](SnapshotSet::close) for those. Either way, the snapshots are removed.
    pub fn complete(mut self) -> Result<BackupDocument> {
        if !self.has_components {
            return Err(Error::InvalidOptions {
                reason: "complete needs selected components; use close instead",
            });
        }
        self.end(Ending::Complete)?.ok_or(Error::Internal {
            reason: "the snapshot thread completed without a document",
        })
    }

    fn end(&mut self, ending: Ending) -> Result<Option<BackupDocument>> {
        let Some(worker) = self.worker.take() else {
            return Ok(None);
        };
        let _ = worker.end.send(ending);
        worker.thread.join().unwrap_or(Err(Error::Internal {
            reason: "the snapshot thread panicked",
        }))
    }
}

impl Drop for SnapshotSet {
    /// Removes the snapshots, ignoring failures. Blocks until done.
    fn drop(&mut self) {
        let _ = self.end(Ending::Close);
    }
}

/// Finds each named volume's GUID path and mount points, and drops repeats of a volume.
fn resolve(names: Vec<(String, String)>) -> Result<Vec<Target>> {
    let mut targets: Vec<Target> = Vec::with_capacity(names.len());
    for (given, name) in names {
        let guid_path =
            sys::volume::guid_path_of(&name).map_err(|_| Error::VolumeNotSupported {
                name: given.clone(),
            })?;
        if targets
            .iter()
            .any(|target| target.guid_path.eq_ignore_ascii_case(&guid_path))
        {
            continue;
        }
        let mount_points = sys::volume::mount_points(&guid_path)?;
        targets.push(Target {
            given,
            name,
            guid_path,
            mount_points,
        });
    }
    Ok(targets)
}

/// The worker thread: creates the snapshots, reports them, waits to be told how to end,
/// then cleans up. Owns every COM object for the life of the set.
fn run(
    targets: &[Target],
    options: &SnapshotOptions,
    created: &Sender<Result<Created>>,
    end: &Receiver<Ending>,
) -> Result<Option<BackupDocument>> {
    let session = match Session::start(targets, options) {
        Ok((session, info)) => {
            let _ = created.send(Ok(info));
            session
        }
        Err(error) => {
            let _ = created.send(Err(error));
            return Ok(None);
        }
    };
    // A closed channel means the set was dropped, which also closes it.
    let succeeded = matches!(end.recv(), Ok(Ending::Complete));
    session.finish(succeeded)
}

/// One snapshot set's VSS session. Fields drop in order, so the backup components object is
/// released before the thread leaves the COM apartment.
struct Session {
    backup: Backup,
    set: GUID,
    consistency: Consistency,
    selected: Vec<Selected>,
    _apartment: Apartment,
}

impl Session {
    fn start(targets: &[Target], options: &SnapshotOptions) -> Result<(Session, Created)> {
        let apartment = Apartment::enter()?;
        let backup = Backup::create(options.timeout())?;
        backup.initialize_for_backup()?;
        backup.set_context(options.consistency())?;
        backup.set_copy_backup(!options.components().is_empty())?;
        let with_writers = options.consistency() == Consistency::Application;
        let mut selected = Vec::new();
        if with_writers {
            backup.gather_writer_metadata()?;
            let added = add_components(&backup, options.components());
            let freed = backup.free_writer_metadata();
            selected = added?;
            freed?;
        }

        let set = backup.start_snapshot_set()?;
        match take_snapshots(&backup, targets, with_writers, &selected) {
            Ok((volumes, writers)) => {
                let created = Created {
                    id: to_guid(set),
                    volumes,
                    writers,
                };
                let session = Session {
                    backup,
                    set,
                    consistency: options.consistency(),
                    selected,
                    _apartment: apartment,
                };
                Ok((session, created))
            }
            Err(error) => {
                // Releasing the backup after this removes anything already created.
                backup.abort_backup();
                Err(error)
            }
        }
    }

    /// Ends the backup and removes the snapshots, attempting every step and returning the
    /// first failure.
    ///
    /// If the backup `succeeded`, the selected writers are checked again and told so, and
    /// the document is saved after `BackupComplete`, so that it includes what writers add
    /// then. Otherwise they're told nothing, which records no backup.
    fn finish(self, succeeded: bool) -> Result<Option<BackupDocument>> {
        let marked = if succeeded {
            self.check_selected_writers().and_then(|_| {
                self.selected
                    .iter()
                    .try_for_each(|component| self.backup.set_backup_succeeded(component))
            })
        } else {
            Ok(())
        };
        let completed = match self.consistency {
            Consistency::Application => self.backup.backup_complete(),
            Consistency::Crash => Ok(()),
        };
        let document = match (&marked, &completed) {
            (Ok(()), Ok(())) if succeeded => self
                .backup
                .save_as_xml()
                .map(|xml| Some(BackupDocument::from_xml(xml))),
            _ => Ok(None),
        };
        let deleted = self.backup.delete_snapshot_set(self.set);
        marked.and(completed).and(deleted)?;
        document
    }

    fn check_selected_writers(&self) -> Result<Vec<WriterStatus>> {
        let statuses = self.backup.writer_statuses()?;
        check_writers(&statuses, &selected_writers(&self.selected))
    }
}

/// Adds each wanted component, once, to the backup. Call between gathering and freeing the
/// writer metadata.
fn add_components(backup: &Backup, wanted: &[ComponentId]) -> Result<Vec<Selected>> {
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let writers = backup.gathered_writers()?;
    let mut selected: Vec<Selected> = Vec::with_capacity(wanted.len());
    for id in wanted {
        if selected.iter().any(|component| component.id == *id) {
            continue;
        }
        let component = find_component(&writers, id).ok_or_else(|| Error::ComponentNotFound {
            component: id.clone(),
        })?;
        backup.add_component(&component)?;
        selected.push(component);
    }
    Ok(selected)
}

/// The writers of the selected components, each once.
pub(crate) fn selected_writers(selected: &[Selected]) -> Vec<Guid> {
    let mut writers = Vec::new();
    for component in selected {
        if !writers.contains(&component.id.writer_id) {
            writers.push(component.id.writer_id);
        }
    }
    writers
}

fn take_snapshots(
    backup: &Backup,
    targets: &[Target],
    with_writers: bool,
    selected: &[Selected],
) -> Result<(Vec<VolumeSnapshot>, Vec<WriterStatus>)> {
    let mut ids = Vec::with_capacity(targets.len());
    for target in targets {
        ids.push(backup.add_to_snapshot_set(&target.guid_path, &target.given)?);
    }
    if with_writers {
        backup.prepare_for_backup()?;
    }
    backup.do_snapshot_set()?;

    let writers = if with_writers {
        backup.writer_statuses()?
    } else {
        Vec::new()
    };
    // A selected component's files aren't consistent if its writer failed, though the
    // snapshot was still taken, so this has to be checked.
    check_writers(&writers, &selected_writers(selected))?;

    let mut volumes = Vec::with_capacity(targets.len());
    for (target, id) in targets.iter().zip(ids) {
        let properties = backup.snapshot_properties(id)?;
        volumes.push(VolumeSnapshot::new(
            to_guid(id),
            target.name.clone(),
            target.guid_path.clone(),
            target.mount_points.clone(),
            &properties.device,
            system_time_from_filetime(properties.created),
        ));
    }
    Ok((volumes, writers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_snapshot_set_can_move_between_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SnapshotSet>();
    }

    #[test]
    fn each_selected_writer_is_listed_once() {
        let sql = Guid::from_u128(1);
        let other = Guid::from_u128(2);
        let selected = [
            Selected::new(ComponentId::new(sql, "", "a"), GUID::zeroed(), 2),
            Selected::new(ComponentId::new(other, "", "b"), GUID::zeroed(), 2),
            Selected::new(ComponentId::new(sql, "", "c"), GUID::zeroed(), 2),
        ];
        assert_eq!(selected_writers(&selected), [sql, other]);
    }

    // Each of these fails before VSS is involved, so none takes a snapshot.

    #[test]
    fn needs_at_least_one_volume() {
        let result = SnapshotSet::create(Vec::<String>::new(), &SnapshotOptions::new());
        assert!(matches!(result, Err(Error::NoVolumes)));
    }

    #[test]
    fn a_bad_name_is_refused_even_after_good_ones() {
        let result = SnapshotSet::create(["C:", "not a volume"], &SnapshotOptions::new());
        match result {
            Err(Error::InvalidVolume { name }) => assert_eq!(name, "not a volume"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bad_options_are_refused() {
        let zero = SnapshotOptions::new().with_timeout(Duration::ZERO);
        assert!(matches!(
            SnapshotSet::create(["C:"], &zero),
            Err(Error::InvalidOptions { .. })
        ));
        let crash_with_components = SnapshotOptions::new()
            .with_consistency(Consistency::Crash)
            .with_components([ComponentId::new(Guid::from_u128(1), "", "x")]);
        assert!(matches!(
            SnapshotSet::create(["C:"], &crash_with_components),
            Err(Error::InvalidOptions { .. })
        ));
    }

    #[test]
    fn a_missing_drive_is_not_supported() {
        let Some(letter) = ('D'..='Z')
            .rev()
            .find(|letter| !Path::new(&format!(r"{letter}:\")).exists())
        else {
            return;
        };
        let missing = format!("{letter}:");
        match SnapshotSet::create([missing.as_str()], &SnapshotOptions::new()) {
            Err(Error::VolumeNotSupported { name }) => assert_eq!(name, missing),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_folder_that_is_not_a_mount_point_is_not_supported() {
        let folder = std::env::temp_dir();
        let folder = folder.to_str().unwrap();
        assert!(matches!(
            SnapshotSet::create([folder], &SnapshotOptions::new()),
            Err(Error::VolumeNotSupported { .. })
        ));
    }
}
