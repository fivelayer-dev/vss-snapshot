//! VSS declarations the `windows` crate doesn't have, from `vsbackup.h` in the Windows SDK.
//!
//! Methods that aren't called are still declared, so that each one after them lands in its
//! vtable slot. The tests at the end check every slot and struct layout. Interfaces that are
//! never queried for are given a nil IID, as theirs is never used.
#![allow(non_snake_case, dead_code, clippy::too_many_arguments)]

use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::core::{interface, IUnknown, IUnknown_Vtbl, Result, BOOL, BSTR, GUID, HRESULT};
use windows::Win32::Storage::Vss::{
    IVssAsync, IVssWMFiledesc, VSS_BACKUP_TYPE, VSS_FILE_RESTORE_STATUS, VSS_OBJECT_TYPE,
    VSS_SNAPSHOT_PROP,
};

/// `IVssBackupComponents`, up to `GetSnapshotProperties`. Later methods are never called,
/// so they're left out.
#[interface("665c1d5f-c218-414d-a05d-7fef5f9d5c86")]
pub(in crate::sys) unsafe trait IVssBackupComponents: IUnknown {
    pub(in crate::sys) fn GetWriterComponentsCount(&self, count: *mut u32) -> Result<()>;
    /// Gives an `IVssWriterComponentsExt`, which `metadata::WriterComponents` wraps.
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
        metadata: *mut Option<IVssExamineWriterMetadata>,
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
    pub(in crate::sys) fn PrepareForBackup(&self, pending: *mut Option<IVssAsync>) -> Result<()>;
    pub(in crate::sys) fn AbortBackup(&self) -> Result<()>;
    pub(in crate::sys) fn GatherWriterStatus(&self, pending: *mut Option<IVssAsync>) -> Result<()>;
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
    pub(in crate::sys) fn ImportSnapshots(&self, pending: *mut Option<IVssAsync>) -> Result<()>;
    pub(in crate::sys) fn BreakSnapshotSet(&self, set: GUID) -> Result<()>;
    pub(in crate::sys) fn GetSnapshotProperties(
        &self,
        snapshot: GUID,
        properties: *mut VSS_SNAPSHOT_PROP,
    ) -> Result<()>;
}

/// `IVssExamineWriterMetadata`, up to `GetRestoreMethod`: what one writer says it looks
/// after.
#[interface("00000000-0000-0000-0000-000000000000")]
pub(in crate::sys) unsafe trait IVssExamineWriterMetadata: IUnknown {
    pub(in crate::sys) fn GetIdentity(
        &self,
        instance: *mut GUID,
        writer: *mut GUID,
        name: *mut BSTR,
        usage: *mut i32,
        source: *mut i32,
    ) -> Result<()>;
    pub(in crate::sys) fn GetFileCounts(
        &self,
        includes: *mut u32,
        excludes: *mut u32,
        components: *mut u32,
    ) -> Result<()>;
    pub(in crate::sys) fn GetIncludeFile(
        &self,
        index: u32,
        file: *mut Option<IVssWMFiledesc>,
    ) -> Result<()>;
    pub(in crate::sys) fn GetExcludeFile(
        &self,
        index: u32,
        file: *mut Option<IVssWMFiledesc>,
    ) -> Result<()>;
    pub(in crate::sys) fn GetComponent(
        &self,
        index: u32,
        component: *mut Option<IVssWMComponent>,
    ) -> Result<()>;
    pub(in crate::sys) fn GetRestoreMethod(
        &self,
        method: *mut i32,
        service: *mut BSTR,
        user_procedure: *mut BSTR,
        writer_restore: *mut i32,
        reboot_required: *mut bool,
        mappings: *mut u32,
    ) -> Result<()>;
}

/// `IVssWMComponent`: one component in a writer's metadata.
#[interface("00000000-0000-0000-0000-000000000000")]
pub(in crate::sys) unsafe trait IVssWMComponent: IUnknown {
    pub(in crate::sys) fn GetComponentInfo(&self, info: *mut *const ComponentInfo) -> Result<()>;
    pub(in crate::sys) fn FreeComponentInfo(&self, info: *const ComponentInfo) -> Result<()>;
    pub(in crate::sys) fn GetFile(
        &self,
        index: u32,
        file: *mut Option<IVssWMFiledesc>,
    ) -> Result<()>;
    pub(in crate::sys) fn GetDatabaseFile(
        &self,
        index: u32,
        file: *mut Option<IVssWMFiledesc>,
    ) -> Result<()>;
    pub(in crate::sys) fn GetDatabaseLogFile(
        &self,
        index: u32,
        file: *mut Option<IVssWMFiledesc>,
    ) -> Result<()>;
}

/// `VSS_COMPONENTINFO`. The strings are BSTRs owned by VSS until `FreeComponentInfo`.
#[repr(C)]
pub(in crate::sys) struct ComponentInfo {
    pub(in crate::sys) kind: i32,
    pub(in crate::sys) logical_path: *const u16,
    pub(in crate::sys) name: *const u16,
    pub(in crate::sys) caption: *const u16,
    pub(in crate::sys) icon: *const u8,
    pub(in crate::sys) icon_bytes: u32,
    pub(in crate::sys) restore_metadata: bool,
    pub(in crate::sys) notify_on_backup_complete: bool,
    pub(in crate::sys) selectable: bool,
    pub(in crate::sys) selectable_for_restore: bool,
    pub(in crate::sys) flags: u32,
    pub(in crate::sys) file_count: u32,
    pub(in crate::sys) database_count: u32,
    pub(in crate::sys) log_file_count: u32,
    pub(in crate::sys) dependency_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    fn slot(offset: usize) -> usize {
        offset / size_of::<usize>()
    }

