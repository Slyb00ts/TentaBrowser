//! Request header assembly: which headers are sent, in which order, with
//! which casing (`docs/spec/03-network.md` §2).
//!
//! The profile's `[headers] order` (navigation) and `subresource_order` are
//! the wire order; names carry the casing the browser uses on HTTP/1.1 and are
//! lower-cased for HTTP/2. Caller headers override profile values of the same
//! name and, when the profile does not name them, follow in the order the
//! caller added them.
//!
//! Everything Fetch derives per request — `Sec-Fetch-Site/Mode/User/Dest`,
//! `Referer`, `Origin`, `Priority` — is computed here from the request's
//! relationship to its initiator, not read from the profile as a constant.
//! The profile pins the ordering, the casing and the "no initiator" defaults.
//! On an origin that is not potentially trustworthy Chrome sends no client
//! hints, no `Sec-Fetch-*` and only `gzip, deflate`, which
//! `profiles/captures/chrome_144_macos/http1_request.txt` records and
//! [`build`] reproduces.

use http::{HeaderMap, HeaderName, HeaderValue};
use url::Url;

use crate::policy::cookies::is_secure_request;
use crate::profile::{HeaderProfile, Profile};
use crate::proxy::format_authority;
use crate::request::{
    FetchSite, ReferrerPolicy, Request, RequestMode, ResourceType, origin_of, same_origin,
};
use crate::stream::Destination;

/// Which wire protocol the headers are being built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireProtocol {
    Http11,
    Http2,
}

/// Headers connection management owns; a profile may name them but a caller
/// may not set them, and HTTP/2 forbids them entirely (RFC 9113 §8.2.2).
const CONNECTION_HEADERS: &[&str] = &[
    "connection",
    "proxy-connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
];

/// Names Chrome sends only to a potentially trustworthy origin.
const SECURE_ONLY_HEADERS: &[&str] = &[
    "sec-ch-ua",
    "sec-ch-ua-mobile",
    "sec-ch-ua-platform",
    "sec-fetch-site",
    "sec-fetch-mode",
    "sec-fetch-user",
    "sec-fetch-dest",
    "sec-fetch-storage-access",
];

/// Everything the request implies that the profile alone cannot say.
struct Derived {
    /// The origin is potentially trustworthy (`https:`, or loopback).
    trustworthy: bool,
    site: FetchSite,
    referer: Option<String>,
    origin: Option<String>,
    hints: ClientHints,
}

/// Where the `sec-ch-ua*` triplet comes from.
///
/// The hints and the `User-Agent` describe one identity, so a caller that
/// overrides the `User-Agent` cannot be left with the profile's hints. A
/// Chromium user agent has its hints rebuilt from its own version and
/// platform; anything else drops them, because no non-Chromium browser sends
/// them at all.
enum ClientHints {
    Profile,
    Derived {
        brands: String,
        mobile: String,
        platform: String,
    },
    None,
}

impl ClientHints {
    /// The hints implied by a caller's `User-Agent`, if there is one.
    fn for_override(headers: &HeaderProfile, user_agent: Option<&str>) -> Self {
        let Some(user_agent) = user_agent else {
            return ClientHints::Profile;
        };
        if user_agent == headers.user_agent {
            return ClientHints::Profile;
        }
        let Some(major) = chromium_major_version(user_agent) else {
            return ClientHints::None;
        };
        let brands = match chromium_major_version(&headers.user_agent) {
            Some(profile_major) if profile_major != major => headers
                .sec_ch_ua
                .replace(&format!("v=\"{profile_major}\""), &format!("v=\"{major}\"")),
            _ => headers.sec_ch_ua.clone(),
        };
        ClientHints::Derived {
            brands,
            mobile: if user_agent.contains("Mobile") {
                "?1".to_owned()
            } else {
                "?0".to_owned()
            },
            platform: format!("\"{}\"", platform_of(user_agent)),
        }
    }
}

