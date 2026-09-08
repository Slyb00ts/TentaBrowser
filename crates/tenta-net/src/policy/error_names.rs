//! Chromium `net::ERR_*` names (`docs/spec/03-network.md` §7).
//!
//! Playwright and Puppeteer assert on these strings in
//! `Page.navigate.errorText` and `Network.loadingFailed.errorText`, so the
//! spelling and the numeric codes are a public contract, not diagnostics.

use std::fmt;
use std::io;

use tenta_core::Error;

macro_rules! error_names {
    ($( $variant:ident = ($code:expr, $name:literal) ),+ $(,)?) => {
        /// A network failure named the way Chromium names it.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[non_exhaustive]
        pub enum ErrorName {
            $($variant),+
        }

        impl ErrorName {
            /// The name as it appears in CDP payloads, `net::` prefix included.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(ErrorName::$variant => concat!("net::", $name)),+
                }
            }

            /// The bare name without the `net::` prefix.
            pub fn bare_name(self) -> &'static str {
                match self {
                    $(ErrorName::$variant => $name),+
                }
            }

            /// Chromium's negative numeric code.
            pub fn code(self) -> i32 {
                match self {
                    $(ErrorName::$variant => $code),+
                }
            }

            /// Every name in the table, in declaration order.
            pub const ALL: &'static [ErrorName] = &[$(ErrorName::$variant),+];

            /// Parses `net::ERR_FOO` or `ERR_FOO`.
            pub fn parse(text: &str) -> Option<ErrorName> {
                let bare = text.strip_prefix("net::").unwrap_or(text);
                match bare {
                    $($name => Some(ErrorName::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

error_names! {
    Failed = (-2, "ERR_FAILED"),
    Aborted = (-3, "ERR_ABORTED"),
    InvalidArgument = (-4, "ERR_INVALID_ARGUMENT"),
    FileTooBig = (-8, "ERR_FILE_TOO_BIG"),
    NotImplemented = (-11, "ERR_NOT_IMPLEMENTED"),
    InsufficientResources = (-12, "ERR_INSUFFICIENT_RESOURCES"),
    AccessDenied = (-10, "ERR_ACCESS_DENIED"),
    SocketNotConnected = (-15, "ERR_SOCKET_NOT_CONNECTED"),
    BlockedByClient = (-20, "ERR_BLOCKED_BY_CLIENT"),
    NetworkChanged = (-21, "ERR_NETWORK_CHANGED"),
    BlockedByResponse = (-27, "ERR_BLOCKED_BY_RESPONSE"),
    BlockedByCsp = (-30, "ERR_BLOCKED_BY_CSP"),
    TimedOut = (-7, "ERR_TIMED_OUT"),
    ConnectionClosed = (-100, "ERR_CONNECTION_CLOSED"),
    ConnectionReset = (-101, "ERR_CONNECTION_RESET"),
    ConnectionRefused = (-102, "ERR_CONNECTION_REFUSED"),
    ConnectionAborted = (-103, "ERR_CONNECTION_ABORTED"),
    ConnectionFailed = (-104, "ERR_CONNECTION_FAILED"),
    NameNotResolved = (-105, "ERR_NAME_NOT_RESOLVED"),
    InternetDisconnected = (-106, "ERR_INTERNET_DISCONNECTED"),
    SslProtocolError = (-107, "ERR_SSL_PROTOCOL_ERROR"),
    AddressUnreachable = (-109, "ERR_ADDRESS_UNREACHABLE"),
    TunnelConnectionFailed = (-111, "ERR_TUNNEL_CONNECTION_FAILED"),
    ProxyAuthRequested = (-127, "ERR_PROXY_AUTH_REQUESTED"),
    SslVersionOrCipherMismatch = (-113, "ERR_SSL_VERSION_OR_CIPHER_MISMATCH"),
    ConnectionTimedOut = (-118, "ERR_CONNECTION_TIMED_OUT"),
    ProxyConnectionFailed = (-130, "ERR_PROXY_CONNECTION_FAILED"),
    NetworkAccessDenied = (-138, "ERR_NETWORK_ACCESS_DENIED"),
    CertCommonNameInvalid = (-200, "ERR_CERT_COMMON_NAME_INVALID"),
    CertDateInvalid = (-201, "ERR_CERT_DATE_INVALID"),
    CertAuthorityInvalid = (-202, "ERR_CERT_AUTHORITY_INVALID"),
    CertRevoked = (-206, "ERR_CERT_REVOKED"),
    CertInvalid = (-207, "ERR_CERT_INVALID"),
    UnknownUrlScheme = (-302, "ERR_UNKNOWN_URL_SCHEME"),
    InvalidResponse = (-320, "ERR_INVALID_RESPONSE"),
    InvalidChunkedEncoding = (-321, "ERR_INVALID_CHUNKED_ENCODING"),
    UnsafePort = (-312, "ERR_UNSAFE_PORT"),
    TooManyRedirects = (-310, "ERR_TOO_MANY_REDIRECTS"),
    UnsafeRedirect = (-311, "ERR_UNSAFE_REDIRECT"),
    InvalidRedirect = (-303, "ERR_INVALID_REDIRECT"),
    EmptyResponse = (-324, "ERR_EMPTY_RESPONSE"),
    ContentDecodingFailed = (-330, "ERR_CONTENT_DECODING_FAILED"),
    Http2ProtocolError = (-337, "ERR_HTTP2_PROTOCOL_ERROR"),
    ResponseHeadersMultipleContentLength =
        (-346, "ERR_RESPONSE_HEADERS_MULTIPLE_CONTENT_LENGTH"),
    ContentLengthMismatch = (-354, "ERR_CONTENT_LENGTH_MISMATCH"),
    IncompleteChunkedEncoding = (-355, "ERR_INCOMPLETE_CHUNKED_ENCODING"),
    ResponseHeadersTooBig = (-325, "ERR_RESPONSE_HEADERS_TOO_BIG"),
    InsecureResponse = (-501, "ERR_INSECURE_RESPONSE"),
    CacheMiss = (-400, "ERR_CACHE_MISS"),
}

impl ErrorName {
    /// Maps a transport `io::Error` onto the name Chromium would report.
    ///
    /// `ErrorKind` is coarser than Chromium's code space, so the mapping is
    /// the closest name rather than a bijection; unknown kinds become
    /// `ERR_FAILED`, which is what Chromium falls back to as well.
    pub fn from_io_error(error: &io::Error) -> ErrorName {
        use io::ErrorKind as K;
        match error.kind() {
            K::ConnectionRefused => ErrorName::ConnectionRefused,
            K::ConnectionReset => ErrorName::ConnectionReset,
            K::ConnectionAborted => ErrorName::ConnectionAborted,
            K::NotConnected => ErrorName::SocketNotConnected,
            K::BrokenPipe | K::UnexpectedEof => ErrorName::ConnectionClosed,
            K::TimedOut => ErrorName::ConnectionTimedOut,
            K::PermissionDenied => ErrorName::AccessDenied,
            K::AddrNotAvailable | K::AddrInUse => ErrorName::AddressUnreachable,
            K::HostUnreachable | K::NetworkUnreachable => ErrorName::AddressUnreachable,
            K::NetworkDown => ErrorName::InternetDisconnected,
            K::Interrupted => ErrorName::Aborted,
            K::InvalidData => ErrorName::InvalidResponse,
            _ => ErrorName::Failed,
        }
    }

    /// Builds the public [`Error::Net`] carrying this name.
    pub fn into_core_error(self, message: impl Into<String>) -> Error {
        Error::Net {
            code: self.as_str().to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ErrorName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&io::Error> for ErrorName {
    fn from(error: &io::Error) -> Self {
        ErrorName::from_io_error(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_carry_the_net_prefix_and_chromium_codes() {
        let expected: &[(ErrorName, i32, &str)] = &[
            (
                ErrorName::NameNotResolved,
                -105,
                "net::ERR_NAME_NOT_RESOLVED",
            ),
            (
                ErrorName::ConnectionRefused,
                -102,
                "net::ERR_CONNECTION_REFUSED",
            ),
            (
                ErrorName::ConnectionReset,
                -101,
                "net::ERR_CONNECTION_RESET",
            ),
            (ErrorName::TimedOut, -7, "net::ERR_TIMED_OUT"),
            (
                ErrorName::CertCommonNameInvalid,
                -200,
                "net::ERR_CERT_COMMON_NAME_INVALID",
            ),
            (
                ErrorName::CertDateInvalid,
                -201,
                "net::ERR_CERT_DATE_INVALID",
            ),
            (
                ErrorName::CertAuthorityInvalid,
                -202,
                "net::ERR_CERT_AUTHORITY_INVALID",
            ),
            (ErrorName::Aborted, -3, "net::ERR_ABORTED"),
            (
                ErrorName::BlockedByClient,
                -20,
                "net::ERR_BLOCKED_BY_CLIENT",
            ),
            (
                ErrorName::TooManyRedirects,
                -310,
                "net::ERR_TOO_MANY_REDIRECTS",
            ),
            (ErrorName::EmptyResponse, -324, "net::ERR_EMPTY_RESPONSE"),
            (
                ErrorName::Http2ProtocolError,
                -337,
                "net::ERR_HTTP2_PROTOCOL_ERROR",
            ),
            (ErrorName::UnsafePort, -312, "net::ERR_UNSAFE_PORT"),
            (ErrorName::AccessDenied, -10, "net::ERR_ACCESS_DENIED"),
            (
                ErrorName::ProxyConnectionFailed,
                -130,
                "net::ERR_PROXY_CONNECTION_FAILED",
            ),
            (
                ErrorName::TunnelConnectionFailed,
                -111,
                "net::ERR_TUNNEL_CONNECTION_FAILED",
            ),
            (
                ErrorName::SslProtocolError,
                -107,
                "net::ERR_SSL_PROTOCOL_ERROR",
            ),
            (ErrorName::Failed, -2, "net::ERR_FAILED"),
            (ErrorName::NetworkChanged, -21, "net::ERR_NETWORK_CHANGED"),
            (
                ErrorName::ConnectionClosed,
                -100,
                "net::ERR_CONNECTION_CLOSED",
            ),
            (
                ErrorName::ConnectionTimedOut,
                -118,
                "net::ERR_CONNECTION_TIMED_OUT",
            ),
            (
                ErrorName::SocketNotConnected,
                -15,
                "net::ERR_SOCKET_NOT_CONNECTED",
            ),
            (
                ErrorName::BlockedByResponse,
                -27,
                "net::ERR_BLOCKED_BY_RESPONSE",
            ),
            (ErrorName::BlockedByCsp, -30, "net::ERR_BLOCKED_BY_CSP"),
            (
                ErrorName::NetworkAccessDenied,
                -138,
                "net::ERR_NETWORK_ACCESS_DENIED",
            ),
            (
                ErrorName::InvalidResponse,
                -320,
                "net::ERR_INVALID_RESPONSE",
            ),
            (
                ErrorName::InvalidChunkedEncoding,
                -321,
                "net::ERR_INVALID_CHUNKED_ENCODING",
            ),
            (
                ErrorName::ResponseHeadersMultipleContentLength,
                -346,
                "net::ERR_RESPONSE_HEADERS_MULTIPLE_CONTENT_LENGTH",
            ),
            (
                ErrorName::IncompleteChunkedEncoding,
                -355,
                "net::ERR_INCOMPLETE_CHUNKED_ENCODING",
            ),
            (
                ErrorName::ContentLengthMismatch,
                -354,
                "net::ERR_CONTENT_LENGTH_MISMATCH",
            ),
            (
                ErrorName::ContentDecodingFailed,
                -330,
                "net::ERR_CONTENT_DECODING_FAILED",
            ),
            (
                ErrorName::InsecureResponse,
                -501,
                "net::ERR_INSECURE_RESPONSE",
            ),
        ];
        for (name, code, text) in expected {
            assert_eq!(name.code(), *code, "code of {text}");
            assert_eq!(name.as_str(), *text);
            assert_eq!(ErrorName::parse(text), Some(*name));
            assert_eq!(ErrorName::parse(name.bare_name()), Some(*name));
        }
    }

    #[test]
    fn codes_and_names_are_unique() {
        let mut codes: Vec<i32> = ErrorName::ALL.iter().map(|n| n.code()).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), before, "duplicate numeric code in the table");

        let mut names: Vec<&str> = ErrorName::ALL.iter().map(|n| n.as_str()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate name in the table");
    }

    #[test]
    fn unknown_names_do_not_parse() {
        assert_eq!(ErrorName::parse("net::ERR_NOT_A_REAL_ERROR"), None);
        assert_eq!(ErrorName::parse(""), None);
    }

    #[test]
    fn io_errors_map_to_transport_names() {
        use io::ErrorKind as K;
        let cases: &[(K, ErrorName)] = &[
            (K::ConnectionRefused, ErrorName::ConnectionRefused),
            (K::ConnectionReset, ErrorName::ConnectionReset),
            (K::ConnectionAborted, ErrorName::ConnectionAborted),
            (K::NotConnected, ErrorName::SocketNotConnected),
            (K::BrokenPipe, ErrorName::ConnectionClosed),
            (K::UnexpectedEof, ErrorName::ConnectionClosed),
            (K::TimedOut, ErrorName::ConnectionTimedOut),
            (K::PermissionDenied, ErrorName::AccessDenied),
            (K::AddrNotAvailable, ErrorName::AddressUnreachable),
            (K::Interrupted, ErrorName::Aborted),
            (K::InvalidData, ErrorName::InvalidResponse),
            (K::Other, ErrorName::Failed),
        ];
        for (kind, expected) in cases {
            let error = io::Error::new(*kind, "boom");
            assert_eq!(ErrorName::from_io_error(&error), *expected, "{kind:?}");
        }
    }

    #[test]
    fn into_core_error_keeps_the_name() {
        let error = ErrorName::NameNotResolved.into_core_error("no such host");
        match error {
            Error::Net { code, message } => {
                assert_eq!(code, "net::ERR_NAME_NOT_RESOLVED");
                assert_eq!(message, "no such host");
            }
            other => panic!("expected Error::Net, got {other:?}"),
        }
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(ErrorName::UnsafePort.to_string(), "net::ERR_UNSAFE_PORT");
    }
}
