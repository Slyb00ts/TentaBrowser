use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

/// A throwaway directory; see the note in `session_cache::tests`.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "tenta-net-cache-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        TempDir(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn url(text: &str) -> Url {
    Url::parse(text).expect("test url")
}

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.append(
            http::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
            http::HeaderValue::from_str(value).expect("header value"),
        );
    }
    map
}

fn date(at: SystemTime) -> String {
    httpdate::fmt_http_date(at)
}

const URL: &str = "https://example.com/resource";

fn get(request_headers: &HeaderMap, target: &Url) -> CacheRequest<'static> {
    // The borrow checker needs owned inputs to outlive the request; tests
    // leak the small header maps rather than thread lifetimes everywhere.
    let headers: &'static HeaderMap = Box::leak(Box::new(request_headers.clone()));
    let url: &'static Url = Box::leak(Box::new(target.clone()));
    CacheRequest::new("GET", url, headers)
}

fn plain_get() -> CacheRequest<'static> {
    get(&HeaderMap::new(), &url(URL))
}

fn store_response(
    cache: &HttpCache,
    response_headers: &[(&str, &str)],
    body: &str,
    at: SystemTime,
) -> StoreOutcome {
    cache.store(
        &plain_get(),
        200,
        &headers(response_headers),
        Bytes::from(body.to_string()),
        at,
    )
}

// ---------------------------------------------------------------- freshness

#[test]
fn freshness_matrix() {
    /// `(case, response headers, seconds later, expected decision kind)`
    #[derive(Debug, PartialEq)]
    enum Kind {
        Fresh,
        Stale,
        Miss,
    }
    /// `(case name, response headers, seconds later, expected kind)`
    type FreshnessCase<'a> = (&'a str, &'a [(&'a str, &'a str)], u64, Kind);
    let cases: &[FreshnessCase<'_>] = &[
        (
            "max-age not yet reached",
            &[("cache-control", "max-age=60")],
            30,
            Kind::Fresh,
        ),
        (
            "max-age exactly reached",
            &[("cache-control", "max-age=60")],
            60,
            Kind::Stale,
        ),
        (
            "max-age exceeded",
            &[("cache-control", "max-age=60")],
            61,
            Kind::Stale,
        ),
        (
            "max-age zero",
            &[("cache-control", "max-age=0")],
            0,
            Kind::Stale,
        ),
        (
            "no-cache always revalidates",
            &[("cache-control", "no-cache, max-age=600")],
            1,
            Kind::Stale,
        ),
        (
            "pragma no-cache always revalidates",
            &[("pragma", "no-cache"), ("cache-control", "max-age=600")],
            1,
            Kind::Stale,
        ),
        (
            "must-revalidate is fresh until it is not",
            &[("cache-control", "max-age=60, must-revalidate")],
            30,
            Kind::Fresh,
        ),
        (
            "private is storable in a private cache",
            &[("cache-control", "private, max-age=60")],
            30,
            Kind::Fresh,
        ),
        (
            "public behaves like any freshness",
            &[("cache-control", "public, max-age=60")],
            30,
            Kind::Fresh,
        ),
        (
            "s-maxage is ignored by a private cache",
            &[("cache-control", "s-maxage=600"), ("etag", "\"v1\"")],
            30,
            Kind::Stale,
        ),
        (
            "no-store is never stored",
            &[("cache-control", "no-store")],
            0,
            Kind::Miss,
        ),
        (
            "immutable still obeys max-age",
            &[("cache-control", "max-age=60, immutable")],
            30,
            Kind::Fresh,
        ),
    ];

    for (case, response_headers, later, expected) in cases {
        let cache = HttpCache::default();
        let mut with_date = response_headers.to_vec();
        let date_value = date(now());
        with_date.push(("date", date_value.as_str()));
        cache.store(
            &plain_get(),
            200,
            &headers(&with_date),
            Bytes::from_static(b"body"),
            now(),
        );
        let decision = cache.lookup(&plain_get(), now() + Duration::from_secs(*later));
        let kind = match decision {
            CacheDecision::Fresh(_) => Kind::Fresh,
            CacheDecision::Stale { .. } => Kind::Stale,
            _ => Kind::Miss,
        };
        assert_eq!(kind, *expected, "{case}");
    }
}

