//! The one error type of the public API (`docs/spec/11-library-api.md` §1):
//! stable variants and a `retryable()` hint.

use std::path::PathBuf;

use crate::budgets::BudgetKind;
use crate::tier::Tier;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Every failure the library reports. Variants are part of the public
/// contract; add new ones at the end and never remove one.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The request needs a tier above the page's or engine's ceiling.
    #[error("operation needs tier {needed} but the ceiling is {ceiling}")]
    TierDenied { needed: Tier, ceiling: Tier },

    /// A memory, time or count budget was exceeded; the page was crashed or
    /// the operation refused (`docs/spec/14-memory.md` §3).
    #[error("{kind} budget exceeded{}", component.as_deref().map(|c| format!(" in {c}")).unwrap_or_default())]
    BudgetExceeded {
        kind: BudgetKind,
        component: Option<String>,
    },

    /// A path was outside the [`crate::FsPolicy`] roots.
    #[error("filesystem access denied by policy: {path}")]
    FsDenied { path: PathBuf },

    /// A network destination was refused by policy (private network,
    /// scheme, redirect limit, size limit).
    #[error("network access denied by policy: {reason}")]
    NetDenied { reason: String },

    /// The operation timed out (settle deadline, evaluate deadline, network).
    #[error("timed out after {millis} ms: {what}")]
    Timeout { what: String, millis: u64 },

    /// The page or its group crashed (panic contained by `catch_unwind`,
    /// watchdog termination, hard budget).
    #[error("page crashed: {reason}")]
    Crashed { reason: String },

    /// The page, context or engine was closed before the operation ran.
    #[error("target closed")]
    Closed,

    /// The feature is compiled out or not available on this platform or
    /// under the current configuration.
    #[error("unsupported: {reason}")]
    Unsupported { reason: String },

    /// Invalid argument from the caller.
    #[error("invalid argument: {reason}")]
    InvalidArgument { reason: String },

    /// A JavaScript exception escaped an evaluation.
    #[error("script error: {message}")]
    Script {
        message: String,
        stack: Option<String>,
    },

    /// A network-level failure, reported with Chromium's `net::ERR_*` name.
    #[error("network error {code}: {message}")]
    Net { code: String, message: String },

    /// An I/O failure outside the network stack.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Anything else, with a stable machine-readable `code`.
    #[error("{code}: {message}")]
    Other { code: String, message: String },

    /// The page lent its document to another operation, such as a render, and
    /// has it back as soon as that operation ends.
    #[error("the page's document is in use by another operation")]
    Busy,
}

impl Error {
    /// Whether a caller may reasonably retry the same operation.
    pub fn retryable(&self) -> bool {
        match self {
            Error::Timeout { .. } | Error::Net { .. } | Error::Busy => true,
            Error::Io(e) => matches!(
                e.kind(),
                std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::ConnectionReset
            ),
            _ => false,
        }
    }

    pub fn unsupported(reason: impl Into<String>) -> Self {
        Error::Unsupported {
            reason: reason.into(),
        }
    }

    pub fn invalid(reason: impl Into<String>) -> Self {
        Error::InvalidArgument {
            reason: reason.into(),
        }
    }

    pub fn timeout(what: impl Into<String>, millis: u64) -> Self {
        Error::Timeout {
            what: what.into(),
            millis,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_hint_is_conservative() {
        assert!(Error::timeout("settle", 30_000).retryable());
        assert!(
            Error::Net {
                code: "ERR_CONNECTION_RESET".into(),
                message: String::new()
            }
            .retryable()
        );
        assert!(!Error::Closed.retryable());
        assert!(
            !Error::FsDenied {
                path: "/etc/passwd".into()
            }
            .retryable()
        );
        assert!(
            !Error::TierDenied {
                needed: Tier::Js,
                ceiling: Tier::Dom
            }
            .retryable()
        );
    }

    #[test]
    fn messages_are_human_readable() {
        let e = Error::BudgetExceeded {
            kind: BudgetKind::PageMemory,
            component: Some("images".into()),
        };
        assert_eq!(e.to_string(), "page memory budget exceeded in images");
        let e = Error::TierDenied {
            needed: Tier::Render,
            ceiling: Tier::Js,
        };
        assert_eq!(
            e.to_string(),
            "operation needs tier render but the ceiling is js"
        );
    }
}
