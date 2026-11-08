//! Reading what writers report: their metadata, and the components a backup document holds.

use std::ffi::c_void;

use windows::core::{IUnknown, Interface, Result as WinResult, BSTR, GUID, PCWSTR};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Storage::Vss::{IVssWMFiledesc, IVssWriterComponents, VSS_COMPONENT_TYPE};

use super::raw::{ComponentInfo, IVssExamineWriterMetadata, IVssWMComponent};
use super::{failed, string_at, to_guid};
use crate::error::{Error, Result};
use crate::hresult::HResult;
use crate::sys::wide::to_wide;
use crate::writer::{Component, ComponentId, ComponentKind, FileSpec, RestoreMethod, WriterInfo};

/// A writer's metadata, with the instance id that only this session can use.
pub(crate) struct GatheredWriter {
    pub(crate) instance: GUID,
    pub(crate) info: WriterInfo,
}

/// A component chosen for a backup or restore, with what VSS needs to refer to it.
pub(crate) struct Selected {
    pub(crate) id: ComponentId,
    pub(super) instance: GUID,
    pub(super) writer: GUID,
    pub(super) kind: i32,
    logical_path: Vec<u16>,
    name: Vec<u16>,
}

impl Selected {
    pub(crate) fn new(id: ComponentId, instance: GUID, kind: i32) -> Selected {
        Selected {
            writer: GUID::from_u128(id.writer_id.to_u128()),
            logical_path: to_wide(&id.logical_path),
            name: to_wide(&id.name),
            id,
            instance,
            kind,
        }
    }

    /// VSS expects null rather than an empty string when there's no logical path.
    pub(super) fn logical_path(&self) -> PCWSTR {
        if self.id.logical_path.is_empty() {
            PCWSTR::null()
        } else {
            PCWSTR(self.logical_path.as_ptr())
        }
    }

    pub(super) fn name(&self) -> PCWSTR {
        PCWSTR(self.name.as_ptr())
    }
}

/// Finds a wanted component among the writers gathered in this session.
pub(crate) fn find_component(writers: &[GatheredWriter], wanted: &ComponentId) -> Option<Selected> {
    writers
        .iter()
        .filter(|writer| writer.info.id == wanted.writer_id)
        .find_map(|writer| {
            let component = writer
                .info
                .components
                .iter()
                .find(|c| c.name == wanted.name && c.logical_path == wanted.logical_path)?;
            Some(Selected::new(
                wanted.clone(),
                writer.instance,
                component.kind.to_raw(),
            ))
        })
}

/// Calls a method that hands back an interface, and takes it.
pub(super) fn take<T>(
    operation: &'static str,
    call: impl FnOnce(*mut Option<T>) -> WinResult<()>,
) -> Result<T> {
    let mut value = None;
    call(&mut value).map_err(failed(operation))?;
    value.ok_or(Error::Vss {
        operation,
        code: HResult::from_code(E_POINTER.0),
    })
}

