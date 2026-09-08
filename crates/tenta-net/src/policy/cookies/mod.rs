//! RFC 6265bis cookie jar (`docs/spec/03-network.md` §6).
//!
//! Third-party cookies are allowed by default, matching Chrome's shipping
//! behaviour; [`CookiePolicy::BlockThirdParty`] is the opt-in. `document.cookie`
//! goes through the same jar as the network stack, with `is_http = false`, so
//! `HttpOnly` means the same thing on both paths.

mod parse;
mod storage_state;

use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use hashbrown::HashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use url::Url;

pub use parse::{
    MAX_ATTRIBUTE_BYTES, MAX_NAME_VALUE_BYTES, canonical_host, default_path, domain_matches,
    is_public_suffix, is_secure_request, parse_cookie_date, parse_set_cookie_in, path_matches,
    registrable_domain,
};
pub use storage_state::{StorageCookie, StorageState};

/// `SameSite` enforcement (RFC 6265bis §5.4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum SameSite {
    /// Never sent on a cross-site request.
    Strict,
    /// Sent on same-site requests and on safe top-level cross-site
    /// navigations. The default when the attribute is absent.
    #[default]
    Lax,
    /// Sent on every request; requires `Secure`.
    None,
}

impl SameSite {
    pub fn as_str(self) -> &'static str {
        match self {
            SameSite::Strict => "Strict",
            SameSite::Lax => "Lax",
            SameSite::None => "None",
        }
    }
}

impl fmt::Display for SameSite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A site: scheme plus registrable domain, the unit third-party and
/// partitioning decisions are made in.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Site {
    pub scheme: String,
    pub registrable_domain: String,
}

impl Site {
    pub fn new(scheme: impl Into<String>, registrable_domain: impl Into<String>) -> Self {
        Self {
            scheme: scheme.into().to_ascii_lowercase(),
            registrable_domain: registrable_domain.into().to_ascii_lowercase(),
        }
    }

    /// `None` for URLs without a host (`data:`, `about:`), which have no site.
    pub fn from_url(url: &Url) -> Option<Site> {
        let host = canonical_host(url)?;
        // `http` and `https` are the same site: schemeful same-site is about
        // the scheme of the *site*, and browsers group ws/wss with them.
        let scheme = match url.scheme() {
            "ws" => "http",
            "wss" => "https",
            other => other,
        };
        Some(Site {
            scheme: scheme.to_string(),
            registrable_domain: registrable_domain(&host),
        })
    }

    /// Same-site comparison ignoring the scheme (schemeless same-site), which
    /// is what cookie `SameSite` enforcement uses.
    pub fn same_registrable_domain(&self, other: &Site) -> bool {
        self.registrable_domain == other.registrable_domain
    }
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme, self.registrable_domain)
    }
}

/// The CHIPS partition key: the top-level site a partitioned cookie belongs
/// to. A partitioned cookie set in an iframe on `a.example` is invisible to
/// the same iframe on `b.example`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PartitionKey(pub Site);

impl PartitionKey {
    pub fn from_url(url: &Url) -> Option<PartitionKey> {
        Site::from_url(url).map(PartitionKey)
    }

    pub fn site(&self) -> &Site {
        &self.0
    }
}

impl fmt::Display for PartitionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// One stored cookie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Canonical, without a leading dot.
    pub domain: String,
    /// Set when no `Domain` attribute was given: only the exact host matches.
    pub host_only: bool,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
    pub partition_key: Option<PartitionKey>,
    pub expires: Option<SystemTime>,
    pub creation: SystemTime,
    pub last_access: SystemTime,
    pub persistent: bool,
}

