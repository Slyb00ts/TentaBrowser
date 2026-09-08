//! RFC 9111 header parsing and freshness arithmetic.

use std::time::{Duration, SystemTime};

use http::{HeaderMap, HeaderName};

/// Statuses this cache is willing to store.
///
/// RFC 9110 §15.1 also lists 206, but partial responses need range
/// bookkeeping that buys nothing for a browser engine's workload, so they are
/// deliberately left out.
pub const CACHEABLE_STATUS: &[u16] = &[200, 203, 204, 300, 301, 308, 404, 405, 410, 414, 501];

/// Methods that invalidate a stored response for the same URL (RFC 9111 §4.4).
pub const UNSAFE_METHODS: &[&str] = &["POST", "PUT", "DELETE", "PATCH"];

/// Fields a 304 must not copy into the stored response (RFC 9111 §4.3.4).
const NON_UPDATABLE: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
];

pub fn is_cacheable_status(status: u16) -> bool {
    CACHEABLE_STATUS.contains(&status)
}

pub fn is_cacheable_method(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD")
}

pub fn is_unsafe_method(method: &str) -> bool {
    UNSAFE_METHODS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(method))
}

/// The directives of one `Cache-Control` field, request or response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheControl {
    pub no_store: bool,
    pub no_cache: bool,
    /// `no-cache="field"`: only those fields must be revalidated.
    pub no_cache_fields: Vec<String>,
    pub must_revalidate: bool,
    pub proxy_revalidate: bool,
    pub public: bool,
    pub private: bool,
    pub immutable: bool,
    pub only_if_cached: bool,
    pub no_transform: bool,
    pub max_age: Option<u64>,
    pub s_maxage: Option<u64>,
    pub min_fresh: Option<u64>,
    /// `Some(None)` is bare `max-stale`, which accepts any staleness.
    pub max_stale: Option<Option<u64>>,
    pub stale_while_revalidate: Option<u64>,
    pub stale_if_error: Option<u64>,
}

impl CacheControl {
    pub fn parse(headers: &HeaderMap) -> CacheControl {
        let mut cc = CacheControl::default();
        for value in headers.get_all("cache-control") {
            let Ok(text) = value.to_str() else { continue };
            for directive in split_directives(text) {
                cc.apply(directive.0, directive.1.as_deref());
            }
        }
        // `Pragma: no-cache` is HTTP/1.0 but still emitted by real servers.
        if !cc.no_cache
            && headers
                .get_all("pragma")
                .iter()
                .filter_map(|value| value.to_str().ok())
                .any(|text| {
                    text.split(',')
                        .any(|t| t.trim().eq_ignore_ascii_case("no-cache"))
                })
        {
            cc.no_cache = true;
        }
        cc
    }

    fn apply(&mut self, name: &str, value: Option<&str>) {
        let seconds = || value.and_then(|v| v.trim().parse::<u64>().ok());
        match name.to_ascii_lowercase().as_str() {
            "no-store" => self.no_store = true,
            "no-cache" => {
                self.no_cache = true;
                if let Some(fields) = value {
                    self.no_cache_fields
                        .extend(fields.split(',').map(|f| f.trim().to_ascii_lowercase()));
                }
            }
            "must-revalidate" => self.must_revalidate = true,
            "proxy-revalidate" => self.proxy_revalidate = true,
            "public" => self.public = true,
            "private" => self.private = true,
            "immutable" => self.immutable = true,
            "only-if-cached" => self.only_if_cached = true,
            "no-transform" => self.no_transform = true,
            "max-age" => self.max_age = seconds(),
            "s-maxage" => self.s_maxage = seconds(),
            "min-fresh" => self.min_fresh = seconds(),
            "max-stale" => self.max_stale = Some(seconds()),
            "stale-while-revalidate" => self.stale_while_revalidate = seconds(),
            "stale-if-error" => self.stale_if_error = seconds(),
            _ => {}
        }
    }
}

/// Splits a `Cache-Control` field value into `(name, value)` pairs, honouring
/// quoted-string values so that `no-cache="set-cookie, x"` stays one token.
fn split_directives(text: &str) -> Vec<(&str, Option<String>)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut in_quotes = false;
    let mut index = 0;
    while index <= bytes.len() {
        let at_end = index == bytes.len();
        let byte = if at_end { b',' } else { bytes[index] };
        if byte == b'"' && !at_end {
            in_quotes = !in_quotes;
        }
        if byte == b',' && !in_quotes {
            if let Some(directive) = parse_directive(&text[start..index]) {
                out.push(directive);
            }
            start = index + 1;
        }
        index += 1;
    }
    out
}

fn parse_directive(chunk: &str) -> Option<(&str, Option<String>)> {
    let chunk = chunk.trim();
    if chunk.is_empty() {
        return None;
    }
    match chunk.find('=') {
        Some(at) => {
            let name = chunk[..at].trim();
            let raw = chunk[at + 1..].trim();
            let value = raw.trim_matches('"').to_string();
            if name.is_empty() {
                None
            } else {
                Some((name, Some(value)))
            }
        }
        None => Some((chunk, None)),
    }
}

