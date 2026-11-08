//! Temporary Windows Volume Shadow Copy (VSS) snapshots.
//!
//! The full interface, including the lifecycle, threading, privileges and cleanup
//! guarantees, is defined in
//! [docs/INTERFACE.md](https://github.com/davejp/vss-snapshot/blob/main/docs/INTERFACE.md).
//! Taking snapshots isn't implemented yet; this release has the types they use.
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
// The internal helpers here are used by the VSS layer, which comes next. Remove this then.
#![allow(dead_code)]

mod document;
mod error;
mod guid;
mod hresult;
mod options;
#[allow(unsafe_code)]
mod sys;
mod volume;
mod writer;

pub use document::BackupDocument;
pub use error::{Error, Result};
pub use guid::{Guid, ParseGuidError};
pub use hresult::HResult;
pub use options::{Consistency, RestoreOptions, SnapshotOptions};
pub use volume::VolumeSnapshot;
pub use writer::{
    Component, ComponentId, ComponentKind, FileSpec, RestoreMethod, WriterInfo, WriterState,
    WriterStatus,
};
