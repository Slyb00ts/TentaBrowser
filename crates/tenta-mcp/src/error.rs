//! Typed tool errors (`docs/spec/10-mcp.md` §3).
//!
//! A tool that ran and failed returns a result with `isError: true` whose
//! structured content names the error type, so an agent can branch on it and
//! the model still sees the message. Protocol errors (unknown tool,
//! malformed request) are JSON-RPC errors and never come from here.

use serde_json::{Value, json};
use tenta_core::Error;

/// The error vocabulary of the tools. The names are stable API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The arguments do not satisfy the tool's schema or its preconditions.
    InvalidArguments,
    NavigationFailed,
    /// A ref is unknown or stale, or the element has no layout box.
    ElementNotFound,
    /// Policy or another element stops the action: private network, scheme,
    /// an overlay that would receive the click.
    Blocked,
    Timeout,
    BudgetExceeded,
    FsDenied,
    /// The build or the engine's tier ceiling lacks what the tool needs.
    TierDenied,
    /// The page's script threw.
    ScriptError,
    /// The page crashed or was closed; the next call opens a fresh page.
    PageClosed,
    Unsupported,
    Internal,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::InvalidArguments => "invalid_arguments",
            ErrorKind::NavigationFailed => "navigation_failed",
            ErrorKind::ElementNotFound => "element_not_found",
            ErrorKind::Blocked => "blocked",
            ErrorKind::Timeout => "timeout",
            ErrorKind::BudgetExceeded => "budget_exceeded",
            ErrorKind::FsDenied => "fs_denied",
            ErrorKind::TierDenied => "tier_denied",
            ErrorKind::ScriptError => "script_error",
            ErrorKind::PageClosed => "page_closed",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::Internal => "internal",
        }
    }
}

/// A tool failure.
#[derive(Clone, Debug)]
pub struct ToolError {
    pub kind: ErrorKind,
    pub message: String,
    pub retryable: bool,
}

impl ToolError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable: false,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidArguments, message)
    }

    pub fn element_not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ElementNotFound, message)
    }

    pub fn tier_denied(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::TierDenied, message)
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    /// The `CallToolResult` for this failure.
    pub fn to_result(&self) -> Value {
        json!({
            "content": [{
                "type": "text",
                "text": format!("{}: {}", self.kind.as_str(), self.message),
            }],
            "structuredContent": {
                "error": {
                    "type": self.kind.as_str(),
                    "message": self.message,
                    "retryable": self.retryable,
                },
            },
            "isError": true,
        })
    }
}

/// A network failure is `blocked` when policy refused the destination (the
/// private-network and scheme rules report `ERR_ACCESS_DENIED`), else a
/// failed navigation.
pub fn net_kind(code: &str) -> ErrorKind {
    if code.contains("ACCESS_DENIED") || code.contains("BLOCKED") {
        ErrorKind::Blocked
    } else {
        ErrorKind::NavigationFailed
    }
}

impl From<Error> for ToolError {
    fn from(error: Error) -> Self {
        let retryable = error.retryable();
        let kind = match &error {
            Error::TierDenied { .. } => ErrorKind::TierDenied,
            Error::BudgetExceeded { .. } => ErrorKind::BudgetExceeded,
            Error::FsDenied { .. } => ErrorKind::FsDenied,
            Error::NetDenied { .. } => ErrorKind::Blocked,
            Error::Timeout { .. } => ErrorKind::Timeout,
            Error::Crashed { .. } | Error::Closed => ErrorKind::PageClosed,
            Error::Unsupported { .. } => ErrorKind::Unsupported,
            Error::InvalidArgument { .. } => ErrorKind::InvalidArguments,
            Error::Script { .. } => ErrorKind::ScriptError,
            Error::Net { code, .. } => net_kind(code),
            // The watchdog ended a script that ran past its budget.
            Error::Other { code, .. } if code == "script_terminated" => ErrorKind::Timeout,
            _ => ErrorKind::Internal,
        };
        Self {
            kind,
            message: error.to_string(),
            // Policy does not change on a retry.
            retryable: retryable && kind != ErrorKind::Blocked,
        }
    }
}
