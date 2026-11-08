# vss-snapshot

[![CI](https://github.com/davejp/vss-snapshot/actions/workflows/ci.yml/badge.svg)](https://github.com/davejp/vss-snapshot/actions/workflows/ci.yml)

Take temporary, read-only Windows Volume Shadow Copy (VSS) snapshots from Rust, so your program
can read every file as it was at one moment, including files that are open or locked.

> **Status:** under development. Taking, reading and removing snapshots is implemented, with
> tests that take real snapshots on GitHub's Windows runners. Listing writers, selecting components and restoring through
> writers are next. The interface is defined in [docs/INTERFACE.md](docs/INTERFACE.md). It
> isn't on crates.io yet.

- **Safe API.** All COM and VSS calls are private. Errors are typed, and the library doesn't
  panic.
- **Leaves nothing behind.** Snapshots are non-persistent and removed when you close or drop them.
  If your process is killed, Windows removes them itself.
- **Works from any thread.** Each snapshot keeps its COM objects on its own thread, whatever your
  thread's COM apartment.
- **Writer support.** Databases and other VSS-aware applications can make their files
  consistent first. You can also select writer components and restore through the writer.
- **No runtime dependencies.** VSS is part of Windows. The only Rust dependency is the
  official [`windows`](https://crates.io/crates/windows) crate.

## Quick start

```toml
[target.'cfg(windows)'.dependencies]
vss-snapshot = { git = "https://github.com/davejp/vss-snapshot" }
```

```rust
use std::path::Path;
use vss_snapshot::{SnapshotOptions, SnapshotSet};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Snapshot C: and D: at the same moment. Needs an elevated process.
    let snapshots = SnapshotSet::create(["C:", "D:"], &SnapshotOptions::new())?;

    // A writer that failed still gets a snapshot, but its files are only crash-consistent.
    for writer in snapshots.writers().iter().filter(|w| w.is_failed()) {
        eprintln!("{} wasn't ready: {}", writer.name, writer.state);
    }

    // Read a file as it was when the snapshot was taken, even if it's locked now.
    if let Some(path) = snapshots.path_in_snapshot(Path::new(r"C:\data\report.xlsx")) {
        let bytes = std::fs::read(path)?;
        println!("{} bytes", bytes.len());
    }

    // Remove the snapshots. Dropping `snapshots` does this too, but can't report errors.
    snapshots.close()?;
    Ok(())
}
```

Without writers, a snapshot is faster and doesn't touch running applications, but databases
look as they would after a power cut:

```rust
use vss_snapshot::{Consistency, SnapshotOptions};

let options = SnapshotOptions::new().with_consistency(Consistency::Crash);
```

## Requirements

- 64-bit Windows, and a 64-bit process.
- An elevated administrator, SYSTEM, or a member of Backup Operators.
- Local NTFS or ReFS volumes.
- Rust 1.82 or later.

Permissions still apply when reading from a snapshot. To read files your account can't open,
enable `SeBackupPrivilege` and open them with `FILE_FLAG_BACKUP_SEMANTICS`.

## What it does and doesn't do

| Does | Doesn't |
|---|---|
| Snapshot several volumes at the same moment | Create persistent snapshots |
| Map live paths into the snapshot | Mount a snapshot as a drive letter |
| Report each writer's state | List or delete other programs' snapshots |
| List writers and their components | Full, incremental or log backups (copy backups only) |
| Back up and restore selected components through their writer | Copy files for you |

Restoring through a writer is tested with SQL Server Express only. Other writers are untested.

See [docs/INTERFACE.md](docs/INTERFACE.md) for the full interface: the lifecycle, errors,
threading, privileges and cleanup guarantees.

## Testing

`cargo test` runs the unit tests, which take no snapshots. Tests that take real snapshots run
only on GitHub Actions, where each run gets a disposable, elevated machine and snapshots small
virtual disks the workflow creates. Everywhere else they print `SKIPPED` and pass, even when
elevated. A passing local `cargo test` doesn't show that snapshots work, so check the CI run.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
