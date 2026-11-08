//! COM setup for the threads that talk to VSS.

use std::marker::PhantomData;

use windows::Win32::Foundation::RPC_E_TOO_LATE;
use windows::Win32::System::Com::{
    CoInitializeEx, CoInitializeSecurity, CoUninitialize, COINIT_MULTITHREADED, EOAC_NONE,
    RPC_C_AUTHN_LEVEL_PKT_PRIVACY, RPC_C_IMP_LEVEL_IDENTIFY,
};

use crate::error::{Error, Result};
use crate::hresult::HResult;

/// This thread's membership of the COM multithreaded apartment. Leaves it when dropped, so
/// it must be dropped after every COM object the thread holds.
///
/// Only threads this crate owns enter, so the apartment is always available. The pointer
/// marker keeps it on the thread that entered, as it's neither `Send` nor `Sync`.
pub(crate) struct Apartment(PhantomData<*const ()>);

impl Apartment {
    /// Joins the multithreaded apartment, and sets the process's COM security if nothing
    /// has yet.
    pub(crate) fn enter() -> Result<Apartment> {
        // SAFETY: no reserved pointer is passed. Every successful call, including S_FALSE,
        // is balanced by `CoUninitialize` in `drop`.
        let code = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if code.is_err() {
            return Err(Error::from_hresult(
                "CoInitializeEx",
                HResult::from_code(code.0),
            ));
        }
        let apartment = Apartment(PhantomData);
        set_security()?;
        Ok(apartment)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances the successful `CoInitializeEx` in `enter`, on the same thread.
        unsafe { CoUninitialize() };
    }
}

/// Writers call back into the requesting process, so it must accept calls at least at
/// identify level. COM security can be set once per process, and `RPC_E_TOO_LATE` means
/// it already was, by the host or by an earlier call here.
fn set_security() -> Result<()> {
    // SAFETY: every pointer argument is null, which asks for the defaults.
    let result = unsafe {
        CoInitializeSecurity(
            None,
            -1,
            None,
            None,
            RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
            RPC_C_IMP_LEVEL_IDENTIFY,
            None,
            EOAC_NONE,
            None,
        )
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.code() == RPC_E_TOO_LATE => Ok(()),
        Err(error) => Err(Error::from_hresult(
            "CoInitializeSecurity",
            HResult::from_code(error.code().0),
        )),
    }
}
