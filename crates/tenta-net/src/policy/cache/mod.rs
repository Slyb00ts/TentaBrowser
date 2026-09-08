//! RFC 9111 HTTP cache (`docs/spec/03-network.md` §5).
//!
//! Memory index plus an optional append-only disk log, per BrowserContext for
//! isolation. Bodies are [`Bytes`] shared with the parser and script rather
//! than copied.
//!
//! This is a **private** cache: `s-maxage` is ignored, `private` responses are
//! storable, and a request carrying `Authorization` does not stop storage.

mod headers;
mod store;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use hashbrown::HashMap;
use http::HeaderMap;
use parking_lot::RwLock;
use tenta_core::Result;
use url::Url;

pub use headers::{
    CACHEABLE_STATUS, CacheControl, UNSAFE_METHODS, conditional_headers, current_age,
    is_cacheable_method, is_cacheable_status, is_unsafe_method, merge_304, vary_is_star,
    vary_names,
};
pub use store::{CacheRecord, CacheStore, DiskStore, MemoryStore};

/// The request fields the cache needs.
#[derive(Debug, Clone, Copy)]
pub struct CacheRequest<'a> {
    pub method: &'a str,
    pub url: &'a Url,
    pub headers: &'a HeaderMap,
}

impl<'a> CacheRequest<'a> {
    pub fn new(method: &'a str, url: &'a Url, headers: &'a HeaderMap) -> Self {
        Self {
            method,
            url,
            headers,
        }
    }

    fn primary_key(&self) -> String {
        let mut url = self.url.clone();
        url.set_fragment(None);
        format!("{} {url}", self.method.to_ascii_uppercase())
    }
}

/// A stored response handed back to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
    /// The response's current age, which the caller must put in `Age`.
    pub age: Duration,
    /// Whether the cache permits serving this stale body while a revalidation
    /// runs in the background (`stale-while-revalidate`, request `max-stale`).
    pub servable_stale: bool,
    key: String,
    variant: u64,
}

impl CachedResponse {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn variant(&self) -> u64 {
        self.variant
    }
}

/// What [`HttpCache::lookup`] concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheDecision {
    /// Usable as is.
    Fresh(CachedResponse),
    /// Stored but not fresh: revalidate with `conditional_headers`. When
    /// `cached.servable_stale` is set the body may be used immediately and
    /// the revalidation run in the background.
    Stale {
        cached: CachedResponse,
        conditional_headers: HeaderMap,
    },
    /// Nothing usable is stored.
    Miss,
    /// The request said `only-if-cached` and nothing usable is stored, so the
    /// caller must synthesise `504 Gateway Timeout` (RFC 9111 §5.2.1.7).
    OnlyIfCachedMiss,
}

impl CacheDecision {
    pub fn is_miss(&self) -> bool {
        matches!(self, CacheDecision::Miss | CacheDecision::OnlyIfCachedMiss)
    }

    pub fn cached(&self) -> Option<&CachedResponse> {
        match self {
            CacheDecision::Fresh(cached) | CacheDecision::Stale { cached, .. } => Some(cached),
            _ => None,
        }
    }
}

/// What [`HttpCache::store`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreOutcome {
    Stored,
    /// The method is not cacheable, or an unsafe method invalidated instead.
    NotCacheable,
    /// `no-store` on the request or the response.
    NoStore,
    /// Larger than `max_entry_bytes`.
    TooLarge,
    /// Nothing to revalidate with and nothing to say when it goes stale.
    NoFreshnessInformation,
}

/// Cache limits and heuristics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheConfig {
    pub max_bytes: u64,
    pub max_entry_bytes: u64,
    /// Fraction of the `Last-Modified` age used as heuristic freshness.
    pub heuristic_fraction: f64,
    /// Upper bound on heuristic freshness.
    pub heuristic_cap: Duration,
    /// A shared cache honours `s-maxage` and refuses `private`; the engine's
    /// per-context cache is private, so this is false.
    pub shared: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
            max_entry_bytes: 8 * 1024 * 1024,
            heuristic_fraction: 0.1,
            heuristic_cap: Duration::from_secs(24 * 3600),
            shared: false,
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    variant: u64,
    status: u16,
    headers: HeaderMap,
    varied: Vec<(String, Option<String>)>,
    vary_star: bool,
    request_time: SystemTime,
    response_time: SystemTime,
    last_access: SystemTime,
    body: Bytes,
}