pub(super) fn read_writer(metadata: &IVssExamineWriterMetadata) -> Result<GatheredWriter> {
    let (mut instance, mut writer) = (GUID::zeroed(), GUID::zeroed());
    let mut name = BSTR::new();
    let (mut usage, mut source) = (0, 0);
    // SAFETY: every out-pointer outlives the call, and `name` frees the string it receives.
    unsafe {
        metadata.GetIdentity(
            &mut instance,
            &mut writer,
            &mut name,
            &mut usage,
            &mut source,
        )
    }
    .map_err(failed("GetIdentity"))?;

    let (mut includes, mut excludes, mut components) = (0, 0, 0);
    // SAFETY: every out-pointer outlives the call.
    unsafe { metadata.GetFileCounts(&mut includes, &mut excludes, &mut components) }
        .map_err(failed("GetFileCounts"))?;
    let excluded_files = (0..excludes)
        .map(|index| {
            // SAFETY: `file` is a valid out-pointer, and `index` is in range.
            read_file(&take("GetExcludeFile", |file| unsafe {
                metadata.GetExcludeFile(index, file)
            })?)
        })
        .collect::<Result<Vec<_>>>()?;
    let components = (0..components)
        .map(|index| {
            // SAFETY: `component` is a valid out-pointer, and `index` is in range.
            read_component(&take("GetComponent", |component| unsafe {
                metadata.GetComponent(index, component)
            })?)
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(GatheredWriter {
        instance,
        info: WriterInfo {
            id: to_guid(writer),
            name: name.to_string(),
            restore_method: read_restore_method(metadata)?,
            components,
            excluded_files,
        },
    })
}

fn read_restore_method(metadata: &IVssExamineWriterMetadata) -> Result<RestoreMethod> {
    let (mut method, mut writer_restore, mut mappings) = (0, 0, 0);
    let (mut service, mut procedure) = (BSTR::new(), BSTR::new());
    let mut reboot = false;
    // SAFETY: every out-pointer outlives the call, and the BSTRs free what they receive.
    unsafe {
        metadata.GetRestoreMethod(
            &mut method,
            &mut service,
            &mut procedure,
            &mut writer_restore,
            &mut reboot,
            &mut mappings,
        )
    }
    .map_err(failed("GetRestoreMethod"))?;
    Ok(RestoreMethod::from_raw(method))
}

fn read_file(file: &IVssWMFiledesc) -> Result<FileSpec> {
    // SAFETY: `file` is a live `IVssWMFiledesc`.
    let path = unsafe { file.GetPath() }.map_err(failed("GetPath"))?;
    // SAFETY: as above.
    let filespec = unsafe { file.GetFilespec() }.map_err(failed("GetFilespec"))?;
    // SAFETY: as above.
    let recursive = unsafe { file.GetRecursive() }.map_err(failed("GetRecursive"))?;
    Ok(FileSpec {
        path: path.to_string(),
        filespec: filespec.to_string(),
        recursive,
    })
}

fn read_component(component: &IVssWMComponent) -> Result<Component> {
    let mut info: *const ComponentInfo = std::ptr::null();
    // SAFETY: `info` outlives the call.
    unsafe { component.GetComponentInfo(&mut info) }.map_err(failed("GetComponentInfo"))?;
    // SAFETY: on success `info` is null or points to a component info that VSS owns until
    // it's freed below.
    let Some(details) = (unsafe { info.as_ref() }) else {
        return Err(Error::Vss {
            operation: "GetComponentInfo",
            code: HResult::from_code(E_POINTER.0),
        });
    };
    let read = read_component_info(component, details);
    // SAFETY: frees the info from `GetComponentInfo`, once, after its last use above.
    let freed = unsafe { component.FreeComponentInfo(info) }.map_err(failed("FreeComponentInfo"));
    let read = read?;
    freed?;
    Ok(read)
}

fn read_component_info(component: &IVssWMComponent, info: &ComponentInfo) -> Result<Component> {
    type GetFile<'a> = &'a dyn Fn(u32, *mut Option<IVssWMFiledesc>) -> WinResult<()>;
    let files = |operation: &'static str, count: u32, get: GetFile<'_>| {
        (0..count)
            .map(|index| read_file(&take(operation, |file| get(index, file))?))
            .collect::<Result<Vec<_>>>()
    };
    // SAFETY: the strings are BSTRs, each nul-terminated or null, which VSS keeps alive
    // until `FreeComponentInfo`.
    let (logical_path, name, caption) = unsafe {
        (
            string_at(info.logical_path),
            string_at(info.name),
            string_at(info.caption),
        )
    };
    Ok(Component {
        kind: ComponentKind::from_raw(info.kind),
        logical_path,
        name,
        caption,
        selectable: info.selectable,
        selectable_for_restore: info.selectable_for_restore,
        // SAFETY (each closure): `file` is a valid out-pointer and `index` is in range.
        files: files("GetFile", info.file_count, &|index, file| unsafe {
            component.GetFile(index, file)
        })?,
        database_files: files(
            "GetDatabaseFile",
            info.database_count,
            &|index, file| unsafe { component.GetDatabaseFile(index, file) },
        )?,
        log_files: files(
            "GetDatabaseLogFile",
            info.log_file_count,
            &|index, file| unsafe { component.GetDatabaseLogFile(index, file) },
        )?,
    })
}

/// An `IVssWriterComponentsExt`, one writer's part of a backup document.
///
/// The `windows` crate's `IVssWriterComponents` never releases it, as that interface doesn't
/// derive from `IUnknown`. The object also implements `IUnknown`, through a second vtable
/// pointer straight after the first, so it's released through that.
struct WriterComponents(IVssWriterComponents);

impl Drop for WriterComponents {
    fn drop(&mut self) {
        // SAFETY: the object's second pointer-sized field is its `IUnknown` vtable pointer.
        // Taking it as an `IUnknown` and dropping it releases the reference that
        // `GetWriterComponents` handed over, once.
        unsafe {
            let unknown = self
                .0
                .as_raw()
                .cast::<*mut c_void>()
                .add(1)
                .cast::<c_void>();
            drop(IUnknown::from_raw(unknown));
        }
    }
}

/// The components in one writer's part of a backup document that were backed up
/// successfully. Takes the reference `GetWriterComponents` handed over in `raw`.
pub(super) fn read_document_writer(raw: *mut c_void) -> Result<Vec<Selected>> {
    if raw.is_null() {
        return Err(Error::Vss {
            operation: "GetWriterComponents",
            code: HResult::from_code(E_POINTER.0),
        });
    }
    // SAFETY: `raw` is a live `IVssWriterComponentsExt`, whose first vtable is
    // `IVssWriterComponents`. `WriterComponents` releases it when dropped.
    let writer_components = WriterComponents(unsafe { IVssWriterComponents::from_raw(raw) });
    let components = &writer_components.0;

    let (mut instance, mut writer) = (GUID::zeroed(), GUID::zeroed());
    // SAFETY: both out-pointers outlive the call.
    unsafe { components.GetWriterInfo(&mut instance, &mut writer) }
        .map_err(failed("GetWriterInfo"))?;
    let mut count = 0;
    // SAFETY: `count` outlives the call.
    unsafe { components.GetComponentCount(&mut count) }.map_err(failed("GetComponentCount"))?;

    let mut found = Vec::new();
    for index in 0..count {
        // SAFETY: `index` is in range. The component is released when dropped.
        let component =
            unsafe { components.GetComponent(index) }.map_err(failed("GetComponent"))?;
        let mut succeeded = false;
        // SAFETY: `succeeded` outlives the call.
        unsafe { component.GetBackupSucceeded(&mut succeeded) }
            .map_err(failed("GetBackupSucceeded"))?;
        if !succeeded {
            continue;
        }
        let (mut logical_path, mut name) = (BSTR::new(), BSTR::new());
        let mut kind = VSS_COMPONENT_TYPE(0);
        // SAFETY: each out-pointer outlives its call, and the BSTRs free what they receive.
        unsafe {
            component
                .GetLogicalPath(&mut logical_path)
                .map_err(failed("GetLogicalPath"))?;
            component
                .GetComponentType(&mut kind)
                .map_err(failed("GetComponentType"))?;
            component
                .GetComponentName(&mut name)
                .map_err(failed("GetComponentName"))?;
        }
        let id = ComponentId::new(to_guid(writer), logical_path.to_string(), name.to_string());
        found.push(Selected::new(id, instance, kind.0));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::Guid;

    const SQL_WRITER: Guid = Guid::from_u128(0xa65faa63_5ea8_4ebc_9dbd_a0c4db26912a);

    fn sql_writer() -> GatheredWriter {
        let component = |name: &str| Component {
            kind: ComponentKind::FileGroup,
            logical_path: r"HOST\SQLEXPRESS".to_owned(),
            name: name.to_owned(),
            caption: String::new(),
            selectable: true,
            selectable_for_restore: true,
            files: Vec::new(),
            database_files: Vec::new(),
            log_files: Vec::new(),
        };
        GatheredWriter {
            instance: GUID::from_u128(7),
            info: WriterInfo {
                id: SQL_WRITER,
                name: "SqlServerWriter".to_owned(),
                restore_method: RestoreMethod::RestoreIfCanReplace,
                components: vec![component("master"), component("sales")],
                excluded_files: Vec::new(),
            },
        }
    }

    #[test]
    fn finds_a_component_with_this_sessions_instance() {
        let wanted = ComponentId::new(SQL_WRITER, r"HOST\SQLEXPRESS", "sales");
        let found = find_component(&[sql_writer()], &wanted).unwrap();
        assert_eq!(found.instance, GUID::from_u128(7));
        assert_eq!(found.writer.to_u128(), SQL_WRITER.to_u128());
        assert_eq!(found.kind, 2);
        assert_eq!(found.id, wanted);
        assert_eq!(found.name, to_wide("sales"));
        assert_eq!(found.logical_path().0, found.logical_path.as_ptr());
    }

    #[test]
    fn a_component_must_match_writer_path_and_name() {
        let writers = [sql_writer()];
        for wanted in [
            ComponentId::new(Guid::from_u128(1), r"HOST\SQLEXPRESS", "sales"),
            ComponentId::new(SQL_WRITER, r"OTHER\SQLEXPRESS", "sales"),
            ComponentId::new(SQL_WRITER, r"HOST\SQLEXPRESS", "Sales"),
            ComponentId::new(SQL_WRITER, "", "sales"),
        ] {
            assert!(find_component(&writers, &wanted).is_none(), "{wanted}");
        }
    }

    #[test]
    fn an_empty_logical_path_is_passed_as_null() {
        let selected = Selected::new(ComponentId::new(SQL_WRITER, "", "sales"), GUID::zeroed(), 2);
        assert!(selected.logical_path().is_null());
        assert!(!selected.name().is_null());
    }
}
