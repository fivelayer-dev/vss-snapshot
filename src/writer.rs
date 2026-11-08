use std::fmt;
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::guid::Guid;
use crate::hresult::HResult;
use crate::options::Consistency;
use crate::sys;
use crate::sys::com::Apartment;
use crate::sys::vss::Backup;

/// What a VSS writer reported after a snapshot was taken, or during a restore.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WriterStatus {
    /// The writer's class id. It's the same on every machine, so match on this rather than
    /// the name.
    pub id: Guid,
    /// The writer's name, such as `SqlServerWriter`.
    pub name: String,
    /// The writer's state.
    pub state: WriterState,
    /// The failure the writer reported, if any.
    pub failure: Option<HResult>,
}

impl WriterStatus {
    pub(crate) fn new(id: Guid, name: String, state: i32, failure: i32) -> WriterStatus {
        WriterStatus {
            id,
            name,
            state: WriterState::from_raw(state),
            failure: (failure != 0).then_some(HResult::from_code(failure)),
        }
    }

    /// Whether the writer failed, in which case its files are only crash-consistent: as they
    /// would be after a power cut.
    pub fn is_failed(&self) -> bool {
        self.state.is_failed() || self.failure.is_some_and(HResult::is_failure)
    }
}

/// A writer's state, `VSS_WRITER_STATE`. `Display` gives the VSS name, such as
/// `VSS_WS_STABLE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WriterState {
    /// `VSS_WS_UNKNOWN`
    Unknown,
    /// `VSS_WS_STABLE`: ready, with nothing in progress.
    Stable,
    /// `VSS_WS_WAITING_FOR_FREEZE`
    WaitingForFreeze,
    /// `VSS_WS_WAITING_FOR_THAW`
    WaitingForThaw,
    /// `VSS_WS_WAITING_FOR_POST_SNAPSHOT`
    WaitingForPostSnapshot,
    /// `VSS_WS_WAITING_FOR_BACKUP_COMPLETE`
    WaitingForBackupComplete,
    /// `VSS_WS_FAILED_AT_IDENTIFY`
    FailedAtIdentify,
    /// `VSS_WS_FAILED_AT_PREPARE_BACKUP`
    FailedAtPrepareBackup,
    /// `VSS_WS_FAILED_AT_PREPARE_SNAPSHOT`
    FailedAtPrepareSnapshot,
    /// `VSS_WS_FAILED_AT_FREEZE`
    FailedAtFreeze,
    /// `VSS_WS_FAILED_AT_THAW`
    FailedAtThaw,
    /// `VSS_WS_FAILED_AT_POST_SNAPSHOT`
    FailedAtPostSnapshot,
    /// `VSS_WS_FAILED_AT_BACKUP_COMPLETE`
    FailedAtBackupComplete,
    /// `VSS_WS_FAILED_AT_PRE_RESTORE`
    FailedAtPreRestore,
    /// `VSS_WS_FAILED_AT_POST_RESTORE`
    FailedAtPostRestore,
    /// `VSS_WS_FAILED_AT_BACKUPSHUTDOWN`
    FailedAtBackupShutdown,
    /// A value newer than this crate.
    Other(i32),
}