#[test]
fn expires_and_date_drive_freshness() {
    let cache = HttpCache::default();
    let date_value = date(now());
    let expires_value = date(now() + Duration::from_secs(100));
    cache.store(
        &plain_get(),
        200,
        &headers(&[("date", &date_value), ("expires", &expires_value)]),
        Bytes::from_static(b"body"),
        now(),
    );
    assert!(matches!(
        cache.lookup(&plain_get(), now() + Duration::from_secs(99)),
        CacheDecision::Fresh(_)
    ));
    assert!(matches!(
        cache.lookup(&plain_get(), now() + Duration::from_secs(101)),
        CacheDecision::Stale { .. }
    ));

    // Expires in the past is immediately stale.
    let cache = HttpCache::default();
    let past = date(now() - Duration::from_secs(10));
    cache.store(
        &plain_get(),
        200,
        &headers(&[("date", &date_value), ("expires", &past), ("etag", "\"v\"")]),
        Bytes::from_static(b"body"),
        now(),
    );
    assert!(matches!(
        cache.lookup(&plain_get(), now()),
        CacheDecision::Stale { .. }
    ));

    // max-age wins over Expires.
    let cache = HttpCache::default();
    cache.store(
        &plain_get(),
        200,
        &headers(&[
            ("date", &date_value),
            ("expires", &past),
            ("cache-control", "max-age=600"),
        ]),
        Bytes::from_static(b"body"),
        now(),
    );
    assert!(matches!(
        cache.lookup(&plain_get(), now() + Duration::from_secs(60)),
        CacheDecision::Fresh(_)
    ));
}

#[test]
fn heuristic_freshness_is_a_tenth_of_the_last_modified_age() {
    let cache = HttpCache::default();
    let date_value = date(now());
    let modified = date(now() - Duration::from_secs(1000));
    cache.store(
        &plain_get(),
        200,
        &headers(&[("date", &date_value), ("last-modified", &modified)]),
        Bytes::from_static(b"body"),
        now(),
    );
    assert!(
        matches!(
            cache.lookup(&plain_get(), now() + Duration::from_secs(99)),
            CacheDecision::Fresh(_)
        ),
        "one tenth of 1000 s is 100 s of freshness"
    );
    assert!(matches!(
        cache.lookup(&plain_get(), now() + Duration::from_secs(101)),
        CacheDecision::Stale { .. }
    ));
}

#[test]
fn heuristic_freshness_is_capped() {
    let config = CacheConfig {
        heuristic_cap: Duration::from_secs(60),
        ..CacheConfig::default()
    };
    let cache = HttpCache::new(config);
    let date_value = date(now());
    let modified = date(now() - Duration::from_secs(100_000));
    cache.store(
        &plain_get(),
        200,
        &headers(&[("date", &date_value), ("last-modified", &modified)]),
        Bytes::from_static(b"body"),
        now(),
    );
    assert!(matches!(
        cache.lookup(&plain_get(), now() + Duration::from_secs(59)),
        CacheDecision::Fresh(_)
    ));
    assert!(matches!(
        cache.lookup(&plain_get(), now() + Duration::from_secs(61)),
        CacheDecision::Stale { .. }
    ));
}

#[test]
fn age_header_counts_against_freshness() {
    let cache = HttpCache::default();
    let date_value = date(now());
    cache.store(
        &plain_get(),
        200,
        &headers(&[
            ("date", &date_value),
            ("age", "50"),
            ("cache-control", "max-age=60"),
        ]),
        Bytes::from_static(b"body"),
        now(),
    );
    let decision = cache.lookup(&plain_get(), now() + Duration::from_secs(5));
    match decision {
        CacheDecision::Fresh(cached) => assert_eq!(cached.age, Duration::from_secs(55)),
        other => panic!("expected Fresh, got {other:?}"),
    }
    assert!(matches!(
        cache.lookup(&plain_get(), now() + Duration::from_secs(11)),
        CacheDecision::Stale { .. }
    ));
}

#[test]
fn a_slow_response_ages_by_the_request_delay() {
    let cache = HttpCache::default();
    let date_value = date(now());
    cache.store_with_timing(
        &plain_get(),
        200,
        &headers(&[("date", &date_value), ("cache-control", "max-age=60")]),
        Bytes::from_static(b"body"),
        now() - Duration::from_secs(20),
        now(),
    );
    match cache.lookup(&plain_get(), now()) {
        CacheDecision::Fresh(cached) => assert_eq!(cached.age, Duration::from_secs(20)),
        other => panic!("expected Fresh, got {other:?}"),
    }
}

#[test]
fn a_stale_date_makes_the_response_start_old() {
    let cache = HttpCache::default();
    let date_value = date(now() - Duration::from_secs(30));
    cache.store(
        &plain_get(),
        200,
        &headers(&[("date", &date_value), ("cache-control", "max-age=60")]),
        Bytes::from_static(b"body"),
        now(),
    );
    match cache.lookup(&plain_get(), now()) {
        CacheDecision::Fresh(cached) => assert_eq!(cached.age, Duration::from_secs(30)),
        other => panic!("expected Fresh, got {other:?}"),
    }
    assert!(matches!(
        cache.lookup(&plain_get(), now() + Duration::from_secs(31)),
        CacheDecision::Stale { .. }
    ));
}

// -------------------------------------------------------- request directives

