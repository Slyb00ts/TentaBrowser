#![no_main]

use std::time::{Duration, UNIX_EPOCH};

use http::{HeaderMap, HeaderName, HeaderValue};
use libfuzzer_sys::fuzz_target;
use tenta_net::policy::cache::{CacheControl, vary_names};
use tenta_net::policy::rate_limit::parse_retry_after;
use tenta_net::policy::robots::RobotsTxt;

// The other policy parsers that read server-controlled bytes: robots.txt,
// Cache-Control / Vary, Retry-After.
fuzz_target!(|data: &[u8]| {
    let robots = RobotsTxt::parse(data);
    let _ = robots.is_empty();

    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let _ = parse_retry_after(data, now);

    if let Ok(value) = HeaderValue::from_bytes(data) {
        let mut headers = HeaderMap::new();
        headers.insert(HeaderName::from_static("cache-control"), value.clone());
        headers.insert(HeaderName::from_static("vary"), value);
        let first = CacheControl::parse(&headers);
        assert_eq!(first, CacheControl::parse(&headers));
        let _ = vary_names(&headers);
    }
});
