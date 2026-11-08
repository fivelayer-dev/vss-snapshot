use std::ffi::{OsStr, OsString};
use std::path::{Component as PathComponent, Path, PathBuf, Prefix};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::guid::Guid;

/// One volume's snapshot, part of a [`SnapshotSet`](crate::SnapshotSet).
///
/// Its paths stay valid until the set is closed, completed or dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeSnapshot {
    id: Guid,
    name: String,
    guid_path: String,
    mount_points: Vec<PathBuf>,
    device_path: PathBuf,
    created: SystemTime,
    /// Every root a live path on this volume can start from: the name it was requested by,
    /// its GUID path and its mount points, without duplicates.
    roots: Vec<MountRoot>,
}

impl VolumeSnapshot {
    pub(crate) fn new(
        id: Guid,
        name: String,
        guid_path: String,
        mount_points: Vec<PathBuf>,
        device_path: &str,
        created: SystemTime,
    ) -> VolumeSnapshot {
        let mut roots: Vec<MountRoot> = Vec::new();
        let candidates = [Path::new(&name), Path::new(&guid_path)]
            .into_iter()
            .chain(mount_points.iter().map(PathBuf::as_path));
        for root in candidates.filter_map(MountRoot::parse) {
            if !roots.iter().any(|r| r.same_as(&root)) {
                roots.push(root);
            }
        }
        VolumeSnapshot {
            id,
            name,
            guid_path,
            mount_points,
            device_path: PathBuf::from(format!("{}\\", device_path.trim_end_matches('\\'))),
            created,
            roots,
        }
    }

    /// The snapshot's VSS id.
    pub fn id(&self) -> Guid {
        self.id
    }

    /// The volume as it was first named, normalised to end in a backslash: `C:\`,
    /// `C:\mnt\data\` or `\\?\Volume{...}\`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The volume's GUID path, `\\?\Volume{...}\`. Unlike a drive letter it can't change, so
    /// use it to identify the volume in saved state.
    pub fn guid_path(&self) -> &str {
        &self.guid_path
    }

    /// Every drive root and folder the volume was mounted on when the snapshot was taken,
    /// such as `C:\` and `D:\data\`.
    pub fn mount_points(&self) -> &[PathBuf] {
        &self.mount_points
    }

    /// The root of the snapshot, such as
    /// `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy7\`, always ending in a backslash.
    ///
    /// Open files under it to read them as they were. It's valid until the snapshot set is
    /// removed.
    pub fn device_path(&self) -> &Path {
        &self.device_path
    }

    /// When VSS took the snapshot.
    pub fn created(&self) -> SystemTime {
        self.created
    }

    /// Turns a path on the live volume into the same path inside this snapshot, as
    /// `C:\docs\a.txt` into `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy7\docs\a.txt`.
    ///
    /// The path must be absolute, and start with one of the volume's mount points or its GUID
    /// path, in any case. Returns `None` otherwise, or if the path contains `..`.
    ///
    /// This is purely about names: it doesn't check that the file exists, or follow links.
    /// A folder that another volume is mounted on is empty in this snapshot, so paths through
    /// it won't be found. `SnapshotSet::path_in_snapshot` picks the right volume.
    pub fn path_in_snapshot(&self, path: &Path) -> Option<PathBuf> {
        let path = SplitPath::parse(path)?;
        let matched = self.longest_match(&path)?;
        Some(self.map(&path, matched))
    }

    /// Whether `name` is a name for this volume: one of its mount points, or its GUID path.
    pub(crate) fn is_named(&self, name: &str) -> bool {
        let Ok(name) = normalize_volume_name(name) else {
            return false;
        };
        let Some(root) = MountRoot::parse(Path::new(&name)) else {
            return false;
        };
        self.roots.iter().any(|r| r.same_as(&root))
    }

    /// How many names of `path` the longest matching root of this volume covers.
    pub(crate) fn longest_match(&self, path: &SplitPath<'_>) -> Option<usize> {
        self.roots.iter().filter_map(|r| r.covers(path)).max()
    }

    /// `path` inside the snapshot, after its first `matched` names.
    pub(crate) fn map(&self, path: &SplitPath<'_>, matched: usize) -> PathBuf {
        let mut mapped = self.device_path.clone();
        for name in path.names.iter().skip(matched) {
            mapped.push(name);
        }
        mapped
    }
}

/// Where an absolute path starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Root {
    /// A drive letter, in upper case.
    Drive(u8),
    /// A volume GUID path.
    Volume(Guid),
}

