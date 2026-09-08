//! The RFC 6265bis parsing and matching algorithms.
//!
//! Every step is deliberately close to the text of the draft: cookie parsing
//! is the one place where "sensible" differs from "what browsers do", and the
//! difference is a security boundary.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use time::{Date, Month, PrimitiveDateTime, Time};
use url::{Host, Url};

use super::{Cookie, PartitionKey, SameSite, Site};

/// Combined `name=value` limit (RFC 6265bis §5.6): a longer `Set-Cookie` is
/// ignored outright rather than truncated.
pub const MAX_NAME_VALUE_BYTES: usize = 4096;

/// Per-attribute value limit (RFC 6265bis §5.6): a longer attribute value
/// makes the cookie-av ignored, not the whole cookie.
pub const MAX_ATTRIBUTE_BYTES: usize = 1024;

/// The largest expiry we will represent; a cookie asking for more is pinned
/// here so that arithmetic cannot overflow `SystemTime`.
pub fn far_future() -> SystemTime {
    // 9999-12-31T23:59:59Z.
    UNIX_EPOCH + Duration::from_secs(253_402_300_799)
}

/// Parses one `Set-Cookie` header value.
///
/// `partition` is the top-level site the response was loaded under; it is
/// only used when the cookie carries `Partitioned` (CHIPS). Callers without a
/// top-level site pass `None`, which makes `Partitioned` a no-op.
pub fn parse_set_cookie_in(
    header: &[u8],
    request_url: &Url,
    now: SystemTime,
    partition: Option<&Site>,
) -> Option<Cookie> {
    // A `Set-Cookie` value is not required to be UTF-8; browsers treat the
    // bytes as opaque and so do we, decoding lossily for storage.
    let text = String::from_utf8_lossy(header);
    let text = text.as_ref();

    let (name_value, unparsed) = match text.find(';') {
        Some(at) => (&text[..at], &text[at + 1..]),
        None => (text, ""),
    };

    let at = name_value.find('=')?;
    let name = name_value[..at].trim_matches(is_wsp);
    let value = name_value[at + 1..].trim_matches(is_wsp);
    if name.is_empty() && value.is_empty() {
        return None;
    }
    if name.len() + value.len() > MAX_NAME_VALUE_BYTES {
        return None;
    }
    if has_forbidden_control(name) || has_forbidden_control(value) {
        return None;
    }

    let attributes = parse_attributes(unparsed);

    let mut expiry: Option<SystemTime> = None;
    let mut persistent = false;
    let mut domain_attribute: Option<String> = None;
    let mut path_attribute: Option<String> = None;
    let mut secure = false;
    let mut http_only = false;
    let mut same_site: Option<SameSite> = None;
    let mut partitioned = false;

    for (attribute, attribute_value) in attributes {
        if attribute.eq_ignore_ascii_case("expires") {
            // Max-Age wins over Expires regardless of order, so an already
            // set persistent expiry is not overwritten here.
            if !persistent && let Some(when) = parse_cookie_date(attribute_value) {
                expiry = Some(when);
            }
        } else if attribute.eq_ignore_ascii_case("max-age") {
            if let Some(when) = max_age_expiry(attribute_value, now) {
                expiry = Some(when);
                persistent = true;
            }
        } else if attribute.eq_ignore_ascii_case("domain") {
            if !attribute_value.is_empty() {
                let domain = attribute_value.strip_prefix('.').unwrap_or(attribute_value);
                domain_attribute = Some(domain.to_ascii_lowercase());
            }
        } else if attribute.eq_ignore_ascii_case("path") {
            if attribute_value.starts_with('/') {
                path_attribute = Some(attribute_value.to_string());
            } else {
                // An empty or relative Path means "use the default path".
                path_attribute = None;
            }
        } else if attribute.eq_ignore_ascii_case("secure") {
            secure = true;
        } else if attribute.eq_ignore_ascii_case("httponly") {
            http_only = true;
        } else if attribute.eq_ignore_ascii_case("samesite") {
            same_site = Some(parse_same_site(attribute_value));
        } else if attribute.eq_ignore_ascii_case("partitioned") {
            partitioned = true;
        }
    }

    let host = canonical_host(request_url)?;
    let secure_request = is_secure_request(request_url);

    // §5.7 storage model: the domain attribute must not be a public suffix
    // unless it is exactly the request host, or one site could set cookies
    // for every site under a registry.
    let (domain, host_only) = match domain_attribute {
        Some(domain) => {
            // An IP host may only ever name itself, and a public suffix may
            // only be named by the registry's own host; both cases yield a
            // host cookie, as in Chromium's `GetCookieDomainWithString`.
            if host_is_ip(&host) || is_public_suffix(&domain) {
                if domain == host {
                    (host.clone(), true)
                } else {
                    return None;
                }
            } else if domain_matches(&host, &domain) {
                (domain, false)
            } else {
                return None;
            }
        }
        None => (host.clone(), true),
    };

    let path = path_attribute.unwrap_or_else(|| default_path(request_url));

    let same_site = same_site.unwrap_or(SameSite::Lax);
    if same_site == SameSite::None && !secure {
        return None;
    }
    if secure && !secure_request {
        return None;
    }
    if has_prefix(name, "__Secure-") && (!secure || !secure_request) {
        return None;
    }
    if has_prefix(name, "__Host-") && (!secure || !secure_request || !host_only || path != "/") {
        return None;
    }
    if partitioned && !secure {
        return None;
    }

    let partition_key = if partitioned {
        partition.cloned().map(PartitionKey)
    } else {
        None
    };

    Some(Cookie {
        name: name.to_string(),
        value: value.to_string(),
        domain,
        host_only,
        path,
        secure,
        http_only,
        same_site,
        partition_key,
        expires: expiry,
        creation: now,
        last_access: now,
        persistent: persistent || expiry.is_some(),
    })
}