#[test]
fn request_max_age_and_min_fresh() {
    let cache = HttpCache::default();
    let date_value = date(now());
    cache.store(
        &plain_get(),
        200,
        &headers(&[("date", &date_value), ("cache-control", "max-age=600")]),
        Bytes::from_static(b"body"),
        now(),
    );
    let at = now() + Duration::from_secs(100);

    assert!(matches!(
        cache.lookup(&plain_get(), at),
        CacheDecision::Fresh(_)
    ));

    let strict = get(&headers(&[("cache-control", "max-age=50")]), &url(URL));
    assert!(matches!(
        cache.lookup(&strict, at),
        CacheDecision::Stale { .. }
    ));

    let lenient = get(&headers(&[("cache-control", "max-age=200")]), &url(URL));
    assert!(matches!(
        cache.lookup(&lenient, at),
        CacheDecision::Fresh(_)
    ));

    let min_fresh_ok = get(&headers(&[("cache-control", "min-fresh=100")]), &url(URL));
    assert!(matches!(
        cache.lookup(&min_fresh_ok, at),
        CacheDecision::Fresh(_)
    ));

    let min_fresh_too_much = get(&headers(&[("cache-control", "min-fresh=600")]), &url(URL));
    assert!(matches!(
        cache.lookup(&min_fresh_too_much, at),
        CacheDecision::Stale { .. }
    ));

    let no_cache = get(&headers(&[("cache-control", "no-cache")]), &url(URL));
    assert!(matches!(
        cache.lookup(&no_cache, at),
        CacheDecision::Stale { .. }
    ));

    let no_store = get(&headers(&[("cache-control", "no-store")]), &url(URL));
    assert!(matches!(cache.lookup(&no_store, at), CacheDecision::Miss));
}

#[test]
fn request_max_stale_permits_serving_stale() {
    let cache = HttpCache::default();
    let date_value = date(now());
    cache.store(
        &plain_get(),
        200,
        &headers(&[
            ("date", &date_value),
            ("cache-control", "max-age=60"),
            ("etag", "\"v1\""),
        ]),
        Bytes::from_static(b"body"),
        now(),
    );
    let at = now() + Duration::from_secs(100);

    match cache.lookup(&plain_get(), at) {
        CacheDecision::Stale { cached, .. } => assert!(!cached.servable_stale),
        other => panic!("expected Stale, got {other:?}"),
    }

    let bounded = get(&headers(&[("cache-control", "max-stale=60")]), &url(URL));
    match cache.lookup(&bounded, at) {
        CacheDecision::Stale { cached, .. } => assert!(cached.servable_stale),
        other => panic!("expected Stale, got {other:?}"),
    }

    let too_small = get(&headers(&[("cache-control", "max-stale=10")]), &url(URL));
    match cache.lookup(&too_small, at) {
        CacheDecision::Stale { cached, .. } => assert!(!cached.servable_stale),
        other => panic!("expected Stale, got {other:?}"),
    }

    let unbounded = get(&headers(&[("cache-control", "max-stale")]), &url(URL));
    match cache.lookup(&unbounded, at) {
        CacheDecision::Stale { cached, .. } => assert!(cached.servable_stale),
        other => panic!("expected Stale, got {other:?}"),
    }
}

#[test]
fn must_revalidate_forbids_serving_stale() {
    let cache = HttpCache::default();
    let date_value = date(now());
    cache.store(
        &plain_get(),
        200,
        &headers(&[
            ("date", &date_value),
            (
                "cache-control",
                "max-age=60, must-revalidate, stale-while-revalidate=600",
            ),
            ("etag", "\"v1\""),
        ]),
        Bytes::from_static(b"body"),
        now(),
    );
    let unbounded = get(&headers(&[("cache-control", "max-stale")]), &url(URL));
    match cache.lookup(&unbounded, now() + Duration::from_secs(100)) {
        CacheDecision::Stale { cached, .. } => assert!(!cached.servable_stale),
        other => panic!("expected Stale, got {other:?}"),
    }
}

#[test]
fn stale_while_revalidate_is_reported_on_the_stale_decision() {
    let cache = HttpCache::default();
    let date_value = date(now());
    cache.store(
        &plain_get(),
        200,
        &headers(&[
            ("date", &date_value),
            ("cache-control", "max-age=60, stale-while-revalidate=120"),
            ("etag", "\"v1\""),
        ]),
        Bytes::from_static(b"body"),
        now(),
    );
    match cache.lookup(&plain_get(), now() + Duration::from_secs(100)) {
        CacheDecision::Stale {
            cached,
            conditional_headers,
        } => {
            assert!(cached.servable_stale, "inside the revalidation window");
            assert_eq!(conditional_headers.get("if-none-match").unwrap(), "\"v1\"");
        }
        other => panic!("expected Stale, got {other:?}"),
    }
    match cache.lookup(&plain_get(), now() + Duration::from_secs(200)) {
        CacheDecision::Stale { cached, .. } => {
            assert!(!cached.servable_stale, "past the revalidation window")
        }
        other => panic!("expected Stale, got {other:?}"),
    }
}

