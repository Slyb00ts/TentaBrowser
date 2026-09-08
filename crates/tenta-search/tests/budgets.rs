//! `S-search-fast` (`docs/spec/15-testing-and-benchmarks.md` §2) and the
//! budgets of `docs/spec/08-search.md` §6.
//!
//! Ignored by default: these are measurements, not assertions about
//! behaviour, and they are only meaningful in a release build.
//!
//! ```bash
//! cargo nextest run -p tenta-search --release --run-ignored all --no-capture
//! ```

mod common;

use std::time::{Duration, Instant};

use common::{Harness, fixture, parse};
use tenta_search::extract::extract;
use tenta_search::{PlanSet, Provider, SearchOptions, SearchRequest};

/// Queries measured per scenario.
const ITERATIONS: usize = 200;
/// Queries run before measurement starts, so the pool, the DNS cache and the
/// allocator are warm.
const WARMUP: usize = 20;

fn percentile(sorted: &[Duration], percent: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let index = ((sorted.len() as f64 - 1.0) * percent / 100.0).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn report(name: &str, mut samples: Vec<Duration>) -> (Duration, Duration) {
    samples.sort();
    let p50 = percentile(&samples, 50.0);
    let p95 = percentile(&samples, 95.0);
    let max = samples.last().copied().unwrap_or_default();
    println!(
        "{name}: n={} p50={:.3} ms p95={:.3} ms max={:.3} ms",
        samples.len(),
        p50.as_secs_f64() * 1e3,
        p95.as_secs_f64() * 1e3,
        max.as_secs_f64() * 1e3,
    );
    (p50, p95)
}

/// Resident set size of this process, in bytes, or `None` where the platform
/// does not hand it over cheaply.
fn resident_bytes() -> Option<u64> {
    if cfg!(target_os = "linux") {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        return Some(pages * 4096);
    }
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    let kilobytes: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    Some(kilobytes * 1024)
}

#[tokio::test]
#[ignore = "budget scenario: run with --release --run-ignored all --no-capture"]
async fn s_search_fast_engine_overhead() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("duckduckgo_html", "rust-browser-engine");
    harness.serve_fixture("bing", "rust-browser-engine");
    harness.serve_fixture("brave", "rust-browser-engine");
    let search = harness.search_with(SearchOptions {
        // Every iteration must reach the server; the cache is measured
        // separately.
        cache_capacity: 8,
        ..SearchOptions::default()
    });
    search.warm().await;

    for provider in [Provider::DuckDuckGo, Provider::Bing, Provider::Brave] {
        let mut samples = Vec::with_capacity(ITERATIONS);
        for index in 0..(WARMUP + ITERATIONS) {
            let response = search
                .query(
                    SearchRequest::new(format!("rust browser engine {index}"))
                        .provider(provider.clone())
                        .limit(20),
                )
                .await
                .expect("query");
            assert!(!response.results.is_empty());
            if index >= WARMUP {
                samples.push(response.timing.engine_overhead);
            }
        }
        let (p50, p95) = report(
            &format!("S-search-fast[{provider}] engine_overhead"),
            samples,
        );
        assert!(
            p50 <= Duration::from_millis(5),
            "{provider}: p50 {p50:?} over the 5 ms budget (08 §6)"
        );
        assert!(
            p95 <= Duration::from_millis(15),
            "{provider}: p95 {p95:?} over the 15 ms budget (08 §6)"
        );
    }
}

#[tokio::test]
#[ignore = "budget scenario: run with --release --run-ignored all --no-capture"]
async fn s_search_fast_fan_out_engine_overhead() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("duckduckgo_html", "rust-browser-engine");
    harness.serve_fixture("bing", "rust-browser-engine");
    harness.serve_fixture("brave", "rust-browser-engine");
    let search = harness.search();
    search.warm().await;

    let mut samples = Vec::with_capacity(ITERATIONS);
    for index in 0..(WARMUP + ITERATIONS) {
        let response = search
            .query(
                SearchRequest::new(format!("rust browser engine {index}"))
                    .providers([Provider::DuckDuckGo, Provider::Bing, Provider::Brave])
                    .limit(50),
            )
            .await
            .expect("query");
        assert_eq!(response.providers.len(), 3);
        if index >= WARMUP {
            samples.push(response.timing.engine_overhead);
        }
    }
    let (p50, p95) = report("S-search-fast[fan-out x3] engine_overhead", samples);
    assert!(p50 <= Duration::from_millis(5), "p50 {p50:?}");
    assert!(p95 <= Duration::from_millis(15), "p95 {p95:?}");
}

#[test]
#[ignore = "budget scenario: run with --release --run-ignored all --no-capture"]
fn parsing_a_300_kb_serp_to_results() {
    // Brave's recorded SERP for "rust browser engine" is 296 KB, the largest
    // in the corpus and the closest to the budget's 300 KB.
    let plans = PlanSet::embedded().expect("plans");
    let plan = plans.get("brave").expect("plan");
    let body = fixture("brave", "rust-browser-engine");
    println!("fixture: {} bytes", body.len());
    let base = plan.request.endpoint.clone();

    let mut samples = Vec::with_capacity(ITERATIONS);
    for index in 0..(WARMUP + ITERATIONS) {
        let started = Instant::now();
        let document = parse(&body);
        let extraction = extract(&document, plan, &base, &Provider::Brave);
        let elapsed = started.elapsed();
        assert_eq!(extraction.results.len(), 20);
        if index >= WARMUP {
            samples.push(elapsed);
        }
    }
    let (p50, _) = report("parse 296 KB SERP to results", samples);
    assert!(
        p50 <= Duration::from_micros(2500),
        "p50 {p50:?} over the 2.5 ms budget (08 §6)"
    );
}

#[tokio::test]
#[ignore = "budget scenario: run with --release --run-ignored all --no-capture"]
async fn idle_memory_of_the_search_subsystem_after_warming() {
    let mut harness = Harness::start().await;
    harness.serve_fixture("duckduckgo_html", "rust-browser-engine");
    harness.serve_fixture("bing", "rust-browser-engine");
    harness.serve_fixture("brave", "rust-browser-engine");

    // The transport is built first and measured out: `08` §6 budgets the
    // search subsystem, and the client's TLS stack, resolver and io thread
    // are `03` §10's business.
    let client = harness.client();
    let before = resident_bytes();

    let search = harness.search_from(
        client,
        SearchOptions {
            providers_enabled: vec![Provider::DuckDuckGo, Provider::Bing, Provider::Brave],
            ..SearchOptions::default()
        },
    );
    let report = search.warm().await;
    assert!(report.is_complete(), "{report:?}");
    let warmed = resident_bytes();

    // One query per provider, then idle: plans loaded, connections open,
    // cache holding three answers.
    for provider in [Provider::DuckDuckGo, Provider::Bing, Provider::Brave] {
        search
            .query(SearchRequest::new("rust browser engine").provider(provider))
            .await
            .expect("query");
    }
    let after = resident_bytes();

    match (before, warmed, after) {
        (Some(before), Some(warmed), Some(after)) => println!(
            "search subsystem RSS: base {:.2} MB, +{:.2} MB after plans and warm pool,              +{:.2} MB after three queries (target <= 3 MB)",
            before as f64 / 1e6,
            warmed.saturating_sub(before) as f64 / 1e6,
            after.saturating_sub(before) as f64 / 1e6,
        ),
        _ => println!("search subsystem idle after warm: RSS unavailable on this platform"),
    }
    // A target (ⓣ), reported rather than gated (`docs/spec/08-search.md` §6).
}
