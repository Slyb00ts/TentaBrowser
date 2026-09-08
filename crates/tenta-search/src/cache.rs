//! The response cache and the in-flight coalescing behind it
//! (`docs/spec/08-search.md` §3.6).
//!
//! Keyed by the normalised query plus everything that changes what the
//! provider returns; entries expire after a provider-appropriate TTL
//! (10 minutes by default) and identical concurrent queries wait on the one
//! request that is already running instead of issuing their own.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use unicode_normalization::UnicodeNormalization;

use crate::error::SearchError;
use crate::model::{Freshness, SearchRequest};

/// Default entry lifetime (`docs/spec/08-search.md` §3.6).
pub const DEFAULT_TTL: Duration = Duration::from_secs(600);

/// Entries kept before the oldest is evicted.
pub const DEFAULT_CAPACITY: usize = 256;

/// The cache key: everything that changes what a provider answers.
///
/// The query is normalised (trimmed, whitespace collapsed, lowercased)
/// because providers treat those as the same query and an agent asking the
/// same question twice should not pay for it twice.
pub fn cache_key(plan_id: &str, request: &SearchRequest) -> String {
    let mut key = String::with_capacity(plan_id.len() + request.query.len() + 32);
    key.push_str(plan_id);
    key.push('\u{1}');
    key.push_str(&normalise_query(&request.query));
    key.push('\u{1}');
    key.push_str(&request.locale.tag());
    key.push('\u{1}');
    key.push_str(request.safe.as_str());
    key.push('\u{1}');
    key.push_str(request.freshness.map_or("-", Freshness::as_str));
    key.push('\u{1}');
    key.push_str(&request.page.to_string());
    key.push('\u{1}');
    key.push_str(request.site.as_deref().unwrap_or("-"));
    key
}

/// Trimmed, whitespace-collapsed, NFC-normalized, lowercased.
pub fn normalise_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for word in query.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        // NFC normalize, then lowercase the graphemes
        let normalized: String = word.nfc().collect();
        out.extend(normalized.chars().flat_map(char::to_lowercase));
    }
    out
}

/// Hits, misses and coalesced waits since the cache was created.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    /// Queries that waited on another query's in-flight request.
    pub coalesced: u64,
    pub entries: usize,
}

struct Entry<V> {
    value: Arc<V>,
    stored: Instant,
}

struct State<V> {
    entries: HashMap<String, Entry<V>>,
    in_flight: HashMap<String, broadcast::Sender<Result<Arc<V>, SearchError>>>,
}

/// A TTL cache whose misses coalesce.
pub struct ResponseCache<V> {
    ttl: Duration,
    capacity: usize,
    state: Mutex<State<V>>,
    hits: AtomicU64,
    misses: AtomicU64,
    coalesced: AtomicU64,
}

impl<V> ResponseCache<V> {
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            ttl,
            capacity: capacity.max(1),
            state: Mutex::new(State {
                entries: HashMap::new(),
                in_flight: HashMap::new(),
            }),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn stats(&self) -> CacheStats {
        let entries = self.lock().entries.len();
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            coalesced: self.coalesced.load(Ordering::Relaxed),
            entries,
        }
    }

    pub fn clear(&self) {
        self.lock().entries.clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State<V>> {
        // A poisoned lock means a caller panicked while holding it; the cache
        // is a performance aid, so recovering the guard is better than
        // propagating the panic into every later query.
        self.state.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Returns the cached value, or the value `fill` produces, plus whether
    /// the caller was served without its own network request.
    ///
    /// The first caller for a key runs `fill`; callers that arrive while it
    /// runs wait for its result. If the leader is dropped before it finishes
    /// (a cancelled query), the waiters retry rather than hang.
    pub async fn get_or_fill<F, Fut>(
        &self,
        key: String,
        fill: F,
    ) -> Result<(Arc<V>, bool), SearchError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<V, SearchError>>,
    {
        enum Step<V> {
            Hit(Arc<V>),
            Wait(broadcast::Receiver<Result<Arc<V>, SearchError>>),
            Fill(broadcast::Sender<Result<Arc<V>, SearchError>>),
        }

        let mut fill = Some(fill);
        for _ in 0..3 {
            let step = {
                let mut state = self.lock();
                match state.entries.get(&key) {
                    Some(entry) if entry.stored.elapsed() < self.ttl => {
                        Step::Hit(Arc::clone(&entry.value))
                    }
                    Some(_) => {
                        state.entries.remove(&key);
                        match state.in_flight.get(&key) {
                            Some(sender) => Step::Wait(sender.subscribe()),
                            None => {
                                let (sender, _) = broadcast::channel(1);
                                state.in_flight.insert(key.clone(), sender.clone());
                                Step::Fill(sender)
                            }
                        }
                    }
                    None => match state.in_flight.get(&key) {
                        Some(sender) => Step::Wait(sender.subscribe()),
                        None => {
                            let (sender, _) = broadcast::channel(1);
                            state.in_flight.insert(key.clone(), sender.clone());
                            Step::Fill(sender)
                        }
                    },
                }
            };

            match step {
                Step::Hit(value) => {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    return Ok((value, true));
                }
                Step::Wait(mut receiver) => {
                    self.coalesced.fetch_add(1, Ordering::Relaxed);
                    match receiver.recv().await {
                        Ok(Ok(value)) => return Ok((value, true)),
                        Ok(Err(error)) => return Err(error),
                        // The leader was dropped or lagged; take another turn,
                        // which may make this caller the leader.
                        Err(_) => continue,
                    }
                }
                Step::Fill(sender) => {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    let guard = FillGuard {
                        cache: self,
                        key: key.clone(),
                        done: false,
                    };
                    let fill = fill.take().expect("fill runs at most once");
                    let outcome = fill().await.map(Arc::new);
                    let mut guard = guard;
                    guard.done = true;
                    {
                        let mut state = self.lock();
                        state.in_flight.remove(&key);
                        if let Ok(value) = &outcome {
                            evict(&mut state, self.capacity, self.ttl);
                            state.entries.insert(
                                key.clone(),
                                Entry {
                                    value: Arc::clone(value),
                                    stored: Instant::now(),
                                },
                            );
                        }
                    }
                    let _ = sender.send(outcome.clone());
                    return outcome.map(|value| (value, false));
                }
            }
        }
        Err(SearchError::InvalidRequest(
            "cache coalescing did not settle; retry the query".into(),
        ))
    }
}

impl<V> std::fmt::Debug for ResponseCache<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseCache")
            .field("ttl", &self.ttl)
            .field("stats", &self.stats())
            .finish()
    }
}