#[test]
fn only_if_cached_reports_its_own_miss() {
    let cache = HttpCache::default();
    let request = get(&headers(&[("cache-control", "only-if-cached")]), &url(URL));
    assert_eq!(
        cache.lookup(&request, now()),
        CacheDecision::OnlyIfCachedMiss
    );

    let date_value = date(now());
    cache.store(
        &plain_get(),
        200,
        &headers(&[("date", &date_value), ("cache-control", "max-age=60")]),
        Bytes::from_static(b"body"),
        now(),
    );
    assert!(matches!(
        cache.lookup(&request, now()),
        CacheDecision::Fresh(_)
    ));
    assert_eq!(
        cache.lookup(&request, now() + Duration::from_secs(100)),
        CacheDecision::OnlyIfCachedMiss
    );
}

// -------------------------------------------------------------------- vary

#[test]
fn vary_selects_a_variant() {
    let cache = HttpCache::default();
    let date_value = date(now());
    let response = headers(&[
        ("date", &date_value),
        ("cache-control", "max-age=600"),
        ("vary", "accept-encoding"),
    ]);

    let gzip = get(&headers(&[("accept-encoding", "gzip")]), &url(URL));
    let brotli = get(&headers(&[("accept-encoding", "br")]), &url(URL));
    cache.store(
        &gzip,
        200,
        &response,
        Bytes::from_static(b"gzip body"),
        now(),
    );

    match cache.lookup(&gzip, now()) {
        CacheDecision::Fresh(cached) => assert_eq!(cached.body, Bytes::from_static(b"gzip body")),
        other => panic!("expected Fresh, got {other:?}"),
    }
    assert_eq!(cache.lookup(&brotli, now()), CacheDecision::Miss);
    assert_eq!(cache.lookup(&plain_get(), now()), CacheDecision::Miss);

    cache.store(
        &brotli,
        200,
        &response,
        Bytes::from_static(b"br body"),
        now(),
    );
    assert_eq!(cache.len(), 2, "two variants of one key");
    match cache.lookup(&brotli, now()) {
        CacheDecision::Fresh(cached) => assert_eq!(cached.body, Bytes::from_static(b"br body")),
        other => panic!("expected Fresh, got {other:?}"),
    }
}

#[test]
fn vary_on_several_headers_and_on_absent_headers() {
    let cache = HttpCache::default();
    let date_value = date(now());
    let response = headers(&[
        ("date", &date_value),
        ("cache-control", "max-age=600"),
        ("vary", "accept-encoding, accept-language"),
    ]);
    let both = get(
        &headers(&[("accept-encoding", "gzip"), ("accept-language", "en")]),
        &url(URL),
    );
    cache.store(&both, 200, &response, Bytes::from_static(b"body"), now());
    assert!(matches!(
        cache.lookup(&both, now()),
        CacheDecision::Fresh(_)
    ));

    let one_differs = get(
        &headers(&[("accept-encoding", "gzip"), ("accept-language", "de")]),
        &url(URL),
    );
    assert_eq!(cache.lookup(&one_differs, now()), CacheDecision::Miss);

    let one_missing = get(&headers(&[("accept-encoding", "gzip")]), &url(URL));
    assert_eq!(cache.lookup(&one_missing, now()), CacheDecision::Miss);

    // A response that varies on a header the request never sends still
    // matches when the request keeps not sending it.
    let cache = HttpCache::default();
    let response = headers(&[
        ("date", &date_value),
        ("cache-control", "max-age=600"),
        ("vary", "x-custom"),
    ]);
    cache.store(
        &plain_get(),
        200,
        &response,
        Bytes::from_static(b"body"),
        now(),
    );
    assert!(matches!(
        cache.lookup(&plain_get(), now()),
        CacheDecision::Fresh(_)
    ));
    let with_custom = get(&headers(&[("x-custom", "1")]), &url(URL));
    assert_eq!(cache.lookup(&with_custom, now()), CacheDecision::Miss);
}

#[test]
fn vary_star_never_matches() {
    let cache = HttpCache::default();
    let date_value = date(now());
    let response = headers(&[
        ("date", &date_value),
        ("cache-control", "max-age=600"),
        ("vary", "*"),
    ]);
    assert_eq!(
        cache.store(
            &plain_get(),
            200,
            &response,
            Bytes::from_static(b"body"),
            now()
        ),
        StoreOutcome::Stored
    );
    assert_eq!(cache.len(), 1, "the entry is stored");
    assert_eq!(cache.lookup(&plain_get(), now()), CacheDecision::Miss);
}

// -------------------------------------------------------------- validators

