//! Temporary Windows Volume Shadow Copy (VSS) snapshots.
//!
//! A [`SnapshotSet`] holds snapshots of one or more volumes, all taken at the same moment.
//! Read files from it as they were then, including files that are open or locked on the
//! live volume, then close it, which removes the snapshots.
//!
//! ```no_run
//! use std::path::Path;
//! use vss_snapshot::{SnapshotOptions, SnapshotSet};
//!
//! # fn main() -> vss_snapshot::Result<()> {
//! // Needs an elevated process.
//! let snapshots = SnapshotSet::create(["C:", "D:"], &SnapshotOptions::new())?;
//!
//! // A failed writer still gets a snapshot, but its files are only crash-consistent.
//! for writer in snapshots.writers().iter().filter(|w| w.is_failed()) {
//!     eprintln!("{} wasn't ready: {}", writer.name, writer.state);
//! }
//!
//! if let Some(path) = snapshots.path_in_snapshot(Path::new(r"C:\data\report.xlsx")) {
//!     let bytes = std::fs::read(path)?;
//! #   let _ = bytes;
//! }
//!
//! // Dropping the set removes the snapshots too, but can't report a failure.
//! snapshots.close()?;
//! # Ok(())
//! # }
//! ```
//!
//! The full interface, including the lifecycle, threading, privileges and cleanup
//! guarantees, is defined in
//! [docs/INTERFACE.md](https://github.com/davejp/vss-snapshot/blob/main/docs/INTERFACE.md).
//!
//! On targets other than Windows this crate is empty.
#![cfg(windows)]
#![deny(unsafe_code)]
#![warn(missing_docs, unsafe_op_in_unsafe_fn)]
#![cfg_attr(
    not(test),
    warn(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]
// Some internal helpers are for listing writers and restoring, which come next. Remove
// this then.
#![allow(dead_code)]

mod document;
mod error;
mod guid;
mod hresult;
mod options;
mod snapshot;
#[allow(unsafe_code)]
mod sys;
mod volume;
mod writer;

pub use document::BackupDocument;
pub use error::{Error, Result};
pub use guid::{Guid, ParseGuidError};
pub use hresult::HResult;
pub use options::{Consistency, RestoreOptions, SnapshotOptions};
pub use snapshot::SnapshotSet;
pub use volume::VolumeSnapshot;
pub use writer::{
    Component, ComponentId, ComponentKind, FileSpec, RestoreMethod, WriterInfo, WriterState,
    WriterStatus,
};
