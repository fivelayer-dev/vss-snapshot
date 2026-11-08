//! Which volume a path is on, and where each volume is mounted.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::{ERROR_MORE_DATA, MAX_PATH};
use windows::Win32::Storage::FileSystem::{
    GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, GetVolumePathNamesForVolumeNameW,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;

use super::wide::{from_wide, split_multi, to_wide};

/// Microsoft documents 50 characters as enough for a volume GUID path, with its nul.
const GUID_PATH_LEN: usize = 50;

/// Expands environment variables such as `%SystemRoot%` in `text`.
pub(crate) fn expand_environment(text: &str) -> io::Result<String> {
    let source = to_wide(text);
    let mut buffer = vec![0u16; source.len().max(256)];
    loop {
        // SAFETY: `source` is nul-terminated and lives until the call returns. The call
        // writes no more than `buffer.len()` units, which the slice passes as its size.
        let needed =
            unsafe { ExpandEnvironmentStringsW(PCWSTR(source.as_ptr()), Some(&mut buffer)) };
        let needed = needed as usize;
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        if needed <= buffer.len() {
            return Ok(from_wide(&buffer));
        }
        buffer.resize(needed, 0);
    }
}

/// The volume holding `path`, as a drive root such as `C:\`, or as a GUID path such as
/// `\\?\Volume{...}\` for a volume mounted only on a folder. Environment variables in the
/// path are expanded first. The path needn't exist.
pub(crate) fn volume_of_path(path: &str) -> io::Result<String> {
    let path = to_wide(&expand_environment(path)?);
    let mut root = vec![0u16; path.len().max(MAX_PATH as usize + 1)];
    // SAFETY: `path` is nul-terminated and lives until the call returns. The call writes no
    // more than `root.len()` units.
    unsafe { GetVolumePathNameW(PCWSTR(path.as_ptr()), &mut root) }?;
    let root = from_wide(&root);
    if is_drive_root(&root) {
        return Ok(root.to_uppercase());
    }
    guid_path_of(&root)
}

/// The GUID path, `\\?\Volume{...}\`, of the volume mounted at `mount_point`, which must end
/// in a backslash. It can be a drive root, a mounted folder, or a GUID path.
pub(crate) fn guid_path_of(mount_point: &str) -> io::Result<String> {
    let mount_point = to_wide(mount_point);
    let mut name = [0u16; GUID_PATH_LEN];
    // SAFETY: `mount_point` is nul-terminated and lives until the call returns. The call
    // writes no more than `name.len()` units.
    unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(mount_point.as_ptr()), &mut name) }?;
    Ok(from_wide(&name))
}

/// Every drive root and folder the volume is mounted on, such as `C:\` and `D:\data\`. The
/// volume is named by its GUID path, ending in a backslash. Empty for a volume that isn't
/// mounted anywhere.
pub(crate) fn mount_points(guid_path: &str) -> io::Result<Vec<PathBuf>> {
    let name = to_wide(guid_path);
    let more_data = HRESULT::from_win32(ERROR_MORE_DATA.0);
    let mut buffer = vec![0u16; 256];
    loop {
        let mut needed = 0u32;
        // SAFETY: `name` is nul-terminated and lives until the call returns. The call writes
        // no more than `buffer.len()` units, and one `u32` to `needed`, which outlives it.
        let result = unsafe {
            GetVolumePathNamesForVolumeNameW(PCWSTR(name.as_ptr()), Some(&mut buffer), &mut needed)
        };
        match result {
            Ok(()) => break,
            Err(error) if error.code() == more_data => {
                // Grow even if the size given is no bigger, so this can't loop forever.
                let grown = (needed as usize).max(buffer.len() * 2);
                buffer.resize(grown, 0);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(split_multi(&buffer)
        .map(|path| PathBuf::from(OsString::from_wide(path)))
        .collect())
}

/// Whether `root` is a drive root such as `C:\`.
fn is_drive_root(root: &str) -> bool {
    let mut chars = root.chars();
    matches!(
        (chars.next(), chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(':'), Some('\\'), None) if letter.is_ascii_alphabetic()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system_drive() -> String {
        format!(r"{}\", std::env::var("SystemDrive").unwrap().to_uppercase())
    }

    #[test]
    fn recognises_drive_roots() {
        assert!(is_drive_root(r"C:\"));
        assert!(is_drive_root(r"z:\"));
        assert!(!is_drive_root(r"C:"));
        assert!(!is_drive_root(r"C:\mnt\"));
        assert!(!is_drive_root(r"\\?\Volume{0}\"));
        assert!(!is_drive_root(r"1:\"));
    }

    // The rest only ask Windows about volumes and the environment, which changes nothing.
    #[test]
    fn expands_environment_variables() {
        let root = std::env::var("SystemRoot").unwrap();
        assert_eq!(
            expand_environment(r"%SystemRoot%\system32").unwrap(),
            format!(r"{root}\system32")
        );
        assert_eq!(expand_environment("plain").unwrap(), "plain");
    }

    #[test]
    fn expands_long_text() {
        let long = format!(r"{}{}", system_drive(), "x".repeat(2000));
        assert_eq!(expand_environment(&long).unwrap(), long);
    }

    #[test]
    fn the_system_drive_has_a_guid_path_and_is_mounted_on_its_letter() {
        let root = system_drive();
        let guid_path = guid_path_of(&root).unwrap();
        assert!(guid_path.starts_with(r"\\?\Volume{"), "{guid_path}");
        assert!(guid_path.ends_with(r"}\"), "{guid_path}");
        assert_eq!(guid_path_of(&guid_path).unwrap(), guid_path);

        let mounted = mount_points(&guid_path).unwrap();
        assert!(
            mounted
                .iter()
                .any(|p| p.to_str().is_some_and(|p| p.eq_ignore_ascii_case(&root))),
            "{mounted:?}"
        );
    }

    #[test]
    fn a_path_that_does_not_exist_is_on_its_drive() {
        let root = system_drive();
        let path = format!(r"{root}no\such\folder\for\vss-snapshot");
        assert_eq!(volume_of_path(&path).unwrap(), root);
    }

    #[test]
    fn an_unknown_volume_is_an_error() {
        let unknown = r"\\?\Volume{00000000-0000-0000-0000-000000000000}\";
        assert!(guid_path_of(unknown).is_err());
    }
}