#[test]
fn validators_produce_conditional_headers() {
    let cache = HttpCache::default();
    let date_value = date(now());
    let modified = date(now() - Duration::from_secs(10));
    cache.store(
        &plain_get(),
        200,
        &headers(&[
            ("date", &date_value),
            ("cache-control", "max-age=1"),
            ("etag", "\"v1\""),
            ("last-modified", &modified),
        ]),
        Bytes::from_static(b"body"),
        now(),
    );
    match cache.lookup(&plain_get(), now() + Duration::from_secs(10)) {
        CacheDecision::Stale {
            conditional_headers,
            ..
        } => {
            assert_eq!(conditional_headers.get("if-none-match").unwrap(), "\"v1\"");
            assert_eq!(
                conditional_headers.get("if-modified-since").unwrap(),
                modified.as_str()
            );
        }
        other => panic!("expected Stale, got {other:?}"),
    }

    // Only Last-Modified: only If-Modified-Since.
    let cache = HttpCache::default();
    cache.store(
        &plain_get(),
        200,
        &headers(&[
            ("date", &date_value),
            ("cache-control", "max-age=1"),
            ("last-modified", &modified),
        ]),
        Bytes::from_static(b"body"),
        now(),
    );
    match cache.lookup(&plain_get(), now() + Duration::from_secs(10)) {
        CacheDecision::Stale {
            conditional_headers,
            ..
        } => {
            assert!(conditional_headers.get("if-none-match").is_none());
            assert!(conditional_headers.get("if-modified-since").is_some());
        }
        other => panic!("expected Stale, got {other:?}"),
    }
}

#[test]
fn a_304_merges_headers_and_refreshes_the_entry() {
    let cache = HttpCache::default();
    let date_value = date(now());
    cache.store(
        &plain_get(),
        200,
        &headers(&[
            ("date", &date_value),
            ("cache-control", "max-age=60"),
            ("etag", "\"v1\""),
            ("content-type", "text/html"),
            ("content-length", "4"),
            ("x-old", "keep"),
        ]),
        Bytes::from_static(b"body"),
        now(),
    );
    let later = now() + Duration::from_secs(100);
    let CacheDecision::Stale { cached, .. } = cache.lookup(&plain_get(), later) else {
        panic!("expected Stale");
    };

    let new_date = date(later);
    let merged = cache.revalidated(
        &plain_get(),
        &cached,
        &headers(&[
            ("date", &new_date),
            ("cache-control", "max-age=600"),
            ("etag", "\"v2\""),
            ("x-new", "added"),
            ("content-length", "9999"),
        ]),
        later,
    );

    assert_eq!(
        merged.body,
        Bytes::from_static(b"body"),
        "the body is reused"
    );
    assert_eq!(merged.headers.get("etag").unwrap(), "\"v2\"");
    assert_eq!(merged.headers.get("x-old").unwrap(), "keep");
    assert_eq!(merged.headers.get("x-new").unwrap(), "added");
    assert_eq!(
        merged.headers.get("content-length").unwrap(),
        "4",
        "Content-Length must not be updated by a 304"
    );
    assert_eq!(merged.headers.get("content-type").unwrap(), "text/html");

    // The refreshed entry is fresh again under the new max-age.
    assert!(matches!(
        cache.lookup(&plain_get(), later + Duration::from_secs(300)),
        CacheDecision::Fresh(_)
    ));
}

// ------------------------------------------------------------ invalidation

#[test]
fn unsafe_methods_invalidate_the_url() {
    for method in UNSAFE_METHODS {
        let cache = HttpCache::default();
        let date_value = date(now());
        cache.store(
            &plain_get(),
            200,
            &headers(&[("date", &date_value), ("cache-control", "max-age=600")]),
            Bytes::from_static(b"body"),
            now(),
        );
        assert_eq!(cache.len(), 1, "{method}");

        let target = url(URL);
        let empty = HeaderMap::new();
        let request = CacheRequest::new(method, &target, &empty);
        assert_eq!(
            cache.store(&request, 200, &HeaderMap::new(), Bytes::new(), now()),
            StoreOutcome::NotCacheable,
            "{method}"
        );
        assert!(cache.is_empty(), "{method} must invalidate the entry");
    }
}

#[test]
fn an_error_response_to_an_unsafe_method_does_not_invalidate() {
    let cache = HttpCache::default();
    let date_value = date(now());
    cache.store(
        &plain_get(),
        200,
        &headers(&[("date", &date_value), ("cache-control", "max-age=600")]),
        Bytes::from_static(b"body"),
        now(),
    );
    let target = url(URL);
    let empty = HeaderMap::new();
    let request = CacheRequest::new("POST", &target, &empty);
    cache.store(&request, 500, &HeaderMap::new(), Bytes::new(), now());
    assert_eq!(cache.len(), 1);
}

#[test]
fn location_and_content_location_are_invalidated_when_same_origin() {
    let cache = HttpCache::default();
    let date_value = date(now());
    let cacheable = headers(&[("date", &date_value), ("cache-control", "max-age=600")]);

    for target in [
        "https://example.com/resource",
        "https://example.com/list",
        "https://example.com/created",
        "https://other.example/elsewhere",
    ] {
        let request = get(&HeaderMap::new(), &url(target));
        cache.store(
            &request,
            200,
            &cacheable,
            Bytes::from_static(b"body"),
            now(),
        );
    }
    assert_eq!(cache.len(), 4);

    let target = url(URL);
    let empty = HeaderMap::new();
    let post = CacheRequest::new("POST", &target, &empty);
    cache.store(
        &post,
        201,
        &headers(&[
            ("location", "https://example.com/created"),
            ("content-location", "https://example.com/list"),
        ]),
        Bytes::new(),
        now(),
    );

    assert_eq!(cache.len(), 1, "only the cross-origin entry survives");
    let survivor = get(&HeaderMap::new(), &url("https://other.example/elsewhere"));
    assert!(matches!(
        cache.lookup(&survivor, now()),
        CacheDecision::Fresh(_)
    ));
}

