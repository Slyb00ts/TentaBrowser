//! The error object of a failed command.

use std::fmt;

/// A JSON-RPC error as Chromium reports it: a code, a message and optional
/// detail text. Handlers return it; the dispatcher turns it into the
/// `error` member of the response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
    pub data: Option<String>,
}

impl CdpError {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// Chromium's generic "the command could not be carried out" code.
    pub const SERVER_ERROR: i64 = -32000;
    pub const SESSION_NOT_FOUND: i64 = -32001;

    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    #[must_use]
    pub fn with_data(mut self, data: impl Into<String>) -> Self {
        self.data = Some(data.into());
        self
    }

    pub(crate) fn parse_error() -> Self {
        Self::new(Self::PARSE_ERROR, "Message must be a valid JSON")
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(Self::INVALID_REQUEST, message)
    }

    /// `method` is the full `Domain.method` name; the message carries it so a
    /// client log shows which call a server does not implement.
    pub fn method_not_found(method: &str) -> Self {
        Self::new(Self::METHOD_NOT_FOUND, format!("'{method}' wasn't found"))
    }

    pub fn invalid_params(detail: impl Into<String>) -> Self {
        Self::new(Self::INVALID_PARAMS, "Invalid parameters").with_data(detail)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Self::INTERNAL_ERROR, message)
    }

    pub fn server(message: impl Into<String>) -> Self {
        Self::new(Self::SERVER_ERROR, message)
    }

    pub(crate) fn session_not_found() -> Self {
        Self::new(Self::SESSION_NOT_FOUND, "Session with given id not found.")
    }
}

impl fmt::Display for CdpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)?;
        if let Some(data) = &self.data {
            write!(f, ": {data}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CdpError {}

impl From<tenta_core::Error> for CdpError {
    fn from(error: tenta_core::Error) -> Self {
        Self::server(error.to_string())
    }
}
