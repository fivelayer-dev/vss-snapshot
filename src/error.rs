use std::fmt;
use std::io;
use std::time::Duration;

use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Storage::Vss as vss;

use crate::hresult::HResult;
use crate::writer::{ComponentId, WriterState};

/// The result type of every fallible call in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a call failed.
///
/// New variants may be added in any release, so match with a `_` arm.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The process may not use VSS. Run it elevated, as SYSTEM, or as a member of
    /// Backup Operators.
    AccessDenied,
    /// The process is 32-bit on 64-bit Windows, which VSS refuses. Build for 64-bit.
    Wow64NotSupported,
    /// No volumes were given.
    NoVolumes,
    /// A volume name couldn't be understood.
    InvalidVolume {
        /// The name as given.
        name: String,
    },
    /// The volume doesn't exist, or no VSS provider can snapshot it, such as a network
    /// share or a FAT volume.
    VolumeNotSupported {
        /// The name as given.
        name: String,
    },
    /// The options can't be used together, or with this call.
    InvalidOptions {
        /// What's wrong.
        reason: &'static str,
    },
    /// A selected component isn't on this machine, for a backup, or isn't in the document
    /// as backed up successfully, for a restore.
    ComponentNotFound {
        /// The component that wasn't found.
        component: ComponentId,
    },
    /// A writer with a selected component failed. In a backup, the usual cause is leaving
    /// out a volume that holds some of the component's files.
    WriterFailed {
        /// The writer's name.
        writer: String,
        /// The state it reported.
        state: WriterState,
        /// The failure it reported, if any.
        failure: Option<HResult>,
    },
    /// Another snapshot set is being created somewhere on this machine. VSS allows one at a
    /// time, so try again later.
    SnapshotInProgress,
    /// There isn't enough free space on a volume to hold the snapshot's changes.
    InsufficientStorage,
    /// A VSS step didn't finish in time, and was cancelled.
    Timeout {
        /// The VSS call that took too long, such as `DoSnapshotSet`.
        operation: &'static str,
        /// The time it was given.
        timeout: Duration,
    },
    /// Any other failure reported by VSS or COM.
    Vss {
        /// The call that failed, such as `DoSnapshotSet`.
        operation: &'static str,
        /// The code it returned.
        code: HResult,
    },
    /// A Windows call outside VSS failed, such as resolving a volume's mount points.
    Io(io::Error),
    /// The crate's private worker thread stopped unexpectedly. This is a bug in this crate.
    Internal {
        /// What went wrong.
        reason: &'static str,
    },
}

/// Writer failures that VSS documents as worth retrying.
const RETRYABLE: &[i32] = &[
    vss::VSS_E_FLUSH_WRITES_TIMEOUT.0,
    vss::VSS_E_HOLD_WRITES_TIMEOUT.0,
    vss::VSS_E_WRITERERROR_TIMEOUT.0,
    vss::VSS_E_WRITERERROR_RETRYABLE.0,
    vss::VSS_E_WRITERERROR_OUTOFRESOURCES.0,
];

fn is_retryable_code(code: HResult) -> bool {
    RETRYABLE.contains(&code.code())
}

