//! A deterministic in-tree fuzz pass over the policy parsers.
//!
//! The real targets live in `crates/tenta-net/fuzz` and run under
//! `cargo fuzz` on the fuzz box (`docs/spec/15-testing-and-benchmarks.md` §1).
//! This test is the cheap version that runs on every commit: a fixed PRNG,
//! a seed corpus of realistic headers, and mutations of them. It asserts only
//! that nothing panics and that parsing is deterministic, which is what a
//! parser fed attacker-controlled bytes has to guarantee.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::{HeaderMap, HeaderName, HeaderValue};
use url::Url;

use super::cache::{CacheControl, vary_names};
use super::cookies::{parse_cookie_date, parse_set_cookie};
use super::private_network::is_private_host;
use super::rate_limit::parse_retry_after;
use super::robots::RobotsTxt;

const ITERATIONS: usize = 20_000;
const MAX_LEN: usize = 512;

/// xorshift64*, so the corpus is identical on every machine and every run.
struct Prng(u64);

impl Prng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() % bound as u64) as usize
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() >> 33) as u8
    }
}

const SEEDS: &[&str] = &[
    "sid=abc123; Domain=example.com; Path=/; Secure; HttpOnly; SameSite=Lax",
    "__Host-token=xyz; Secure; Path=/; SameSite=Strict",
    "__Secure-a=b; Secure; Max-Age=3600; Partitioned",
    "a=b; Expires=Wed, 09 Jun 2021 10:18:14 GMT",
    "a=b; Expires=Sunday, 06-Nov-94 08:49:37 GMT; Max-Age=-1",
    "=; ; ;;; Domain=; Path=; Max-Age=",
    "n=v; Domain=co.uk; Path=////; SameSite=None",
    "User-agent: *\nDisallow: /private\nAllow: /private/public\nCrawl-delay: 2\n",
    "User-agent: Googlebot\nDisallow: /*.php$\nSitemap: https://e.example/s.xml\n",
    "user-agent googlebot\ndisallow /\n# comment\n\n\nallow: /a$b*c\n",
    "max-age=600, s-maxage=120, no-cache=\"set-cookie\", stale-while-revalidate=30",
    "no-store, no-cache, must-revalidate, max-stale, min-fresh=10, private",
    "Wed, 09 Jun 2021 10:18:14 GMT",
    "Sun Nov  6 08:49:37 1994",
    "Accept-Encoding, Accept-Language, *",
    "120",
    "localhost.localdomain",
    "",
];

fn mutate(prng: &mut Prng, input: &mut Vec<u8>) {
    match prng.below(6) {
        0 if !input.is_empty() => {
            let at = prng.below(input.len());
            input[at] ^= 1 << prng.below(8);
        }
        1 if input.len() < MAX_LEN => {
            let at = prng.below(input.len() + 1);
            input.insert(at, prng.byte());
        }
        2 if !input.is_empty() => {
            let at = prng.below(input.len());
            input.remove(at);
        }
        3 if !input.is_empty() && input.len() * 2 < MAX_LEN => {
            let at = prng.below(input.len());
            let len = prng.below(input.len() - at) + 1;
            let slice = input[at..at + len].to_vec();
            input.splice(at..at, slice);
        }
        4 => {
            let seed = SEEDS[prng.below(SEEDS.len())].as_bytes();
            let take = prng.below(seed.len().min(MAX_LEN.saturating_sub(input.len())) + 1);
            input.extend_from_slice(&seed[..take]);
        }
        _ => {
            input.truncate(prng.below(input.len() + 1));
        }
    }
    input.truncate(MAX_LEN);
}

