use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::document::BackupDocument;
use crate::error::{Error, Result};
use crate::options::RestoreOptions;
use crate::snapshot::selected_writers;
use crate::sys;
use crate::sys::com::Apartment;
use crate::sys::vss::{Backup, Selected};
use crate::writer::{check_writers, ComponentId, WriterStatus};

/// A restore through VSS writers, from the [`BackupDocument`] a backup of selected
/// components produced.
///
/// [`begin`](Restore::begin) lets the writers prepare. Copy the components' stored files back
/// to where they were, then call [`finish`](Restore::finish), which lets the writers bring
/// them back into service. For SQL Server, that's when the database comes online. If the
/// files can't be copied, call [`abort`](Restore::abort). Dropping an unfinished restore
/// aborts it too.
///
/// **`begin` changes the live system before any file is copied.** SQL Server takes a
/// database that's online out of service at that point. Check the backup is readable first.
/// Aborting before copying anything leaves the application as it was; after a partial copy,
/// it may be unusable until a restore succeeds. If the process is killed between `begin` and
/// `finish`, the application stays in its restoring state, as nothing tells the writer the
/// process has gone. Running the restore again fixes it, so record that a restore has started
/// before calling `begin`.
///
/// Tested with SQL Server Express only. Other writers are untested.
///
/// ```no_run
/// use vss_snapshot::{BackupDocument, ComponentId, Guid, Restore, RestoreOptions};
///
/// # fn main() -> vss_snapshot::Result<()> {
/// # let stored_xml = String::new();
/// # let component = ComponentId::new(Guid::default(), "", "sales");
/// // The document and component id, stored with the backup.
/// let document = BackupDocument::from_xml(stored_xml);
/// let restore = Restore::begin(&document, &[component], &RestoreOptions::new())?;
/// // Copy the component's stored files back to where they were.
/// restore.finish()?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Restore {
    writers: Vec<WriterStatus>,
    worker: Option<Worker>,
}

/// The private thread that holds the restore's COM objects.
#[derive(Debug)]
struct Worker {
    /// Sending tells the writers the files are in place. Dropping it without sending tells
    /// them the restore failed.
    restored: Sender<()>,
    thread: JoinHandle<Result<Vec<WriterStatus>>>,
}