#[test]
fn a_cross_origin_location_is_not_invalidated() {
    let cache = HttpCache::default();
    let date_value = date(now());
    let other = get(&HeaderMap::new(), &url("https://other.example/x"));
    cache.store(
        &other,
        200,
        &headers(&[("date", &date_value), ("cache-control", "max-age=600")]),
        Bytes::from_static(b"body"),
        now(),
    );
    let target = url(URL);
    let empty = HeaderMap::new();
    let post = CacheRequest::new("POST", &target, &empty);
    cache.store(
        &post,
        200,
        &headers(&[("location", "https://other.example/x")]),
        Bytes::new(),
        now(),
    );
    assert_eq!(cache.len(), 1);
}

// -------------------------------------------------------------- storability

#[test]
fn cacheable_status_codes() {
    let date_value = date(now());
    let response: Vec<(&str, &str)> = vec![
        ("date", date_value.as_str()),
        ("cache-control", "max-age=600"),
    ];
    for status in CACHEABLE_STATUS {
        let cache = HttpCache::default();
        assert_eq!(
            cache.store(
                &plain_get(),
                *status,
                &headers(&response),
                Bytes::from_static(b"body"),
                now()
            ),
            StoreOutcome::Stored,
            "{status}"
        );
    }
    for status in [201u16, 206, 302, 307, 400, 401, 403, 500, 502, 503] {
        let cache = HttpCache::default();
        assert_eq!(
            cache.store(
                &plain_get(),
                status,
                &headers(&response),
                Bytes::from_static(b"body"),
                now()
            ),
            StoreOutcome::NotCacheable,
            "{status}"
        );
    }
    assert!(
        !CACHEABLE_STATUS.contains(&206),
        "partial content is not cached"
    );
}

#[test]
fn store_refuses_what_it_cannot_use() {
    let cache = HttpCache::default();
    let date_value = date(now());

    assert_eq!(
        store_response(&cache, &[("date", &date_value)], "body", now()),
        StoreOutcome::NoFreshnessInformation
    );
    assert_eq!(
        store_response(
            &cache,
            &[("date", &date_value), ("cache-control", "no-store")],
            "body",
            now()
        ),
        StoreOutcome::NoStore
    );

    let request = get(&headers(&[("cache-control", "no-store")]), &url(URL));
    assert_eq!(
        cache.store(
            &request,
            200,
            &headers(&[("date", &date_value), ("cache-control", "max-age=60")]),
            Bytes::from_static(b"body"),
            now()
        ),
        StoreOutcome::NoStore
    );

    // A validator alone is enough to store.
    assert_eq!(
        store_response(
            &cache,
            &[("date", &date_value), ("etag", "\"v\"")],
            "body",
            now()
        ),
        StoreOutcome::Stored
    );
}

#[test]
fn per_entry_size_limit() {
    let cache = HttpCache::new(CacheConfig {
        max_entry_bytes: 16,
        ..CacheConfig::default()
    });
    let date_value = date(now());
    let response = headers(&[("date", &date_value), ("cache-control", "max-age=600")]);
    assert_eq!(
        cache.store(
            &plain_get(),
            200,
            &response,
            Bytes::from_static(b"0123456789012345"),
            now()
        ),
        StoreOutcome::Stored
    );
    assert_eq!(
        cache.store(
            &plain_get(),
            200,
            &response,
            Bytes::from_static(b"01234567890123456"),
            now()
        ),
        StoreOutcome::TooLarge
    );
}

#[test]
fn total_size_limit_evicts_least_recently_used() {
    let cache = HttpCache::new(CacheConfig {
        max_bytes: 512 * 3 + 30,
        max_entry_bytes: 1024,
        ..CacheConfig::default()
    });
    let date_value = date(now());
    let response = headers(&[("date", &date_value), ("cache-control", "max-age=600")]);
    let urls: Vec<Url> = (0..3)
        .map(|index| url(&format!("https://example.com/{index}")))
        .collect();
    for (index, target) in urls.iter().enumerate() {
        let request = get(&HeaderMap::new(), target);
        cache.store(
            &request,
            200,
            &response,
            Bytes::from_static(b"0123456789"),
            now() + Duration::from_secs(index as u64),
        );
    }
    assert_eq!(cache.len(), 3);

    // Touch the oldest so that the middle one becomes the eviction victim.
    let first = get(&HeaderMap::new(), &urls[0]);
    assert!(matches!(
        cache.lookup(&first, now() + Duration::from_secs(10)),
        CacheDecision::Fresh(_)
    ));

    let fourth = get(&HeaderMap::new(), &url("https://example.com/3"));
    cache.store(
        &fourth,
        200,
        &response,
        Bytes::from_static(b"0123456789"),
        now() + Duration::from_secs(11),
    );
    assert_eq!(cache.len(), 3);
    assert!(matches!(
        cache.lookup(&first, now() + Duration::from_secs(12)),
        CacheDecision::Fresh(_)
    ));
    let second = get(&HeaderMap::new(), &urls[1]);
    assert_eq!(
        cache.lookup(&second, now() + Duration::from_secs(12)),
        CacheDecision::Miss
    );
}