/// Removes the in-flight marker if the leader's future is dropped, so the
/// waiters see a closed channel and retry instead of waiting forever.
struct FillGuard<'a, V> {
    cache: &'a ResponseCache<V>,
    key: String,
    done: bool,
}

impl<V> Drop for FillGuard<'_, V> {
    fn drop(&mut self) {
        if !self.done {
            self.cache.lock().in_flight.remove(&self.key);
        }
    }
}

fn evict<V>(state: &mut State<V>, capacity: usize, ttl: Duration) {
    if state.entries.len() < capacity {
        return;
    }
    state
        .entries
        .retain(|_, entry| entry.stored.elapsed() < ttl);
    while state.entries.len() >= capacity {
        let Some(oldest) = state
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.stored)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        state.entries.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;

    use super::*;
    use crate::model::{Locale, Provider, SafeSearch};

    #[test]
    fn the_query_is_normalised_before_it_becomes_a_key() {
        assert_eq!(
            normalise_query("  Rust   Browser\tEngine\n"),
            "rust browser engine"
        );
        assert_eq!(normalise_query("ŁÓDŹ"), "łódź");

        // Unicode normalization: "café" (U+00E9) and "café" (e + U+0301) should be the same
        let cafe_nfc = "café"; // U+00E9
        let cafe_nfd = "cafe\u{0301}"; // e + combining accent
        assert_eq!(normalise_query(cafe_nfc), normalise_query(cafe_nfd));

        let base = SearchRequest::new("Rust  Engine").provider(Provider::Bing);
        let same = SearchRequest::new("rust engine").provider(Provider::Bing);
        assert_eq!(cache_key("bing", &base), cache_key("bing", &same));

        // Everything that changes the provider's answer changes the key.
        assert_ne!(cache_key("bing", &base), cache_key("brave", &base));
        for different in [
            base.clone().locale(Locale::parse("pl-PL").expect("locale")),
            base.clone().safe(SafeSearch::Strict),
            base.clone().freshness(Freshness::Day),
            base.clone().page(2),
            base.clone().site("servo.org"),
        ] {
            assert_ne!(
                cache_key("bing", &base),
                cache_key("bing", &different),
                "{different:?}"
            );
        }
        // The limit is applied after the merge, so it is not part of the key.
        assert_eq!(
            cache_key("bing", &base),
            cache_key("bing", &base.clone().limit(42))
        );
    }

    #[tokio::test]
    async fn a_second_caller_is_served_from_the_entry_the_first_stored() {
        let cache: ResponseCache<u32> = ResponseCache::new(Duration::from_secs(60), 8);
        let fills = AtomicU32::new(0);
        let fill = || async {
            fills.fetch_add(1, Ordering::Relaxed);
            Ok(7u32)
        };

        let (first, hit) = cache.get_or_fill("k".into(), fill).await.expect("fill");
        assert_eq!((*first, hit), (7, false));
        let (second, hit) = cache.get_or_fill("k".into(), fill).await.expect("hit");
        assert_eq!((*second, hit), (7, true));
        assert_eq!(fills.load(Ordering::Relaxed), 1);

        let stats = cache.stats();
        assert_eq!((stats.hits, stats.misses, stats.entries), (1, 1, 1));
        cache.clear();
        assert_eq!(cache.stats().entries, 0);
    }

    #[tokio::test]
    async fn a_failed_fill_is_not_cached() {
        let cache: ResponseCache<u32> = ResponseCache::new(Duration::from_secs(60), 8);
        let error = cache
            .get_or_fill("k".into(), || async {
                Err(SearchError::InvalidRequest("no".into()))
            })
            .await
            .expect_err("the fill failed");
        assert!(matches!(error, SearchError::InvalidRequest(_)));
        assert_eq!(cache.stats().entries, 0);

        let (value, hit) = cache
            .get_or_fill("k".into(), || async { Ok(1u32) })
            .await
            .expect("retry");
        assert_eq!((*value, hit), (1, false));
    }

    #[tokio::test]
    async fn the_oldest_entry_is_evicted_when_the_cache_is_full() {
        let cache: ResponseCache<u32> = ResponseCache::new(Duration::from_secs(60), 2);
        for index in 0..3u32 {
            cache
                .get_or_fill(index.to_string(), || async move { Ok(index) })
                .await
                .expect("fill");
        }
        assert!(cache.stats().entries <= 2);
    }
}
