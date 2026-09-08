//! Transport failures, named the way Chromium names them.
//!
//! The names themselves live in [`crate::policy::error_names`]: Playwright
//! and CDP clients assert on them (`docs/spec/03-network.md` §7), so this
//! module only adds the transport's message and the mapping from `io::Error`
//! and from a policy refusal.

use std::fmt;
use std::io;

use tenta_core::Error;

pub use crate::policy::error_names::ErrorName;

/// A transport failure: a Chromium error name plus a human-readable detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetError {
    pub name: ErrorName,
    pub message: String,
}

impl NetError {
    pub fn new(name: ErrorName, message: impl Into<String>) -> Self {
        Self {
            name,
            message: message.into(),
        }
    }

    /// The Chromium name of this failure.
    pub fn name(&self) -> ErrorName {
        self.name
    }

    /// The name as it appears in `Network.loadingFailed.errorText`.
    pub fn error_text(&self) -> String {
        self.name.as_str().to_owned()
    }

    /// Whether the same request may reasonably be sent again, which is what
    /// makes a pooled connection the peer closed while idle invisible.
    pub fn retryable(&self) -> bool {
        matches!(
            self.name,
            ErrorName::ConnectionReset
                | ErrorName::ConnectionClosed
                | ErrorName::ConnectionAborted
                | ErrorName::ConnectionFailed
                | ErrorName::ConnectionTimedOut
                | ErrorName::EmptyResponse
        )
    }

    /// Maps a connect-time I/O error. A timeout while connecting is
    /// `ERR_CONNECTION_TIMED_OUT`, not `ERR_TIMED_OUT`.
    pub fn from_connect_io(err: &io::Error) -> Self {
        let name = match err.kind() {
            io::ErrorKind::ConnectionAborted => ErrorName::ConnectionClosed,
            io::ErrorKind::TimedOut => ErrorName::ConnectionTimedOut,
            other => match ErrorName::from_io_error(err) {
                ErrorName::Failed if other != io::ErrorKind::Other => ErrorName::ConnectionFailed,
                ErrorName::Failed => ErrorName::ConnectionFailed,
                name => name,
            },
        };
        Self::new(name, err.to_string())
    }

    /// Maps an I/O error seen while a request or response was in flight.
    pub fn from_stream_io(err: &io::Error) -> Self {
        let name = match err.kind() {
            io::ErrorKind::TimedOut => ErrorName::TimedOut,
            _ => match ErrorName::from_io_error(err) {
                ErrorName::Failed => ErrorName::ConnectionFailed,
                name => name,
            },
        };
        Self::new(name, err.to_string())
    }

    /// Maps a refusal from [`crate::policy`], which reports itself as a
    /// `tenta_core::Error` carrying the name in its text.
    pub fn from_policy(err: Error) -> Self {
        match err {
            Error::NetDenied { reason } => {
                let (name, detail) = reason.split_once(':').unwrap_or((reason.as_str(), ""));
                // `NetDenied` reads `net::ERR_X: detail`, so the name ends at
                // the second colon of the `net::` prefix.
                let (name, detail) = match name {
                    "net" => detail
                        .trim_start_matches(':')
                        .split_once(':')
                        .unwrap_or((detail, "")),
                    _ => (name, detail),
                };
                Self::new(
                    ErrorName::parse(name.trim()).unwrap_or(ErrorName::BlockedByClient),
                    detail.trim(),
                )
            }
            Error::Net { code, message } => Self::new(
                ErrorName::parse(&code).unwrap_or(ErrorName::Failed),
                message,
            ),
            Error::Io(err) => Self::from_stream_io(&err),
            other => Self::new(ErrorName::Failed, other.to_string()),
        }
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{}: {}", self.name, self.message)
        }
    }
}

impl std::error::Error for NetError {}

impl From<NetError> for Error {
    fn from(err: NetError) -> Self {
        err.name.into_core_error(err.message)
    }
}

/// Result alias used across the transport.
pub type Result<T, E = NetError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_chromium_and_reach_the_public_error() {
        let err = NetError::new(ErrorName::NameNotResolved, "no such host");
        assert_eq!(err.error_text(), "net::ERR_NAME_NOT_RESOLVED");
        assert_eq!(err.name().code(), -105);
        match Error::from(err) {
            Error::Net { code, message } => {
                assert_eq!(code, "net::ERR_NAME_NOT_RESOLVED");
                assert_eq!(message, "no such host");
            }
            other => panic!("expected Error::Net, got {other:?}"),
        }
    }

    #[test]
    fn io_errors_map_by_kind() {
        let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
        assert_eq!(
            NetError::from_connect_io(&refused).name,
            ErrorName::ConnectionRefused
        );
        let eof = io::Error::from(io::ErrorKind::UnexpectedEof);
        assert_eq!(
            NetError::from_stream_io(&eof).name,
            ErrorName::ConnectionClosed
        );
        let slow = io::Error::from(io::ErrorKind::TimedOut);
        assert_eq!(
            NetError::from_connect_io(&slow).name,
            ErrorName::ConnectionTimedOut,
            "a timeout while connecting is not the request deadline"
        );
        assert_eq!(NetError::from_stream_io(&slow).name, ErrorName::TimedOut);
        assert!(NetError::new(ErrorName::ConnectionReset, "").retryable());
        assert!(!NetError::new(ErrorName::CertDateInvalid, "").retryable());
    }

    #[test]
    fn a_policy_refusal_keeps_its_name() {
        let denied = Error::NetDenied {
            reason: "net::ERR_UNSAFE_PORT: port 25 is on the restricted port list".to_owned(),
        };
        let err = NetError::from_policy(denied);
        assert_eq!(err.name, ErrorName::UnsafePort);
        assert!(err.message.contains("port 25"));
    }
}
