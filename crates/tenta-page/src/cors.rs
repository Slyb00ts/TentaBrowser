//! The CORS protocol for the page's `fetch()` and `XMLHttpRequest`
//! (<https://fetch.spec.whatwg.org/#http-cors-protocol>).
//!
//! These are the decisions; `crate::fetch` makes the requests. A request to
//! another origin is sent with an `Origin` header (which `tenta-net` writes
//! from the request's initiator) and without credentials unless the page asked
//! for them, after a preflight when its method or headers are not ones a
//! plain form could have sent. Its response is handed to script only when the
//! server names the page's origin, and then only with the headers the server
//! exposes.

use http::Method;
use url::Url;

/// Why a cross-origin response was not handed to script. Script sees only a
/// network error; this is for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CorsFailure {
    /// No `Access-Control-Allow-Origin`, or one that names another origin.
    Origin(String),
    /// Credentials were sent and the server did not allow them.
    Credentials,
    /// The preflight was not answered with an ok status.
    PreflightStatus(u16),
    /// The preflight did not allow the method.
    Method(String),
    /// The preflight did not allow a request header.
    Header(String),
}

impl std::fmt::Display for CorsFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CorsFailure::Origin(found) => {
                write!(f, "Access-Control-Allow-Origin is {found:?}, which does not allow the page's origin")
            }
            CorsFailure::Credentials => f.write_str(
                "the request carried credentials and the response does not set Access-Control-Allow-Credentials: true",
            ),
            CorsFailure::PreflightStatus(status) => {
                write!(f, "the preflight was answered with HTTP {status}")
            }
            CorsFailure::Method(method) => write!(f, "the preflight does not allow the method {method}"),
            CorsFailure::Header(name) => {
                write!(f, "the preflight does not allow the request header {name}")
            }
        }
    }
}

/// The page's origin as the `Origin` header and `Access-Control-Allow-Origin`
/// spell it.
pub(crate) fn serialized_origin(document: &Url) -> String {
    document.origin().ascii_serialization()
}

/// A byte a CORS-safelisted request-header value may not contain.
fn is_cors_unsafe_byte(byte: u8) -> bool {
    (byte < 0x20 && byte != 0x09)
        || matches!(
            byte,
            b'"' | b'('
                | b')'
                | b':'
                | b'<'
                | b'>'
                | b'?'
                | b'@'
                | b'['
                | b'\\'
                | b']'
                | b'{'
                | b'}'
                | 0x7f
        )
}

/// Whether a `Content-Type` value is one a form could have sent: its essence
/// is `application/x-www-form-urlencoded`, `multipart/form-data` or
/// `text/plain`.
fn is_simple_content_type(value: &str) -> bool {
    let essence = value.split(';').next().unwrap_or("").trim();
    [
        "application/x-www-form-urlencoded",
        "multipart/form-data",
        "text/plain",
    ]
    .iter()
    .any(|simple| essence.eq_ignore_ascii_case(simple))
}

/// Whether `name: value` is a CORS-safelisted request-header: one of the four
/// names, with a short value of harmless bytes.
fn is_safelisted_header(name: &str, value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    match name.to_ascii_lowercase().as_str() {
        "accept" => !value.bytes().any(is_cors_unsafe_byte),
        "accept-language" | "content-language" => value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b' ' | b'*' | b',' | b'-' | b'.' | b';' | b'=')
        }),
        "content-type" => !value.bytes().any(is_cors_unsafe_byte) && is_simple_content_type(value),
        _ => false,
    }
}

/// The names of the author headers that are not CORS-safelisted, lower-cased,
/// sorted and without duplicates: the `Access-Control-Request-Headers` list.
pub(crate) fn unsafe_header_names(headers: &[(String, String)]) -> Vec<String> {
    let mut names: Vec<String> = headers
        .iter()
        .filter(|(name, value)| !is_safelisted_header(name, value))
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Whether the method is CORS-safelisted.
fn is_simple_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::POST)
}

/// Whether a cross-origin request with this method and these author headers
/// needs a preflight.
pub(crate) fn needs_preflight(method: &Method, headers: &[(String, String)]) -> bool {
    !is_simple_method(method) || !unsafe_header_names(headers).is_empty()
}

/// The values of one response header, split at commas and trimmed.
fn list_values<'a>(headers: &'a [(String, Vec<u8>)], name: &str) -> Vec<&'a str> {
    headers
        .iter()
        .filter(|(header, _)| header.eq_ignore_ascii_case(name))
        .filter_map(|(_, value)| std::str::from_utf8(value).ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect()
}

/// The single value of a response header, or `None` when it is absent or
/// given more than once (which the CORS check treats as a failure).
fn single_value<'a>(headers: &'a [(String, Vec<u8>)], name: &str) -> Option<&'a str> {
    let mut values = headers
        .iter()
        .filter(|(header, _)| header.eq_ignore_ascii_case(name))
        .filter_map(|(_, value)| std::str::from_utf8(value).ok());
    let first = values.next()?;
    values.next().is_none().then(|| first.trim())
}

