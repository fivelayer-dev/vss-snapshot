//! VSS's requester interface, `IVssBackupComponents`, behind a safe wrapper.
//!
//! The `windows` crate has most of what VSS needs, including `IVssAsync`, `IVssComponent`,
//! `IVssWMFiledesc` and `VSS_SNAPSHOT_PROP`. What it lacks is declared in [`raw`].
//!
//! A [`Backup`] must be used and dropped on the thread that made it, inside an
//! [`Apartment`](super::com::Apartment). It isn't `Send`, so the compiler holds callers to
//! that.

mod metadata;
mod raw;

use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{s, w, Error as WinError, Result as WinResult, BOOL, BSTR, GUID, HRESULT};
use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Storage::Vss::{
    IVssAsync, VSS_BT_COPY, VSS_CTX_BACKUP, VSS_CTX_FILE_SHARE_BACKUP, VSS_OBJECT_SNAPSHOT_SET,
    VSS_RS_ALL, VSS_RS_FAILED, VSS_SNAPSHOT_PROP, VSS_S_ASYNC_CANCELLED, VSS_S_ASYNC_FINISHED,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};

pub(crate) use metadata::{find_component, GatheredWriter, Selected};

use super::wide::to_wide;
use crate::error::{Error, Result};
use crate::guid::Guid;
use crate::hresult::HResult;
use crate::options::Consistency;
use crate::writer::WriterStatus;
use raw::IVssBackupComponents;

type CreateBackupComponents =
    unsafe extern "system" fn(*mut Option<IVssBackupComponents>) -> HRESULT;
type FreeSnapshotProperties = unsafe extern "system" fn(*mut VSS_SNAPSHOT_PROP);

/// The two functions `VssApi.dll` exports that this crate calls.
struct Api {
    create_backup_components: CreateBackupComponents,
    free_snapshot_properties: FreeSnapshotProperties,
}

/// `VssApi.dll`, loaded once. It stays loaded for the life of the process, because the COM
/// objects it makes must never outlive it.
fn api() -> Result<&'static Api> {
    static API: OnceLock<std::result::Result<Api, i32>> = OnceLock::new();
    API.get_or_init(load_api)
        .as_ref()
        .map_err(|&code| Error::from_hresult("loading VssApi.dll", HResult::from_code(code)))
}

