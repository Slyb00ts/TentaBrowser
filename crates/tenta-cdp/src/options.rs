//! Server configuration and the exposure rules of `docs/spec/19-security.md` §3.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use tenta_browser::ContextOptions;
use tenta_core::{Error, Result};

/// Shortest token accepted for a bind that is reachable from the network.
pub const MIN_REMOTE_TOKEN_LEN: usize = 16;

fn is_url_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

/// How a [`crate::CdpServer`] listens and what it admits.
#[derive(Clone)]
pub struct CdpOptions {
    /// Where to listen. Port 0 picks a free port.
    pub bind: SocketAddr,
    /// A non-loopback `bind` is refused unless this is set and `token` is a
    /// token of at least [`MIN_REMOTE_TOKEN_LEN`] bytes.
    pub allow_non_loopback: bool,
    /// When set, every HTTP and WebSocket request must carry it as the
    /// `token` query parameter. Clients must connect with the full
    /// `browserWSEndpoint` URL (`?token=` included): Puppeteer's
    /// `browserURL` mode joins paths onto the URL and drops a query string.
    /// Logging added later must redact query strings.
    pub token: Option<String>,
    /// Open connections, HTTP and WebSocket together; more get HTTP 503.
    pub max_connections: usize,
    /// `Origin` values admitted on top of "no `Origin` header", matched
    /// exactly (there is no `*` wildcard). A page in an ordinary browser
    /// always sends one, so the default list is empty and a web page cannot
    /// drive the server.
    pub allowed_origins: Vec<String>,
    /// Host names admitted besides IP literals and `localhost`.
    pub allowed_hosts: Vec<String>,
    /// Largest WebSocket message accepted from a client. `None` picks
    /// Chromium's 256 MiB on loopback and 16 MiB on any other bind.
    pub max_message_bytes: Option<usize>,
    /// Page targets that may exist at once; `/json/new` and
    /// `Target.createTarget` fail beyond it.
    pub max_targets: usize,
    /// A WebSocket that sends nothing for this long is closed. `None`
    /// never closes an idle connection on loopback, as Chromium; with
    /// `allow_non_loopback` it defaults to the longer of `command_timeout`
    /// and ten minutes, because idle sockets there hold connection slots.
    /// An explicit value must not be shorter than `command_timeout`, or a
    /// slow command would lose its connection first.
    pub idle_timeout: Option<Duration>,
    /// How long a command may run before it is answered with an error.
    pub command_timeout: Duration,
    /// How long a navigation started by `Page.navigate` or `Page.reload` may
    /// take to load before the frame is reported as stopped. Chromium has no
    /// such limit; an engine that serves automation should not hold a page
    /// in a load that never ends.
    pub navigation_timeout: Duration,
    /// How long a client may take to send its HTTP request head.
    pub head_timeout: Duration,
    /// The `Browser` field of `/json/version` and `Browser.getVersion`.
    pub product: String,
    pub user_agent: String,
    /// The context `/json/new` and `Target.createTarget` use by default.
    pub default_context: ContextOptions,
}

impl Default for CdpOptions {
    fn default() -> Self {
        Self {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9222),
            allow_non_loopback: false,
            token: None,
            max_connections: 64,
            allowed_origins: Vec::new(),
            allowed_hosts: Vec::new(),
            max_message_bytes: None,
            max_targets: 64,
            idle_timeout: None,
            command_timeout: Duration::from_secs(300),
            navigation_timeout: Duration::from_secs(60),
            head_timeout: Duration::from_secs(10),
            product: format!("TentaBrowser/{}", env!("CARGO_PKG_VERSION")),
            user_agent: format!(
                "Mozilla/5.0 (compatible; TentaBrowser/{})",
                env!("CARGO_PKG_VERSION")
            ),
            default_context: ContextOptions::default(),
        }
    }
}

impl fmt::Debug for CdpOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdpOptions")
            .field("bind", &self.bind)
            .field("allow_non_loopback", &self.allow_non_loopback)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("max_connections", &self.max_connections)
            .field("allowed_origins", &self.allowed_origins)
            .field("allowed_hosts", &self.allowed_hosts)
            .field("max_message_bytes", &self.max_message_bytes)
            .field("max_targets", &self.max_targets)
            .field("product", &self.product)
            .finish_non_exhaustive()
    }
}

impl CdpOptions {
    /// The message limit in force: the configured one, else the default for
    /// how exposed the bind is.
    pub fn message_limit(&self) -> usize {
        self.max_message_bytes
            .unwrap_or(if self.bind.ip().is_loopback() {
                256 * 1024 * 1024
            } else {
                16 * 1024 * 1024
            })
    }