const WRITER_STATES: &[(WriterState, &str)] = &[
    (WriterState::Unknown, "VSS_WS_UNKNOWN"),
    (WriterState::Stable, "VSS_WS_STABLE"),
    (WriterState::WaitingForFreeze, "VSS_WS_WAITING_FOR_FREEZE"),
    (WriterState::WaitingForThaw, "VSS_WS_WAITING_FOR_THAW"),
    (
        WriterState::WaitingForPostSnapshot,
        "VSS_WS_WAITING_FOR_POST_SNAPSHOT",
    ),
    (
        WriterState::WaitingForBackupComplete,
        "VSS_WS_WAITING_FOR_BACKUP_COMPLETE",
    ),
    (WriterState::FailedAtIdentify, "VSS_WS_FAILED_AT_IDENTIFY"),
    (
        WriterState::FailedAtPrepareBackup,
        "VSS_WS_FAILED_AT_PREPARE_BACKUP",
    ),
    (
        WriterState::FailedAtPrepareSnapshot,
        "VSS_WS_FAILED_AT_PREPARE_SNAPSHOT",
    ),
    (WriterState::FailedAtFreeze, "VSS_WS_FAILED_AT_FREEZE"),
    (WriterState::FailedAtThaw, "VSS_WS_FAILED_AT_THAW"),
    (
        WriterState::FailedAtPostSnapshot,
        "VSS_WS_FAILED_AT_POST_SNAPSHOT",
    ),
    (
        WriterState::FailedAtBackupComplete,
        "VSS_WS_FAILED_AT_BACKUP_COMPLETE",
    ),
    (
        WriterState::FailedAtPreRestore,
        "VSS_WS_FAILED_AT_PRE_RESTORE",
    ),
    (
        WriterState::FailedAtPostRestore,
        "VSS_WS_FAILED_AT_POST_RESTORE",
    ),
    (
        WriterState::FailedAtBackupShutdown,
        "VSS_WS_FAILED_AT_BACKUPSHUTDOWN",
    ),
];

impl WriterState {
    /// The state for a raw `VSS_WRITER_STATE`, whose values count up from 0 in the order
    /// of `WRITER_STATES`.
    pub(crate) fn from_raw(raw: i32) -> WriterState {
        usize::try_from(raw)
            .ok()
            .and_then(|index| WRITER_STATES.get(index))
            .map_or(WriterState::Other(raw), |(state, _)| *state)
    }

    /// Whether this is one of the `FailedAt` states.
    pub fn is_failed(self) -> bool {
        matches!(
            self,
            WriterState::FailedAtIdentify
                | WriterState::FailedAtPrepareBackup
                | WriterState::FailedAtPrepareSnapshot
                | WriterState::FailedAtFreeze
                | WriterState::FailedAtThaw
                | WriterState::FailedAtPostSnapshot
                | WriterState::FailedAtBackupComplete
                | WriterState::FailedAtPreRestore
                | WriterState::FailedAtPostRestore
                | WriterState::FailedAtBackupShutdown
        )
    }

    /// The VSS constant's name, such as `"VSS_WS_STABLE"`, or `"unknown"` for
    /// [`Other`](WriterState::Other).
    pub fn vss_name(self) -> &'static str {
        WRITER_STATES
            .iter()
            .find(|(state, _)| *state == self)
            .map_or("unknown", |(_, name)| *name)
    }
}

impl fmt::Display for WriterState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriterState::Other(raw) => write!(f, "unknown writer state {raw}"),
            state => f.write_str(state.vss_name()),
        }
    }
}

/// A writer on this machine and what it looks after, from [`list_writers`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WriterInfo {
    /// The writer's class id. It's the same on every machine.
    pub id: Guid,
    /// The writer's name, such as `SqlServerWriter`.
    pub name: String,
    /// How the writer wants its files restored.
    pub restore_method: RestoreMethod,
    /// What the writer can back up.
    pub components: Vec<Component>,
    /// Files the writer says not to back up, such as temporary files.
    pub excluded_files: Vec<FileSpec>,
}

impl WriterInfo {
    /// Identifies one of this writer's components, for
    /// [`SnapshotOptions::with_components`](crate::SnapshotOptions::with_components).
    pub fn component_id(&self, component: &Component) -> ComponentId {
        ComponentId::new(
            self.id,
            component.logical_path.clone(),
            component.name.clone(),
        )
    }
}

/// Something a writer can back up, such as one SQL Server database.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Component {
    /// Whether it's a database or a group of files.
    pub kind: ComponentKind,
    /// Its place in the writer's hierarchy. Often empty; SQL Server puts the instance here.
    /// Together with `name` it identifies the component within its writer.
    pub logical_path: String,
    /// The component's name.
    pub name: String,
    /// A description for people.
    pub caption: String,
    /// Whether it can be chosen on its own. If not, it's backed up with its writer.
    pub selectable: bool,
    /// Whether it can be chosen on its own for a restore.
    pub selectable_for_restore: bool,
    /// Its files. A writer chooses which of these three lists to use, so a backup must copy
    /// all of them. SQL Server lists a database's data and log files here.
    pub files: Vec<FileSpec>,
    /// Its database files, for a writer that lists them separately.
    pub database_files: Vec<FileSpec>,
    /// Its database log files, for a writer that lists them separately.
    pub log_files: Vec<FileSpec>,
}