fn exercise(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    let request = Url::parse("https://sub.example.com/a/b?q=1").expect("static url");
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    let first = parse_set_cookie(bytes, &request, now);
    let second = parse_set_cookie(bytes, &request, now);
    assert_eq!(first, second, "cookie parsing must be deterministic");
    if let Some(cookie) = first {
        assert!(!cookie.domain.is_empty());
        assert!(cookie.path.starts_with('/'));
        assert!(cookie.name.len() + cookie.value.len() <= super::cookies::MAX_NAME_VALUE_BYTES);
    }

    let date = parse_cookie_date(&text);
    assert_eq!(date, parse_cookie_date(&text));
    if let Some(date) = date {
        // The algorithm rejects years before 1601 and the calendar caps at
        // 9999, so every accepted date sits inside that window.
        let earliest = UNIX_EPOCH - Duration::from_secs(11_644_473_600);
        let latest = UNIX_EPOCH + Duration::from_secs(253_402_300_800);
        assert!(date >= earliest && date <= latest, "{date:?}");
    }

    let robots = RobotsTxt::parse(bytes);
    let _ = robots.is_allowed("TentaBot/1.0", &text);
    let _ = robots.is_allowed("*", "https://example.com/a/b?c=d#e");
    let _ = robots.crawl_delay("TentaBot");
    let _ = robots.sitemaps().len();

    if let Ok(value) = HeaderValue::from_bytes(bytes) {
        let mut headers = HeaderMap::new();
        headers.insert(HeaderName::from_static("cache-control"), value.clone());
        headers.insert(HeaderName::from_static("vary"), value.clone());
        headers.insert(HeaderName::from_static("expires"), value.clone());
        headers.insert(HeaderName::from_static("date"), value.clone());
        headers.insert(HeaderName::from_static("last-modified"), value.clone());
        headers.insert(HeaderName::from_static("age"), value);
        let cc = CacheControl::parse(&headers);
        assert_eq!(cc, CacheControl::parse(&headers));
        let _ = vary_names(&headers);
        let _ = super::cache::current_age(&headers, now, now, now);
        let _ = super::cache::conditional_headers(&headers);
    }

    let _ = parse_retry_after(bytes, now);
    let _ = is_private_host(&text);
}

#[test]
fn parsers_survive_twenty_thousand_mutated_inputs() {
    let mut prng = Prng(0x9e37_79b9_7f4a_7c15);
    let mut checked = 0usize;

    for seed in SEEDS {
        exercise(seed.as_bytes());
        checked += 1;
    }

    let mut input: Vec<u8> = Vec::new();
    while checked < ITERATIONS {
        if checked.is_multiple_of(64) {
            // Restart from a seed so the corpus keeps some structure.
            input = SEEDS[prng.below(SEEDS.len())].as_bytes().to_vec();
        }
        let rounds = 1 + prng.below(4);
        for _ in 0..rounds {
            mutate(&mut prng, &mut input);
        }
        exercise(&input);
        checked += 1;
    }

    // Purely random bytes as well: the seeds keep the corpus near valid input,
    // random bytes probe the paths the seeds never reach.
    for _ in 0..2_000 {
        let len = prng.below(MAX_LEN);
        let random: Vec<u8> = (0..len).map(|_| prng.byte()).collect();
        exercise(&random);
    }

    assert_eq!(checked, ITERATIONS);
}

#[test]
fn oversized_and_degenerate_inputs_do_not_panic() {
    let request = Url::parse("https://example.com/").expect("static url");
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    let huge = vec![b'a'; 1 << 16];
    assert!(parse_set_cookie(&huge, &request, now).is_none());

    let many_semicolons = vec![b';'; 1 << 14];
    assert!(parse_set_cookie(&many_semicolons, &request, now).is_none());

    let mut deep = b"a=b".to_vec();
    for _ in 0..1_000 {
        deep.extend_from_slice(b"; Path=/x");
    }
    assert!(parse_set_cookie(&deep, &request, now).is_some());

    let stars = format!("user-agent: *\ndisallow: /{}\n", "*a".repeat(200));
    let robots = RobotsTxt::parse(stars.as_bytes());
    let _ = robots.is_allowed("bot", &format!("/{}", "a".repeat(400)));

    let newlines = vec![b'\n'; 1 << 14];
    assert!(RobotsTxt::parse(&newlines).is_empty());

    let commas = vec![b','; 1 << 12];
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_bytes(&commas).expect("value"),
    );
    assert_eq!(CacheControl::parse(&headers), CacheControl::default());
}