    /// The idle limit in force; see [`CdpOptions::idle_timeout`].
    pub fn idle_limit(&self) -> Option<Duration> {
        self.idle_timeout.or_else(|| {
            self.allow_non_loopback
                .then(|| self.command_timeout.max(Duration::from_secs(600)))
        })
    }

    /// Checked before the socket is bound, so a refused configuration never
    /// opens a port.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.max_connections == 0 {
            return Err(Error::invalid("max_connections must be at least 1"));
        }
        if self
            .idle_timeout
            .is_some_and(|idle| idle < self.command_timeout)
        {
            return Err(Error::invalid(
                "idle_timeout must not be shorter than command_timeout",
            ));
        }
        if self.max_message_bytes == Some(0) || self.max_targets == 0 {
            return Err(Error::invalid(
                "max_message_bytes and max_targets must be at least 1",
            ));
        }
        if let Some(token) = &self.token
            && (token.is_empty() || !token.bytes().all(is_url_safe))
        {
            // The token travels in a query string and is compared verbatim,
            // so it must survive a URL unchanged.
            return Err(Error::invalid(
                "the CDP token must be non-empty and use only letters, digits, '-', '_', '.' and '~'",
            ));
        }
        if !self.bind.ip().is_loopback() {
            if !self.allow_non_loopback {
                return Err(Error::invalid(format!(
                    "refusing to bind {} : a non-loopback CDP endpoint needs allow_non_loopback and a token",
                    self.bind
                )));
            }
            match &self.token {
                Some(token) if token.len() >= MIN_REMOTE_TOKEN_LEN => {}
                _ => {
                    return Err(Error::invalid(format!(
                        "a non-loopback CDP endpoint needs a token of at least {MIN_REMOTE_TOKEN_LEN} bytes"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote() -> CdpOptions {
        CdpOptions {
            bind: "192.0.2.1:9222".parse().unwrap(),
            allow_non_loopback: true,
            token: Some("0123456789abcdef".to_owned()),
            ..CdpOptions::default()
        }
    }

    #[test]
    fn loopback_needs_nothing_else() {
        assert!(CdpOptions::default().validate().is_ok());
        let v6 = CdpOptions {
            bind: "[::1]:0".parse().unwrap(),
            ..CdpOptions::default()
        };
        assert!(v6.validate().is_ok());
    }

    #[test]
    fn a_remote_bind_needs_the_explicit_option_and_a_long_token() {
        assert!(remote().validate().is_ok());
        let no_flag = CdpOptions {
            allow_non_loopback: false,
            ..remote()
        };
        assert!(no_flag.validate().is_err());
        let no_token = CdpOptions {
            token: None,
            ..remote()
        };
        assert!(no_token.validate().is_err());
        let short = CdpOptions {
            token: Some("short".to_owned()),
            ..remote()
        };
        assert!(short.validate().is_err());
        let unspecified = CdpOptions {
            bind: "0.0.0.0:9222".parse().unwrap(),
            ..CdpOptions::default()
        };
        assert!(unspecified.validate().is_err());
    }

    #[test]
    fn a_token_must_survive_a_url() {
        for token in ["", "a b", "a&b", "a%20b", "a+b"] {
            let options = CdpOptions {
                token: Some(token.to_owned()),
                ..CdpOptions::default()
            };
            assert!(options.validate().is_err(), "{token:?}");
        }
    }

    #[test]
    fn the_default_message_limit_shrinks_with_exposure() {
        assert_eq!(CdpOptions::default().message_limit(), 256 * 1024 * 1024);
        assert_eq!(remote().message_limit(), 16 * 1024 * 1024);
        let explicit = CdpOptions {
            max_message_bytes: Some(7),
            ..remote()
        };
        assert_eq!(explicit.message_limit(), 7);
    }

    #[test]
    fn the_idle_limit_defaults_by_exposure_and_never_undercuts_commands() {
        assert_eq!(CdpOptions::default().idle_limit(), None);
        assert_eq!(remote().idle_limit(), Some(Duration::from_secs(600)));
        let slow = CdpOptions {
            command_timeout: Duration::from_secs(900),
            ..remote()
        };
        assert_eq!(slow.idle_limit(), Some(Duration::from_secs(900)));
        let too_short = CdpOptions {
            idle_timeout: Some(Duration::from_secs(1)),
            ..remote()
        };
        assert!(too_short.validate().is_err());
        let explicit = CdpOptions {
            idle_timeout: Some(Duration::from_secs(400)),
            ..remote()
        };
        assert_eq!(explicit.idle_limit(), Some(Duration::from_secs(400)));
    }

    #[test]
    fn debug_output_hides_the_token() {
        let shown = format!("{:?}", remote());
        assert!(!shown.contains("0123456789abcdef"));
    }
}
