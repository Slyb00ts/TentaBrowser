//! The request the transport takes and the knobs a caller sets on it.

use std::time::Duration;

use http::{HeaderName, HeaderValue, Method};
use url::Url;

use crate::body::Body;
use crate::error::{ErrorName, NetError};
use crate::policy::cookies::{SameSiteContext, Site};
use crate::proxy::Proxy;

/// What a request is for. Drives the default priority
/// (`docs/spec/03-network.md` §3) and, later, the fetch policy of a tier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResourceType {
    Document,
    Stylesheet,
    Script,
    /// A script the parser must wait for.
    BlockingScript,
    Font,
    Image,
    Media,
    Xhr,
    #[default]
    Fetch,
    Prefetch,
    Other,
}

/// Chromium's request priority classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PriorityClass {
    /// Prefetches and speculative work.
    VeryLow,
    /// Images outside the viewport, async scripts.
    Low,
    /// Images in the viewport, XHR.
    Medium,
    /// Fonts, synchronous scripts.
    High,
    /// The document and its stylesheets.
    VeryHigh,
}

/// A request's place in the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Priority {
    pub class: PriorityClass,
    /// Whether a partial response is useful (progressive images, streamed
    /// HTML) or only the complete body is.
    pub incremental: bool,
}

impl Default for Priority {
    fn default() -> Self {
        Self {
            class: PriorityClass::Medium,
            incremental: false,
        }
    }
}

impl ResourceType {
    /// Fetch's mode for this resource type: navigations navigate, `fetch()`,
    /// XHR and fonts are CORS-flagged, everything else is `no-cors`
    /// (measured in `profiles/captures/chrome_144_macos/h2_requests.json`).
    pub fn default_mode(self) -> RequestMode {
        match self {
            ResourceType::Document => RequestMode::Navigate,
            ResourceType::Xhr | ResourceType::Fetch | ResourceType::Font => RequestMode::Cors,
            _ => RequestMode::NoCors,
        }
    }

    /// The priority Chromium gives this resource type before viewport
    /// information is available.
    pub fn default_priority(self) -> Priority {
        let (class, incremental) = match self {
            ResourceType::Document => (PriorityClass::VeryHigh, true),
            ResourceType::Stylesheet => (PriorityClass::VeryHigh, false),
            ResourceType::BlockingScript => (PriorityClass::High, false),
            ResourceType::Font => (PriorityClass::High, false),
            ResourceType::Script => (PriorityClass::Medium, false),
            ResourceType::Xhr | ResourceType::Fetch => (PriorityClass::Medium, false),
            ResourceType::Image => (PriorityClass::Low, true),
            ResourceType::Media => (PriorityClass::Low, true),
            ResourceType::Other => (PriorityClass::Low, false),
            ResourceType::Prefetch => (PriorityClass::VeryLow, false),
        };
        Priority { class, incremental }
    }
}

/// How the request relates to its initiator (Fetch §2.2.5, "mode").
///
/// It decides `Sec-Fetch-Mode` and whether an `Origin` header is sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RequestMode {
    /// A top-level or nested document navigation.
    Navigate,
    /// A CORS-flagged fetch: `fetch()`, XHR, fonts, modules.
    #[default]
    Cors,
    /// An image, stylesheet or classic script: no CORS, opaque on failure.
    NoCors,
    /// A fetch that must not leave the origin.
    SameOrigin,
}

impl RequestMode {
    /// The `Sec-Fetch-Mode` token.
    pub fn as_str(self) -> &'static str {
        match self {
            RequestMode::Navigate => "navigate",
            RequestMode::Cors => "cors",
            RequestMode::NoCors => "no-cors",
            RequestMode::SameOrigin => "same-origin",
        }
    }
}

/// The referrer policy that turns a referrer URL into a `Referer` header
/// (Referrer Policy §8.1). Chrome's default is
/// `strict-origin-when-cross-origin`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ReferrerPolicy {
    NoReferrer,
    NoReferrerWhenDowngrade,
    SameOrigin,
    Origin,
    StrictOrigin,
    OriginWhenCrossOrigin,
    #[default]
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

/// How a request's URL relates to the site that initiated it (Fetch §8.1),
/// which is what `Sec-Fetch-Site` reports.
///
/// The ordering is "least trusted first": a redirect chain takes the minimum
/// of the value each hop produces, so a chain can only ever become more
/// distant, never less (Chromium's `SetSecFetchSiteHeader`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FetchSite {
    CrossSite,
    SameSite,
    SameOrigin,
    /// No initiator at all: a user-typed or driver-issued navigation.
    NoOrigin,
}