impl Cookie {
    /// A session cookie for the URL's host and default path.
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
        url: &Url,
        now: SystemTime,
    ) -> Option<Cookie> {
        let host = canonical_host(url)?;
        Some(Cookie {
            name: name.into(),
            value: value.into(),
            domain: host,
            host_only: true,
            path: default_path(url),
            secure: false,
            http_only: false,
            same_site: SameSite::Lax,
            partition_key: None,
            expires: None,
            creation: now,
            last_access: now,
            persistent: false,
        })
    }

    pub fn is_expired(&self, now: SystemTime) -> bool {
        self.expires.is_some_and(|expires| expires <= now)
    }

    /// `name=value`, the form used in a `Cookie` header.
    pub fn pair(&self) -> String {
        format!("{}={}", self.name, self.value)
    }

    fn key(&self) -> CookieKey {
        CookieKey {
            partition: self.partition_key.clone(),
            domain: self.domain.clone(),
            path: self.path.clone(),
            name: self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CookieKey {
    partition: Option<PartitionKey>,
    domain: String,
    path: String,
    name: String,
}

/// Whether third-party cookies are allowed (`docs/spec/03-network.md` §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CookiePolicy {
    /// Chrome's shipping behaviour.
    #[default]
    AllowAll,
    /// No cookies at all on requests whose site differs from the top-level
    /// site. Partitioned cookies are still refused: blocking means blocking.
    BlockThirdParty,
}

/// The same-site classification of one request, computed by the fetch layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SameSiteContext {
    /// The request's site equals the site for cookies (the top-level site and
    /// every ancestor frame).
    pub same_site: bool,
    /// The request is a top-level navigation, which is what lets a `Lax`
    /// cookie ride along on a cross-site link.
    pub top_level_navigation: bool,
    /// The request method is safe (`GET`, `HEAD`). A cross-site `POST`
    /// navigation does not carry `Lax` cookies.
    pub safe_method: bool,
}

impl Default for SameSiteContext {
    fn default() -> Self {
        Self::same_site()
    }
}

impl SameSiteContext {
    pub fn same_site() -> Self {
        Self {
            same_site: true,
            top_level_navigation: false,
            safe_method: true,
        }
    }

    /// A cross-site subresource request: only `SameSite=None` rides along.
    pub fn cross_site() -> Self {
        Self {
            same_site: false,
            top_level_navigation: false,
            safe_method: true,
        }
    }

    /// A cross-site top-level `GET` navigation: `Lax` and `None` ride along.
    pub fn cross_site_navigation() -> Self {
        Self {
            same_site: false,
            top_level_navigation: true,
            safe_method: true,
        }
    }

    /// A cross-site top-level form `POST`: only `None` rides along.
    pub fn cross_site_unsafe_navigation() -> Self {
        Self {
            same_site: false,
            top_level_navigation: true,
            safe_method: false,
        }
    }

    fn allows(&self, same_site: SameSite) -> bool {
        match same_site {
            SameSite::None => true,
            SameSite::Lax => self.same_site || (self.top_level_navigation && self.safe_method),
            SameSite::Strict => self.same_site,
        }
    }
}

/// What [`CookieJar::store`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreOutcome {
    Stored,
    /// Replaced a cookie with the same (partition, domain, path, name), which
    /// keeps the original creation time (RFC 6265bis §5.7 step 20).
    Replaced,
    /// Already expired: any cookie with the same key was removed instead.
    Expired,
    /// A non-HTTP API tried to set or overwrite an `HttpOnly` cookie.
    RejectedHttpOnly,
    /// The cookie's domain does not match the request host.
    RejectedDomain,
    /// The jar's limits left no room for the cookie.
    RejectedFull,
}

impl StoreOutcome {
    pub fn is_stored(self) -> bool {
        matches!(self, StoreOutcome::Stored | StoreOutcome::Replaced)
    }
}

/// Jar capacity, following the limits in RFC 6265bis §5.6 and §6.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CookieLimits {
    pub max_per_domain: usize,
    pub max_total: usize,
}

impl Default for CookieLimits {
    fn default() -> Self {
        Self {
            max_per_domain: 180,
            max_total: 3000,
        }
    }
}

