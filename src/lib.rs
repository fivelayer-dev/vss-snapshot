//! Temporary Windows Volume Shadow Copy (VSS) snapshots.
//!
//! The interface is defined in `docs/INTERFACE.md`. It is not implemented yet.
//!
//! On targets other than Windows this crate is empty.
#![cfg(windows)]
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