impl Error {
    /// Whether the same call might succeed if tried again later.
    ///
    /// True for [`SnapshotInProgress`](Error::SnapshotInProgress), [`Timeout`](Error::Timeout),
    /// and the failures VSS marks as temporary: writers too slow to flush or hold writes,
    /// writer timeouts, and writers that ran out of resources or asked to be retried.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::SnapshotInProgress | Error::Timeout { .. } => true,
            Error::Vss { code, .. } => is_retryable_code(*code),
            Error::WriterFailed {
                failure: Some(code),
                ..
            } => is_retryable_code(*code),
            _ => false,
        }
    }

    /// Turns a failed VSS or COM call into an error, giving the common failures their own
    /// variant.
    pub(crate) fn from_hresult(operation: &'static str, code: HResult) -> Error {
        match code.code() {
            c if c == E_ACCESSDENIED.0 => Error::AccessDenied,
            c if c == vss::VSS_E_SNAPSHOT_SET_IN_PROGRESS.0 => Error::SnapshotInProgress,
            c if c == vss::VSS_E_INSUFFICIENT_STORAGE.0 => Error::InsufficientStorage,
            _ => Error::Vss { operation, code },
        }
    }

    /// Turns a failure to add a volume to the snapshot set into an error naming it.
    pub(crate) fn from_add_volume(name: &str, code: HResult) -> Error {
        let unsupported = [
            vss::VSS_E_OBJECT_NOT_FOUND.0,
            vss::VSS_E_VOLUME_NOT_SUPPORTED.0,
            vss::VSS_E_VOLUME_NOT_SUPPORTED_BY_PROVIDER.0,
            vss::VSS_E_VOLUME_NOT_LOCAL.0,
        ];
        if unsupported.contains(&code.code()) {
            Error::VolumeNotSupported {
                name: name.to_owned(),
            }
        } else {
            Error::from_hresult("AddToSnapshotSet", code)
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::AccessDenied => f.write_str(
                "access denied: VSS needs an elevated process, SYSTEM, or a Backup Operator",
            ),
            Error::Wow64NotSupported => {
                f.write_str("VSS refuses 32-bit processes on 64-bit Windows")
            }
            Error::NoVolumes => f.write_str("no volumes were given"),
            Error::InvalidVolume { name } => write!(f, "not a volume name: {name:?}"),
            Error::VolumeNotSupported { name } => {
                write!(
                    f,
                    "the volume doesn't exist or can't be snapshotted: {name}"
                )
            }
            Error::InvalidOptions { reason } => write!(f, "invalid options: {reason}"),
            Error::ComponentNotFound { component } => {
                write!(f, "component not found: {component}")
            }
            Error::WriterFailed {
                writer,
                state,
                failure,
            } => {
                write!(f, "writer {writer} failed ({state})")?;
                match failure {
                    Some(code) => write!(f, ": {code}"),
                    None => Ok(()),
                }
            }
            Error::SnapshotInProgress => {
                f.write_str("another snapshot set is being created on this machine")
            }
            Error::InsufficientStorage => {
                f.write_str("not enough free disk space to hold the snapshot")
            }
            Error::Timeout { operation, timeout } => {
                write!(f, "{operation} didn't finish within {timeout:?}")
            }
            Error::Vss { operation, code } => write!(f, "{operation} failed: {code}"),
            Error::Io(error) => write!(f, "{error}"),
            Error::Internal { reason } => write!(f, "internal error: {reason}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::Guid;

    fn code(value: u32) -> HResult {
        HResult::from_code(value as i32)
    }

    #[test]
    fn errors_can_cross_threads() {
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<Error>();
    }

    #[test]
    fn common_failures_get_their_own_variant() {
        assert!(matches!(
            Error::from_hresult("x", code(0x80070005)),
            Error::AccessDenied
        ));
        assert!(matches!(
            Error::from_hresult("x", code(0x80042316)),
            Error::SnapshotInProgress
        ));
        assert!(matches!(
            Error::from_hresult("x", code(0x8004231F)),
            Error::InsufficientStorage
        ));
        assert!(matches!(
            Error::from_hresult("DoSnapshotSet", code(0x80042301)),
            Error::Vss {
                operation: "DoSnapshotSet",
                ..
            }
        ));
    }

    #[test]
    fn an_unsupported_volume_is_named() {
        for value in [0x80042308, 0x8004230C, 0x8004230E, 0x8004232D] {
            match Error::from_add_volume(r"Z:\", code(value)) {
                Error::VolumeNotSupported { name } => assert_eq!(name, r"Z:\"),
                other => panic!("{value:x}: {other:?}"),
            }
        }
        assert!(matches!(
            Error::from_add_volume(r"Z:\", code(0x80070005)),
            Error::AccessDenied
        ));
    }

    #[test]
    fn only_temporary_failures_are_retryable() {
        assert!(Error::SnapshotInProgress.is_retryable());
        assert!(Error::Timeout {
            operation: "x",
            timeout: Duration::from_secs(1)
        }
        .is_retryable());
        assert!(Error::from_hresult("x", code(0x80042313)).is_retryable());
        assert!(Error::from_hresult("x", code(0x80042314)).is_retryable());
        assert!(!Error::AccessDenied.is_retryable());
        assert!(!Error::InsufficientStorage.is_retryable());
        assert!(!Error::from_hresult("x", code(0x80042301)).is_retryable());
    }

    #[test]
    fn a_writer_failure_is_retryable_only_if_vss_says_so() {
        let failed = |failure| Error::WriterFailed {
            writer: "w".to_owned(),
            state: WriterState::FailedAtFreeze,
            failure,
        };
        assert!(failed(Some(code(0x800423F2))).is_retryable());
        assert!(failed(Some(code(0x800423F3))).is_retryable());
        assert!(!failed(Some(code(0x800423F4))).is_retryable());
        assert!(!failed(None).is_retryable());
    }

    #[test]
    fn messages_name_the_operation_and_code() {
        assert_eq!(
            Error::from_hresult("DoSnapshotSet", code(0x80042301)).to_string(),
            "DoSnapshotSet failed: VSS_E_BAD_STATE (0x80042301)"
        );
        assert_eq!(
            Error::from_hresult("DoSnapshotSet", code(0x80004321)).to_string(),
            "DoSnapshotSet failed: 0x80004321"
        );
    }

    #[test]
    fn writer_failures_name_the_writer_state_and_code() {
        let error = Error::WriterFailed {
            writer: "SqlServerWriter".to_owned(),
            state: WriterState::FailedAtPostRestore,
            failure: Some(code(0x800423F4)),
        };
        assert_eq!(
            error.to_string(),
            "writer SqlServerWriter failed (VSS_WS_FAILED_AT_POST_RESTORE): \
             VSS_E_WRITERERROR_NONRETRYABLE (0x800423F4)"
        );
    }

    #[test]
    fn a_missing_component_is_described() {
        let error = Error::ComponentNotFound {
            component: ComponentId::new(
                Guid::from_u128(0xa65faa63_5ea8_4ebc_9dbd_a0c4db26912a),
                r"HOST\SQLEXPRESS",
                "sales",
            ),
        };
        assert_eq!(
            error.to_string(),
            r"component not found: HOST\SQLEXPRESS\sales (writer {a65faa63-5ea8-4ebc-9dbd-a0c4db26912a})"
        );
    }

    #[test]
    fn io_errors_are_the_source() {
        use std::error::Error as _;
        let error = Error::from(io::Error::from(io::ErrorKind::NotFound));
        assert!(error.source().is_some());
        assert!(Error::NoVolumes.source().is_none());
    }
}