impl Component {
    /// The volumes holding this component's files, each listed once, to pass to
    /// [`SnapshotSet::create`](crate::SnapshotSet::create). Each is a drive such as `C:\`,
    /// or a GUID path for a volume with no drive letter.
    ///
    /// Environment variables in the paths, such as `%SystemRoot%`, are expanded. This only
    /// looks at where each listed path is, so it won't find another volume mounted somewhere
    /// under a recursive path.
    pub fn volumes(&self) -> Result<Vec<String>> {
        let mut volumes: Vec<String> = Vec::new();
        for spec in self
            .files
            .iter()
            .chain(&self.database_files)
            .chain(&self.log_files)
        {
            let volume = sys::volume::volume_of_path(&spec.path)?;
            if !volumes.iter().any(|v| v.eq_ignore_ascii_case(&volume)) {
                volumes.push(volume);
            }
        }
        Ok(volumes)
    }
}

/// Identifies a writer's component on any machine, and across sessions. Store it with a
/// backup that selected the component.
///
/// `Display` gives the logical path, name and writer, such as
/// `HOST\SQLEXPRESS\sales (writer {a65faa63-5ea8-4ebc-9dbd-a0c4db26912a})`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ComponentId {
    /// The writer's class id.
    pub writer_id: Guid,
    /// The component's logical path, which is often empty.
    pub logical_path: String,
    /// The component's name.
    pub name: String,
}

impl ComponentId {
    /// Identifies a component by its writer, logical path and name, such as one read back
    /// from storage.
    pub fn new(
        writer_id: Guid,
        logical_path: impl Into<String>,
        name: impl Into<String>,
    ) -> ComponentId {
        ComponentId {
            writer_id,
            logical_path: logical_path.into(),
            name: name.into(),
        }
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.logical_path.is_empty() {
            write!(f, "{}\\", self.logical_path)?;
        }
        write!(f, "{} (writer {})", self.name, self.writer_id)
    }
}

/// A set of files a writer names. It's a pattern rather than a list: `path` can contain
/// environment variables, and `filespec` can contain wildcards.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileSpec {
    /// The folder, such as `%SystemRoot%\system32\config`.
    pub path: String,
    /// The file names, such as `*.mdf`.
    pub filespec: String,
    /// Whether it also covers the folders under `path`.
    pub recursive: bool,
}

/// The kind of a writer's component, `VSS_COMPONENT_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ComponentKind {
    /// `VSS_CT_DATABASE`
    Database,
    /// `VSS_CT_FILEGROUP`
    FileGroup,
    /// A value newer than this crate.
    Other(i32),
}

impl ComponentKind {
    pub(crate) fn from_raw(raw: i32) -> ComponentKind {
        match raw {
            1 => ComponentKind::Database,
            2 => ComponentKind::FileGroup,
            other => ComponentKind::Other(other),
        }
    }

    pub(crate) fn to_raw(self) -> i32 {
        match self {
            ComponentKind::Database => 1,
            ComponentKind::FileGroup => 2,
            ComponentKind::Other(raw) => raw,
        }
    }
}

/// How a writer wants its files restored, `VSS_RESTOREMETHOD_ENUM`. `Display` gives the VSS
/// name, such as `VSS_RME_RESTORE_IF_CAN_REPLACE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RestoreMethod {
    /// `VSS_RME_UNDEFINED`
    Undefined,
    /// `VSS_RME_RESTORE_IF_NOT_THERE`
    RestoreIfNotThere,
    /// `VSS_RME_RESTORE_IF_CAN_REPLACE`
    RestoreIfCanReplace,
    /// `VSS_RME_STOP_RESTORE_START`
    StopRestoreStart,
    /// `VSS_RME_RESTORE_TO_ALTERNATE_LOCATION`
    RestoreToAlternateLocation,
    /// `VSS_RME_RESTORE_AT_REBOOT`
    RestoreAtReboot,
    /// `VSS_RME_RESTORE_AT_REBOOT_IF_CANNOT_REPLACE`
    RestoreAtRebootIfCannotReplace,
    /// `VSS_RME_CUSTOM`
    Custom,
    /// `VSS_RME_RESTORE_STOP_START`
    RestoreStopStart,
    /// A value newer than this crate.
    Other(i32),
}

