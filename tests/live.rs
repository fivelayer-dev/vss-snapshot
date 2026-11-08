//! Tests that take real snapshots.
//!
//! They run only on GitHub Actions, where each run gets a disposable, elevated machine. The
//! workflow makes two small test volumes and names them in `VSS_SNAPSHOT_TEST_VOLUMES`, such
//! as `V:,W:`, with the second also mounted on a folder of the first, `V:\mnt\w`. Everywhere
//! else each test prints `SKIPPED` and passes, even when elevated.
#![cfg(windows)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime};

use vss_snapshot::{Consistency, SnapshotOptions, SnapshotSet};

/// The tests take turns, so each one's checks see only its own snapshots.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A test's turn, and the volumes it may snapshot.
struct Live {
    _turn: MutexGuard<'static, ()>,
    /// The first test volume, such as `V:`.
    first: String,
    /// The second, such as `W:`, which is also mounted at `<first>\mnt\w`.
    second: String,
}

impl Live {
    fn mount_folder(&self) -> String {
        format!(r"{}\mnt\w", self.first)
    }
}

fn live(test: &str) -> Option<Live> {
    if std::env::var_os("GITHUB_ACTIONS").is_none() {
        eprintln!("SKIPPED {test}: real snapshots are only taken on GitHub Actions");
        return None;
    }
    let volumes = std::env::var("VSS_SNAPSHOT_TEST_VOLUMES")
        .expect("the workflow should set VSS_SNAPSHOT_TEST_VOLUMES");
    let mut volumes = volumes.split(',').map(str::to_owned);
    let (Some(first), Some(second)) = (volumes.next(), volumes.next()) else {
        panic!("VSS_SNAPSHOT_TEST_VOLUMES should name two volumes");
    };
    let turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    Some(Live {
        _turn: turn,
        first,
        second,
    })
}

/// A folder for one test's files, emptied first and removed afterwards.
struct TestDir(PathBuf);