impl Entry {
    fn weight(&self) -> u64 {
        self.body.len() as u64 + 512
    }

    fn to_record(&self, key: &str) -> CacheRecord {
        CacheRecord {
            key: key.to_string(),
            variant: self.variant,
            status: self.status,
            headers: self
                .headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.as_str().to_string(), value.to_string()))
                })
                .collect(),
            varied: self.varied.clone(),
            vary_star: self.vary_star,
            request_time: to_unix(self.request_time),
            response_time: to_unix(self.response_time),
            last_access: to_unix(self.last_access),
            body: self.body.clone(),
        }
    }

    fn from_record(record: CacheRecord) -> (String, Entry) {
        let mut headers = HeaderMap::new();
        for (name, value) in &record.headers {
            if let (Ok(name), Ok(value)) = (
                http::HeaderName::from_bytes(name.as_bytes()),
                http::HeaderValue::from_str(value),
            ) {
                headers.append(name, value);
            }
        }
        let entry = Entry {
            variant: record.variant,
            status: record.status,
            headers,
            varied: record.varied,
            vary_star: record.vary_star,
            request_time: from_unix(record.request_time),
            response_time: from_unix(record.response_time),
            last_access: from_unix(record.last_access),
            body: record.body,
        };
        (record.key, entry)
    }
}

#[derive(Debug, Default)]
struct Index {
    entries: HashMap<String, Vec<Entry>>,
    bytes: u64,
}

#[derive(Debug)]
struct Inner {
    config: CacheConfig,
    store: Box<dyn CacheStore>,
    index: RwLock<Index>,
}

/// The response cache of one BrowserContext. Cloning shares the storage.
#[derive(Debug, Clone)]
pub struct HttpCache {
    inner: Arc<Inner>,
}

impl Default for HttpCache {
    fn default() -> Self {
        Self::new(CacheConfig::default())
    }
}

impl HttpCache {
    /// A memory-only cache.
    pub fn new(config: CacheConfig) -> Self {
        Self::with_store(config, Box::new(MemoryStore))
            .expect("the memory store cannot fail to load")
    }