impl FetchSite {
    /// The `Sec-Fetch-Site` token.
    pub fn as_str(self) -> &'static str {
        match self {
            FetchSite::CrossSite => "cross-site",
            FetchSite::SameSite => "same-site",
            FetchSite::SameOrigin => "same-origin",
            FetchSite::NoOrigin => "none",
        }
    }

    /// Classifies `url` against the origin that initiated the request.
    pub fn classify(initiator: Option<&Url>, url: &Url) -> Self {
        let Some(initiator) = initiator else {
            return FetchSite::NoOrigin;
        };
        if same_origin(initiator, url) {
            return FetchSite::SameOrigin;
        }
        match (Site::from_url(initiator), Site::from_url(url)) {
            (Some(left), Some(right)) if left == right => FetchSite::SameSite,
            _ => FetchSite::CrossSite,
        }
    }
}

/// Origin equality: scheme, host and port (RFC 6454).
pub fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

/// `scheme://host[:port]`, the serialisation an `Origin` header carries.
pub fn origin_of(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let scheme = url.scheme();
    let default_port = if scheme == "https" { 443 } else { 80 };
    match url.port().filter(|port| *port != default_port) {
        Some(port) => Some(format!("{scheme}://{host}:{port}")),
        None => Some(format!("{scheme}://{host}")),
    }
}

/// What to do with a 3xx response (Fetch, §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedirectPolicy {
    /// Follow up to `max` redirects; Chrome's limit is 20.
    Follow { max: u32 },
    /// Return the 3xx response as-is.
    Manual,
    /// Fail with `ERR_UNSAFE_REDIRECT`.
    Error,
}

impl Default for RedirectPolicy {
    fn default() -> Self {
        RedirectPolicy::Follow { max: 20 }
    }
}

/// Limits applied to one request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequestLimits {
    /// Whole-request deadline, from `send()` to the last body byte.
    pub total: Option<Duration>,
    /// Deadline for the response head.
    pub headers: Option<Duration>,
    /// Maximum decoded response body size.
    pub max_body_bytes: Option<u64>,
}

/// One HTTP request.
#[derive(Debug)]
pub struct Request {
    pub method: Method,
    pub url: Url,
    /// Caller headers in the order they were added. The profile decides the
    /// final order and casing; headers not named by the profile keep this
    /// relative order after the ones that are.
    pub headers: Vec<(String, HeaderValue)>,
    pub body: Body,
    pub priority: Priority,
    pub redirect: RedirectPolicy,
    pub resource_type: ResourceType,
    pub limits: RequestLimits,
    /// Overrides the client's proxy for this request.
    pub proxy: Option<Proxy>,
    /// Opt out of coalescing for an otherwise identical in-flight GET.
    pub allow_coalescing: bool,
    /// Skip the cache lookup and go to the network. The response is still
    /// stored, which is what makes this a reload rather than a bypass; the
    /// background refresh of a `stale-while-revalidate` hit sets it.
    pub bypass_cache_lookup: bool,
    /// The site of the document this request belongs to. It decides whether
    /// the request is third-party and which partition its cookies live in
    /// (`docs/spec/03-network.md` §6); `None` means "no document", which the
    /// jar treats as first-party.
    pub first_party: Option<Site>,
    /// How the cookie jar must classify this request. `None` — the default —
    /// means the transport derives it from `first_party`, `initiator`, `mode`
    /// and the method, so a caller that says nothing gets Chrome's answer for
    /// a top-level navigation. A caller that knows better overrides it with
    /// [`Request::same_site`]; [`Request::same_site_context`] returns whatever
    /// is in force.
    pub same_site: Option<SameSiteContext>,
    /// The URL of the document that initiated this request. `None` is a
    /// user-typed or driver-issued navigation, which is what makes
    /// `Sec-Fetch-Site: none` legitimate.
    pub initiator: Option<Url>,
    /// The referrer Fetch computed for this request, before the policy is
    /// applied. It survives redirects unchanged: `Referer` on hop N is
    /// computed from this URL, never from hop N-1.
    pub referrer: Option<Url>,
    pub referrer_policy: ReferrerPolicy,
    /// Fetch's request mode; set together with the resource type and
    /// overridable afterwards.
    pub mode: RequestMode,
    /// Whether a user gesture started this navigation, which is the only
    /// thing that makes Chrome send `Sec-Fetch-User: ?1`.
    pub user_activated: bool,
    /// The least-trusted classification any hop of the redirect chain has
    /// produced so far. `Sec-Fetch-Site` never rises above it.
    pub site_floor: FetchSite,
    /// Username and password extracted from the URL (RFC 3986 userinfo).
    /// Stripped from the URL before the request reaches the wire; stored here
    /// for a future 401 Basic challenge (P2: HttpAuthCache) if an auth path
    /// already exists for the origin.
    pub userinfo: Option<(String, Option<String>)>,
}

