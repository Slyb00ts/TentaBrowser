//! Log redaction (`docs/spec/19-security.md` §7).
//!
//! Nothing that reaches a log line at `info` or above may carry credentials,
//! cookies, tokens or query strings. The helpers here produce the redacted
//! rendering; the `--log-unredacted` developer flag is the only thing that
//! may bypass them, and it does so by not calling them.

use url::Url;

/// The header names whose values are always replaced by a placeholder.
const SENSITIVE_HEADERS: &[&str] = &[
    "cookie",
    "set-cookie",
    "authorization",
    "proxy-authorization",
    "www-authenticate",
    "proxy-authenticate",
];

/// Renders a URL for a log line: userinfo, query and fragment removed.
///
/// The path is kept because it is what makes a log line useful; query
/// strings are not, because they carry session tokens on real sites.
pub fn url_for_log(url: &Url) -> String {
    let mut redacted = url.clone();
    // `set_username`/`set_password` fail for cannot-be-a-base URLs, which
    // have no userinfo to strip in the first place.
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);
    redacted.set_query(None);
    redacted.set_fragment(None);
    let mut out: String = redacted.into();
    if url.query().is_some() {
        out = format!("{out}?<redacted>");
    }
    out
}

/// Whether a header value must never be logged verbatim.
pub fn is_sensitive_header(name: &str) -> bool {
    SENSITIVE_HEADERS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// Renders one header for a log line. Sensitive values are replaced by their
/// byte length so that a size regression is still debuggable.
pub fn header_for_log(name: &str, value: &[u8]) -> String {
    if is_sensitive_header(name) {
        return format!("{name}: <redacted {} bytes>", value.len());
    }
    match std::str::from_utf8(value) {
        Ok(text) => format!("{name}: {text}"),
        Err(_) => format!("{name}: <{} non-utf8 bytes>", value.len()),
    }
}

/// Renders a whole header map for a log line.
pub fn headers_for_log(headers: &http::HeaderMap) -> Vec<String> {
    headers
        .iter()
        .map(|(name, value)| header_for_log(name.as_str(), value.as_bytes()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_userinfo_and_query() {
        let url = Url::parse("https://user:secret@example.com/a/b?token=abc#frag").unwrap();
        assert_eq!(url_for_log(&url), "https://example.com/a/b?<redacted>");
    }

    #[test]
    fn keeps_path_and_port_without_query() {
        let url = Url::parse("http://example.com:8080/path").unwrap();
        assert_eq!(url_for_log(&url), "http://example.com:8080/path");
    }

    #[test]
    fn empty_query_is_still_marked() {
        let url = Url::parse("https://example.com/?").unwrap();
        assert_eq!(url_for_log(&url), "https://example.com/?<redacted>");
    }

    #[test]
    fn non_special_scheme_survives() {
        let url = Url::parse("data:text/plain,hello").unwrap();
        assert_eq!(url_for_log(&url), "data:text/plain,hello");
    }

    #[test]
    fn masks_credential_headers() {
        assert_eq!(
            header_for_log("Cookie", b"sid=abc"),
            "Cookie: <redacted 7 bytes>"
        );
        assert_eq!(
            header_for_log("proxy-authorization", b"Basic Zm9v"),
            "proxy-authorization: <redacted 10 bytes>"
        );
        assert_eq!(
            header_for_log("Set-Cookie", b"a=b"),
            "Set-Cookie: <redacted 3 bytes>"
        );
        assert_eq!(header_for_log("Accept", b"text/html"), "Accept: text/html");
    }

    #[test]
    fn non_utf8_values_are_summarised() {
        assert_eq!(
            header_for_log("X-Thing", &[0xff, 0xfe]),
            "X-Thing: <2 non-utf8 bytes>"
        );
    }

    #[test]
    fn header_map_rendering_redacts() {
        let mut headers = http::HeaderMap::new();
        headers.insert("authorization", "Bearer t".parse().unwrap());
        headers.insert("accept", "*/*".parse().unwrap());
        let rendered = headers_for_log(&headers);
        assert!(
            rendered
                .iter()
                .any(|l| l == "authorization: <redacted 8 bytes>")
        );
        assert!(rendered.iter().any(|l| l == "accept: */*"));
    }
}