    /// A cache backed by `store`, whose contents are loaded into the index.
    pub fn with_store(config: CacheConfig, store: Box<dyn CacheStore>) -> Result<Self> {
        let mut index = Index::default();
        for record in store.load()? {
            let (key, entry) = Entry::from_record(record);
            index.bytes += entry.weight();
            index.entries.entry(key).or_default().push(entry);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                store,
                index: RwLock::new(index),
            }),
        })
    }

    /// A cache backed by an append-only log under `dir`.
    ///
    /// `dir` must already have passed the host's `FsPolicy` check
    /// (`docs/spec/19-security.md` §1); this constructor does not check it.
    pub fn open_disk(config: CacheConfig, dir: impl Into<std::path::PathBuf>) -> Result<Self> {
        Self::with_store(config, Box::new(DiskStore::open(dir)?))
    }

    pub fn config(&self) -> CacheConfig {
        self.inner.config
    }

    /// Whether storing an entry writes to the filesystem.
    pub fn writes_block(&self) -> bool {
        self.inner.store.writes_block()
    }

    /// Bytes accounted for by stored entries, bodies plus a fixed overhead.
    pub fn bytes(&self) -> u64 {
        self.inner.index.read().bytes
    }

    pub fn len(&self) -> usize {
        self.inner.index.read().entries.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Decides what to do with a request.
    pub fn lookup(&self, request: &CacheRequest<'_>, now: SystemTime) -> CacheDecision {
        let request_cc = CacheControl::parse(request.headers);
        if request_cc.no_store {
            return CacheDecision::Miss;
        }
        if !is_cacheable_method(request.method) {
            return miss(&request_cc);
        }

        let key = request.primary_key();
        let mut index = self.inner.index.write();
        let Some(entries) = index.entries.get_mut(&key) else {
            return miss(&request_cc);
        };
        let Some(entry) = entries.iter_mut().find(|entry| {
            !entry.vary_star && headers::varied_matches(request.headers, &entry.varied)
        }) else {
            return miss(&request_cc);
        };
        entry.last_access = now;

        let response_cc = CacheControl::parse(&entry.headers);
        let age = current_age(&entry.headers, entry.request_time, entry.response_time, now);
        let lifetime = self.freshness_lifetime(&entry.headers, &response_cc, entry.response_time);

        let mut fresh = lifetime > age;
        if response_cc.no_cache || (self.inner.config.shared && response_cc.proxy_revalidate) {
            fresh = false;
        }
        if request_cc.no_cache {
            fresh = false;
        }
        if let Some(max_age) = request_cc.max_age
            && age > Duration::from_secs(max_age)
        {
            fresh = false;
        }
        if let Some(min_fresh) = request_cc.min_fresh
            && lifetime.saturating_sub(age) < Duration::from_secs(min_fresh)
        {
            fresh = false;
        }

        let staleness = age.saturating_sub(lifetime);
        let may_serve_stale = !response_cc.must_revalidate
            && !response_cc.no_cache
            && !request_cc.no_cache
            && (response_cc
                .stale_while_revalidate
                .is_some_and(|window| staleness <= Duration::from_secs(window))
                || match request_cc.max_stale {
                    Some(None) => true,
                    Some(Some(limit)) => staleness <= Duration::from_secs(limit),
                    None => false,
                });

        let cached = CachedResponse {
            status: entry.status,
            headers: entry.headers.clone(),
            body: entry.body.clone(),
            age,
            servable_stale: !fresh && may_serve_stale,
            key: key.clone(),
            variant: entry.variant,
        };
        let conditionals = conditional_headers(&entry.headers);

        if fresh {
            return CacheDecision::Fresh(cached);
        }
        if request_cc.only_if_cached && !cached.servable_stale {
            return CacheDecision::OnlyIfCachedMiss;
        }
        CacheDecision::Stale {
            cached,
            conditional_headers: conditionals,
        }
    }

    /// Stores a response, and invalidates the URL when the method is unsafe.
    pub fn store(
        &self,
        request: &CacheRequest<'_>,
        status: u16,
        response_headers: &HeaderMap,
        body: Bytes,
        now: SystemTime,
    ) -> StoreOutcome {
        self.store_with_timing(request, status, response_headers, body, now, now)
    }

    /// [`HttpCache::store`] with a separate request time, which is what makes
    /// the `Age` correction for a slow response meaningful.
    pub fn store_with_timing(
        &self,
        request: &CacheRequest<'_>,
        status: u16,
        response_headers: &HeaderMap,
        body: Bytes,
        request_time: SystemTime,
        response_time: SystemTime,
    ) -> StoreOutcome {
        if is_unsafe_method(request.method) {
            // A non-error response to an unsafe method invalidates what the
            // cache knows about the target (RFC 9111 §4.4).
            if (200..400).contains(&status) {
                self.invalidate_for(request, response_headers);
            }
            return StoreOutcome::NotCacheable;
        }
        if !is_cacheable_method(request.method) || !is_cacheable_status(status) {
            return StoreOutcome::NotCacheable;
        }

        let request_cc = CacheControl::parse(request.headers);
        let response_cc = CacheControl::parse(response_headers);
        if request_cc.no_store || response_cc.no_store {
            return StoreOutcome::NoStore;
        }
        if self.inner.config.shared && response_cc.private {
            return StoreOutcome::NoStore;
        }
        if body.len() as u64 > self.inner.config.max_entry_bytes {
            return StoreOutcome::TooLarge;
        }

        // Without a validator and without any freshness signal an entry can
        // never be used: it would be stale on the next lookup with nothing to
        // revalidate against.
        let has_freshness = headers::explicit_freshness_lifetime(
            response_headers,
            &response_cc,
            self.inner.config.shared,
            response_time,
        )
        .is_some()
            || response_headers.contains_key("last-modified");
        if !has_freshness && !headers::has_validator(response_headers) {
            return StoreOutcome::NoFreshnessInformation;
        }

        let names = vary_names(response_headers);
        let vary_star = vary_is_star(response_headers);
        let varied = names
            .as_deref()
            .map(|names| headers::varied_values(request.headers, names))
            .unwrap_or_default();
        let variant = variant_id(&varied);

        let entry = Entry {
            variant,
            status,
            headers: response_headers.clone(),
            varied,
            vary_star,
            request_time,
            response_time,
            last_access: response_time,
            body,
        };
        let key = request.primary_key();
        let record = entry.to_record(&key);

        {
            let mut index = self.inner.index.write();
            let weight = entry.weight();
            let entries = index.entries.entry(key).or_default();
            let replaced = match entries.iter_mut().find(|e| e.variant == variant) {
                Some(existing) => {
                    let previous = existing.weight();
                    *existing = entry;
                    Some(previous)
                }
                None => {
                    entries.push(entry);
                    None
                }
            };
            index.bytes = index.bytes + weight - replaced.unwrap_or(0);
        }
        if let Err(error) = self.inner.store.append(&record) {
            tracing::warn!(%error, "cache record could not be written");
        }
        self.evict_to_fit();
        self.maybe_compact();
        StoreOutcome::Stored
    }

    /// Applies a `304 Not Modified` to a stored response and returns the
    /// merged result (RFC 9111 §4.3.4).
    pub fn revalidated(
        &self,
        request: &CacheRequest<'_>,
        cached: &CachedResponse,
        response_headers: &HeaderMap,
        now: SystemTime,
    ) -> CachedResponse {
        let key = request.primary_key();
        let mut merged = cached.headers.clone();
        merge_304(&mut merged, response_headers);

        let mut updated = CachedResponse {
            status: cached.status,
            headers: merged.clone(),
            body: cached.body.clone(),
            age: Duration::ZERO,
            servable_stale: false,
            key: key.clone(),
            variant: cached.variant,
        };

        let record = {
            let mut index = self.inner.index.write();
            let Some(entries) = index.entries.get_mut(&key) else {
                return updated;
            };
            let Some(entry) = entries.iter_mut().find(|e| e.variant == cached.variant) else {
                return updated;
            };
            entry.headers = merged;
            entry.request_time = now;
            entry.response_time = now;
            entry.last_access = now;
            updated.age = current_age(&entry.headers, now, now, now);
            entry.to_record(&key)
        };
        if let Err(error) = self.inner.store.append(&record) {
            tracing::warn!(%error, "revalidated cache record could not be written");
        }
        self.maybe_compact();
        updated
    }

    /// Drops every variant stored for a method and URL.
    pub fn invalidate(&self, method: &str, url: &Url) -> usize {
        let mut url = url.clone();
        url.set_fragment(None);
        let key = format!("{} {url}", method.to_ascii_uppercase());
        let removed = {
            let mut index = self.inner.index.write();
            match index.entries.remove(&key) {
                Some(entries) => {
                    for entry in &entries {
                        index.bytes = index.bytes.saturating_sub(entry.weight());
                    }
                    entries.into_iter().map(|entry| entry.variant).collect()
                }
                None => Vec::new(),
            }
        };
        for variant in &removed {
            if let Err(error) = self.inner.store.remove(&key, *variant) {
                tracing::warn!(%error, "cache tombstone could not be written");
            }
        }
        removed.len()
    }

    /// Invalidates the request URL and, when they are same-origin, the
    /// `Location` and `Content-Location` targets (RFC 9111 §4.4).
    pub fn invalidate_for(
        &self,
        request: &CacheRequest<'_>,
        response_headers: &HeaderMap,
    ) -> usize {
        let mut removed = self.invalidate_all_methods(request.url);
        for name in ["location", "content-location"] {
            let Some(value) = response_headers.get(name).and_then(|v| v.to_str().ok()) else {
                continue;
            };
            let Ok(target) = request.url.join(value) else {
                continue;
            };
            if target.origin() != request.url.origin() {
                continue;
            }
            removed += self.invalidate_all_methods(&target);
        }
        removed
    }

    fn invalidate_all_methods(&self, url: &Url) -> usize {
        self.invalidate("GET", url) + self.invalidate("HEAD", url)
    }

    pub fn clear(&self) {
        let keys: Vec<(String, u64)> = {
            let mut index = self.inner.index.write();
            let keys = index
                .entries
                .iter()
                .flat_map(|(key, entries)| {
                    entries
                        .iter()
                        .map(move |entry| (key.clone(), entry.variant))
                })
                .collect();
            index.entries.clear();
            index.bytes = 0;
            keys
        };
        for (key, variant) in keys {
            let _ = self.inner.store.remove(&key, variant);
        }
    }

    /// Rewrites the log with only the live entries in it.
    pub fn compact(&self) -> Result<()> {
        let live = self.live_records();
        self.inner.store.compact(&live)
    }

    /// Bytes the durable store occupies, zero for a memory-only cache.
    pub fn disk_bytes(&self) -> u64 {
        self.inner.store.disk_bytes()
    }

    fn live_records(&self) -> Vec<CacheRecord> {
        let index = self.inner.index.read();
        index
            .entries
            .iter()
            .flat_map(|(key, entries)| entries.iter().map(|entry| entry.to_record(key)))
            .collect()
    }

    fn maybe_compact(&self) {
        if !self.inner.store.needs_compaction() {
            return;
        }
        let live = self.live_records();
        if let Err(error) = self.inner.store.compact(&live) {
            tracing::warn!(%error, "cache compaction failed");
        }
    }

    fn freshness_lifetime(
        &self,
        response_headers: &HeaderMap,
        cc: &CacheControl,
        response_time: SystemTime,
    ) -> Duration {
        if let Some(lifetime) = headers::explicit_freshness_lifetime(
            response_headers,
            cc,
            self.inner.config.shared,
            response_time,
        ) {
            return lifetime;
        }
        headers::heuristic_freshness_lifetime(
            response_headers,
            response_time,
            self.inner.config.heuristic_fraction,
            self.inner.config.heuristic_cap,
        )
        .unwrap_or(Duration::ZERO)
    }

    /// Evicts least-recently-accessed entries until the byte budget holds.
    fn evict_to_fit(&self) {
        let mut victims: Vec<(String, u64)> = Vec::new();
        {
            let mut index = self.inner.index.write();
            if index.bytes <= self.inner.config.max_bytes {
                return;
            }
            let mut candidates: Vec<(String, u64, SystemTime, u64)> = index
                .entries
                .iter()
                .flat_map(|(key, entries)| {
                    entries.iter().map(move |entry| {
                        (
                            key.clone(),
                            entry.variant,
                            entry.last_access,
                            entry.weight(),
                        )
                    })
                })
                .collect();
            candidates.sort_by_key(|candidate| candidate.2);

            for (key, variant, _, weight) in candidates {
                if index.bytes <= self.inner.config.max_bytes {
                    break;
                }
                if let Some(entries) = index.entries.get_mut(&key) {
                    entries.retain(|entry| entry.variant != variant);
                    if entries.is_empty() {
                        index.entries.remove(&key);
                    }
                }
                index.bytes = index.bytes.saturating_sub(weight);
                victims.push((key, variant));
            }
        }
        for (key, variant) in victims {
            let _ = self.inner.store.remove(&key, variant);
        }
    }
}

fn miss(request_cc: &CacheControl) -> CacheDecision {
    if request_cc.only_if_cached {
        CacheDecision::OnlyIfCachedMiss
    } else {
        CacheDecision::Miss
    }
}

/// A stable id for one `Vary` variant of a key.
fn variant_id(varied: &[(String, Option<String>)]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for (name, value) in varied {
        mix(name.as_bytes());
        mix(b"\x01");
        match value {
            Some(value) => mix(value.as_bytes()),
            None => mix(b"\x00"),
        }
        mix(b"\x02");
    }
    hash
}

fn to_unix(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn from_unix(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

#[cfg(test)]
mod tests;