    /// A method in the wrong slot calls a different method, and nothing would catch that
    /// before a real snapshot crashed. These are the positions in `vsbackup.h`.
    #[test]
    fn backup_components_methods_are_in_their_slots() {
        use IVssBackupComponents_Vtbl as V;
        let slots = [
            (offset_of!(V, GetWriterComponentsCount), 3),
            (offset_of!(V, GetWriterComponents), 4),
            (offset_of!(V, InitializeForBackup), 5),
            (offset_of!(V, SetBackupState), 6),
            (offset_of!(V, InitializeForRestore), 7),
            (offset_of!(V, SetRestoreState), 8),
            (offset_of!(V, GatherWriterMetadata), 9),
            (offset_of!(V, GetWriterMetadataCount), 10),
            (offset_of!(V, GetWriterMetadata), 11),
            (offset_of!(V, FreeWriterMetadata), 12),
            (offset_of!(V, AddComponent), 13),
            (offset_of!(V, PrepareForBackup), 14),
            (offset_of!(V, AbortBackup), 15),
            (offset_of!(V, GatherWriterStatus), 16),
            (offset_of!(V, GetWriterStatusCount), 17),
            (offset_of!(V, FreeWriterStatus), 18),
            (offset_of!(V, GetWriterStatus), 19),
            (offset_of!(V, SetBackupSucceeded), 20),
            (offset_of!(V, SetBackupOptions), 21),
            (offset_of!(V, SetSelectedForRestore), 22),
            (offset_of!(V, SetRestoreOptions), 23),
            (offset_of!(V, SetAdditionalRestores), 24),
            (offset_of!(V, SetPreviousBackupStamp), 25),
            (offset_of!(V, SaveAsXML), 26),
            (offset_of!(V, BackupComplete), 27),
            (offset_of!(V, AddAlternativeLocationMapping), 28),
            (offset_of!(V, AddRestoreSubcomponent), 29),
            (offset_of!(V, SetFileRestoreStatus), 30),
            (offset_of!(V, AddNewTarget), 31),
            (offset_of!(V, SetRangesFilePath), 32),
            (offset_of!(V, PreRestore), 33),
            (offset_of!(V, PostRestore), 34),
            (offset_of!(V, SetContext), 35),
            (offset_of!(V, StartSnapshotSet), 36),
            (offset_of!(V, AddToSnapshotSet), 37),
            (offset_of!(V, DoSnapshotSet), 38),
            (offset_of!(V, DeleteSnapshots), 39),
            (offset_of!(V, ImportSnapshots), 40),
            (offset_of!(V, BreakSnapshotSet), 41),
            (offset_of!(V, GetSnapshotProperties), 42),
        ];
        for (index, (offset, expected)) in slots.into_iter().enumerate() {
            assert_eq!(slot(offset), expected, "entry {index}");
        }
        assert_eq!(size_of::<V>(), 43 * size_of::<usize>());
    }

    #[test]
    fn writer_metadata_methods_are_in_their_slots() {
        use IVssExamineWriterMetadata_Vtbl as V;
        assert_eq!(slot(offset_of!(V, GetIdentity)), 3);
        assert_eq!(slot(offset_of!(V, GetFileCounts)), 4);
        assert_eq!(slot(offset_of!(V, GetIncludeFile)), 5);
        assert_eq!(slot(offset_of!(V, GetExcludeFile)), 6);
        assert_eq!(slot(offset_of!(V, GetComponent)), 7);
        assert_eq!(slot(offset_of!(V, GetRestoreMethod)), 8);
    }

    #[test]
    fn component_methods_are_in_their_slots() {
        use IVssWMComponent_Vtbl as V;
        assert_eq!(slot(offset_of!(V, GetComponentInfo)), 3);
        assert_eq!(slot(offset_of!(V, FreeComponentInfo)), 4);
        assert_eq!(slot(offset_of!(V, GetFile)), 5);
        assert_eq!(slot(offset_of!(V, GetDatabaseFile)), 6);
        assert_eq!(slot(offset_of!(V, GetDatabaseLogFile)), 7);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn component_info_matches_the_c_layout() {
        assert_eq!(offset_of!(ComponentInfo, logical_path), 8);
        assert_eq!(offset_of!(ComponentInfo, name), 16);
        assert_eq!(offset_of!(ComponentInfo, caption), 24);
        assert_eq!(offset_of!(ComponentInfo, icon), 32);
        assert_eq!(offset_of!(ComponentInfo, icon_bytes), 40);
        assert_eq!(offset_of!(ComponentInfo, restore_metadata), 44);
        assert_eq!(offset_of!(ComponentInfo, selectable), 46);
        assert_eq!(offset_of!(ComponentInfo, selectable_for_restore), 47);
        assert_eq!(offset_of!(ComponentInfo, flags), 48);
        assert_eq!(offset_of!(ComponentInfo, file_count), 52);
        assert_eq!(offset_of!(ComponentInfo, dependency_count), 64);
        assert_eq!(size_of::<ComponentInfo>(), 72);
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
}