/// The CORS check: the response may be shared with a page at `origin`.
/// `credentials` says whether the request carried them, in which case the
/// wildcard does not do and the server must allow credentials by name.
pub(crate) fn check_response(
    origin: &str,
    credentials: bool,
    headers: &[(String, Vec<u8>)],
) -> Result<(), CorsFailure> {
    let allowed = single_value(headers, "access-control-allow-origin").unwrap_or("");
    let wildcard = allowed == "*" && !credentials;
    if !wildcard && allowed != origin {
        return Err(CorsFailure::Origin(allowed.to_owned()));
    }
    if credentials && single_value(headers, "access-control-allow-credentials") != Some("true") {
        return Err(CorsFailure::Credentials);
    }
    Ok(())
}

/// The CORS-preflight check: the preflight's response allows the real
/// request's origin, method and unsafe headers.
pub(crate) fn check_preflight(
    origin: &str,
    credentials: bool,
    method: &Method,
    unsafe_headers: &[String],
    status: u16,
    headers: &[(String, Vec<u8>)],
) -> Result<(), CorsFailure> {
    check_response(origin, credentials, headers)?;
    if !(200..300).contains(&status) {
        return Err(CorsFailure::PreflightStatus(status));
    }
    let methods = list_values(headers, "access-control-allow-methods");
    let method_allowed = is_simple_method(method)
        || methods.iter().any(|allowed| *allowed == method.as_str())
        || (!credentials && methods.contains(&"*"));
    if !method_allowed {
        return Err(CorsFailure::Method(method.to_string()));
    }
    let allowed_headers = list_values(headers, "access-control-allow-headers");
    let wildcard = !credentials && allowed_headers.contains(&"*");
    for name in unsafe_headers {
        // The wildcard never covers `Authorization`.
        let by_wildcard = wildcard && name != "authorization";
        let by_name = allowed_headers
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(name));
        if !by_wildcard && !by_name {
            return Err(CorsFailure::Header(name.clone()));
        }
    }
    Ok(())
}