fn load_api() -> std::result::Result<Api, i32> {
    // SAFETY: loads a system DLL, searching only System32 so a DLL planted elsewhere can't
    // be picked up. It's never freed.
    let library = unsafe { LoadLibraryExW(w!("vssapi.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
        .map_err(|error| error.code().0)?;
    let find = |name: PCSTR| {
        // SAFETY: `library` is loaded, and `name` is a nul-terminated literal.
        unsafe { GetProcAddress(library, name) }.ok_or_else(|| WinError::from_thread().code().0)
    };
    let create = find(s!("CreateVssBackupComponentsInternal"))?;
    let free = find(s!("VssFreeSnapshotPropertiesInternal"))?;
    // SAFETY: these exports have these signatures, as `vsbackup.h` declares them.
    unsafe {
        Ok(Api {
            create_backup_components: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                CreateBackupComponents,
            >(create),
            free_snapshot_properties: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                FreeSnapshotProperties,
            >(free),
        })
    }
}

/// Turns a failed call into an error naming it.
fn failed(operation: &'static str) -> impl Fn(WinError) -> Error {
    move |error| Error::from_hresult(operation, HResult::from_code(error.code().0))
}

pub(crate) fn to_guid(id: GUID) -> Guid {
    Guid::from_u128(id.to_u128())
}

/// The nul-terminated UTF-16 string at `text`, or an empty string for null.
///
/// # Safety
///
/// `text` must be null or point to a nul-terminated UTF-16 string.
unsafe fn string_at(text: *const u16) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: the caller guarantees a nul-terminated string.
    String::from_utf16_lossy(unsafe { PCWSTR(text).as_wide() })
}

/// What this crate needs from a snapshot's properties.
pub(crate) struct SnapshotProperties {
    /// Such as `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy7`.
    pub(crate) device: String,
    /// When it was taken, as a `FILETIME`.
    pub(crate) created: i64,
}

/// An `IVssBackupComponents`: one backup or restore session with VSS.
pub(crate) struct Backup {
    raw: IVssBackupComponents,
    timeout: Duration,
}

impl Backup {
    /// Starts a session, whose asynchronous steps each wait up to `timeout`.
    pub(crate) fn create(timeout: Duration) -> Result<Backup> {
        let api = api()?;
        let mut raw = None;
        // SAFETY: `raw` outlives the call, and receives an owned reference on success.
        unsafe { (api.create_backup_components)(&mut raw) }
            .ok()
            .map_err(failed("CreateVssBackupComponents"))?;
        let raw = raw.ok_or(Error::Internal {
            reason: "VSS returned no backup components object",
        })?;
        Ok(Backup { raw, timeout })
    }

    // Setting up.

    pub(crate) fn initialize_for_backup(&self) -> Result<()> {
        // SAFETY: a null document starts a new backup.
        unsafe { self.raw.InitializeForBackup(std::ptr::null()) }
            .map_err(failed("InitializeForBackup"))
    }

    /// Chooses non-persistent snapshots that VSS removes automatically, with or without
    /// writers.
    pub(crate) fn set_context(&self, consistency: Consistency) -> Result<()> {
        let context = match consistency {
            Consistency::Application => VSS_CTX_BACKUP,
            Consistency::Crash => VSS_CTX_FILE_SHARE_BACKUP,
        };
        // SAFETY: plain value argument.
        unsafe { self.raw.SetContext(context.0) }.map_err(failed("SetContext"))
    }

    /// Makes this a copy backup, which tells writers not to change anything because of it,
    /// such as truncating a database's logs.
    pub(crate) fn set_copy_backup(&self, select_components: bool) -> Result<()> {
        // SAFETY: plain value arguments.
        unsafe {
            self.raw
                .SetBackupState(select_components, false, VSS_BT_COPY, false)
        }
        .map_err(failed("SetBackupState"))
    }

    // Writer metadata.

    pub(crate) fn gather_writer_metadata(&self) -> Result<()> {
        // SAFETY: `pending` is a valid out-pointer for the call.
        self.run("GatherWriterMetadata", |pending| unsafe {
            self.raw.GatherWriterMetadata(pending)
        })
    }

    /// Reads what `gather_writer_metadata` collected. Every metadata object is released
    /// before this returns, as `free_writer_metadata` requires.
    pub(crate) fn gathered_writers(&self) -> Result<Vec<GatheredWriter>> {
        let mut count = 0;
        // SAFETY: `count` outlives the call.
        unsafe { self.raw.GetWriterMetadataCount(&mut count) }
            .map_err(failed("GetWriterMetadataCount"))?;
        (0..count)
            .map(|index| {
                let mut instance = GUID::zeroed();
                // SAFETY: `instance` and `metadata` outlive the call, and `index` is in range.
                let metadata = metadata::take("GetWriterMetadata", |metadata| unsafe {
                    self.raw.GetWriterMetadata(index, &mut instance, metadata)
                })?;
                metadata::read_writer(&metadata)
            })
            .collect()
    }

    /// Call once everything gathered by `gather_writer_metadata` has been released.
    pub(crate) fn free_writer_metadata(&self) -> Result<()> {
        // SAFETY: no arguments.
        unsafe { self.raw.FreeWriterMetadata() }.map_err(failed("FreeWriterMetadata"))
    }

    /// Adds a component to a backup.
    pub(crate) fn add_component(&self, component: &Selected) -> Result<()> {
        // SAFETY: the strings live in `component`, which outlives the call.
        unsafe {
            self.raw.AddComponent(
                component.instance,
                component.writer,
                component.kind,
                component.logical_path(),
                component.name(),
            )
        }
        .map_err(failed("AddComponent"))
    }

    // Taking snapshots.

    pub(crate) fn start_snapshot_set(&self) -> Result<GUID> {
        let mut set = GUID::zeroed();
        // SAFETY: `set` outlives the call.
        unsafe { self.raw.StartSnapshotSet(&mut set) }.map_err(failed("StartSnapshotSet"))?;
        Ok(set)
    }

    /// Adds a volume, named as VSS expects with a trailing backslash. Errors name it as
    /// `given`, the name the caller used.
    pub(crate) fn add_to_snapshot_set(&self, volume: &str, given: &str) -> Result<GUID> {
        let volume = to_wide(volume);
        let mut snapshot = GUID::zeroed();
        // SAFETY: `volume` is nul-terminated, and it and `snapshot` outlive the call. The
        // null provider lets VSS choose one.
        unsafe {
            self.raw
                .AddToSnapshotSet(PCWSTR(volume.as_ptr()), GUID::zeroed(), &mut snapshot)
        }
        .map_err(|error| Error::from_add_volume(given, HResult::from_code(error.code().0)))?;
        Ok(snapshot)
    }

    pub(crate) fn prepare_for_backup(&self) -> Result<()> {
        // SAFETY: `pending` is a valid out-pointer for the call.
        self.run("PrepareForBackup", |pending| unsafe {
            self.raw.PrepareForBackup(pending)
        })
    }

    pub(crate) fn do_snapshot_set(&self) -> Result<()> {
        // SAFETY: `pending` is a valid out-pointer for the call.
        self.run("DoSnapshotSet", |pending| unsafe {
            self.raw.DoSnapshotSet(pending)
        })
    }

    pub(crate) fn snapshot_properties(&self, snapshot: GUID) -> Result<SnapshotProperties> {
        let api = api()?;
        // SAFETY: `VSS_SNAPSHOT_PROP` is plain data, for which all zeroes is a valid value.
        let mut properties: VSS_SNAPSHOT_PROP = unsafe { std::mem::zeroed() };
        // SAFETY: `properties` outlives the call.
        unsafe { self.raw.GetSnapshotProperties(snapshot, &mut properties) }
            .map_err(failed("GetSnapshotProperties"))?;
        // SAFETY: on success VSS filled in the strings, each nul-terminated or null.
        let device = unsafe { string_at(properties.m_pwszSnapshotDeviceObject) };
        let created = properties.m_tsCreationTimestamp;
        // SAFETY: frees what `GetSnapshotProperties` allocated, once. Nothing read from it is
        // used afterwards.
        unsafe { (api.free_snapshot_properties)(&mut properties) };
        Ok(SnapshotProperties { device, created })
    }

    /// What every writer reports now.
    pub(crate) fn writer_statuses(&self) -> Result<Vec<WriterStatus>> {
        // SAFETY: `pending` is a valid out-pointer for the call.
        self.run("GatherWriterStatus", |pending| unsafe {
            self.raw.GatherWriterStatus(pending)
        })?;
        let statuses = self.read_writer_statuses();
        // SAFETY: no arguments. Called whether or not reading succeeded.
        let freed = unsafe { self.raw.FreeWriterStatus() }.map_err(failed("FreeWriterStatus"));
        let statuses = statuses?;
        freed?;
        Ok(statuses)
    }

    fn read_writer_statuses(&self) -> Result<Vec<WriterStatus>> {
        let mut count = 0;
        // SAFETY: `count` outlives the call.
        unsafe { self.raw.GetWriterStatusCount(&mut count) }
            .map_err(failed("GetWriterStatusCount"))?;
        (0..count)
            .map(|index| {
                let (mut instance, mut writer) = (GUID::zeroed(), GUID::zeroed());
                let mut name = BSTR::new();
                let (mut state, mut failure) = (0, HRESULT(0));
                // SAFETY: every out-pointer outlives the call. `name` takes ownership of
                // the string VSS allocates, and frees it when dropped.
                unsafe {
                    self.raw.GetWriterStatus(
                        index,
                        &mut instance,
                        &mut writer,
                        &mut name,
                        &mut state,
                        &mut failure,
                    )
                }
                .map_err(failed("GetWriterStatus"))?;
                Ok(WriterStatus::new(
                    to_guid(writer),
                    name.to_string(),
                    state,
                    failure.0,
                ))
            })
            .collect()
    }

    /// Cancels the backup after a failure. Releasing the session afterwards removes any
    /// snapshot it made. A failure here can't be acted on, so it's ignored.
    pub(crate) fn abort_backup(&self) {
        // SAFETY: no arguments.
        let _ = unsafe { self.raw.AbortBackup() };
    }

    // Ending a backup.

    pub(crate) fn set_backup_succeeded(&self, component: &Selected) -> Result<()> {
        // SAFETY: the strings live in `component`, which outlives the call.
        unsafe {
            self.raw.SetBackupSucceeded(
                component.instance,
                component.writer,
                component.kind,
                component.logical_path(),
                component.name(),
                true,
            )
        }
        .map_err(failed("SetBackupSucceeded"))
    }

    pub(crate) fn backup_complete(&self) -> Result<()> {
        // SAFETY: `pending` is a valid out-pointer for the call.
        self.run("BackupComplete", |pending| unsafe {
            self.raw.BackupComplete(pending)
        })
    }

    /// The backup components document, which a restore needs.
    pub(crate) fn save_as_xml(&self) -> Result<String> {
        let mut xml = BSTR::new();
        // SAFETY: `xml` outlives the call, and frees the string it receives.
        unsafe { self.raw.SaveAsXML(&mut xml) }.map_err(failed("SaveAsXML"))?;
        Ok(xml.to_string())
    }

    pub(crate) fn delete_snapshot_set(&self, set: GUID) -> Result<()> {
        let (mut deleted, mut not_deleted) = (0, GUID::zeroed());
        // SAFETY: both out-pointers outlive the call.
        unsafe {
            self.raw.DeleteSnapshots(
                set,
                VSS_OBJECT_SNAPSHOT_SET,
                BOOL::from(true),
                &mut deleted,
                &mut not_deleted,
            )
        }
        .map_err(failed("DeleteSnapshots"))
    }

    // Restoring.

    pub(crate) fn initialize_for_restore(&self, document: &str) -> Result<()> {
        let xml = BSTR::from(document).into_raw();
        // SAFETY: `xml` is a BSTR that lives until it's freed below, after the call.
        let result = unsafe { self.raw.InitializeForRestore(xml) };
        // SAFETY: takes back the BSTR made above, which frees it once.
        drop(unsafe { BSTR::from_raw(xml) });
        result.map_err(failed("InitializeForRestore"))
    }

    /// The components the document records as backed up successfully. Writers may no longer
    /// list them, as what they backed up may be gone by now.
    pub(crate) fn document_components(&self) -> Result<Vec<Selected>> {
        let mut count = 0;
        // SAFETY: `count` outlives the call.
        unsafe { self.raw.GetWriterComponentsCount(&mut count) }
            .map_err(failed("GetWriterComponentsCount"))?;
        let mut found = Vec::new();
        for index in 0..count {
            let mut components = std::ptr::null_mut();
            // SAFETY: `components` outlives the call, and `index` is in range. The reference
            // it receives is handed straight to `read_document_writer`, which releases it.
            unsafe { self.raw.GetWriterComponents(index, &mut components) }
                .map_err(failed("GetWriterComponents"))?;
            found.extend(metadata::read_document_writer(components)?);
        }
        Ok(found)
    }

    /// Selects a component for restore, and says no restores of it will follow, so its
    /// writer finishes recovery. For SQL Server, that brings the database online.
    pub(crate) fn select_for_restore(&self, component: &Selected) -> Result<()> {
        // SAFETY: the strings live in `component`, which outlives both calls.
        unsafe {
            self.raw.SetSelectedForRestore(
                component.writer,
                component.kind,
                component.logical_path(),
                component.name(),
                true,
            )
        }
        .map_err(failed("SetSelectedForRestore"))?;
        // SAFETY: as above.
        unsafe {
            self.raw.SetAdditionalRestores(
                component.writer,
                component.kind,
                component.logical_path(),
                component.name(),
                false,
            )
        }
        .map_err(failed("SetAdditionalRestores"))
    }

    pub(crate) fn pre_restore(&self) -> Result<()> {
        // SAFETY: `pending` is a valid out-pointer for the call.
        self.run("PreRestore", |pending| unsafe {
            self.raw.PreRestore(pending)
        })
    }

    pub(crate) fn set_file_restore_status(
        &self,
        component: &Selected,
        restored: bool,
    ) -> Result<()> {
        let status = if restored { VSS_RS_ALL } else { VSS_RS_FAILED };
        // SAFETY: the strings live in `component`, which outlives the call.
        unsafe {
            self.raw.SetFileRestoreStatus(
                component.writer,
                component.kind,
                component.logical_path(),
                component.name(),
                status,
            )
        }
        .map_err(failed("SetFileRestoreStatus"))
    }

    pub(crate) fn post_restore(&self) -> Result<()> {
        // SAFETY: `pending` is a valid out-pointer for the call.
        self.run("PostRestore", |pending| unsafe {
            self.raw.PostRestore(pending)
        })
    }

    // Waiting.

    /// Starts an asynchronous step and waits for it.
    fn run(
        &self,
        operation: &'static str,
        start: impl FnOnce(*mut Option<IVssAsync>) -> WinResult<()>,
    ) -> Result<()> {
        let pending = metadata::take(operation, start)?;
        self.wait(operation, &pending)
    }

    /// Waits for an asynchronous step to finish, up to the timeout, then cancels it. `Wait`
    /// isn't documented to report a timeout in any particular way, so `QueryStatus` decides.
    fn wait(&self, operation: &'static str, pending: &IVssAsync) -> Result<()> {
        let deadline = Instant::now().checked_add(self.timeout);
        loop {
            let left = deadline.map_or(Duration::from_secs(1), |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            let wait_ms = u32::try_from(left.as_millis().clamp(1, 1000)).unwrap_or(1000);
            // SAFETY: `pending` is a live `IVssAsync`.
            let waited = unsafe { pending.Wait(wait_ms) };

            let mut status = HRESULT(0);
            // SAFETY: `status` outlives the call, and the reserved pointer must be null.
            unsafe { pending.QueryStatus(&mut status, std::ptr::null_mut()) }
                .map_err(failed(operation))?;
            if status == VSS_S_ASYNC_FINISHED {
                return Ok(());
            }
            if status == VSS_S_ASYNC_CANCELLED {
                return Err(Error::Vss {
                    operation,
                    code: HResult::from_code(status.0),
                });
            }
            if status.is_err() {
                return Err(Error::from_hresult(operation, HResult::from_code(status.0)));
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                // SAFETY: as above. The step stops as soon as VSS can stop it.
                let _ = unsafe { pending.Cancel() };
                return Err(Error::Timeout {
                    operation,
                    timeout: self.timeout,
                });
            }
            if waited.is_err() {
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_string_or_null() {
        let text = to_wide("abc");
        // SAFETY: `text` is nul-terminated.
        assert_eq!(unsafe { string_at(text.as_ptr()) }, "abc");
        // SAFETY: null is allowed.
        assert_eq!(unsafe { string_at(std::ptr::null()) }, "");
    }

    #[test]
    fn converts_guids_in_printed_order() {
        let id = GUID::from_u128(0xa65faa63_5ea8_4ebc_9dbd_a0c4db26912a);
        assert_eq!(
            to_guid(id).to_string(),
            "{a65faa63-5ea8-4ebc-9dbd-a0c4db26912a}"
        );
    }
}