impl Request {
    pub fn new(method: Method, url: Url) -> Self {
        // BLOCKING-4: Run the same validation and userinfo extraction as parse_url.
        // Port validation is not applied here since the URL is already parsed, but
        // execute() will run the full validation when the request reaches it.
        let userinfo = if url.username().is_empty() && url.password().is_none() {
            None
        } else {
            extract_userinfo(url.as_str())
        };
        Self {
            method,
            url,
            headers: Vec::new(),
            body: Body::empty(),
            priority: Priority::default(),
            redirect: RedirectPolicy::default(),
            resource_type: ResourceType::default(),
            limits: RequestLimits::default(),
            proxy: None,
            allow_coalescing: true,
            bypass_cache_lookup: false,
            first_party: None,
            same_site: None,
            initiator: None,
            referrer: None,
            referrer_policy: ReferrerPolicy::default(),
            mode: RequestMode::default(),
            user_activated: false,
            site_floor: FetchSite::NoOrigin,
            userinfo,
        }
    }

    /// Parses `url` and builds a GET request. With no initiator set, this is
    /// treated as a driver-issued top-level navigation: `resource_type` is
    /// `Document` and `mode` is `Navigate`. `first_party` is NOT set; the
    /// fetch layer must set it on the request (P2: sub-resources must pass
    /// the document's site; a bare request with no context is top-level).
    /// Callers must explicitly set `user_activated(true)` to mark user-initiated navigations.
    pub fn get(url: &str) -> Result<Self, NetError> {
        let userinfo = extract_userinfo(url);
        let parsed = parse_url(url)?;
        let mut request = Self::new(Method::GET, parsed);
        request.resource_type = ResourceType::Document;
        request.mode = RequestMode::Navigate;
        request.userinfo = userinfo;
        Ok(request)
    }

    /// Parses `url` and builds a POST request with `body`. With no initiator
    /// set, this is treated as a driver-issued top-level navigation (see
    /// `Request::get`).
    pub fn post(url: &str, body: impl Into<Body>) -> Result<Self, NetError> {
        let userinfo = extract_userinfo(url);
        let parsed = parse_url(url)?;
        let mut request = Self::new(Method::POST, parsed);
        request.resource_type = ResourceType::Document;
        request.mode = RequestMode::Navigate;
        request.body = body.into();
        request.userinfo = userinfo;
        Ok(request)
    }

    /// Replaces the request body.
    pub fn with_body(mut self, body: impl Into<Body>) -> Self {
        self.body = body.into();
        self
    }

    /// Adds a header, keeping the order of previously added ones.
    pub fn header(mut self, name: &str, value: &str) -> Result<Self, NetError> {
        let value = HeaderValue::from_str(value).map_err(|_| {
            NetError::new(
                ErrorName::InvalidArgument,
                format!("header {name} has a value that is not valid on the wire"),
            )
        })?;
        HeaderName::try_from(name).map_err(|_| {
            NetError::new(
                ErrorName::InvalidArgument,
                format!("invalid header {name:?}"),
            )
        })?;
        self.headers.push((name.to_owned(), value));
        Ok(self)
    }

    pub fn resource_type(mut self, resource_type: ResourceType) -> Self {
        self.resource_type = resource_type;
        self.priority = resource_type.default_priority();
        self.mode = resource_type.default_mode();
        self
    }

    /// Sets the document that initiated this request, which decides
    /// `Sec-Fetch-Site`, `Origin` and the default `Referer`.
    pub fn initiator(mut self, url: Url) -> Self {
        self.initiator = Some(url);
        self
    }

    /// Sets the referrer Fetch computed for this request.
    pub fn referrer(mut self, url: Url) -> Self {
        self.referrer = Some(url);
        self
    }

    pub fn referrer_policy(mut self, policy: ReferrerPolicy) -> Self {
        self.referrer_policy = policy;
        self
    }

    /// Overrides the mode the resource type implies.
    pub fn mode(mut self, mode: RequestMode) -> Self {
        self.mode = mode;
        self
    }