/// The `Chrome/<major>` version a user agent claims, if it claims one.
fn chromium_major_version(user_agent: &str) -> Option<u32> {
    let rest = user_agent.split("Chrome/").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The `Sec-Ch-Ua-Platform` value a user agent implies (Chromium's
/// `GetUserAgentPlatform`).
fn platform_of(user_agent: &str) -> &'static str {
    if user_agent.contains("Android") {
        "Android"
    } else if user_agent.contains("iPhone") || user_agent.contains("iPad") {
        "iOS"
    } else if user_agent.contains("Windows") {
        "Windows"
    } else if user_agent.contains("Macintosh") || user_agent.contains("Mac OS X") {
        "macOS"
    } else if user_agent.contains("CrOS") {
        "Chrome OS"
    } else if user_agent.contains("Linux") || user_agent.contains("X11") {
        "Linux"
    } else {
        "Unknown"
    }
}

/// The request headers, in wire order.
pub fn build(
    profile: &Profile,
    request: &Request,
    destination: &Destination,
    protocol: WireProtocol,
    cookie: Option<String>,
    body_length: Option<u64>,
) -> Vec<(String, HeaderValue)> {
    let mut ordered: Vec<(String, HeaderValue)> = Vec::new();
    let mut caller: Vec<(String, HeaderValue)> = request.headers.clone();
    let derived = Derived {
        trustworthy: is_secure_request(&request.url),
        site: request.fetch_site(),
        referer: referer_for(request),
        origin: request
            .sends_origin()
            .then(|| request.initiator.as_ref().and_then(origin_of))
            .flatten(),
        hints: match profile.client_profile() {
            Some(client_profile) => ClientHints::for_override(
                &client_profile.headers,
                request
                    .header_value("user-agent")
                    .and_then(|value| value.to_str().ok()),
            ),
            None => ClientHints::Profile,
        },
    };

    if let Some(client_profile) = profile.client_profile() {
        let headers = &client_profile.headers;
        for name in headers.order_for(request.resource_type) {
            let lower = name.to_ascii_lowercase();
            if protocol == WireProtocol::Http2 && is_connection_header(&lower) {
                continue;
            }
            // A caller value wins over the profile's.
            if let Some(index) = caller
                .iter()
                .position(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            {
                let (_, value) = caller.remove(index);
                push(&mut ordered, name, value);
                continue;
            }
            if !derived.trustworthy && SECURE_ONLY_HEADERS.contains(&lower.as_str()) {
                continue;
            }
            if let Some(value) = profile_value(
                &lower,
                headers,
                request,
                destination,
                protocol,
                cookie.as_deref(),
                body_length,
                &derived,
            ) {
                push(&mut ordered, name, value);
            }
        }
    }

    // Caller headers the profile does not name keep their own order.
    for (name, value) in caller {
        if protocol == WireProtocol::Http2 && is_connection_header(&name.to_ascii_lowercase()) {
            continue;
        }
        push(&mut ordered, &name, value);
    }

    // Whatever the profile and the caller left out but the protocol needs.
    if protocol == WireProtocol::Http11 && !contains(&ordered, "host") {
        let authority = format_authority(&destination.host, destination.port);
        let authority = strip_default_port(&authority, destination);
        if let Ok(value) = HeaderValue::from_str(&authority) {
            // Host goes first when the profile did not place it, which is
            // where every HTTP/1.1 client puts it.
            ordered.insert(0, ("Host".to_owned(), value));
        }
    }
    if profile.client_profile().is_none()
        && protocol == WireProtocol::Http11
        && !contains(&ordered, "connection")
    {
        push(
            &mut ordered,
            "Connection",
            HeaderValue::from_static("keep-alive"),
        );
    }
    // `Origin` is part of the CORS protocol rather than of a fingerprint: a
    // server decides whether to share its response by it, so a request that
    // owes one carries it even when no profile placed it.
    if let Some(origin) = &derived.origin
        && !contains(&ordered, "origin")
        && let Ok(value) = HeaderValue::from_str(origin)
    {
        push(&mut ordered, "Origin", value);
    }
    if let Some(cookie) = cookie
        && !contains(&ordered, "cookie")
        && let Ok(value) = HeaderValue::from_str(&cookie)
    {
        push(&mut ordered, "Cookie", value);
    }
    if let Some(length) = body_length
        && !contains(&ordered, "content-length")
        && sends_content_length(request, length)
    {
        push(&mut ordered, "Content-Length", HeaderValue::from(length));
    }

    if protocol == WireProtocol::Http2 {
        for (name, _) in ordered.iter_mut() {
            *name = name.to_ascii_lowercase();
        }
    }
    ordered
}

/// A zero-length body still gets a `Content-Length` when the method implies one.
fn sends_content_length(request: &Request, length: u64) -> bool {
    length > 0 || request.method == http::Method::POST || request.method == http::Method::PUT
}

/// The value the profile supplies for one header name.
#[allow(clippy::too_many_arguments)]
fn profile_value(
    lower: &str,
    headers: &HeaderProfile,
    request: &Request,
    destination: &Destination,
    protocol: WireProtocol,
    cookie: Option<&str>,
    body_length: Option<u64>,
    derived: &Derived,
) -> Option<HeaderValue> {
    let text = match lower {
        "host" => {
            if protocol == WireProtocol::Http2 {
                return None;
            }
            let authority = format_authority(&destination.host, destination.port);
            strip_default_port(&authority, destination)
        }
        "connection" => {
            if protocol == WireProtocol::Http2 {
                return None;
            }
            "keep-alive".to_owned()
        }
        "cookie" => cookie?.to_owned(),
        "content-length" => {
            let length = body_length?;
            if !sends_content_length(request, length) {
                return None;
            }
            length.to_string()
        }
        // Only a caller (or the cache) sets these; the profile just places them.
        "content-type" | "cache-control" => return None,
        "origin" => derived.origin.clone()?,
        "referer" => derived.referer.clone()?,
        "priority" => {
            // RFC 9218; Chrome sends it on HTTP/2 and HTTP/3 only.
            if protocol != WireProtocol::Http2 {
                return None;
            }
            headers
                .priority
                .for_resource(request.resource_type)
                .to_owned()
        }
        "user-agent" => headers.user_agent.clone(),
        "accept" => accept_for(request.resource_type, headers),
        "accept-language" => headers.accept_language.clone(),
        "accept-encoding" => {
            if derived.trustworthy || headers.insecure_accept_encoding.is_empty() {
                headers.accept_encoding.clone()
            } else {
                headers.insecure_accept_encoding.clone()
            }
        }
        "sec-ch-ua" | "sec-ch-ua-mobile" | "sec-ch-ua-platform" => match &derived.hints {
            ClientHints::None => return None,
            ClientHints::Profile => match lower {
                "sec-ch-ua" => headers.sec_ch_ua.clone(),
                "sec-ch-ua-mobile" => headers.sec_ch_ua_mobile.clone(),
                _ => headers.sec_ch_ua_platform.clone(),
            },
            ClientHints::Derived {
                brands,
                mobile,
                platform,
            } => match lower {
                "sec-ch-ua" => brands.clone(),
                "sec-ch-ua-mobile" => mobile.clone(),
                _ => platform.clone(),
            },
        },
        // Chrome sends this on navigations, secure or not.
        "upgrade-insecure-requests" => {
            if request.mode != RequestMode::Navigate {
                return None;
            }
            headers.upgrade_insecure_requests.clone()
        }
        // ... but `Sec-Fetch-User` only when a gesture started the navigation.
        "sec-fetch-user" => {
            if request.mode != RequestMode::Navigate || !request.user_activated {
                return None;
            }
            headers.sec_fetch_user.clone()
        }
        "sec-fetch-site" => derived.site.as_str().to_owned(),
        "sec-fetch-mode" => request.mode.as_str().to_owned(),
        "sec-fetch-dest" => sec_fetch_dest(request.resource_type, headers),
        "sec-fetch-storage-access" => {
            // Chrome 144 sends it on cross-site sub-resources only.
            if derived.site != FetchSite::CrossSite || request.mode == RequestMode::Navigate {
                return None;
            }
            headers.sec_fetch_storage_access.clone()
        }
        _ => return None,
    };
    if text.is_empty() {
        return None;
    }
    HeaderValue::from_str(&text).ok()
}

/// `Referer` for this request, from the referrer Fetch recorded and the
/// policy that applies to it (Referrer Policy §8.1).
///
/// The referrer is the request's own, not the previous redirect hop's, so a
/// redirect chain never discloses the URL of an intermediate redirector, and a
/// request that started without a referrer never gains one.
fn referer_for(request: &Request) -> Option<String> {
    let referrer = request.referrer.as_ref()?;
    let mut source = referrer.clone();
    source.set_fragment(None);
    let _ = source.set_username("");
    let _ = source.set_password(None);
    let same = same_origin(&source, &request.url);
    // A referrer from a trustworthy origin is not disclosed to one that is not.
    let downgrade = is_trustworthy_url(&source) && !is_trustworthy_url(&request.url);
    let full = |url: &Url| Some(url.to_string());
    let origin_only = |url: &Url| origin_of(url).map(|origin| format!("{origin}/"));
    match request.referrer_policy {
        ReferrerPolicy::NoReferrer => None,
        ReferrerPolicy::UnsafeUrl => full(&source),
        ReferrerPolicy::Origin => origin_only(&source),
        ReferrerPolicy::SameOrigin => same.then(|| source.to_string()),
        ReferrerPolicy::StrictOrigin => (!downgrade).then(|| origin_only(&source)).flatten(),
        ReferrerPolicy::OriginWhenCrossOrigin => {
            if same {
                full(&source)
            } else {
                origin_only(&source)
            }
        }
        ReferrerPolicy::NoReferrerWhenDowngrade => (!downgrade).then(|| source.to_string()),
        ReferrerPolicy::StrictOriginWhenCrossOrigin => {
            if downgrade {
                None
            } else if same {
                full(&source)
            } else {
                origin_only(&source)
            }
        }
    }
}

fn is_trustworthy_url(url: &Url) -> bool {
    is_secure_request(url)
}

/// Chrome's `Accept` per resource type. The profile carries the navigation
/// value; the others are fixed strings in Chromium, measured in
/// `profiles/captures/chrome_144_macos/h2_requests.json`.
fn accept_for(resource_type: ResourceType, headers: &HeaderProfile) -> String {
    match resource_type {
        ResourceType::Document => headers.accept.clone(),
        ResourceType::Stylesheet => "text/css,*/*;q=0.1".to_owned(),
        ResourceType::Image => {
            "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".to_owned()
        }
        _ => "*/*".to_owned(),
    }
}

fn sec_fetch_dest(resource_type: ResourceType, headers: &HeaderProfile) -> String {
    match resource_type {
        ResourceType::Document => headers.sec_fetch_dest.clone(),
        ResourceType::Stylesheet => "style".to_owned(),
        ResourceType::Script | ResourceType::BlockingScript => "script".to_owned(),
        ResourceType::Font => "font".to_owned(),
        ResourceType::Image => "image".to_owned(),
        ResourceType::Media => "video".to_owned(),
        ResourceType::Xhr | ResourceType::Fetch => "empty".to_owned(),
        ResourceType::Prefetch => "empty".to_owned(),
        ResourceType::Other => "empty".to_owned(),
    }
}

/// `example.com:443` is written `example.com` when the port is the default.
fn strip_default_port(authority: &str, destination: &Destination) -> String {
    let default = if destination.secure { 443 } else { 80 };
    if destination.port == default {
        destination.host.clone()
    } else {
        authority.to_owned()
    }
}

fn push(target: &mut Vec<(String, HeaderValue)>, name: &str, value: HeaderValue) {
    target.push((name.to_owned(), value));
}

fn contains(headers: &[(String, HeaderValue)], lower: &str) -> bool {
    headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case(lower))
}

fn is_connection_header(lower: &str) -> bool {
    CONNECTION_HEADERS.contains(&lower)
}

/// Converts the ordered list to a `HeaderMap`.
///
/// `HeaderMap` iterates in insertion order, which is what both hyper and
/// `tenta-h2` write to the wire; the invariant is asserted in the tests
/// below because the whole header-order fingerprint rests on it.
pub fn to_header_map(headers: &[(String, HeaderValue)]) -> HeaderMap {
    let mut map = HeaderMap::with_capacity(headers.len());
    for (name, value) in headers {
        if let Ok(name) = HeaderName::try_from(name.as_str()) {
            map.append(name, value.clone());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ClientProfile;
    use crate::request::ReferrerPolicy;
    use std::sync::Arc;

    fn chrome() -> Profile {
        Profile::Browser(Arc::new(
            ClientProfile::builtin("chrome_144_macos").expect("profile"),
        ))
    }

    fn destination() -> Destination {
        Destination {
            secure: true,
            host: "example.com".into(),
            port: 443,
        }
    }

    fn names(headers: &[(String, HeaderValue)]) -> Vec<String> {
        headers.iter().map(|(name, _)| name.clone()).collect()
    }

    fn value(headers: &[(String, HeaderValue)], name: &str) -> Option<String> {
        headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.to_str().expect("utf-8").to_owned())
    }

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }

    #[test]
    fn a_navigation_over_http2_matches_the_captured_chrome_order() {
        // `profiles/captures/chrome_144_macos/h2_requests.json`, request `/`:
        // a user-typed navigation, no cookie, no referer.
        let request = Request::get("https://example.com/")
            .expect("request")
            .resource_type(ResourceType::Document)
            .user_activated(true);
        let headers = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http2,
            None,
            None,
        );
        assert_eq!(
            names(&headers),
            [
                "sec-ch-ua",
                "sec-ch-ua-mobile",
                "sec-ch-ua-platform",
                "upgrade-insecure-requests",
                "user-agent",
                "accept",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-user",
                "sec-fetch-dest",
                "accept-encoding",
                "accept-language",
                "priority",
            ]
        );
        assert_eq!(value(&headers, "sec-fetch-site").as_deref(), Some("none"));
        assert_eq!(
            value(&headers, "sec-fetch-mode").as_deref(),
            Some("navigate")
        );
        assert_eq!(value(&headers, "priority").as_deref(), Some("u=0, i"));
    }

    #[test]
    fn a_script_driven_navigation_carries_cookie_and_referer_but_no_sec_fetch_user() {
        // The `/second` request of the same capture.
        let request = Request::get("https://example.com/second")
            .expect("request")
            .resource_type(ResourceType::Document)
            .initiator(url("https://example.com/form"))
            .referrer(url("https://example.com/form"));
        let headers = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http2,
            Some("fp=1".to_owned()),
            None,
        );
        assert_eq!(
            names(&headers),
            [
                "sec-ch-ua",
                "sec-ch-ua-mobile",
                "sec-ch-ua-platform",
                "upgrade-insecure-requests",
                "user-agent",
                "accept",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
                "referer",
                "accept-encoding",
                "accept-language",
                "cookie",
                "priority",
            ]
        );
        assert_eq!(
            value(&headers, "sec-fetch-site").as_deref(),
            Some("same-origin")
        );
        assert_eq!(
            value(&headers, "referer").as_deref(),
            Some("https://example.com/form")
        );
    }

    #[test]
    fn a_subresource_uses_chromes_other_order() {
        // The `/sub.css` request of the capture.
        let request = Request::get("https://example.com/a.css")
            .expect("request")
            .resource_type(ResourceType::Stylesheet)
            .initiator(url("https://example.com/"))
            .referrer(url("https://example.com/"));
        let headers = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http2,
            Some("fp=1".to_owned()),
            None,
        );
        assert_eq!(
            names(&headers),
            [
                "sec-ch-ua-platform",
                "user-agent",
                "sec-ch-ua",
                "sec-ch-ua-mobile",
                "accept",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
                "referer",
                "accept-encoding",
                "accept-language",
                "cookie",
                "priority",
            ]
        );
        assert_eq!(
            value(&headers, "accept").as_deref(),
            Some("text/css,*/*;q=0.1")
        );
        assert_eq!(value(&headers, "sec-fetch-dest").as_deref(), Some("style"));
        assert_eq!(
            value(&headers, "sec-fetch-mode").as_deref(),
            Some("no-cors")
        );
        assert_eq!(value(&headers, "priority").as_deref(), Some("u=0"));
    }

    #[test]
    fn a_cross_site_subresource_says_so_and_asks_about_storage_access() {
        let destination = Destination {
            secure: true,
            host: "cdn.example.org".into(),
            port: 443,
        };
        let request = Request::get("https://cdn.example.org/a.png")
            .expect("request")
            .resource_type(ResourceType::Image)
            .initiator(url("https://example.com/"))
            .referrer(url("https://example.com/"));
        let headers = build(
            &chrome(),
            &request,
            &destination,
            WireProtocol::Http2,
            None,
            None,
        );
        assert_eq!(
            value(&headers, "sec-fetch-site").as_deref(),
            Some("cross-site")
        );
        assert_eq!(
            value(&headers, "sec-fetch-storage-access").as_deref(),
            Some("none")
        );
        assert_eq!(value(&headers, "priority").as_deref(), Some("u=2, i"));
        // Strict-origin-when-cross-origin: the origin, not the path.
        assert_eq!(
            value(&headers, "referer").as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            value(&headers, "accept").as_deref(),
            Some("image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8")
        );
    }

    #[test]
    fn a_same_site_request_is_not_cross_site() {
        let request = Request::get("https://sub.example.com/a.png")
            .expect("request")
            .resource_type(ResourceType::Image)
            .initiator(url("https://example.com/"));
        let headers = build(
            &chrome(),
            &request,
            &Destination {
                secure: true,
                host: "sub.example.com".into(),
                port: 443,
            },
            WireProtocol::Http2,
            None,
            None,
        );
        assert_eq!(
            value(&headers, "sec-fetch-site").as_deref(),
            Some("same-site")
        );
        assert_eq!(
            value(&headers, "sec-fetch-storage-access"),
            None,
            "same-site needs no storage-access hint"
        );
    }

    #[test]
    fn a_redirect_chain_only_lowers_sec_fetch_site_from_the_initiator() {
        // Chromium classifies every hop against the original initiator.
        // A chain to same-origin can only lower the value if an earlier
        // hop was cross-site or same-site; if the initiator is same-origin,
        // all hops are same-origin.
        let mut request = Request::get("https://example.com/back")
            .expect("request")
            .resource_type(ResourceType::Document)
            .initiator(url("https://example.com/"));
        // The floor tracks the worst hop seen; the current URL is same-origin
        // with the initiator, but an earlier hop was cross-site.
        request.site_floor = FetchSite::CrossSite;
        let headers = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http2,
            None,
            None,
        );
        assert_eq!(
            value(&headers, "sec-fetch-site").as_deref(),
            Some("cross-site")
        );
    }

    #[test]
    fn a_no_initiator_redirect_chain_stays_none() {
        // A driver-issued navigation with no initiator sends `none` on every
        // hop, regardless of the redirect chain.
        let mut request = Request::get("https://example.org/redirect/to/cross-site")
            .expect("request")
            .resource_type(ResourceType::Document)
            .user_activated(true);
        // A cross-site hop in a chain with no initiator: still `none`.
        request.site_floor = FetchSite::NoOrigin;
        let destination = Destination {
            secure: true,
            host: "example.org".into(),
            port: 443,
        };
        let headers = build(
            &chrome(),
            &request,
            &destination,
            WireProtocol::Http2,
            None,
            None,
        );
        assert_eq!(value(&headers, "sec-fetch-site").as_deref(), Some("none"));
    }

    #[test]
    fn an_insecure_origin_gets_no_hints_no_sec_fetch_and_no_brotli() {
        // `profiles/captures/chrome_144_macos/http1_request.txt`.
        let destination = Destination {
            secure: false,
            host: "example.com".into(),
            port: 80,
        };
        let request = Request::get("http://example.com/")
            .expect("request")
            .resource_type(ResourceType::Document)
            .user_activated(true);
        let headers = build(
            &chrome(),
            &request,
            &destination,
            WireProtocol::Http11,
            None,
            None,
        );
        assert_eq!(
            names(&headers),
            [
                "Host",
                "Connection",
                "Upgrade-Insecure-Requests",
                "User-Agent",
                "Accept",
                "Accept-Encoding",
                "Accept-Language",
            ]
        );
        assert_eq!(
            value(&headers, "accept-encoding").as_deref(),
            Some("gzip, deflate")
        );
    }

    #[test]
    fn loopback_over_plain_http_is_a_trustworthy_origin() {
        // Chromium treats loopback as potentially trustworthy, so a local
        // fixture server sees the same header set a real origin does.
        let destination = Destination {
            secure: false,
            host: "127.0.0.1".into(),
            port: 8080,
        };
        let request = Request::get("http://127.0.0.1:8080/")
            .expect("request")
            .resource_type(ResourceType::Document);
        let headers = build(
            &chrome(),
            &request,
            &destination,
            WireProtocol::Http11,
            None,
            None,
        );
        assert!(names(&headers).contains(&"sec-ch-ua".to_owned()));
        assert_eq!(
            value(&headers, "accept-encoding").as_deref(),
            Some("gzip, deflate, br, zstd")
        );
        assert_eq!(value(&headers, "Host").as_deref(), Some("127.0.0.1:8080"));
    }

    #[test]
    fn a_post_places_content_length_content_type_and_origin_where_chrome_does() {
        // The `/form` request of the capture.
        let request = Request::post("https://example.com/form", "a=1")
            .expect("request")
            .resource_type(ResourceType::Document)
            .initiator(url("https://example.com/"))
            .referrer(url("https://example.com/"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .expect("header")
            .header("Cache-Control", "max-age=0")
            .expect("header");
        let headers = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http2,
            Some("fp=1".to_owned()),
            Some(3),
        );
        assert_eq!(
            names(&headers),
            [
                "content-length",
                "cache-control",
                "sec-ch-ua",
                "sec-ch-ua-mobile",
                "sec-ch-ua-platform",
                "origin",
                "content-type",
                "upgrade-insecure-requests",
                "user-agent",
                "accept",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
                "referer",
                "accept-encoding",
                "accept-language",
                "cookie",
                "priority",
            ]
        );
        assert_eq!(
            value(&headers, "origin").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(value(&headers, "content-length").as_deref(), Some("3"));
    }

    #[test]
    fn a_referrer_is_never_invented_and_never_leaks_over_a_downgrade() {
        let plain = Request::get("https://example.com/")
            .expect("request")
            .resource_type(ResourceType::Document);
        assert_eq!(referer_for(&plain), None, "no referrer, no header");

        let downgraded = Request::get("http://example.org/")
            .expect("request")
            .referrer(url("https://example.com/secret"));
        assert_eq!(
            referer_for(&downgraded),
            None,
            "https to http discloses nothing"
        );

        let cross = Request::get("https://example.org/")
            .expect("request")
            .referrer(url("https://example.com/secret?q=1#f"));
        assert_eq!(referer_for(&cross).as_deref(), Some("https://example.com/"));

        let same = Request::get("https://example.com/other")
            .expect("request")
            .referrer(url("https://example.com/secret?q=1#f"));
        assert_eq!(
            referer_for(&same).as_deref(),
            Some("https://example.com/secret?q=1"),
            "the fragment is stripped, the path is not"
        );

        let unsafe_url = Request::get("http://example.org/")
            .expect("request")
            .referrer(url("https://example.com/secret"))
            .referrer_policy(ReferrerPolicy::UnsafeUrl);
        assert_eq!(
            referer_for(&unsafe_url).as_deref(),
            Some("https://example.com/secret")
        );

        let none = Request::get("https://example.com/x")
            .expect("request")
            .referrer(url("https://example.com/y"))
            .referrer_policy(ReferrerPolicy::NoReferrer);
        assert_eq!(referer_for(&none), None);
    }

    #[test]
    fn http1_keeps_the_profiles_casing_and_adds_host_and_connection() {
        let request = Request::get("https://example.com/")
            .expect("request")
            .resource_type(ResourceType::Document);
        let headers = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http11,
            None,
            None,
        );
        let names = names(&headers);
        assert_eq!(names[0], "Host");
        assert_eq!(names[1], "Connection");
        assert_eq!(names[2], "sec-ch-ua", "client hints stay lower-case");
        assert!(names.contains(&"User-Agent".to_owned()));
        assert!(
            !names.contains(&"priority".to_owned()),
            "the priority header is HTTP/2 only"
        );
        assert!(
            !names.contains(&"Cookie".to_owned()),
            "no cookie, no header"
        );
        assert_eq!(
            value(&headers, "Host").as_deref(),
            Some("example.com"),
            "the default port is not written"
        );
    }

    #[test]
    fn a_chromium_user_agent_override_rebuilds_the_client_hints() {
        let request = Request::get("https://example.com/")
            .expect("request")
            .resource_type(ResourceType::Document)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36",
            )
            .expect("header");
        let headers = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http2,
            None,
            None,
        );
        assert_eq!(
            value(&headers, "sec-ch-ua").as_deref(),
            Some("\"Not(A:Brand\";v=\"8\", \"Chromium\";v=\"139\", \"Google Chrome\";v=\"139\""),
            "the hints follow the overridden version"
        );
        assert_eq!(
            value(&headers, "sec-ch-ua-platform").as_deref(),
            Some("\"Windows\""),
            "and the overridden platform"
        );
        assert_eq!(value(&headers, "sec-ch-ua-mobile").as_deref(), Some("?0"));
    }

    #[test]
    fn a_non_chromium_user_agent_override_drops_the_client_hints() {
        let request = Request::get("https://example.com/")
            .expect("request")
            .resource_type(ResourceType::Document)
            .header("User-Agent", "CriticBot/1.0")
            .expect("header");
        let headers = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http2,
            None,
            None,
        );
        assert_eq!(
            value(&headers, "user-agent").as_deref(),
            Some("CriticBot/1.0")
        );
        for hint in ["sec-ch-ua", "sec-ch-ua-mobile", "sec-ch-ua-platform"] {
            assert_eq!(
                value(&headers, hint),
                None,
                "a non-Chromium user agent sends no {hint}"
            );
        }
        // The `Sec-Fetch-*` set is not a Chromium extension and stays.
        assert_eq!(value(&headers, "sec-fetch-site").as_deref(), Some("none"));
    }

    #[test]
    fn a_mobile_user_agent_override_says_so() {
        let request = Request::get("https://example.com/")
            .expect("request")
            .resource_type(ResourceType::Document)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/144.0.0.0 Mobile Safari/537.36",
            )
            .expect("header");
        let headers = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http2,
            None,
            None,
        );
        assert_eq!(value(&headers, "sec-ch-ua-mobile").as_deref(), Some("?1"));
        assert_eq!(
            value(&headers, "sec-ch-ua-platform").as_deref(),
            Some("\"Android\"")
        );
    }

    #[test]
    fn caller_headers_override_the_profile_and_keep_their_place() {
        let request = Request::get("https://example.com/")
            .expect("request")
            .resource_type(ResourceType::Document)
            .header("User-Agent", "custom/1.0")
            .expect("header")
            .header("X-One", "1")
            .expect("header")
            .header("X-Two", "2")
            .expect("header");
        let headers = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http2,
            None,
            None,
        );
        let names = names(&headers);
        let user_agent = names.iter().position(|n| n == "user-agent").expect("ua");
        let accept = names.iter().position(|n| n == "accept").expect("accept");
        assert!(user_agent < accept, "the profile decides the position");
        assert_eq!(
            headers[user_agent].1.to_str().expect("utf-8"),
            "custom/1.0",
            "the caller decides the value"
        );
        assert_eq!(&names[names.len() - 2..], ["x-one", "x-two"]);
    }

    #[test]
    fn profile_none_sends_only_what_the_caller_asked_for() {
        let request = Request::get("https://example.com/")
            .expect("request")
            .header("Accept", "application/json")
            .expect("header");
        let headers = build(
            &Profile::None,
            &request,
            &destination(),
            WireProtocol::Http11,
            None,
            Some(0),
        );
        assert_eq!(names(&headers), ["Host", "Accept", "Connection"]);
    }

    #[test]
    fn a_body_gets_a_content_length_and_http2_drops_connection_headers() {
        let request = Request::post("https://example.com/", "payload")
            .expect("request")
            .header("Content-Type", "text/plain")
            .expect("header")
            .header("Connection", "close")
            .expect("header");
        let h2 = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http2,
            None,
            Some(7),
        );
        let names = names(&h2);
        assert!(!names.iter().any(|n| n == "connection"));
        assert_eq!(value(&h2, "content-length").as_deref(), Some("7"));
    }

    #[test]
    fn header_maps_iterate_in_insertion_order() {
        // The entire header-order fingerprint depends on this property of
        // `http::HeaderMap`, so it is asserted rather than assumed.
        let request = Request::get("https://example.com/")
            .expect("request")
            .resource_type(ResourceType::Document);
        let headers = build(
            &chrome(),
            &request,
            &destination(),
            WireProtocol::Http2,
            Some("a=1".to_owned()),
            None,
        );
        let map = to_header_map(&headers);
        let from_map: Vec<String> = map
            .iter()
            .map(|(name, _)| name.as_str().to_owned())
            .collect();
        assert_eq!(from_map, names(&headers));
    }

    #[test]
    fn sec_fetch_none_never_pairs_with_cors_mode() {
        // `Sec-Fetch-Site: none` requires no initiator. A request with no
        // initiator is a driver-issued navigation with `Navigate` mode, never
        // `Cors` mode. Fetch §2.2.5 forbids the `none + cors` combination.
        let request = Request::get("https://example.com/")
            .expect("request")
            // Request::get() now sets Document/Navigate defaults, so this test
            // verifies the default shape never produces the impossible triple.
            .user_activated(true);

        let headers = build(
            &chrome(),
            &request,
            &Destination {
                secure: true,
                host: "example.com".into(),
                port: 443,
            },
            WireProtocol::Http2,
            None,
            None,
        );

        let site = value(&headers, "sec-fetch-site");
        let mode = value(&headers, "sec-fetch-mode");

        // If site is "none", mode must not be "cors".
        if site.as_deref() == Some("none") {
            assert_ne!(
                mode.as_deref(),
                Some("cors"),
                "Sec-Fetch-Site: none cannot pair with Sec-Fetch-Mode: cors"
            );
        }
        // For completeness, verify the default shape sends navigate mode.
        assert_eq!(
            mode.as_deref(),
            Some("navigate"),
            "driver navigation must use navigate mode"
        );
        assert_eq!(
            site.as_deref(),
            Some("none"),
            "no-initiator navigation sends Sec-Fetch-Site: none"
        );
    }
}