/// An absolute path, split into where it starts and the names after that.
#[derive(Debug)]
pub(crate) struct SplitPath<'a> {
    root: Root,
    names: Vec<&'a OsStr>,
}

impl SplitPath<'_> {
    /// Splits an absolute path such as `C:\docs\a.txt` or `\\?\Volume{...}\docs`. Returns
    /// `None` for a relative path, a network path, or one containing `..`.
    pub(crate) fn parse(path: &Path) -> Option<SplitPath<'_>> {
        let (root, has_root_dir, names) = split(path)?;
        has_root_dir.then_some(SplitPath { root, names })
    }
}

/// Splits a path into its root, whether a backslash follows the root, and the names after
/// it. `C:` gives no backslash and no names. `.` is skipped.
fn split(path: &Path) -> Option<(Root, bool, Vec<&OsStr>)> {
    let mut components = path.components();
    let root = match components.next()? {
        PathComponent::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) if letter.is_ascii_alphabetic() => {
                Root::Drive(letter.to_ascii_uppercase())
            }
            Prefix::Verbatim(name) => Root::Volume(volume_guid(name)?),
            _ => return None,
        },
        _ => return None,
    };
    let mut has_root_dir = false;
    let mut names = Vec::new();
    for component in components {
        match component {
            PathComponent::RootDir => has_root_dir = true,
            PathComponent::Normal(name) if has_root_dir => names.push(name),
            PathComponent::CurDir => {}
            _ => return None,
        }
    }
    Some((root, has_root_dir, names))
}

/// The GUID in a verbatim prefix such as `Volume{...}`, in any case.
fn volume_guid(prefix: &OsStr) -> Option<Guid> {
    let text = prefix.to_str()?;
    let (head, guid) = text.split_at_checked("Volume".len())?;
    if !head.eq_ignore_ascii_case("Volume") || !guid.starts_with('{') {
        return None;
    }
    guid.parse().ok()
}

/// A place a volume is mounted, such as `C:\`, `D:\data\` or its GUID path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MountRoot {
    root: Root,
    names: Vec<OsString>,
}

impl MountRoot {
    fn parse(path: &Path) -> Option<MountRoot> {
        let (root, _, names) = split(path)?;
        Some(MountRoot {
            root,
            names: names.into_iter().map(OsStr::to_os_string).collect(),
        })
    }

    /// If `path` is at or under this mount point, how many of its names this covers.
    fn covers(&self, path: &SplitPath<'_>) -> Option<usize> {
        let covered = self.root == path.root
            && self.names.len() <= path.names.len()
            && self
                .names
                .iter()
                .zip(&path.names)
                .all(|(a, b)| same_name(a, b));
        covered.then_some(self.names.len())
    }

    fn same_as(&self, other: &MountRoot) -> bool {
        self.root == other.root
            && self.names.len() == other.names.len()
            && self
                .names
                .iter()
                .zip(&other.names)
                .all(|(a, b)| same_name(a, b))
    }
}

/// Whether two file names are the same, ignoring case as Windows does for most names.
fn same_name(a: &OsStr, b: &OsStr) -> bool {
    match (a.to_str(), b.to_str()) {
        (Some(a), Some(b)) => a
            .chars()
            .flat_map(char::to_lowercase)
            .eq(b.chars().flat_map(char::to_lowercase)),
        _ => a == b,
    }
}