#[derive(Debug)]
struct Inner {
    limits: CookieLimits,
    policy: RwLock<CookiePolicy>,
    cookies: RwLock<HashMap<CookieKey, Cookie>>,
}

/// The cookie store of one BrowserContext. Cloning shares the storage.
#[derive(Debug, Clone)]
pub struct CookieJar {
    inner: Arc<Inner>,
}

impl Default for CookieJar {
    fn default() -> Self {
        Self::new()
    }
}

impl CookieJar {
    pub fn new() -> Self {
        Self::with_limits(CookieLimits::default())
    }

    pub fn with_limits(limits: CookieLimits) -> Self {
        Self {
            inner: Arc::new(Inner {
                limits,
                policy: RwLock::new(CookiePolicy::default()),
                cookies: RwLock::new(HashMap::new()),
            }),
        }
    }

    pub fn policy(&self) -> CookiePolicy {
        *self.inner.policy.read()
    }

    pub fn set_policy(&self, policy: CookiePolicy) {
        *self.inner.policy.write() = policy;
    }

    pub fn limits(&self) -> CookieLimits {
        self.inner.limits
    }

    /// Parses and stores one `Set-Cookie` header value.
    pub fn set_cookie(
        &self,
        header: &[u8],
        request_url: &Url,
        is_http: bool,
        now: SystemTime,
    ) -> Option<StoreOutcome> {
        self.set_cookie_in(header, request_url, is_http, now, None)
    }

    /// Like [`CookieJar::set_cookie`] with a known top-level site, which is
    /// what makes `Partitioned` meaningful.
    pub fn set_cookie_in(
        &self,
        header: &[u8],
        request_url: &Url,
        is_http: bool,
        now: SystemTime,
        partition: Option<&Site>,
    ) -> Option<StoreOutcome> {
        let cookie = parse_set_cookie_in(header, request_url, now, partition)?;
        Some(self.store(cookie, request_url, is_http))
    }

    /// Stores a cookie that has already been parsed and validated against
    /// `request_url`.
    pub fn store(&self, mut cookie: Cookie, request_url: &Url, is_http: bool) -> StoreOutcome {
        let now = cookie.creation;
        let Some(host) = canonical_host(request_url) else {
            return StoreOutcome::RejectedDomain;
        };
        let host_ok = if cookie.host_only {
            cookie.domain == host
        } else {
            domain_matches(&host, &cookie.domain)
        };
        if !host_ok {
            return StoreOutcome::RejectedDomain;
        }

        let mut cookies = self.inner.cookies.write();
        let key = cookie.key();
        let existing = cookies.get(&key);

        // A non-HTTP API may neither create an HttpOnly cookie nor overwrite
        // one (RFC 6265bis §5.7 steps 15 and 21).
        if !is_http && (cookie.http_only || existing.is_some_and(|old| old.http_only)) {
            return StoreOutcome::RejectedHttpOnly;
        }

        if cookie.is_expired(now) {
            cookies.remove(&key);
            return StoreOutcome::Expired;
        }

        if let Some(old) = existing {
            cookie.creation = old.creation;
            cookies.insert(key, cookie);
            return StoreOutcome::Replaced;
        }

        let domain = cookie.domain.clone();
        cookies.insert(key.clone(), cookie);
        purge_expired(&mut cookies, now);
        evict_domain(&mut cookies, &domain, self.inner.limits.max_per_domain);
        evict_total(&mut cookies, self.inner.limits.max_total);

        // With a jar configured smaller than one cookie per domain, eviction
        // can remove the cookie that was just inserted.
        if cookies.contains_key(&key) {
            StoreOutcome::Stored
        } else {
            StoreOutcome::RejectedFull
        }
    }