pub fn header_date(headers: &HeaderMap, name: &str) -> Option<SystemTime> {
    let value = headers.get(name)?.to_str().ok()?;
    httpdate::parse_http_date(value).ok()
}

pub fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// RFC 9111 §4.2.3 current age.
pub fn current_age(
    headers: &HeaderMap,
    request_time: SystemTime,
    response_time: SystemTime,
    now: SystemTime,
) -> Duration {
    let date = header_date(headers, "date").unwrap_or(response_time);
    let apparent_age = response_time.duration_since(date).unwrap_or(Duration::ZERO);
    let response_delay = response_time
        .duration_since(request_time)
        .unwrap_or(Duration::ZERO);
    let age_value = Duration::from_secs(header_u64(headers, "age").unwrap_or(0));
    let corrected_age_value = age_value + response_delay;
    let corrected_initial_age = apparent_age.max(corrected_age_value);
    let resident_time = now.duration_since(response_time).unwrap_or(Duration::ZERO);
    corrected_initial_age + resident_time
}

/// RFC 9111 §4.2.1 freshness lifetime. `None` means no lifetime could be
/// derived, which is not the same as zero: it is what enables the heuristic.
pub fn explicit_freshness_lifetime(
    headers: &HeaderMap,
    cc: &CacheControl,
    shared: bool,
    response_time: SystemTime,
) -> Option<Duration> {
    if shared && let Some(seconds) = cc.s_maxage {
        return Some(Duration::from_secs(seconds));
    }
    if let Some(seconds) = cc.max_age {
        return Some(Duration::from_secs(seconds));
    }
    let expires = header_date(headers, "expires")?;
    let date = header_date(headers, "date").unwrap_or(response_time);
    Some(expires.duration_since(date).unwrap_or(Duration::ZERO))
}

/// RFC 9111 §4.2.2 heuristic freshness: a fraction of the time since the
/// resource last changed, clamped so a very old document is not cached
/// forever.
pub fn heuristic_freshness_lifetime(
    headers: &HeaderMap,
    response_time: SystemTime,
    fraction: f64,
    cap: Duration,
) -> Option<Duration> {
    let last_modified = header_date(headers, "last-modified")?;
    let date = header_date(headers, "date").unwrap_or(response_time);
    let age = date.duration_since(last_modified).ok()?;
    Some(age.mul_f64(fraction).min(cap))
}

/// The names a stored response varies on, lowercased. `Some(None)` marks
/// `Vary: *`.
pub fn vary_names(headers: &HeaderMap) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for value in headers.get_all("vary") {
        let Ok(text) = value.to_str() else { continue };
        for name in text.split(',') {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            names.push(name.to_ascii_lowercase());
        }
    }
    if names.is_empty() { None } else { Some(names) }
}

pub fn vary_is_star(headers: &HeaderMap) -> bool {
    vary_names(headers).is_some_and(|names| names.iter().any(|name| name == "*"))
}

/// Snapshots the request header values a response varies on.
pub fn varied_values(
    request_headers: &HeaderMap,
    names: &[String],
) -> Vec<(String, Option<String>)> {
    names
        .iter()
        .filter(|name| *name != "*")
        .map(|name| {
            let value = joined_header(request_headers, name);
            (name.clone(), value)
        })
        .collect()
}

/// Whether a request selects a stored variant (RFC 9111 §4.1).
pub fn varied_matches(request_headers: &HeaderMap, varied: &[(String, Option<String>)]) -> bool {
    varied
        .iter()
        .all(|(name, stored)| joined_header(request_headers, name).as_deref() == stored.as_deref())
}

fn joined_header(headers: &HeaderMap, name: &str) -> Option<String> {
    let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
        return None;
    };
    let mut values = headers.get_all(&name).iter().peekable();
    values.peek()?;
    let joined = values
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(", ");
    Some(joined)
}

/// The conditional request headers that revalidate a stored response.
pub fn conditional_headers(stored: &HeaderMap) -> HeaderMap {
    let mut conditionals = HeaderMap::new();
    if let Some(etag) = stored.get("etag") {
        conditionals.insert("if-none-match", etag.clone());
    }
    if let Some(last_modified) = stored.get("last-modified") {
        conditionals.insert("if-modified-since", last_modified.clone());
    }
    conditionals
}

pub fn has_validator(headers: &HeaderMap) -> bool {
    headers.contains_key("etag") || headers.contains_key("last-modified")
}

/// Applies a 304's headers to the stored ones (RFC 9111 §4.3.4).
pub fn merge_304(stored: &mut HeaderMap, fresh: &HeaderMap) {
    let mut seen: Vec<&HeaderName> = Vec::new();
    for name in fresh.keys() {
        if NON_UPDATABLE.contains(&name.as_str()) {
            continue;
        }
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        stored.remove(name);
        for value in fresh.get_all(name) {
            stored.append(name, value.clone());
        }
    }
}
