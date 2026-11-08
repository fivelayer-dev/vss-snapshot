//! The VSS requester interface, `IVssBackupComponents`, behind a safe wrapper.
//!
//! The `windows` crate has most of what VSS needs, including `IVssAsync` and
//! `VSS_SNAPSHOT_PROP`, but not `IVssBackupComponents` or the function that creates it.
//! Those are declared in [`raw`], in the order of `vsbackup.h` in the Windows SDK, which the
//! slot tests at the end check.
//!
//! A [`Backup`] must be used and dropped on the thread that made it, inside an
//! [`Apartment`](super::com::Apartment). It isn't `Send`, so the compiler holds callers to
//! that.

use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{s, w, Error as WinError, Result as WinResult, BOOL, BSTR, GUID, HRESULT};
use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Storage::Vss::{
    IVssAsync, VSS_BT_COPY, VSS_CTX_BACKUP, VSS_CTX_FILE_SHARE_BACKUP, VSS_OBJECT_SNAPSHOT_SET,
    VSS_SNAPSHOT_PROP, VSS_S_ASYNC_CANCELLED, VSS_S_ASYNC_FINISHED,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};

use super::wide::to_wide;
use crate::error::{Error, Result};
use crate::guid::Guid;
use crate::hresult::HResult;
use crate::options::Consistency;
use crate::writer::WriterStatus;

/// Declarations the `windows` crate doesn't have. Methods that aren't called are still
/// declared, so that each one after them lands in its slot.
pub(super) mod raw {
    #![allow(non_snake_case, dead_code, clippy::too_many_arguments)]

    use std::ffi::c_void;

    use windows::core::PCWSTR;
    use windows::core::{interface, IUnknown, IUnknown_Vtbl, Result, BOOL, BSTR, GUID, HRESULT};
    use windows::Win32::Storage::Vss::{
        IVssAsync, VSS_BACKUP_TYPE, VSS_FILE_RESTORE_STATUS, VSS_OBJECT_TYPE, VSS_SNAPSHOT_PROP,
    };