/// Splits the unparsed attribute string into `(name, value)` pairs, applying
/// the per-attribute size limit.
fn parse_attributes(unparsed: &str) -> Vec<(&str, &str)> {
    let mut attributes = Vec::new();
    for chunk in unparsed.split(';') {
        let (name, value) = match chunk.find('=') {
            Some(at) => (&chunk[..at], &chunk[at + 1..]),
            None => (chunk, ""),
        };
        let name = name.trim_matches(is_wsp);
        let value = value.trim_matches(is_wsp);
        if name.is_empty() {
            continue;
        }
        if value.len() > MAX_ATTRIBUTE_BYTES {
            continue;
        }
        attributes.push((name, value));
    }
    attributes
}

fn parse_same_site(value: &str) -> SameSite {
    if value.eq_ignore_ascii_case("strict") {
        SameSite::Strict
    } else if value.eq_ignore_ascii_case("none") {
        SameSite::None
    } else {
        // Including "Lax" and anything unrecognised: the default enforcement
        // is Lax (RFC 6265bis §5.4.7).
        SameSite::Lax
    }
}

fn max_age_expiry(value: &str, now: SystemTime) -> Option<SystemTime> {
    let bytes = value.as_bytes();
    let first = *bytes.first()?;
    if !first.is_ascii_digit() && first != b'-' {
        return None;
    }
    if !bytes[1..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    if first == b'-' && bytes.len() == 1 {
        return None;
    }
    match value.parse::<i64>() {
        Ok(seconds) if seconds <= 0 => Some(UNIX_EPOCH),
        Ok(seconds) => Some(
            now.checked_add(Duration::from_secs(seconds as u64))
                .unwrap_or_else(far_future)
                .min(far_future()),
        ),
        // Out of `i64` range: a huge positive value, or a huge negative one.
        Err(_) if first == b'-' => Some(UNIX_EPOCH),
        Err(_) => Some(far_future()),
    }
}

/// Cookie name prefixes are matched case-insensitively (RFC 6265bis §4.1.3).
pub fn has_prefix(name: &str, prefix: &str) -> bool {
    name.len() >= prefix.len()
        && name.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn is_wsp(ch: char) -> bool {
    ch == ' ' || ch == '\t'
}

/// CTLs other than HTAB are not allowed in a cookie name or value
/// (RFC 6265bis §5.7): they are how header injection gets through.
fn has_forbidden_control(text: &str) -> bool {
    text.chars()
        .any(|ch| (ch < '\u{09}') || ('\u{0a}'..='\u{1f}').contains(&ch) || ch == '\u{7f}')
}

/// The RFC 6265 §5.1.1 cookie-date algorithm.
///
/// It is deliberately permissive: it scans tokens for something that looks
/// like a time, a day, a month and a year in any order, which is why it
/// accepts the many malformed `Expires` values sites emit.
pub fn parse_cookie_date(value: &str) -> Option<SystemTime> {
    let mut time_of_day: Option<(u8, u8, u8)> = None;
    let mut day: Option<u8> = None;
    let mut month: Option<u8> = None;
    let mut year: Option<i32> = None;

    for token in value.split(is_date_delimiter) {
        if token.is_empty() {
            continue;
        }
        if time_of_day.is_none()
            && let Some(parsed) = parse_time(token)
        {
            time_of_day = Some(parsed);
            continue;
        }
        if day.is_none()
            && let Some(parsed) = leading_digits(token, 1, 2)
        {
            day = Some(parsed as u8);
            continue;
        }
        if month.is_none()
            && let Some(parsed) = parse_month(token)
        {
            month = Some(parsed);
            continue;
        }
        if year.is_none()
            && let Some(parsed) = leading_digits(token, 2, 4)
        {
            year = Some(parsed as i32);
            continue;
        }
    }

    let (hour, minute, second) = time_of_day?;
    let day = day?;
    let month = month?;
    let mut year = year?;
    if (70..=99).contains(&year) {
        year += 1900;
    } else if (0..=69).contains(&year) {
        year += 2000;
    }

    if !(1..=31).contains(&day) || year < 1601 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let date = Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()?;
    let clock = Time::from_hms(hour, minute, second).ok()?;
    let stamp = PrimitiveDateTime::new(date, clock).assume_utc();
    Some(SystemTime::from(stamp))
}

/// %x09 / %x20-2F / %x3B-40 / %x5B-60 / %x7B-7E (RFC 6265 §5.1.1).
fn is_date_delimiter(ch: char) -> bool {
    ch == '\u{09}'
        || ('\u{20}'..='\u{2f}').contains(&ch)
        || ('\u{3b}'..='\u{40}').contains(&ch)
        || ('\u{5b}'..='\u{60}').contains(&ch)
        || ('\u{7b}'..='\u{7e}').contains(&ch)
}

fn parse_time(token: &str) -> Option<(u8, u8, u8)> {
    let bytes = token.as_bytes();
    let mut at = 0;
    let mut parts = [0u8; 3];
    for (index, part) in parts.iter_mut().enumerate() {
        if index > 0 {
            if bytes.get(at) != Some(&b':') {
                return None;
            }
            at += 1;
        }
        let start = at;
        while at < bytes.len() && at - start < 2 && bytes[at].is_ascii_digit() {
            at += 1;
        }
        if at == start {
            return None;
        }
        *part = token[start..at].parse().ok()?;
    }
    // Trailing characters are allowed as long as they are not digits.
    if bytes.get(at).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    Some((parts[0], parts[1], parts[2]))
}

fn leading_digits(token: &str, min: usize, max: usize) -> Option<u32> {
    let bytes = token.as_bytes();
    let mut at = 0;
    while at < bytes.len() && at < max && bytes[at].is_ascii_digit() {
        at += 1;
    }
    if at < min {
        return None;
    }
    if bytes.get(at).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    token[..at].parse().ok()
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn parse_month(token: &str) -> Option<u8> {
    let bytes = token.as_bytes();
    if bytes.len() < 3 {
        return None;
    }
    // Byte comparison: the token may hold multi-byte UTF-8 and slicing it at
    // a fixed length would split a character.
    let prefix = &bytes[..3];
    MONTHS
        .iter()
        .position(|month| prefix.eq_ignore_ascii_case(month.as_bytes()))
        .map(|index| index as u8 + 1)
}

/// The canonicalised request host: lowercase, no trailing dot, no brackets.
pub fn canonical_host(url: &Url) -> Option<String> {
    match url.host()? {
        Host::Domain(domain) => Some(domain.trim_end_matches('.').to_ascii_lowercase()),
        Host::Ipv4(v4) => Some(v4.to_string()),
        Host::Ipv6(v6) => Some(v6.to_string()),
    }
}

/// Whether the host is a literal IP address, which can only ever hold
/// host-only cookies (RFC 6265bis §5.1.3).
pub fn host_is_ip(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
}

/// Whether a `Set-Cookie` from this URL may set `Secure` cookies.
///
/// `http://localhost` counts as a secure context, as in Chromium: developers
/// would otherwise be unable to test `Secure` cookies locally.
pub fn is_secure_request(url: &Url) -> bool {
    match url.scheme() {
        "https" | "wss" => true,
        // Chromium treats loopback as a trustworthy origin so that `Secure`
        // cookies are testable locally; no other private address qualifies.
        "http" | "ws" => url.host_str().is_some_and(is_loopback_host),
        _ => false,
    }
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .trim_end_matches('.')
        .trim_matches(|c| c == '[' || c == ']');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return ip.is_loopback();
    }
    host.eq_ignore_ascii_case("localhost") || {
        let lower = host.to_ascii_lowercase();
        lower.ends_with(".localhost")
    }
}

/// RFC 6265 §5.1.4 default-path.
pub fn default_path(url: &Url) -> String {
    let path = url.path();
    if !path.starts_with('/') {
        return "/".to_string();
    }
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(at) => path[..at].to_string(),
    }
}

/// RFC 6265 §5.1.3 domain matching.
pub fn domain_matches(host: &str, domain: &str) -> bool {
    if host == domain {
        return true;
    }
    if host_is_ip(host) {
        return false;
    }
    host.len() > domain.len()
        && host.ends_with(domain)
        && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
}

/// RFC 6265 §5.1.4 path matching.
pub fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/') || request_path.as_bytes()[cookie_path.len()] == b'/'
}

/// Whether the name is exactly a public suffix, so no site may claim it.
///
/// An IP literal is never a suffix, and an unknown single-label name (`local`,
/// an intranet host) is one under the list's `*` rule, which is why the caller
/// still allows a domain attribute that equals the request host.
pub fn is_public_suffix(domain: &str) -> bool {
    if domain.is_empty() || host_is_ip(domain) {
        return false;
    }
    let lower = domain.to_ascii_lowercase();
    match psl::suffix_str(&lower) {
        Some(suffix) => suffix == lower,
        None => false,
    }
}

/// The registrable domain of a host, falling back to the host itself for IP
/// literals and for names that have no registrable domain.
pub fn registrable_domain(host: &str) -> String {
    if host_is_ip(host) {
        return host.to_string();
    }
    let lower = host.to_ascii_lowercase();
    psl::domain_str(&lower).map(str::to_string).unwrap_or(lower)
}