    /// The cookies to send with a request, in `Cookie` header order:
    /// longer paths first, then earlier creation time (RFC 6265 §5.4).
    ///
    /// Returns owned cookies rather than references: the jar is shared behind
    /// a lock, so a borrow could not outlive the call.
    pub fn cookies_for(
        &self,
        request_url: &Url,
        first_party: Option<&Site>,
        is_http: bool,
        context: SameSiteContext,
        now: SystemTime,
    ) -> Vec<Cookie> {
        let Some(host) = canonical_host(request_url) else {
            return Vec::new();
        };
        let request_site = Site::from_url(request_url);
        let secure = is_secure_request(request_url);
        let path = request_url.path();
        let path = if path.is_empty() { "/" } else { path };

        let third_party = match (first_party, request_site.as_ref()) {
            (Some(top), Some(site)) => !top.same_registrable_domain(site),
            _ => false,
        };
        if third_party && self.policy() == CookiePolicy::BlockThirdParty {
            return Vec::new();
        }
        let partition = first_party.cloned().map(PartitionKey);

        let mut cookies = self.inner.cookies.write();
        purge_expired(&mut cookies, now);

        let mut selected: Vec<Cookie> = cookies
            .values_mut()
            .filter(|cookie| {
                if cookie.host_only {
                    cookie.domain == host
                } else {
                    domain_matches(&host, &cookie.domain)
                }
            })
            .filter(|cookie| path_matches(path, &cookie.path))
            .filter(|cookie| !cookie.secure || secure)
            .filter(|cookie| is_http || !cookie.http_only)
            .filter(|cookie| context.allows(cookie.same_site))
            .filter(|cookie| match &cookie.partition_key {
                Some(key) => Some(key) == partition.as_ref(),
                None => true,
            })
            .map(|cookie| {
                cookie.last_access = now;
                cookie.clone()
            })
            .collect();

        selected.sort_by(|a, b| {
            b.path
                .len()
                .cmp(&a.path.len())
                .then_with(|| a.creation.cmp(&b.creation))
                .then_with(|| a.name.cmp(&b.name))
        });
        selected
    }

    /// The `Cookie` header value for a request, or `None` when no cookie
    /// applies (a `Cookie:` header with an empty value is not sent).
    pub fn cookie_header(
        &self,
        request_url: &Url,
        first_party: Option<&Site>,
        is_http: bool,
        context: SameSiteContext,
        now: SystemTime,
    ) -> Option<String> {
        let cookies = self.cookies_for(request_url, first_party, is_http, context, now);
        if cookies.is_empty() {
            return None;
        }
        Some(header_value(&cookies))
    }

    /// Removes one cookie by its identity.
    pub fn remove(&self, name: &str, domain: &str, path: &str) -> bool {
        self.remove_partitioned(name, domain, path, None)
    }

    pub fn remove_partitioned(
        &self,
        name: &str,
        domain: &str,
        path: &str,
        partition: Option<&PartitionKey>,
    ) -> bool {
        let key = CookieKey {
            partition: partition.cloned(),
            domain: domain.to_ascii_lowercase(),
            path: path.to_string(),
            name: name.to_string(),
        };
        self.inner.cookies.write().remove(&key).is_some()
    }

    pub fn clear(&self) {
        self.inner.cookies.write().clear();
    }

    /// Every cookie in the jar, in no particular order.
    pub fn cookies(&self) -> Vec<Cookie> {
        self.inner.cookies.read().values().cloned().collect()
    }

    /// Snapshot iterator over the jar.
    pub fn iter(&self) -> std::vec::IntoIter<Cookie> {
        self.cookies().into_iter()
    }

    pub fn len(&self) -> usize {
        self.inner.cookies.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.cookies.read().is_empty()
    }

    /// Drops expired cookies and returns how many were removed.
    pub fn purge_expired(&self, now: SystemTime) -> usize {
        let mut cookies = self.inner.cookies.write();
        purge_expired(&mut cookies, now)
    }