const RESTORE_METHODS: &[(RestoreMethod, &str)] = &[
    (RestoreMethod::Undefined, "VSS_RME_UNDEFINED"),
    (
        RestoreMethod::RestoreIfNotThere,
        "VSS_RME_RESTORE_IF_NOT_THERE",
    ),
    (
        RestoreMethod::RestoreIfCanReplace,
        "VSS_RME_RESTORE_IF_CAN_REPLACE",
    ),
    (
        RestoreMethod::StopRestoreStart,
        "VSS_RME_STOP_RESTORE_START",
    ),
    (
        RestoreMethod::RestoreToAlternateLocation,
        "VSS_RME_RESTORE_TO_ALTERNATE_LOCATION",
    ),
    (RestoreMethod::RestoreAtReboot, "VSS_RME_RESTORE_AT_REBOOT"),
    (
        RestoreMethod::RestoreAtRebootIfCannotReplace,
        "VSS_RME_RESTORE_AT_REBOOT_IF_CANNOT_REPLACE",
    ),
    (RestoreMethod::Custom, "VSS_RME_CUSTOM"),
    (
        RestoreMethod::RestoreStopStart,
        "VSS_RME_RESTORE_STOP_START",
    ),
];

impl RestoreMethod {
    /// The method for a raw `VSS_RESTOREMETHOD_ENUM`, whose values count up from 0 in the
    /// order of `RESTORE_METHODS`.
    pub(crate) fn from_raw(raw: i32) -> RestoreMethod {
        usize::try_from(raw)
            .ok()
            .and_then(|index| RESTORE_METHODS.get(index))
            .map_or(RestoreMethod::Other(raw), |(method, _)| *method)
    }

    /// The VSS constant's name, such as `"VSS_RME_CUSTOM"`, or `"unknown"` for
    /// [`Other`](RestoreMethod::Other).
    pub fn vss_name(self) -> &'static str {
        RESTORE_METHODS
            .iter()
            .find(|(method, _)| *method == self)
            .map_or("unknown", |(_, name)| *name)
    }
}

impl fmt::Display for RestoreMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RestoreMethod::Other(raw) => write!(f, "unknown restore method {raw}"),
            method => f.write_str(method.vss_name()),
        }
    }
}

/// Lists the VSS writers on this machine and what each looks after, such as each SQL Server
/// database and its files.
///
/// This takes no snapshot and pauses nothing. `timeout` is the longest to wait for the
/// writers to answer. Needs an elevated process, SYSTEM, or a member of Backup Operators.
///
/// ```no_run
/// use std::time::Duration;
/// use vss_snapshot::list_writers;
///
/// # fn main() -> vss_snapshot::Result<()> {
/// for writer in list_writers(Duration::from_secs(60))? {
///     println!("{} {}", writer.id, writer.name);
///     for component in &writer.components {
///         println!("  {} {}", component.logical_path, component.name);
///     }
/// }
/// # Ok(())
/// # }
/// ```
pub fn list_writers(timeout: Duration) -> Result<Vec<WriterInfo>> {
    if timeout.is_zero() {
        return Err(Error::InvalidOptions {
            reason: "the timeout must be more than zero",
        });
    }
    if cfg!(target_pointer_width = "32") && sys::process::is_wow64()? {
        return Err(Error::Wow64NotSupported);
    }
    thread::Builder::new()
        .name("vss-list-writers".to_owned())
        .spawn(move || {
            let _apartment = Apartment::enter()?;
            let backup = Backup::create(timeout)?;
            backup.initialize_for_backup()?;
            backup.set_context(Consistency::Application)?;
            backup.set_copy_backup(false)?;
            backup.gather_writer_metadata()?;
            let writers = backup.gathered_writers();
            let freed = backup.free_writer_metadata();
            let writers = writers?;
            freed?;
            Ok(writers.into_iter().map(|writer| writer.info).collect())
        })?
        .join()
        .unwrap_or(Err(Error::Internal {
            reason: "the thread listing writers panicked",
        }))
}