#[test]
fn clear_and_manual_invalidate() {
    let cache = HttpCache::default();
    let date_value = date(now());
    store_response(
        &cache,
        &[("date", &date_value), ("cache-control", "max-age=60")],
        "body",
        now(),
    );
    assert_eq!(cache.invalidate("GET", &url(URL)), 1);
    assert!(cache.is_empty());

    store_response(
        &cache,
        &[("date", &date_value), ("cache-control", "max-age=60")],
        "body",
        now(),
    );
    cache.clear();
    assert!(cache.is_empty());
    assert_eq!(cache.bytes(), 0);
}

#[test]
fn fragments_do_not_split_the_key() {
    let cache = HttpCache::default();
    let date_value = date(now());
    let with_fragment = get(&HeaderMap::new(), &url("https://example.com/resource#a"));
    cache.store(
        &with_fragment,
        200,
        &headers(&[("date", &date_value), ("cache-control", "max-age=60")]),
        Bytes::from_static(b"body"),
        now(),
    );
    assert!(matches!(
        cache.lookup(&plain_get(), now()),
        CacheDecision::Fresh(_)
    ));
}

// --------------------------------------------------------------- disk store

#[test]
fn disk_store_survives_a_reopen() {
    let dir = TempDir::new("reopen");
    let date_value = date(now());
    let response = headers(&[("date", &date_value), ("cache-control", "max-age=600")]);
    {
        let cache = HttpCache::open_disk(CacheConfig::default(), dir.path()).expect("open");
        cache.store(
            &plain_get(),
            200,
            &response,
            Bytes::from_static(b"persisted"),
            now(),
        );
        let other = get(&HeaderMap::new(), &url("https://example.com/other"));
        cache.store(&other, 200, &response, Bytes::from_static(b"second"), now());
        assert_eq!(cache.len(), 2);
    }

    let cache = HttpCache::open_disk(CacheConfig::default(), dir.path()).expect("reopen");
    assert_eq!(cache.len(), 2);
    match cache.lookup(&plain_get(), now() + Duration::from_secs(10)) {
        CacheDecision::Fresh(cached) => {
            assert_eq!(cached.body, Bytes::from_static(b"persisted"));
            assert_eq!(cached.status, 200);
            assert_eq!(cached.headers.get("cache-control").unwrap(), "max-age=600");
        }
        other => panic!("expected Fresh, got {other:?}"),
    }
}

#[test]
fn disk_store_recovers_from_a_truncated_last_record() {
    let dir = TempDir::new("truncate");
    let date_value = date(now());
    let response = headers(&[("date", &date_value), ("cache-control", "max-age=600")]);
    let path = {
        let store = DiskStore::open(dir.path()).expect("open");
        let path = store.path().to_path_buf();
        let cache = HttpCache::with_store(CacheConfig::default(), Box::new(store)).expect("cache");
        cache.store(
            &plain_get(),
            200,
            &response,
            Bytes::from_static(b"first"),
            now(),
        );
        let other = get(&HeaderMap::new(), &url("https://example.com/other"));
        cache.store(&other, 200, &response, Bytes::from_static(b"second"), now());
        path
    };

    let contents = std::fs::read(&path).expect("read log");
    // Cut into the middle of the last record, as a crash would.
    std::fs::write(&path, &contents[..contents.len() - 4]).expect("truncate");

    let cache = HttpCache::open_disk(CacheConfig::default(), dir.path()).expect("reopen");
    assert_eq!(cache.len(), 1, "the intact record survives");
    match cache.lookup(&plain_get(), now()) {
        CacheDecision::Fresh(cached) => assert_eq!(cached.body, Bytes::from_static(b"first")),
        other => panic!("expected Fresh, got {other:?}"),
    }

    // The file was truncated to the last complete record, so writing again
    // does not append after garbage.
    let third = get(&HeaderMap::new(), &url("https://example.com/third"));
    cache.store(&third, 200, &response, Bytes::from_static(b"third"), now());
    drop(cache);
    let cache = HttpCache::open_disk(CacheConfig::default(), dir.path()).expect("reopen again");
    assert_eq!(cache.len(), 2);
}

