//! Calls into Windows. All of the crate's `unsafe` code lives under this module, which
//! offers safe functions to the rest of the crate. Nothing here is public.

pub(crate) mod com;
pub(crate) mod process;
pub(crate) mod volume;
pub(crate) mod vss;
pub(crate) mod wide;
