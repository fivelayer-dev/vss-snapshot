use std::fmt;

use windows::Win32::Foundation as foundation;
use windows::Win32::Storage::Vss as vss;

/// A COM `HRESULT`, as returned by VSS and its writers.
///
/// `Display` gives the constant's name when this crate knows it, and the code in hex:
/// `VSS_E_WRITERERROR_TIMEOUT (0x800423F2)`, or just `0x80004321` for an unknown code.
///
/// ```
/// # #[cfg(windows)] {
/// use vss_snapshot::HResult;
///
/// let code = HResult::from_code(0x800423F2_u32 as i32);
/// assert_eq!(code.name(), Some("VSS_E_WRITERERROR_TIMEOUT"));
/// assert_eq!(code.to_string(), "VSS_E_WRITERERROR_TIMEOUT (0x800423F2)");
/// # }
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct HResult(i32);

impl HResult {
    /// Wraps a raw `HRESULT` value.
    pub const fn from_code(code: i32) -> HResult {
        HResult(code)
    }

    /// The raw `HRESULT` value.
    pub const fn code(self) -> i32 {
        self.0
    }

    /// The constant's name, such as `"VSS_E_BAD_STATE"`, if this crate knows it.
    pub fn name(self) -> Option<&'static str> {
        KNOWN
            .iter()
            .find(|(code, _)| *code == self.0)
            .map(|(_, name)| *name)
    }

    /// Whether this code reports a failure, as opposed to success or information.
    pub(crate) const fn is_failure(self) -> bool {
        self.0 < 0
    }
}

impl fmt::Display for HResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{name} (0x{:08X})", self.0 as u32),
            None => write!(f, "0x{:08X}", self.0 as u32),
        }
    }
}

impl fmt::Debug for HResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HResult({self})")
    }
}

macro_rules! known {
    ($($module:ident :: $name:ident),* $(,)?) => {
        &[$(($module::$name.0, stringify!($name))),*]
    };
}

/// The codes `Display` names: the VSS codes a requester can see, and the COM codes that
/// come up when using it.
const KNOWN: &[(i32, &str)] = known![
    foundation::E_ACCESSDENIED,
    foundation::E_FAIL,
    foundation::E_INVALIDARG,
    foundation::E_NOTIMPL,
    foundation::E_OUTOFMEMORY,
    foundation::E_POINTER,
    foundation::E_UNEXPECTED,
    foundation::CO_E_NOTINITIALIZED,
    foundation::RPC_E_CHANGED_MODE,
    foundation::RPC_E_TOO_LATE,
    vss::VSS_S_ASYNC_PENDING,
    vss::VSS_S_ASYNC_FINISHED,
    vss::VSS_S_ASYNC_CANCELLED,
    vss::VSS_E_BAD_STATE,
    vss::VSS_E_UNEXPECTED,
    vss::VSS_E_PROVIDER_ALREADY_REGISTERED,
    vss::VSS_E_PROVIDER_NOT_REGISTERED,
    vss::VSS_E_PROVIDER_VETO,
    vss::VSS_E_PROVIDER_IN_USE,
    vss::VSS_E_OBJECT_NOT_FOUND,
    vss::VSS_E_VOLUME_NOT_SUPPORTED,
    vss::VSS_E_OBJECT_ALREADY_EXISTS,
    vss::VSS_E_VOLUME_NOT_SUPPORTED_BY_PROVIDER,
    vss::VSS_E_UNEXPECTED_PROVIDER_ERROR,
    vss::VSS_E_CORRUPT_XML_DOCUMENT,
    vss::VSS_E_INVALID_XML_DOCUMENT,
    vss::VSS_E_MAXIMUM_NUMBER_OF_VOLUMES_REACHED,
    vss::VSS_E_FLUSH_WRITES_TIMEOUT,
    vss::VSS_E_HOLD_WRITES_TIMEOUT,
    vss::VSS_E_UNEXPECTED_WRITER_ERROR,
    vss::VSS_E_SNAPSHOT_SET_IN_PROGRESS,
    vss::VSS_E_MAXIMUM_NUMBER_OF_SNAPSHOTS_REACHED,
    vss::VSS_E_WRITER_INFRASTRUCTURE,
    vss::VSS_E_WRITER_NOT_RESPONDING,
    vss::VSS_E_WRITER_ALREADY_SUBSCRIBED,
    vss::VSS_E_UNSUPPORTED_CONTEXT,
    vss::VSS_E_VOLUME_IN_USE,
    vss::VSS_E_MAXIMUM_DIFFAREA_ASSOCIATIONS_REACHED,
    vss::VSS_E_INSUFFICIENT_STORAGE,
    vss::VSS_E_REVERT_IN_PROGRESS,
    vss::VSS_E_REBOOT_REQUIRED,
    vss::VSS_E_TRANSACTION_FREEZE_TIMEOUT,
    vss::VSS_E_TRANSACTION_THAW_TIMEOUT,
    vss::VSS_E_UNSELECTED_VOLUME,
    vss::VSS_E_SNAPSHOT_NOT_IN_SET,
    vss::VSS_E_NESTED_VOLUME_LIMIT,
    vss::VSS_E_VOLUME_NOT_LOCAL,
    vss::VSS_E_CLUSTER_TIMEOUT,
    vss::VSS_E_NOT_SUPPORTED,
    vss::VSS_E_WRITERERROR_PARTIAL_FAILURE,
    vss::VSS_E_WRITERERROR_INCONSISTENTSNAPSHOT,
    vss::VSS_E_WRITERERROR_OUTOFRESOURCES,
    vss::VSS_E_WRITERERROR_TIMEOUT,
    vss::VSS_E_WRITERERROR_RETRYABLE,
    vss::VSS_E_WRITERERROR_NONRETRYABLE,
    vss::VSS_E_WRITERERROR_RECOVERY_FAILED,
    vss::VSS_E_WRITER_STATUS_NOT_AVAILABLE,
    vss::VSS_E_FSS_TIMEOUT,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_known_codes() {
        let bad_state = HResult::from_code(0x80042301_u32 as i32);
        assert_eq!(bad_state.name(), Some("VSS_E_BAD_STATE"));
        assert_eq!(bad_state.to_string(), "VSS_E_BAD_STATE (0x80042301)");
        let denied = HResult::from_code(0x80070005_u32 as i32);
        assert_eq!(denied.name(), Some("E_ACCESSDENIED"));
    }

    #[test]
    fn shows_unknown_codes_in_hex() {
        let unknown = HResult::from_code(0x8000_4321_u32 as i32);
        assert_eq!(unknown.name(), None);
        assert_eq!(unknown.to_string(), "0x80004321");
        assert_eq!(format!("{unknown:?}"), "HResult(0x80004321)");
    }

    #[test]
    fn every_known_code_appears_once() {
        for (index, (code, name)) in KNOWN.iter().enumerate() {
            let later = KNOWN.iter().skip(index + 1);
            assert!(later.clone().all(|(c, _)| c != code), "{name} twice");
            assert!(later.clone().all(|(_, n)| n != name), "{name} twice");
        }
    }

    #[test]
    fn success_codes_are_not_failures() {
        assert!(!HResult::from_code(0).is_failure());
        assert!(!HResult::from_code(vss::VSS_S_ASYNC_FINISHED.0).is_failure());
        assert!(HResult::from_code(vss::VSS_E_BAD_STATE.0).is_failure());
    }
}