    /// `IVssBackupComponents`, from `vsbackup.h`, up to `GetSnapshotProperties`. Later
    /// methods are never called, so they're left out.
    #[interface("665c1d5f-c218-414d-a05d-7fef5f9d5c86")]
    pub(in crate::sys) unsafe trait IVssBackupComponents: IUnknown {
        pub(in crate::sys) fn GetWriterComponentsCount(&self, count: *mut u32) -> Result<()>;
        pub(in crate::sys) fn GetWriterComponents(
            &self,
            index: u32,
            components: *mut *mut c_void,
        ) -> Result<()>;
        pub(in crate::sys) fn InitializeForBackup(&self, xml: *const u16) -> Result<()>;
        pub(in crate::sys) fn SetBackupState(
            &self,
            select_components: bool,
            bootable_system_state: bool,
            backup_type: VSS_BACKUP_TYPE,
            partial_file_support: bool,
        ) -> Result<()>;
        pub(in crate::sys) fn InitializeForRestore(&self, xml: *const u16) -> Result<()>;
        pub(in crate::sys) fn SetRestoreState(&self, restore_type: i32) -> Result<()>;
        pub(in crate::sys) fn GatherWriterMetadata(
            &self,
            pending: *mut Option<IVssAsync>,
        ) -> Result<()>;
        pub(in crate::sys) fn GetWriterMetadataCount(&self, count: *mut u32) -> Result<()>;
        pub(in crate::sys) fn GetWriterMetadata(
            &self,
            index: u32,
            instance: *mut GUID,
            metadata: *mut *mut c_void,
        ) -> Result<()>;
        pub(in crate::sys) fn FreeWriterMetadata(&self) -> Result<()>;
        pub(in crate::sys) fn AddComponent(
            &self,
            instance: GUID,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
        ) -> Result<()>;
        pub(in crate::sys) fn PrepareForBackup(
            &self,
            pending: *mut Option<IVssAsync>,
        ) -> Result<()>;
        pub(in crate::sys) fn AbortBackup(&self) -> Result<()>;
        pub(in crate::sys) fn GatherWriterStatus(
            &self,
            pending: *mut Option<IVssAsync>,
        ) -> Result<()>;
        pub(in crate::sys) fn GetWriterStatusCount(&self, count: *mut u32) -> Result<()>;
        pub(in crate::sys) fn FreeWriterStatus(&self) -> Result<()>;
        pub(in crate::sys) fn GetWriterStatus(
            &self,
            index: u32,
            instance: *mut GUID,
            writer: *mut GUID,
            name: *mut BSTR,
            state: *mut i32,
            failure: *mut HRESULT,
        ) -> Result<()>;
        pub(in crate::sys) fn SetBackupSucceeded(
            &self,
            instance: GUID,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
            succeeded: bool,
        ) -> Result<()>;
        pub(in crate::sys) fn SetBackupOptions(
            &self,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
            options: PCWSTR,
        ) -> Result<()>;
        pub(in crate::sys) fn SetSelectedForRestore(
            &self,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
            selected: bool,
        ) -> Result<()>;
        pub(in crate::sys) fn SetRestoreOptions(
            &self,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
            options: PCWSTR,
        ) -> Result<()>;
        pub(in crate::sys) fn SetAdditionalRestores(
            &self,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
            additional: bool,
        ) -> Result<()>;
        pub(in crate::sys) fn SetPreviousBackupStamp(
            &self,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
            stamp: PCWSTR,
        ) -> Result<()>;
        pub(in crate::sys) fn SaveAsXML(&self, xml: *mut BSTR) -> Result<()>;
        pub(in crate::sys) fn BackupComplete(&self, pending: *mut Option<IVssAsync>) -> Result<()>;
        pub(in crate::sys) fn AddAlternativeLocationMapping(
            &self,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
            path: PCWSTR,
            filespec: PCWSTR,
            recursive: bool,
            destination: PCWSTR,
        ) -> Result<()>;
        pub(in crate::sys) fn AddRestoreSubcomponent(
            &self,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
            sub_logical_path: PCWSTR,
            sub_name: PCWSTR,
            repair: bool,
        ) -> Result<()>;
        pub(in crate::sys) fn SetFileRestoreStatus(
            &self,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
            status: VSS_FILE_RESTORE_STATUS,
        ) -> Result<()>;
        pub(in crate::sys) fn AddNewTarget(
            &self,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
            path: PCWSTR,
            file_name: PCWSTR,
            recursive: bool,
            alternate_path: PCWSTR,
        ) -> Result<()>;
        pub(in crate::sys) fn SetRangesFilePath(
            &self,
            writer: GUID,
            kind: i32,
            logical_path: PCWSTR,
            name: PCWSTR,
            partial_file_index: u32,
            ranges_file: PCWSTR,
        ) -> Result<()>;
        pub(in crate::sys) fn PreRestore(&self, pending: *mut Option<IVssAsync>) -> Result<()>;
        pub(in crate::sys) fn PostRestore(&self, pending: *mut Option<IVssAsync>) -> Result<()>;
        pub(in crate::sys) fn SetContext(&self, context: i32) -> Result<()>;
        pub(in crate::sys) fn StartSnapshotSet(&self, set: *mut GUID) -> Result<()>;
        pub(in crate::sys) fn AddToSnapshotSet(
            &self,
            volume: PCWSTR,
            provider: GUID,
            snapshot: *mut GUID,
        ) -> Result<()>;
        pub(in crate::sys) fn DoSnapshotSet(&self, pending: *mut Option<IVssAsync>) -> Result<()>;
        pub(in crate::sys) fn DeleteSnapshots(
            &self,
            source: GUID,
            source_type: VSS_OBJECT_TYPE,
            force: BOOL,
            deleted: *mut i32,
            not_deleted: *mut GUID,
        ) -> Result<()>;
        pub(in crate::sys) fn ImportSnapshots(&self, pending: *mut Option<IVssAsync>)
            -> Result<()>;
        pub(in crate::sys) fn BreakSnapshotSet(&self, set: GUID) -> Result<()>;
        pub(in crate::sys) fn GetSnapshotProperties(
            &self,
            snapshot: GUID,
            properties: *mut VSS_SNAPSHOT_PROP,
        ) -> Result<()>;
    }
}

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

    pub(crate) fn gather_writer_metadata(&self) -> Result<()> {
        // SAFETY: `pending` is a valid out-pointer for the call.
        self.run("GatherWriterMetadata", |pending| unsafe {
            self.raw.GatherWriterMetadata(pending)
        })
    }

    /// Call once everything gathered by `gather_writer_metadata` has been released.
    pub(crate) fn free_writer_metadata(&self) -> Result<()> {
        // SAFETY: no arguments.
        unsafe { self.raw.FreeWriterMetadata() }.map_err(failed("FreeWriterMetadata"))
    }

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

    /// Cancels the backup after a failure. Releasing the session afterwards removes any
    /// snapshot it made. A failure here can't be acted on, so it's ignored.
    pub(crate) fn abort_backup(&self) {
        // SAFETY: no arguments.
        let _ = unsafe { self.raw.AbortBackup() };
    }

    pub(crate) fn backup_complete(&self) -> Result<()> {
        // SAFETY: `pending` is a valid out-pointer for the call.
        self.run("BackupComplete", |pending| unsafe {
            self.raw.BackupComplete(pending)
        })
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

    /// Starts an asynchronous step and waits for it.
    fn run(
        &self,
        operation: &'static str,
        start: impl FnOnce(*mut Option<IVssAsync>) -> WinResult<()>,
    ) -> Result<()> {
        let mut pending = None;
        start(&mut pending).map_err(failed(operation))?;
        let pending = pending.ok_or(Error::Internal {
            reason: "VSS started a step without a way to wait for it",
        })?;
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

#[cfg(test)]
mod tests {
    use super::raw::IVssBackupComponents_Vtbl as Vtbl;
    use super::*;
    use std::mem::{offset_of, size_of};

    fn slot(offset: usize) -> usize {
        offset / size_of::<usize>()
    }

    /// A method in the wrong slot calls a different method, and nothing would catch that
    /// before a real snapshot crashed. These are the positions in `vsbackup.h`.
    #[test]
    fn methods_are_in_their_vtable_slots() {
        let slots = [
            (offset_of!(Vtbl, GetWriterComponentsCount), 3),
            (offset_of!(Vtbl, GetWriterComponents), 4),
            (offset_of!(Vtbl, InitializeForBackup), 5),
            (offset_of!(Vtbl, SetBackupState), 6),
            (offset_of!(Vtbl, InitializeForRestore), 7),
            (offset_of!(Vtbl, SetRestoreState), 8),
            (offset_of!(Vtbl, GatherWriterMetadata), 9),
            (offset_of!(Vtbl, GetWriterMetadataCount), 10),
            (offset_of!(Vtbl, GetWriterMetadata), 11),
            (offset_of!(Vtbl, FreeWriterMetadata), 12),
            (offset_of!(Vtbl, AddComponent), 13),
            (offset_of!(Vtbl, PrepareForBackup), 14),
            (offset_of!(Vtbl, AbortBackup), 15),
            (offset_of!(Vtbl, GatherWriterStatus), 16),
            (offset_of!(Vtbl, GetWriterStatusCount), 17),
            (offset_of!(Vtbl, FreeWriterStatus), 18),
            (offset_of!(Vtbl, GetWriterStatus), 19),
            (offset_of!(Vtbl, SetBackupSucceeded), 20),
            (offset_of!(Vtbl, SetBackupOptions), 21),
            (offset_of!(Vtbl, SetSelectedForRestore), 22),
            (offset_of!(Vtbl, SetRestoreOptions), 23),
            (offset_of!(Vtbl, SetAdditionalRestores), 24),
            (offset_of!(Vtbl, SetPreviousBackupStamp), 25),
            (offset_of!(Vtbl, SaveAsXML), 26),
            (offset_of!(Vtbl, BackupComplete), 27),
            (offset_of!(Vtbl, AddAlternativeLocationMapping), 28),
            (offset_of!(Vtbl, AddRestoreSubcomponent), 29),
            (offset_of!(Vtbl, SetFileRestoreStatus), 30),
            (offset_of!(Vtbl, AddNewTarget), 31),
            (offset_of!(Vtbl, SetRangesFilePath), 32),
            (offset_of!(Vtbl, PreRestore), 33),
            (offset_of!(Vtbl, PostRestore), 34),
            (offset_of!(Vtbl, SetContext), 35),
            (offset_of!(Vtbl, StartSnapshotSet), 36),
            (offset_of!(Vtbl, AddToSnapshotSet), 37),
            (offset_of!(Vtbl, DoSnapshotSet), 38),
            (offset_of!(Vtbl, DeleteSnapshots), 39),
            (offset_of!(Vtbl, ImportSnapshots), 40),
            (offset_of!(Vtbl, BreakSnapshotSet), 41),
            (offset_of!(Vtbl, GetSnapshotProperties), 42),
        ];
        for (index, (offset, expected)) in slots.into_iter().enumerate() {
            assert_eq!(slot(offset), expected, "entry {index}");
        }
        assert_eq!(size_of::<Vtbl>(), 43 * size_of::<usize>());
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn snapshot_properties_match_the_c_layout() {
        assert_eq!(
            offset_of!(VSS_SNAPSHOT_PROP, m_pwszSnapshotDeviceObject),
            40
        );
        assert_eq!(offset_of!(VSS_SNAPSHOT_PROP, m_tsCreationTimestamp), 112);
        assert_eq!(size_of::<VSS_SNAPSHOT_PROP>(), 128);
    }

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