impl Restore {
    /// Selects the components from the document and lets their writers prepare.
    ///
    /// Fails with [`Error::ComponentNotFound`] if a component isn't in the document as backed
    /// up successfully, and with [`Error::WriterFailed`] if one of their writers failed to
    /// prepare. Needs an elevated process, SYSTEM, or a member of Backup Operators.
    pub fn begin(
        document: &BackupDocument,
        components: &[ComponentId],
        options: &RestoreOptions,
    ) -> Result<Restore> {
        options.validate()?;
        if components.is_empty() {
            return Err(Error::InvalidOptions {
                reason: "a restore needs at least one component",
            });
        }
        if cfg!(target_pointer_width = "32") && sys::process::is_wow64()? {
            return Err(Error::Wow64NotSupported);
        }
        let document = document.as_xml().to_owned();
        let mut wanted: Vec<ComponentId> = Vec::with_capacity(components.len());
        for component in components {
            if !wanted.contains(component) {
                wanted.push(component.clone());
            }
        }
        let timeout = options.timeout();
        let (ready_tx, ready_rx) = channel();
        let (restored_tx, restored_rx) = channel();
        let thread = thread::Builder::new()
            .name("vss-restore".to_owned())
            .spawn(move || run(&document, &wanted, timeout, &ready_tx, &restored_rx))?;

        match ready_rx.recv() {
            Ok(Ok(writers)) => Ok(Restore {
                writers,
                worker: Some(Worker {
                    restored: restored_tx,
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
                    reason: "the restore thread stopped while preparing",
                })
            }
        }
    }

    /// What the selected components' writers reported after preparing.
    pub fn writers(&self) -> &[WriterStatus] {
        &self.writers
    }

    /// Tells the writers the files are in place, and waits for them to finish. Returns what
    /// they reported, or [`Error::WriterFailed`] if one of them failed.
    pub fn finish(mut self) -> Result<Vec<WriterStatus>> {
        self.end(true)
    }

    /// Tells the writers the restore failed, and reports any failure in doing so, which
    /// dropping can't. Before any file is copied, this leaves the application as it was.
    pub fn abort(mut self) -> Result<()> {
        self.end(false).map(|_| ())
    }

    fn end(&mut self, restored: bool) -> Result<Vec<WriterStatus>> {
        let Some(worker) = self.worker.take() else {
            return Ok(Vec::new());
        };
        if restored {
            let _ = worker.restored.send(());
        }
        drop(worker.restored);
        worker.thread.join().unwrap_or(Err(Error::Internal {
            reason: "the restore thread panicked",
        }))
    }
}

impl Drop for Restore {
    /// Tells the writers the restore failed, ignoring failures. Blocks until done.
    fn drop(&mut self) {
        let _ = self.end(false);
    }
}

/// The worker thread: lets the writers prepare, reports their state, waits to hear whether
/// the files were restored, then lets the writers finish. Owns every COM object.
fn run(
    document: &str,
    components: &[ComponentId],
    timeout: Duration,
    ready: &Sender<Result<Vec<WriterStatus>>>,
    restored: &Receiver<()>,
) -> Result<Vec<WriterStatus>> {
    let session = match Session::begin(document, components, timeout) {
        Ok((session, writers)) => {
            let _ = ready.send(Ok(writers));
            session
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(Vec::new());
        }
    };
    // A closed channel means the restore was aborted or dropped.
    let restored = restored.recv().is_ok();
    session.finish(restored)
}

/// One restore's VSS session. Fields drop in order, so the backup components object is
/// released before the thread leaves the COM apartment.
struct Session {
    backup: Backup,
    selected: Vec<Selected>,
    _apartment: Apartment,
}

impl Session {
    fn begin(
        document: &str,
        components: &[ComponentId],
        timeout: Duration,
    ) -> Result<(Session, Vec<WriterStatus>)> {
        let apartment = Apartment::enter()?;
        let backup = Backup::create(timeout)?;
        backup.initialize_for_restore(document)?;
        backup.gather_writer_metadata()?;
        let selected = select(&backup, components);
        let freed = backup.free_writer_metadata();
        let selected = selected?;
        freed?;

        backup.pre_restore()?;
        let session = Session {
            backup,
            selected,
            _apartment: apartment,
        };
        match session.check_writers() {
            Ok(writers) => Ok((session, writers)),
            Err(error) => {
                // A writer has already prepared, so let it back out before giving up.
                let _ = session.finish(false);
                Err(error)
            }
        }
    }

    /// Tells the writers whether the files were restored, then lets them finish. Every step
    /// is attempted, and the first failure is returned.
    fn finish(&self, restored: bool) -> Result<Vec<WriterStatus>> {
        let marked = self.selected.iter().fold(Ok(()), |result, component| {
            let status = self.backup.set_file_restore_status(component, restored);
            result.and(status)
        });
        let finished = self.backup.post_restore();
        marked.and(finished)?;
        self.check_writers()
    }

    fn check_writers(&self) -> Result<Vec<WriterStatus>> {
        let statuses = self.backup.writer_statuses()?;
        check_writers(&statuses, &selected_writers(&self.selected))
    }
}

/// Selects each wanted component that the document records as backed up successfully.
fn select(backup: &Backup, wanted: &[ComponentId]) -> Result<Vec<Selected>> {
    let mut available = backup.document_components()?;
    let mut selected = Vec::with_capacity(wanted.len());
    for id in wanted {
        let index = available
            .iter()
            .position(|component| component.id == *id)
            .ok_or_else(|| Error::ComponentNotFound {
                component: id.clone(),
            })?;
        let component = available.swap_remove(index);
        backup.select_for_restore(&component)?;
        selected.push(component);
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::Guid;

    #[test]
    fn a_restore_can_move_between_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Restore>();
    }

    // These fail before VSS is involved.

    #[test]
    fn needs_at_least_one_component() {
        let result = Restore::begin(
            &BackupDocument::from_xml("<x/>"),
            &[],
            &RestoreOptions::new(),
        );
        assert!(matches!(result, Err(Error::InvalidOptions { .. })));
    }

    #[test]
    fn a_zero_timeout_is_refused() {
        let result = Restore::begin(
            &BackupDocument::from_xml("<x/>"),
            &[ComponentId::new(Guid::from_u128(1), "", "x")],
            &RestoreOptions::new().with_timeout(Duration::ZERO),
        );
        assert!(matches!(result, Err(Error::InvalidOptions { .. })));
    }
}