    /// Session cookies are dropped when the context closes.
    pub fn remove_session_cookies(&self) -> usize {
        let mut cookies = self.inner.cookies.write();
        let before = cookies.len();
        cookies.retain(|_, cookie| cookie.persistent);
        before - cookies.len()
    }

    /// Playwright's `storageState` shape, for moving state between
    /// TentaBrowser and Playwright (`docs/spec/03-network.md` §6).
    pub fn to_storage_state(&self) -> StorageState {
        StorageState {
            cookies: self
                .cookies()
                .iter()
                .map(StorageCookie::from_cookie)
                .collect(),
            origins: Vec::new(),
        }
    }

    /// Rebuilds a jar from a `storageState`. Cookies whose domain is missing
    /// are rejected with [`tenta_core::Error::InvalidArgument`].
    pub fn from_storage_state(
        state: &StorageState,
        now: SystemTime,
    ) -> tenta_core::Result<CookieJar> {
        let jar = CookieJar::new();
        jar.load_storage_state(state, now)?;
        Ok(jar)
    }

    /// Merges a `storageState` into this jar.
    pub fn load_storage_state(
        &self,
        state: &StorageState,
        now: SystemTime,
    ) -> tenta_core::Result<usize> {
        let mut loaded = 0;
        let mut cookies = self.inner.cookies.write();
        for stored in &state.cookies {
            let cookie = stored.to_cookie(now)?;
            if cookie.is_expired(now) {
                continue;
            }
            cookies.insert(cookie.key(), cookie);
            loaded += 1;
        }
        evict_total(&mut cookies, self.inner.limits.max_total);
        Ok(loaded)
    }
}

/// Renders cookies as a `Cookie` header value.
pub fn header_value(cookies: &[Cookie]) -> String {
    let mut out = String::new();
    for cookie in cookies {
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(&cookie.name);
        out.push('=');
        out.push_str(&cookie.value);
    }
    out
}

/// Parses one `Set-Cookie` header value without partitioning context.
pub fn parse_set_cookie(header: &[u8], request_url: &Url, now: SystemTime) -> Option<Cookie> {
    parse_set_cookie_in(header, request_url, now, None)
}

fn purge_expired(cookies: &mut HashMap<CookieKey, Cookie>, now: SystemTime) -> usize {
    let before = cookies.len();
    cookies.retain(|_, cookie| !cookie.is_expired(now));
    before - cookies.len()
}

/// Evicts the least recently accessed cookies of one registrable domain down
/// to `limit`.
///
/// The quota is per eTLD+1, as it is in Chromium: counting per exact host
/// would give `a.example.com`, `b.example.com` and every other sub-domain a
/// budget of its own, so one site could hold many times the limit.
fn evict_domain(cookies: &mut HashMap<CookieKey, Cookie>, domain: &str, limit: usize) {
    let site = registrable_domain(domain);
    let mut of_domain: Vec<(CookieKey, SystemTime)> = cookies
        .iter()
        .filter(|(key, _)| registrable_domain(&key.domain) == site)
        .map(|(key, cookie)| (key.clone(), cookie.last_access))
        .collect();
    if of_domain.len() <= limit {
        return;
    }
    of_domain.sort_by_key(|(_, last_access)| *last_access);
    for (key, _) in of_domain.iter().take(of_domain.len() - limit) {
        cookies.remove(key);
    }
}

/// Evicts the least recently accessed cookies overall down to `limit`.
fn evict_total(cookies: &mut HashMap<CookieKey, Cookie>, limit: usize) {
    if cookies.len() <= limit {
        return;
    }
    let mut all: Vec<(CookieKey, SystemTime)> = cookies
        .iter()
        .map(|(key, cookie)| (key.clone(), cookie.last_access))
        .collect();
    all.sort_by_key(|(_, last_access)| *last_access);
    for (key, _) in all.iter().take(all.len() - limit) {
        cookies.remove(key);
    }
}

#[cfg(test)]
mod tests;