/// The statuses of the given writers, or [`Error::WriterFailed`] for the first of them that
/// failed. Other writers may have failed for reasons unrelated to these components, so
/// they're left out.
pub(crate) fn check_writers(
    statuses: &[WriterStatus],
    writers: &[Guid],
) -> Result<Vec<WriterStatus>> {
    let selected: Vec<WriterStatus> = statuses
        .iter()
        .filter(|status| writers.contains(&status.id))
        .cloned()
        .collect();
    match selected.iter().find(|status| status.is_failed()) {
        Some(failed) => Err(Error::WriterFailed {
            writer: failed.name.clone(),
            state: failed.state,
            failure: failed.failure,
        }),
        None => Ok(selected),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_given_writers_are_checked() {
        let other = Guid::from_u128(2);
        let statuses = [
            WriterStatus::new(SQL_WRITER, "SqlServerWriter".to_owned(), 1, 0),
            WriterStatus::new(other, "Other".to_owned(), 9, 0),
        ];
        let checked = check_writers(&statuses, &[SQL_WRITER]).unwrap();
        assert_eq!(checked, statuses[..1]);
        assert!(check_writers(&statuses, &[]).unwrap().is_empty());
        match check_writers(&statuses, &[SQL_WRITER, other]) {
            Err(Error::WriterFailed { writer, state, .. }) => {
                assert_eq!(writer, "Other");
                assert_eq!(state, WriterState::FailedAtFreeze);
            }
            result => panic!("{result:?}"),
        }
    }

    #[test]
    fn a_zero_timeout_is_refused_without_touching_vss() {
        assert!(matches!(
            list_writers(Duration::ZERO),
            Err(Error::InvalidOptions { .. })
        ));
    }

    const SQL_WRITER: Guid = Guid::from_u128(0xa65faa63_5ea8_4ebc_9dbd_a0c4db26912a);

    fn component(name: &str) -> Component {
        Component {
            kind: ComponentKind::FileGroup,
            logical_path: r"HOST\SQLEXPRESS".to_owned(),
            name: name.to_owned(),
            caption: String::new(),
            selectable: true,
            selectable_for_restore: true,
            files: Vec::new(),
            database_files: Vec::new(),
            log_files: Vec::new(),
        }
    }

    fn with_paths(paths: &[&str]) -> Component {
        Component {
            files: paths
                .iter()
                .map(|path| FileSpec {
                    path: (*path).to_owned(),
                    filespec: "*".to_owned(),
                    recursive: false,
                })
                .collect(),
            ..component("x")
        }
    }

    fn system_drive() -> String {
        format!(r"{}\", std::env::var("SystemDrive").unwrap().to_uppercase())
    }

    #[test]
    fn writer_states_follow_the_vss_numbering() {
        assert_eq!(WriterState::from_raw(0), WriterState::Unknown);
        assert_eq!(WriterState::from_raw(1), WriterState::Stable);
        assert_eq!(
            WriterState::from_raw(5),
            WriterState::WaitingForBackupComplete
        );
        assert_eq!(WriterState::from_raw(6), WriterState::FailedAtIdentify);
        assert_eq!(
            WriterState::from_raw(15),
            WriterState::FailedAtBackupShutdown
        );
        assert_eq!(WriterState::from_raw(16), WriterState::Other(16));
        assert_eq!(WriterState::from_raw(-1), WriterState::Other(-1));
    }

    #[test]
    fn exactly_the_failed_at_states_are_failures() {
        for raw in 0..=16 {
            let state = WriterState::from_raw(raw);
            assert_eq!(state.is_failed(), (6..=15).contains(&raw), "{state}");
            assert_eq!(
                state.is_failed(),
                state.vss_name().starts_with("VSS_WS_FAILED_AT_"),
                "{state}"
            );
        }
    }

    #[test]
    fn writer_states_display_their_vss_name() {
        assert_eq!(WriterState::Stable.to_string(), "VSS_WS_STABLE");
        assert_eq!(WriterState::Other(99).vss_name(), "unknown");
        assert_eq!(
            WriterState::Other(99).to_string(),
            "unknown writer state 99"
        );
    }

    #[test]
    fn a_writer_with_a_failure_code_has_failed_even_if_stable() {
        let fine = WriterStatus::new(SQL_WRITER, "w".to_owned(), 1, 0);
        assert!(!fine.is_failed());
        assert_eq!(fine.failure, None);

        let code = 0x800423F2_u32 as i32;
        let failed = WriterStatus::new(SQL_WRITER, "w".to_owned(), 1, code);
        assert!(failed.is_failed());
        assert_eq!(failed.failure, Some(HResult::from_code(code)));

        assert!(WriterStatus::new(SQL_WRITER, "w".to_owned(), 9, 0).is_failed());
    }

    #[test]
    fn restore_methods_follow_the_vss_numbering() {
        assert_eq!(RestoreMethod::from_raw(0), RestoreMethod::Undefined);
        assert_eq!(
            RestoreMethod::from_raw(2),
            RestoreMethod::RestoreIfCanReplace
        );
        assert_eq!(RestoreMethod::from_raw(8), RestoreMethod::RestoreStopStart);
        assert_eq!(RestoreMethod::from_raw(9), RestoreMethod::Other(9));
        assert_eq!(
            RestoreMethod::RestoreIfCanReplace.to_string(),
            "VSS_RME_RESTORE_IF_CAN_REPLACE"
        );
        assert_eq!(RestoreMethod::Other(9).vss_name(), "unknown");
    }

    #[test]
    fn component_kinds_keep_their_raw_value() {
        for raw in -1..=5 {
            assert_eq!(ComponentKind::from_raw(raw).to_raw(), raw);
        }
        assert_eq!(ComponentKind::from_raw(1), ComponentKind::Database);
        assert_eq!(ComponentKind::from_raw(2), ComponentKind::FileGroup);
    }

    #[test]
    fn component_id_identifies_the_component() {
        let sales = component("sales");
        let writer = WriterInfo {
            id: SQL_WRITER,
            name: "SqlServerWriter".to_owned(),
            restore_method: RestoreMethod::RestoreIfCanReplace,
            components: vec![sales.clone()],
            excluded_files: Vec::new(),
        };
        let id = writer.component_id(&sales);
        assert_eq!(
            id,
            ComponentId::new(SQL_WRITER, r"HOST\SQLEXPRESS", "sales")
        );
        assert_eq!(
            id.to_string(),
            r"HOST\SQLEXPRESS\sales (writer {a65faa63-5ea8-4ebc-9dbd-a0c4db26912a})"
        );
        let no_path = ComponentId::new(SQL_WRITER, "", "sales");
        assert_eq!(
            no_path.to_string(),
            "sales (writer {a65faa63-5ea8-4ebc-9dbd-a0c4db26912a})"
        );
    }

    // These only ask Windows which volume a path is on, which changes nothing.
    #[test]
    fn a_path_is_on_its_drive_even_if_it_does_not_exist() {
        let root = system_drive();
        let path = format!("{root}no-such-folder-for-vss-snapshot-tests");
        assert_eq!(with_paths(&[&path]).volumes().unwrap(), [root]);
    }

    #[test]
    fn environment_variables_in_paths_are_expanded() {
        let component = with_paths(&[r"%SystemRoot%\system32"]);
        assert_eq!(component.volumes().unwrap(), [system_drive()]);
    }

    #[test]
    fn each_volume_is_listed_once() {
        let root = system_drive();
        let mut component = with_paths(&[&root, &root.to_lowercase()]);
        component.log_files = component.files.clone();
        assert_eq!(component.volumes().unwrap(), [root]);
    }

    #[test]
    fn a_component_with_no_files_is_on_no_volumes() {
        assert!(component("x").volumes().unwrap().is_empty());
    }
}