#[test]
fn disk_store_tombstones_and_compacts() {
    let dir = TempDir::new("compact");
    let date_value = date(now());
    let response = headers(&[("date", &date_value), ("cache-control", "max-age=600")]);
    let store = DiskStore::open(dir.path()).expect("open");
    let cache = HttpCache::with_store(CacheConfig::default(), Box::new(store)).expect("cache");

    // Rewriting the same key repeatedly is what makes dead bytes accumulate.
    for index in 0..8 {
        cache.store(
            &plain_get(),
            200,
            &response,
            Bytes::from(format!("body-{index}")),
            now() + Duration::from_secs(index),
        );
    }
    assert_eq!(cache.len(), 1);
    // Compaction runs automatically once dead bytes pass half the file.
    assert!(cache.disk_bytes() < 8 * 200, "the log was compacted");

    cache.invalidate("GET", &url(URL));
    cache.compact().expect("manual compaction");
    drop(cache);

    let cache = HttpCache::open_disk(CacheConfig::default(), dir.path()).expect("reopen");
    assert!(cache.is_empty(), "the tombstoned entry does not come back");
}

#[test]
fn disk_store_keeps_the_newest_record_of_a_key() {
    let dir = TempDir::new("newest");
    let date_value = date(now());
    let response = headers(&[("date", &date_value), ("cache-control", "max-age=600")]);
    {
        let cache = HttpCache::open_disk(CacheConfig::default(), dir.path()).expect("open");
        cache.store(
            &plain_get(),
            200,
            &response,
            Bytes::from_static(b"old"),
            now(),
        );
        cache.store(
            &plain_get(),
            200,
            &response,
            Bytes::from_static(b"new"),
            now() + Duration::from_secs(1),
        );
    }
    let cache = HttpCache::open_disk(CacheConfig::default(), dir.path()).expect("reopen");
    assert_eq!(cache.len(), 1);
    match cache.lookup(&plain_get(), now() + Duration::from_secs(2)) {
        CacheDecision::Fresh(cached) => assert_eq!(cached.body, Bytes::from_static(b"new")),
        other => panic!("expected Fresh, got {other:?}"),
    }
}

#[test]
fn memory_store_is_a_no_op_layer() {
    let store = MemoryStore;
    assert!(store.load().expect("load").is_empty());
    assert!(!store.needs_compaction());
    assert_eq!(store.disk_bytes(), 0);
    let cache = HttpCache::default();
    assert_eq!(cache.disk_bytes(), 0);
}

// ----------------------------------------------------------- header parsing

#[test]
fn cache_control_parsing() {
    let cc = CacheControl::parse(&headers(&[(
        "cache-control",
        "max-age=60, s-maxage=120, no-cache=\"set-cookie, x-secret\", public, must-revalidate, stale-while-revalidate=30",
    )]));
    assert_eq!(cc.max_age, Some(60));
    assert_eq!(cc.s_maxage, Some(120));
    assert!(cc.no_cache);
    assert_eq!(cc.no_cache_fields, ["set-cookie", "x-secret"]);
    assert!(cc.public);
    assert!(cc.must_revalidate);
    assert_eq!(cc.stale_while_revalidate, Some(30));
    assert!(!cc.no_store);

    let cc = CacheControl::parse(&headers(&[
        ("cache-control", "no-store"),
        ("cache-control", "private"),
    ]));
    assert!(cc.no_store);
    assert!(cc.private);

    let cc = CacheControl::parse(&headers(&[("cache-control", "MAX-AGE=5, NO-CACHE")]));
    assert_eq!(cc.max_age, Some(5));
    assert!(cc.no_cache);

    let cc = CacheControl::parse(&headers(&[("cache-control", "max-age=nonsense, ,,")]));
    assert_eq!(cc.max_age, None);

    let cc = CacheControl::parse(&headers(&[("cache-control", "max-stale")]));
    assert_eq!(cc.max_stale, Some(None));
    let cc = CacheControl::parse(&headers(&[("cache-control", "max-stale=30")]));
    assert_eq!(cc.max_stale, Some(Some(30)));

    assert_eq!(
        CacheControl::parse(&HeaderMap::new()),
        CacheControl::default()
    );
}

#[test]
fn vary_name_parsing() {
    assert_eq!(
        vary_names(&headers(&[("vary", "Accept-Encoding, Accept-Language")])),
        Some(vec!["accept-encoding".into(), "accept-language".into()])
    );
    assert_eq!(
        vary_names(&headers(&[("vary", "accept"), ("vary", "origin")])),
        Some(vec!["accept".into(), "origin".into()])
    );
    assert_eq!(vary_names(&HeaderMap::new()), None);
    assert!(vary_is_star(&headers(&[("vary", "*")])));
    assert!(vary_is_star(&headers(&[("vary", "accept, *")])));
    assert!(!vary_is_star(&headers(&[("vary", "accept")])));
}

#[test]
fn method_and_status_predicates() {
    assert!(is_cacheable_method("GET"));
    assert!(is_cacheable_method("head"));
    assert!(!is_cacheable_method("POST"));
    assert!(is_unsafe_method("post"));
    assert!(is_unsafe_method("DELETE"));
    assert!(!is_unsafe_method("GET"));
    assert!(is_cacheable_status(200));
    assert!(is_cacheable_status(404));
    assert!(!is_cacheable_status(206));
    assert!(!is_cacheable_status(500));
}
