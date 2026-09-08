//! Exit codes and the messages that go with them.
//!
//! A shell script reads the exit code; a person reads stderr. Both have to
//! say the same thing, so every failure carries one code and one line.

use std::fmt;

use tentabrowser::{BlockClass, Error, SearchError};

/// What the process exits with.
///
/// | Code | Meaning |
/// |------|---------|
/// | 0 | the command did what it was asked |
/// | 1 | the request never reached a server (`net::ERR_*` on stderr) |
/// | 2 | the command line or the filesystem policy refused the request |
/// | 3 | the server answered 4xx or 5xx (`--allow-http-errors` to accept) |
/// | 4 | a search provider blocked the query (the class is on stderr) |
/// | 5 | the request needs something this build does not have |
pub const EXIT_OK: u8 = 0;
pub const EXIT_TRANSPORT: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_HTTP: u8 = 3;
pub const EXIT_BLOCKED: u8 = 4;
pub const EXIT_UNSUPPORTED: u8 = 5;

pub type CliResult<T> = Result<T, CliError>;

#[derive(Debug)]
pub struct CliError {
    code: u8,
    message: String,
    /// The `net::ERR_*` name, when the failure has one.
    error_name: Option<String>,
}

impl CliError {
    pub fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            error_name: None,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(EXIT_USAGE, message)
    }

    pub fn transport(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: EXIT_TRANSPORT,
            message: message.into(),
            error_name: Some(name.into()),
        }
    }

    pub fn http(status: u16, url: &str) -> Self {
        Self::new(
            EXIT_HTTP,
            format!("{url} answered HTTP {status}; pass --allow-http-errors to accept it"),
        )
    }

    pub fn exit_code(&self) -> u8 {
        self.code
    }

    pub fn error_name(&self) -> Option<&str> {
        self.error_name.as_deref()
    }

    /// The line the process prints on stderr before exiting. The `net::ERR_*`
    /// name comes first so that `grep net::ERR` finds it.
    pub fn report(&self) -> String {
        match &self.error_name {
            Some(name) => format!("tentabrowser: {name}: {}", self.message),
            None => format!("tentabrowser: {}", self.message),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

impl From<Error> for CliError {
    fn from(error: Error) -> Self {
        match error {
            Error::Net { code, message } => {
                let hint = if message.contains("private-network destination") {
                    " (pass --allow-private-network to reach loopback and RFC 1918 addresses)"
                } else {
                    ""
                };
                CliError::transport(code, format!("{message}{hint}"))
            }
            Error::FsDenied { ref path } => CliError::usage(format!(
                "filesystem access denied by policy: {} (pass --allow-read or --allow-write)",
                path.display()
            )),
            Error::Unsupported { reason } => CliError::new(EXIT_UNSUPPORTED, reason),
            Error::InvalidArgument { reason } => CliError::usage(reason),
            Error::Timeout { .. } => CliError::transport("net::ERR_TIMED_OUT", error.to_string()),
            other => CliError::new(EXIT_TRANSPORT, other.to_string()),
        }
    }
}

impl From<SearchError> for CliError {
    fn from(error: SearchError) -> Self {
        match error {
            SearchError::Blocked {
                ref provider,
                reason,
                ref detail,
            } => CliError::new(
                EXIT_BLOCKED,
                format!("{provider} blocked the query ({}): {detail}", class(reason)),
            ),
            SearchError::Unsupported { provider, mode } => CliError::new(
                EXIT_UNSUPPORTED,
                unsupported_message(&provider.to_string(), &mode.to_string()),
            ),
            SearchError::InvalidRequest(reason) => CliError::usage(reason),
            SearchError::Net(net) => CliError::transport(net.name().as_str(), net.to_string()),
            other => CliError::new(EXIT_TRANSPORT, other.to_string()),
        }
    }
}

fn class(reason: BlockClass) -> &'static str {
    reason.as_str()
}

/// Google needs JavaScript since 15 January 2025, so it is `Mode::Full` only
/// and `Mode::Full` lands in P2 (`docs/spec/08-search.md` §2).
fn unsupported_message(provider: &str, mode: &str) -> String {
    if provider == "google" {
        return "google serves results only to a browser that runs JavaScript; \
                TentaBrowser gets that in P2. Use --provider ddg, bing or brave."
            .to_owned();
    }
    format!("{provider} has no {mode}-mode plan in this build")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tentabrowser::{Mode, Provider};

    #[test]
    fn a_transport_failure_reports_its_error_name_and_exits_one() {
        let error = CliError::from(Error::Net {
            code: "net::ERR_CONNECTION_REFUSED".into(),
            message: "connect refused".into(),
        });
        assert_eq!(error.exit_code(), EXIT_TRANSPORT);
        assert_eq!(error.error_name(), Some("net::ERR_CONNECTION_REFUSED"));
        assert!(error.report().contains("net::ERR_CONNECTION_REFUSED"));
    }

    #[test]
    fn a_private_network_refusal_names_the_flag_that_allows_it() {
        let error = CliError::from(Error::Net {
            code: "net::ERR_ACCESS_DENIED".into(),
            message: "127.0.0.1 is a private-network destination".into(),
        });
        assert_eq!(error.exit_code(), EXIT_TRANSPORT);
        assert!(
            error.report().contains("--allow-private-network"),
            "{}",
            error.report()
        );
    }

    #[test]
    fn google_is_refused_with_the_reason_and_the_phase() {
        let error = CliError::from(SearchError::Unsupported {
            provider: Provider::Google,
            mode: Mode::Full,
        });
        assert_eq!(error.exit_code(), EXIT_UNSUPPORTED);
        let report = error.report();
        assert!(report.contains("JavaScript"), "{report}");
        assert!(report.contains("P2"), "{report}");
    }

    #[test]
    fn a_blocked_provider_exits_four_with_its_class() {
        let error = CliError::from(SearchError::Blocked {
            provider: Provider::Bing,
            reason: BlockClass::RateLimited,
            detail: "HTTP 429".into(),
        });
        assert_eq!(error.exit_code(), EXIT_BLOCKED);
        assert!(
            error.report().contains("rate-limited"),
            "{}",
            error.report()
        );
    }

    #[test]
    fn an_http_status_error_names_the_override() {
        let error = CliError::http(503, "http://example.test/");
        assert_eq!(error.exit_code(), EXIT_HTTP);
        assert!(error.report().contains("--allow-http-errors"));
    }
}
