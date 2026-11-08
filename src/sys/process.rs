//! Facts about the current process.

use std::io;

use windows::core::BOOL;
use windows::Win32::System::Threading::{GetCurrentProcess, IsWow64Process};

/// Whether this is a 32-bit process on 64-bit Windows, which VSS refuses.
pub(crate) fn is_wow64() -> io::Result<bool> {
    let mut wow64 = BOOL::default();
    // SAFETY: the pseudo-handle from `GetCurrentProcess` needn't be closed, and `wow64`
    // outlives the call.
    unsafe { IsWow64Process(GetCurrentProcess(), &mut wow64) }?;
    Ok(wow64.as_bool())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_native_process_is_not_wow64() {
        if cfg!(target_pointer_width = "64") {
            assert!(!is_wow64().unwrap());
        }
    }
}