impl TestDir {
    fn new(volume: &str, test: &str) -> TestDir {
        let path = PathBuf::from(format!(r"{volume}\vss-snapshot-tests\{test}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TestDir(path)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn crash() -> SnapshotOptions {
    SnapshotOptions::new().with_consistency(Consistency::Crash)
}

/// Changes the live files after the snapshot, and checks the snapshot still has them as
/// they were.
fn check_holds_files_as_they_were(options: &SnapshotOptions, volume: &str, test: &str) {
    let dir = TestDir::new(volume, test);
    let (changed, deleted, added) = (dir.file("changed"), dir.file("deleted"), dir.file("added"));
    fs::write(&changed, "before").unwrap();
    fs::write(&deleted, "before").unwrap();

    let snapshots = SnapshotSet::create([volume], options).unwrap();
    fs::write(&changed, "after").unwrap();
    fs::remove_file(&deleted).unwrap();
    fs::write(&added, "after").unwrap();

    let read = |path: &Path| fs::read_to_string(snapshots.path_in_snapshot(path).unwrap());
    assert_eq!(read(&changed).unwrap(), "before");
    assert_eq!(read(&deleted).unwrap(), "before");
    assert!(
        read(&added).is_err(),
        "a file added afterwards is in the snapshot"
    );
    snapshots.close().unwrap();
}

#[test]
fn a_crash_consistent_snapshot_holds_files_as_they_were() {
    let Some(live) = live("a_crash_consistent_snapshot_holds_files_as_they_were") else {
        return;
    };
    check_holds_files_as_they_were(&crash(), &live.first, "crash");
}

#[test]
fn an_application_consistent_snapshot_holds_files_as_they_were() {
    let Some(live) = live("an_application_consistent_snapshot_holds_files_as_they_were") else {
        return;
    };
    check_holds_files_as_they_were(&SnapshotOptions::new(), &live.first, "application");
}

#[test]
fn writers_are_reported_only_with_application_consistency() {
    let Some(live) = live("writers_are_reported_only_with_application_consistency") else {
        return;
    };
    let snapshots = SnapshotSet::create([&live.first], &SnapshotOptions::new()).unwrap();
    assert!(!snapshots.writers().is_empty(), "no writers reported");
    for writer in snapshots.writers() {
        eprintln!(
            "{} {} {:?} failed={}",
            writer.id,
            writer.name,
            writer.state,
            writer.is_failed()
        );
        assert!(!writer.name.is_empty());
    }
    snapshots.close().unwrap();

    let snapshots = SnapshotSet::create([&live.first], &crash()).unwrap();
    assert!(snapshots.writers().is_empty());
    snapshots.close().unwrap();
}

#[test]
fn reads_a_file_that_is_locked_on_the_live_volume() {
    let Some(live) = live("reads_a_file_that_is_locked_on_the_live_volume") else {
        return;
    };
    let dir = TestDir::new(&live.first, "locked");
    let path = dir.file("locked");
    let mut held = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    held.write_all(b"locked contents").unwrap();
    held.sync_all().unwrap();
    assert!(fs::read(&path).is_err(), "the live file should be locked");

    let snapshots = SnapshotSet::create([&live.first], &crash()).unwrap();
    let copy = fs::read(snapshots.path_in_snapshot(&path).unwrap()).unwrap();
    assert_eq!(copy, b"locked contents");
    drop(held);
    snapshots.close().unwrap();
}

#[test]
fn describes_each_volume() {
    let Some(live) = live("describes_each_volume") else {
        return;
    };
    let before = SystemTime::now() - Duration::from_secs(60);
    let snapshots = SnapshotSet::create([&live.first], &crash()).unwrap();
    let after = SystemTime::now() + Duration::from_secs(60);
    let volume = &snapshots.volumes()[0];
    eprintln!("{} {volume:?}", snapshots.id());

    assert_eq!(volume.name(), format!(r"{}\", live.first));
    assert!(volume.guid_path().starts_with(r"\\?\Volume{"));
    let device = volume.device_path().to_str().unwrap();
    assert!(device.starts_with(r"\\?\GLOBALROOT\Device\"), "{device}");
    assert!(device.ends_with('\\'), "{device}");
    assert!(volume
        .mount_points()
        .iter()
        .any(|p| p == Path::new(&format!(r"{}\", live.first))));
    assert!((before..after).contains(&volume.created()));
    assert_ne!(volume.id(), snapshots.id());
    assert!(fs::read_dir(volume.device_path()).unwrap().count() > 0);

    for name in [live.first.clone(), volume.guid_path().to_owned()] {
        assert_eq!(snapshots.volume(&name), Some(volume), "{name}");
    }
    assert_eq!(snapshots.volume(&live.second), None);
    snapshots.close().unwrap();
}

#[test]
fn snapshots_two_volumes_together() {
    let Some(live) = live("snapshots_two_volumes_together") else {
        return;
    };
    let first = TestDir::new(&live.first, "together");
    let second = TestDir::new(&live.second, "together");
    fs::write(first.file("a"), "first").unwrap();
    fs::write(second.file("b"), "second").unwrap();

    let snapshots = SnapshotSet::create([&live.first, &live.second], &crash()).unwrap();
    assert_eq!(snapshots.volumes().len(), 2);
    let [one, two] = snapshots.volumes() else {
        unreachable!()
    };
    assert_ne!(one.device_path(), two.device_path());

    let read = |path: PathBuf| fs::read_to_string(snapshots.path_in_snapshot(&path).unwrap());
    assert_eq!(read(first.file("a")).unwrap(), "first");
    assert_eq!(read(second.file("b")).unwrap(), "second");
    snapshots.close().unwrap();
}

#[test]
fn naming_a_volume_several_ways_takes_one_snapshot() {
    let Some(live) = live("naming_a_volume_several_ways_takes_one_snapshot") else {
        return;
    };
    let lower = format!(r"{}\", live.first.to_lowercase());
    let snapshots = SnapshotSet::create([live.first.clone(), lower], &crash()).unwrap();
    assert_eq!(snapshots.volumes().len(), 1, "{:?}", snapshots.volumes());
    snapshots.close().unwrap();

    // The second volume, by its drive letter and the folder it's mounted on.
    let snapshots =
        SnapshotSet::create([live.second.clone(), live.mount_folder()], &crash()).unwrap();
    assert_eq!(snapshots.volumes().len(), 1, "{:?}", snapshots.volumes());
    let guid_path = snapshots.volumes()[0].guid_path().to_owned();
    snapshots.close().unwrap();

    // And by its GUID path.
    let snapshots = SnapshotSet::create([guid_path, live.second.clone()], &crash()).unwrap();
    assert_eq!(snapshots.volumes().len(), 1, "{:?}", snapshots.volumes());
    snapshots.close().unwrap();
}

#[test]
fn a_path_through_a_mount_folder_maps_to_the_mounted_volume() {
    let Some(live) = live("a_path_through_a_mount_folder_maps_to_the_mounted_volume") else {
        return;
    };
    let dir = TestDir::new(&live.second, "mounted");
    fs::write(dir.file("x"), "on the second volume").unwrap();
    let through_folder = PathBuf::from(format!(
        r"{}\vss-snapshot-tests\mounted\x",
        live.mount_folder()
    ));
    assert!(through_folder.exists(), "{through_folder:?}");

    let snapshots = SnapshotSet::create([&live.first, &live.second], &crash()).unwrap();
    let second = snapshots.volume(&live.second).unwrap();
    assert!(
        second
            .mount_points()
            .iter()
            .any(|p| p.to_str().is_some_and(|p| p
                .trim_end_matches('\\')
                .eq_ignore_ascii_case(&live.mount_folder()))),
        "{:?}",
        second.mount_points()
    );
    assert_eq!(snapshots.volume(&live.mount_folder()), Some(second));

    let mapped = snapshots.path_in_snapshot(&through_folder).unwrap();
    assert!(mapped.starts_with(second.device_path()), "{mapped:?}");
    assert_eq!(fs::read_to_string(mapped).unwrap(), "on the second volume");
    snapshots.close().unwrap();
}

#[test]
fn a_volume_can_be_named_by_the_folder_it_is_mounted_on() {
    let Some(live) = live("a_volume_can_be_named_by_the_folder_it_is_mounted_on") else {
        return;
    };
    let snapshots = SnapshotSet::create([live.mount_folder()], &crash()).unwrap();
    let volume = &snapshots.volumes()[0];
    assert_eq!(volume.name(), format!(r"{}\", live.mount_folder()));
    assert_eq!(snapshots.volume(&live.second), Some(volume));
    snapshots.close().unwrap();
}

/// Checks a snapshot is gone once `end` has run.
fn check_removed(options: &SnapshotOptions, volume: &str, end: impl FnOnce(SnapshotSet)) {
    let snapshots = SnapshotSet::create([volume], options).unwrap();
    let device = snapshots.volumes()[0].device_path().to_path_buf();
    assert!(device.exists(), "{device:?}");
    end(snapshots);
    assert!(!device.exists(), "{device:?} still exists");
}

#[test]
fn closing_removes_the_snapshot() {
    let Some(live) = live("closing_removes_the_snapshot") else {
        return;
    };
    check_removed(&crash(), &live.first, |s| s.close().unwrap());
    check_removed(&SnapshotOptions::new(), &live.first, |s| s.close().unwrap());
}

#[test]
fn dropping_removes_the_snapshot() {
    let Some(live) = live("dropping_removes_the_snapshot") else {
        return;
    };
    check_removed(&crash(), &live.first, drop);
    check_removed(&SnapshotOptions::new(), &live.first, drop);
}

#[test]
fn can_be_closed_from_another_thread() {
    let Some(live) = live("can_be_closed_from_another_thread") else {
        return;
    };
    check_removed(&crash(), &live.first, |s| {
        thread::spawn(move || s.close().unwrap()).join().unwrap();
    });
}

#[test]
fn works_from_a_single_threaded_com_apartment() {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    let Some(live) = live("works_from_a_single_threaded_com_apartment") else {
        return;
    };
    // On a new thread, as a thread's apartment can't change once it has chosen one.
    let volume = live.first.clone();
    thread::spawn(move || {
        // SAFETY: balanced by `CoUninitialize` below, on this thread.
        let code = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        assert!(code.is_ok(), "CoInitializeEx: {code:?}");
        let snapshots = SnapshotSet::create([&volume], &SnapshotOptions::new()).unwrap();
        snapshots.close().unwrap();
        // SAFETY: balances the call above.
        unsafe { CoUninitialize() };
    })
    .join()
    .unwrap();
}

#[test]
fn sets_created_at_once_from_two_threads_both_succeed() {
    let Some(live) = live("sets_created_at_once_from_two_threads_both_succeed") else {
        return;
    };
    let (first, second) = (live.first.clone(), live.second.clone());
    let one = thread::spawn(move || SnapshotSet::create([first], &crash()));
    let two = thread::spawn(move || SnapshotSet::create([second], &crash()));
    let one = one.join().unwrap().unwrap();
    let two = two.join().unwrap().unwrap();
    assert_ne!(one.id(), two.id());
    one.close().unwrap();
    two.close().unwrap();
}