    /// Marks the request as started by a user gesture.
    pub fn user_activated(mut self, activated: bool) -> Self {
        self.user_activated = activated;
        self
    }

    /// `Sec-Fetch-Site` for this request, after the redirect chain.
    pub fn fetch_site(&self) -> FetchSite {
        FetchSite::classify(self.initiator.as_ref(), &self.url).min(self.site_floor)
    }

    /// Whether Fetch sends an `Origin` header on this request.
    ///
    /// Chrome sends it for every method other than `GET`/`HEAD` and for a
    /// CORS-flagged fetch that leaves the initiator's origin; a same-origin
    /// `fetch()` gets none (measured, `profiles/captures/`).
    pub fn sends_origin(&self) -> bool {
        if self.method != Method::GET && self.method != Method::HEAD {
            return self.initiator.is_some();
        }
        self.mode == RequestMode::Cors
            && self
                .initiator
                .as_ref()
                .is_some_and(|initiator| !same_origin(initiator, &self.url))
    }

    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.limits.total = Some(timeout);
        self
    }

    pub fn max_body_bytes(mut self, max: u64) -> Self {
        self.limits.max_body_bytes = Some(max);
        self
    }

    pub fn redirect(mut self, policy: RedirectPolicy) -> Self {
        self.redirect = policy;
        self
    }

    pub fn proxy(mut self, proxy: Proxy) -> Self {
        self.proxy = Some(proxy);
        self
    }

    /// Goes to the network even when the cache holds an answer, and stores
    /// what comes back: a reload.
    pub fn reload(mut self) -> Self {
        self.bypass_cache_lookup = true;
        self
    }

    /// A copy of this request with no body, for a background revalidation.
    ///
    /// Only `GET`/`HEAD` responses are cached, so the body that is dropped is
    /// always empty; everything that decides which headers go on the wire is
    /// carried over, so the refresh asks the same question as the request it
    /// serves.
    pub fn without_body(&self) -> Self {
        Self {
            method: self.method.clone(),
            url: self.url.clone(),
            headers: self.headers.clone(),
            body: Body::empty(),
            priority: self.priority,
            redirect: self.redirect,
            resource_type: self.resource_type,
            limits: self.limits,
            proxy: self.proxy.clone(),
            allow_coalescing: self.allow_coalescing,
            bypass_cache_lookup: self.bypass_cache_lookup,
            first_party: self.first_party.clone(),
            same_site: self.same_site,
            initiator: self.initiator.clone(),
            referrer: self.referrer.clone(),
            referrer_policy: self.referrer_policy,
            mode: self.mode,
            user_activated: self.user_activated,
            site_floor: self.site_floor,
            userinfo: self.userinfo.clone(),
        }
    }

    /// Sets the site of the document this request belongs to.
    pub fn first_party(mut self, site: Site) -> Self {
        self.first_party = Some(site);
        self
    }

    /// Overrides the same-site classification the cookie jar applies.
    pub fn same_site(mut self, context: SameSiteContext) -> Self {
        self.same_site = Some(context);
        self
    }

    /// The same-site classification in force for this request.
    ///
    /// Without an override it follows Chromium's site-for-cookies rules:
    /// For navigations (top-level or iframe), derive same_site from the
    /// initiator; a request with no initiator is treated as top-level
    /// (first-party). For sub-resources, derive from the document's site
    /// (`first_party`); the initiator is not consulted for SameSite cookies.
    pub fn same_site_context(&self) -> SameSiteContext {
        if let Some(context) = self.same_site {
            return context;
        }

        let site_of_url = Site::from_url(&self.url);
        let same_site = if self.mode == RequestMode::Navigate {
            // For navigations, derive from initiator. No initiator means top-level.
            match &self.initiator {
                Some(initiator) => match (Site::from_url(initiator), site_of_url) {
                    (Some(left), Some(right)) => left == right,
                    _ => false,
                },
                None => true, // No initiator: top-level navigation is first-party
            }
        } else {
            // For sub-resources, derive from first_party (site-for-cookies).
            match &self.first_party {
                Some(first_party) => site_of_url.as_ref().is_some_and(|site| site == first_party),
                None => true, // No document context: treat as first-party
            }
        };

        SameSiteContext {
            same_site,
            top_level_navigation: self.mode == RequestMode::Navigate,
            safe_method: self.method == Method::GET || self.method == Method::HEAD,
        }
    }

    /// Looks up a caller header, case-insensitively.
    pub fn header_value(&self, name: &str) -> Option<&HeaderValue> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value)
    }

    /// True when the request may be coalesced with an identical in-flight one.
    pub fn is_coalescable(&self) -> bool {
        self.allow_coalescing && self.method == Method::GET && self.body.is_empty()
    }
}