/// Parses a volume name into the form VSS expects, which always ends in a backslash: a
/// drive (`C`, `C:`, `C:\` to `C:\`), a GUID path (`\\?\Volume{...}`, with or without the
/// backslash), or a folder a volume is mounted on (`C:\mnt\data` to `C:\mnt\data\`).
///
/// This only checks the form of the name. Whether a volume is there is found out later.
pub(crate) fn normalize_volume_name(name: &str) -> Result<String> {
    let invalid = || Error::InvalidVolume {
        name: name.to_owned(),
    };
    let mut chars = name.chars();
    if let (Some(letter), None) = (chars.next(), chars.next()) {
        return if letter.is_ascii_alphabetic() {
            Ok(format!("{}:\\", letter.to_ascii_uppercase()))
        } else {
            Err(invalid())
        };
    }
    let (root, _, names) = split(Path::new(name)).ok_or_else(invalid)?;
    let mut normalized = match root {
        Root::Drive(letter) => format!("{}:\\", char::from(letter)),
        Root::Volume(guid) => format!(r"\\?\Volume{guid}\"),
    };
    for part in names {
        normalized.push_str(part.to_str().ok_or_else(invalid)?);
        normalized.push('\\');
    }
    Ok(normalized)
}

/// The time in a Windows `FILETIME`: 100-nanosecond intervals since 1601. Out-of-range
/// values give the Unix epoch.
pub(crate) fn system_time_from_filetime(filetime: i64) -> SystemTime {
    /// 100-nanosecond intervals between 1601 and 1970.
    const UNIX_EPOCH_AS_FILETIME: i64 = 116_444_736_000_000_000;
    let Some(since_unix) = filetime.checked_sub(UNIX_EPOCH_AS_FILETIME) else {
        return UNIX_EPOCH;
    };
    let offset = Duration::from_nanos(since_unix.unsigned_abs().saturating_mul(100));
    let time = if since_unix >= 0 {
        UNIX_EPOCH.checked_add(offset)
    } else {
        UNIX_EPOCH.checked_sub(offset)
    };
    time.unwrap_or(UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: &str = r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\";
    const DEVICE: &str = r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy7";

    fn snapshot(name: &str, mount_points: &[&str]) -> VolumeSnapshot {
        VolumeSnapshot::new(
            Guid::from_u128(7),
            name.to_owned(),
            GUID.to_owned(),
            mount_points.iter().map(PathBuf::from).collect(),
            DEVICE,
            UNIX_EPOCH,
        )
    }

    fn mapped(volume: &VolumeSnapshot, path: &str) -> Option<String> {
        volume
            .path_in_snapshot(Path::new(path))
            .map(|p| p.to_str().unwrap().to_owned())
    }

    #[test]
    fn normalizes_drive_names() {
        for name in ["C", "c", "C:", "c:", r"C:\", r"c:\", "C:/", r"\\?\C:\"] {
            assert_eq!(normalize_volume_name(name).unwrap(), r"C:\", "{name}");
        }
    }

    #[test]
    fn normalizes_guid_paths() {
        assert_eq!(normalize_volume_name(GUID).unwrap(), GUID);
        assert_eq!(
            normalize_volume_name(GUID.trim_end_matches('\\')).unwrap(),
            GUID
        );
        assert_eq!(
            normalize_volume_name(&GUID.to_uppercase().replace(r"\\?\VOLUME", r"\\?\Volume"))
                .unwrap(),
            GUID
        );
        assert_eq!(
            normalize_volume_name(&GUID.replace("Volume", "volume")).unwrap(),
            GUID
        );
    }

    #[test]
    fn normalizes_mount_folders() {
        assert_eq!(
            normalize_volume_name(r"c:\Mnt\Data").unwrap(),
            r"C:\Mnt\Data\"
        );
        assert_eq!(
            normalize_volume_name(r"C:\mnt\.\data\").unwrap(),
            r"C:\mnt\data\"
        );
        assert_eq!(
            normalize_volume_name("C:/mnt/data").unwrap(),
            r"C:\mnt\data\"
        );
        assert_eq!(
            normalize_volume_name(&format!(r"{GUID}mnt")).unwrap(),
            format!(r"{GUID}mnt\")
        );
    }

    #[test]
    fn rejects_other_names() {
        for name in [
            "",
            "1",
            "CC",
            r"\",
            r"\docs",
            "docs",
            r"C:docs",
            r"C:\..\docs",
            r"C:\docs\..",
            r"\\server\share",
            r"\\?\UNC\server\share",
            r"\\?\Volume{",
            r"\\?\Volume01234567-89ab-cdef-0123-456789abcdef\",
            r"\\?\Volum{01234567-89ab-cdef-0123-456789abcdef}\",
            r"\\.\C:",
        ] {
            match normalize_volume_name(name) {
                Err(Error::InvalidVolume { name: given }) => assert_eq!(given, name),
                other => panic!("{name:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn device_path_always_ends_in_a_backslash() {
        let expected = format!(r"{DEVICE}\");
        assert_eq!(snapshot(r"C:\", &[]).device_path(), Path::new(&expected));
        let volume = VolumeSnapshot::new(
            Guid::default(),
            r"C:\".to_owned(),
            GUID.to_owned(),
            Vec::new(),
            &expected,
            UNIX_EPOCH,
        );
        assert_eq!(volume.device_path().to_str().unwrap(), expected);
    }

    #[test]
    fn maps_a_drive_path_in_any_form() {
        let want = format!(r"{DEVICE}\docs\a.txt");
        let volume = snapshot(r"C:\", &[r"C:\"]);
        for path in [
            r"C:\docs\a.txt",
            r"c:\docs\a.txt",
            "C:/docs/a.txt",
            r"\\?\C:\docs\a.txt",
            r"C:\.\docs\a.txt",
        ] {
            assert_eq!(mapped(&volume, path).unwrap(), want, "{path}");
        }
    }

    #[test]
    fn maps_the_volume_root() {
        let volume = snapshot(r"C:\", &[]);
        assert_eq!(mapped(&volume, r"C:\").unwrap(), format!(r"{DEVICE}\"));
    }

    #[test]
    fn maps_by_guid_path_and_by_every_mount_point() {
        let want = format!(r"{DEVICE}\docs\a.txt");
        let volume = snapshot(GUID, &[r"E:\", r"C:\mnt\data\"]);
        assert_eq!(
            mapped(&volume, &format!(r"{GUID}docs\a.txt")).unwrap(),
            want
        );
        assert_eq!(
            mapped(&volume, &format!(r"{}docs\a.txt", GUID.to_uppercase())).unwrap(),
            want
        );
        assert_eq!(mapped(&volume, r"E:\docs\a.txt").unwrap(), want);
        assert_eq!(mapped(&volume, r"C:\MNT\Data\docs\a.txt").unwrap(), want);
    }

    #[test]
    fn a_mount_folder_does_not_match_a_longer_name() {
        let volume = snapshot(r"C:\mnt\data\", &[r"C:\mnt\data\"]);
        assert_eq!(mapped(&volume, r"C:\mnt\database\a.txt"), None);
        assert_eq!(mapped(&volume, r"C:\mnt\a.txt"), None);
    }

    #[test]
    fn prefers_the_longest_mount_point() {
        let volume = snapshot(r"C:\", &[r"C:\", r"C:\mnt\"]);
        let path = SplitPath::parse(Path::new(r"C:\mnt\a.txt")).unwrap();
        assert_eq!(volume.longest_match(&path), Some(1));
    }

    #[test]
    fn keeps_names_that_are_not_valid_unicode() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let mut path: Vec<u16> = r"C:\a".encode_utf16().collect();
        path.push(0xD800);
        let path = PathBuf::from(OsString::from_wide(&path));
        let out = snapshot(r"C:\", &[]).path_in_snapshot(&path).unwrap();
        assert_eq!(out.as_os_str().encode_wide().last(), Some(0xD800));
    }

    #[test]
    fn refuses_paths_elsewhere() {
        let volume = snapshot(r"C:\", &[r"C:\"]);
        for path in [
            r"D:\docs\a.txt",
            r"\\?\D:\docs\a.txt",
            r"\\?\Volume{ffffffff-89ab-cdef-0123-456789abcdef}\a.txt",
            r"\\server\share\a.txt",
            r"\docs\a.txt",
            r"docs\a.txt",
            "C:",
            r"C:docs\a.txt",
        ] {
            assert_eq!(mapped(&volume, path), None, "{path}");
        }
    }

    #[test]
    fn refuses_paths_that_climb() {
        let volume = snapshot(r"C:\", &[]);
        assert_eq!(mapped(&volume, r"C:\docs\..\a.txt"), None);
        assert_eq!(mapped(&volume, r"C:\..\a.txt"), None);
    }

    #[test]
    fn knows_its_names() {
        let volume = snapshot(r"C:\", &[r"C:\", r"D:\mnt\"]);
        for name in ["C", "c:", r"C:\", GUID, r"d:\MNT", r"D:\mnt\"] {
            assert!(volume.is_named(name), "{name}");
        }
        for name in ["D:", r"D:\mnt\x", "nonsense"] {
            assert!(!volume.is_named(name), "{name}");
        }
    }

    #[test]
    fn duplicate_roots_are_kept_once() {
        let volume = snapshot(r"C:\", &[r"c:\", r"C:\"]);
        assert_eq!(volume.roots.len(), 2, "{:?}", volume.roots);
    }

    #[test]
    fn converts_filetimes() {
        assert_eq!(
            system_time_from_filetime(116_444_736_000_000_000),
            UNIX_EPOCH
        );
        assert_eq!(
            system_time_from_filetime(116_444_736_000_000_000 + 10_000_000),
            UNIX_EPOCH + Duration::from_secs(1)
        );
        assert_eq!(
            system_time_from_filetime(116_444_736_000_000_000 - 10_000_000),
            UNIX_EPOCH - Duration::from_secs(1)
        );
        assert_eq!(system_time_from_filetime(i64::MIN), UNIX_EPOCH);
    }
}