/// The response headers script may read from a cross-origin response: the
/// CORS-safelisted ones and those the server exposes.
pub(crate) fn exposed_headers(
    credentials: bool,
    headers: Vec<(String, Vec<u8>)>,
) -> Vec<(String, Vec<u8>)> {
    const SAFELISTED: [&str; 7] = [
        "cache-control",
        "content-language",
        "content-length",
        "content-type",
        "expires",
        "last-modified",
        "pragma",
    ];
    let exposed: Vec<String> = list_values(&headers, "access-control-expose-headers")
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect();
    let all = !credentials && exposed.iter().any(|name| name == "*");
    headers
        .into_iter()
        .filter(|(name, _)| {
            let name = name.to_ascii_lowercase();
            // Cookies are never script's to read, whatever the server says.
            if name == "set-cookie" || name == "set-cookie2" {
                return false;
            }
            all || SAFELISTED.contains(&name.as_str()) || exposed.contains(&name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn response(list: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
        list.iter()
            .map(|(name, value)| ((*name).to_owned(), value.as_bytes().to_vec()))
            .collect()
    }

    const ORIGIN: &str = "https://app.example";

    #[test]
    fn simple_requests_need_no_preflight() {
        for method in [Method::GET, Method::HEAD, Method::POST] {
            assert!(!needs_preflight(&method, &[]));
        }
        let form = pairs(&[
            (
                "Content-Type",
                "application/x-www-form-urlencoded; charset=UTF-8",
            ),
            ("Accept", "application/json"),
            ("Accept-Language", "pl-PL,pl;q=0.9"),
        ]);
        assert!(!needs_preflight(&Method::POST, &form));
    }

    #[test]
    fn other_methods_and_headers_need_a_preflight() {
        assert!(needs_preflight(&Method::PUT, &[]));
        assert!(needs_preflight(&Method::DELETE, &[]));
        let json = pairs(&[("content-type", "application/json")]);
        assert!(needs_preflight(&Method::POST, &json));
        let custom = pairs(&[
            ("X-Requested-With", "XMLHttpRequest"),
            ("Authorization", "Bearer t"),
            ("x-requested-with", "again"),
            ("Accept", "text/html"),
        ]);
        assert_eq!(
            unsafe_header_names(&custom),
            ["authorization", "x-requested-with"],
            "lower-cased, sorted, each once, without the safelisted ones"
        );
        let long = pairs(&[("Accept", &"a".repeat(129))]);
        assert!(
            needs_preflight(&Method::GET, &long),
            "a long value is not safelisted"
        );
        let odd = pairs(&[("Accept-Language", "en<script>")]);
        assert!(needs_preflight(&Method::GET, &odd));
    }

    #[test]
    fn a_response_is_shared_only_with_the_origin_it_names() {
        assert_eq!(
            check_response(
                ORIGIN,
                false,
                &response(&[("access-control-allow-origin", ORIGIN)])
            ),
            Ok(())
        );
        assert_eq!(
            check_response(
                ORIGIN,
                false,
                &response(&[("Access-Control-Allow-Origin", "*")])
            ),
            Ok(())
        );
        for headers in [
            response(&[]),
            response(&[("access-control-allow-origin", "https://other.example")]),
            response(&[("access-control-allow-origin", "https://app.example/")]),
            response(&[("access-control-allow-origin", "null")]),
            response(&[
                ("access-control-allow-origin", ORIGIN),
                ("access-control-allow-origin", ORIGIN),
            ]),
        ] {
            assert!(
                matches!(
                    check_response(ORIGIN, false, &headers),
                    Err(CorsFailure::Origin(_))
                ),
                "{headers:?}"
            );
        }
    }

    #[test]
    fn credentials_need_the_origin_by_name_and_the_servers_consent() {
        let named = response(&[
            ("access-control-allow-origin", ORIGIN),
            ("access-control-allow-credentials", "true"),
        ]);
        assert_eq!(check_response(ORIGIN, true, &named), Ok(()));
        assert!(matches!(
            check_response(
                ORIGIN,
                true,
                &response(&[("access-control-allow-origin", "*")])
            ),
            Err(CorsFailure::Origin(_))
        ));
        assert_eq!(
            check_response(
                ORIGIN,
                true,
                &response(&[("access-control-allow-origin", ORIGIN)])
            ),
            Err(CorsFailure::Credentials)
        );
        let wrong_case = response(&[
            ("access-control-allow-origin", ORIGIN),
            ("access-control-allow-credentials", "True"),
        ]);
        assert_eq!(
            check_response(ORIGIN, true, &wrong_case),
            Err(CorsFailure::Credentials)
        );
    }

    #[test]
    fn a_preflight_must_allow_the_method_and_every_unsafe_header() {
        let unsafe_headers = vec!["authorization".to_owned(), "content-type".to_owned()];
        let allowing = response(&[
            ("access-control-allow-origin", ORIGIN),
            ("access-control-allow-methods", "GET, PUT"),
            (
                "access-control-allow-headers",
                "Content-Type, Authorization",
            ),
        ]);
        assert_eq!(
            check_preflight(ORIGIN, false, &Method::PUT, &unsafe_headers, 204, &allowing),
            Ok(())
        );
        assert_eq!(
            check_preflight(
                ORIGIN,
                false,
                &Method::DELETE,
                &unsafe_headers,
                204,
                &allowing
            ),
            Err(CorsFailure::Method("DELETE".to_owned()))
        );
        assert_eq!(
            check_preflight(ORIGIN, false, &Method::PUT, &unsafe_headers, 403, &allowing),
            Err(CorsFailure::PreflightStatus(403))
        );
        let no_auth = response(&[
            ("access-control-allow-origin", ORIGIN),
            ("access-control-allow-methods", "*"),
            ("access-control-allow-headers", "*"),
        ]);
        assert_eq!(
            check_preflight(ORIGIN, false, &Method::PUT, &unsafe_headers, 200, &no_auth),
            Err(CorsFailure::Header("authorization".to_owned())),
            "the wildcard does not cover Authorization"
        );
        assert_eq!(
            check_preflight(
                ORIGIN,
                false,
                &Method::PUT,
                &["x-custom".to_owned()],
                200,
                &no_auth
            ),
            Ok(())
        );
        // A simple method needs no Access-Control-Allow-Methods.
        let headers_only = response(&[
            ("access-control-allow-origin", ORIGIN),
            ("access-control-allow-headers", "x-custom"),
        ]);
        assert_eq!(
            check_preflight(
                ORIGIN,
                false,
                &Method::POST,
                &["x-custom".to_owned()],
                200,
                &headers_only
            ),
            Ok(())
        );
    }

    #[test]
    fn only_safelisted_and_exposed_headers_reach_script() {
        let headers = response(&[
            ("content-type", "application/json"),
            ("x-request-id", "abc"),
            ("x-secret", "hidden"),
            ("set-cookie", "sid=1"),
            ("access-control-expose-headers", "X-Request-Id, Set-Cookie"),
            ("cache-control", "no-store"),
        ]);
        let names: Vec<String> = exposed_headers(false, headers.clone())
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["content-type", "x-request-id", "cache-control"]);
        let mut everything = headers;
        everything.push(("access-control-expose-headers".to_owned(), b"*".to_vec()));
        let all: Vec<String> = exposed_headers(false, everything.clone())
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(all.contains(&"x-secret".to_owned()) && !all.contains(&"set-cookie".to_owned()));
        let with_credentials: Vec<String> = exposed_headers(true, everything)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(
            !with_credentials.contains(&"x-secret".to_owned()),
            "the wildcard does not expose headers of a credentialed response"
        );
    }

    #[test]
    fn the_origin_is_serialised_without_a_path() {
        let document = Url::parse("https://app.example:8443/dir/page.html?x=1").unwrap();
        assert_eq!(serialized_origin(&document), "https://app.example:8443");
    }
}