/// Parses and validates a URL the transport is willing to fetch, stripping userinfo.
/// Credentials are stripped from the URL before it reaches the wire and must be
/// extracted separately by the caller.
pub fn parse_url(url: &str) -> Result<Url, NetError> {
    let mut parsed = Url::parse(url).map_err(|err| {
        NetError::new(
            ErrorName::InvalidArgument,
            format!("invalid URL {url:?}: {err}"),
        )
    })?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(NetError::new(
                ErrorName::UnknownUrlScheme,
                format!("scheme {other:?} is not handled by the HTTP transport"),
            ));
        }
    }
    if parsed.host_str().is_none() {
        return Err(NetError::new(
            ErrorName::InvalidArgument,
            format!("URL {url:?} has no host"),
        ));
    }
    check_port(&parsed)?;

    // Strip userinfo (RFC 3986 userinfo) from the URL before it reaches the wire.
    // Credentials must be extracted separately and stored on the Request for a
    // future 401 Basic challenge (P2).
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);

    Ok(parsed)
}

/// Extracts and returns the username and password from a URL string,
/// without modifying the URL itself.
pub fn extract_userinfo(url: &str) -> Option<(String, Option<String>)> {
    let parsed = Url::parse(url).ok()?;
    match (parsed.username(), parsed.password()) {
        (username, password) if !username.is_empty() || password.is_some() => {
            let username = username.to_owned();
            let password = password.map(|p| p.to_owned());
            if !username.is_empty() {
                Some((username, password))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Chromium's restricted-port list, minus the ports the schemes need.
///
/// Blocking these is what `net::ERR_UNSAFE_PORT` is for: an HTTP request to
/// port 25 is a way to make a browser speak SMTP.
pub const UNSAFE_PORTS: &[u16] = &[
    1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101, 102,
    103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427, 465,
    512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990, 993,
    995, 1719, 1720, 1723, 2049, 3659, 4045, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668, 6669,
    6697, 10080,
];

pub fn check_port(url: &Url) -> Result<(), NetError> {
    if let Some(port) = url.port()
        && UNSAFE_PORTS.contains(&port)
    {
        return Err(NetError::new(
            ErrorName::UnsafePort,
            format!("port {port} is on the restricted list"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_types_carry_chromiums_priorities() {
        assert_eq!(
            ResourceType::Document.default_priority().class,
            PriorityClass::VeryHigh
        );
        assert!(ResourceType::Document.default_priority().incremental);
        assert_eq!(
            ResourceType::Prefetch.default_priority().class,
            PriorityClass::VeryLow
        );
        assert!(
            ResourceType::Stylesheet.default_priority().class
                > ResourceType::Image.default_priority().class
        );
        let request = Request::get("https://example.com/a.png")
            .expect("url")
            .resource_type(ResourceType::Image);
        assert_eq!(request.priority.class, PriorityClass::Low);
    }

    #[test]
    fn urls_are_validated_before_anything_is_opened() {
        assert!(Request::get("https://example.com/").is_ok());
        assert_eq!(
            Request::get("ftp://example.com/").unwrap_err().name,
            ErrorName::UnknownUrlScheme
        );
        assert_eq!(
            Request::get("https://example.com:25/").unwrap_err().name,
            ErrorName::UnsafePort
        );
        assert_eq!(
            Request::get("not a url").unwrap_err().name,
            ErrorName::InvalidArgument
        );
        assert!(Request::get("https://example.com:8443/").is_ok());
    }

    #[test]
    fn headers_keep_the_order_they_were_added_in() {
        let request = Request::get("https://example.com/")
            .expect("url")
            .header("x-a", "1")
            .expect("header")
            .header("x-b", "2")
            .expect("header");
        let names: Vec<&str> = request.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["x-a", "x-b"]);
        assert_eq!(
            request.header_value("X-A").map(|v| v.as_bytes()),
            Some(&b"1"[..])
        );
        assert!(request.header("bad header", "1").is_err());
    }

    #[test]
    fn only_bodyless_gets_are_coalescable() {
        assert!(
            Request::get("https://example.com/")
                .unwrap()
                .is_coalescable()
        );
        assert!(
            !Request::post("https://example.com/", "body")
                .unwrap()
                .is_coalescable()
        );
    }
}
