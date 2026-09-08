//! The Tier 1 budget rows of `docs/spec/14-memory.md` §1 and the scenarios of
//! `docs/spec/15-testing-and-benchmarks.md` §2, measured in process.
//!
//! These are measurements, not behaviour, so they are `#[ignore]`d: a loaded
//! machine must not fail an ordinary test run. Run them in release and read
//! the printed numbers:
//!
//! ```text
//! cargo nextest run --release -p tenta-browser --run-ignored all -E 'binary(budgets)'
//! ```
//!
//! Set `BENCH_STRICT=1` to enforce exact budgets (useful for CI where noise is controlled);
//! without it, budgets are applied with a 2× tolerance.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tenta_browser::{ContextOptions, Engine, Goto, PageOptions, Settle};
use tenta_net::Request;
use tenta_net::hooks::Observer;

fn is_strict_mode() -> bool {
    std::env::var("BENCH_STRICT").ok().as_deref() == Some("1")
}

fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    let index = ((sorted.len() as f64 - 1.0) * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// `S-static-text`: the 1 MB article fixture, `--rtt 0`, `goto` to settled
/// text output. Budget: ≤ 15 ms, RSS delta ≤ 8 MB for the first page and
/// ≤ 3 MB for a marginal one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measurement, not behaviour"]
async fn s_static_text() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let url = server.url("/static-article/index.html");

    // One warm-up page: it pays for the connection, the atom table and every
    // other one-off cache, which is what "first page" in the budget means.
    let baseline = support::rss_bytes();
    let first_started = Instant::now();
    let first = context.fetch(&url).await.expect("document");
    let first_text = first.text();
    let first_elapsed = first_started.elapsed();
    let after_first = support::rss_bytes();

    let mut samples = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        let document = context.fetch(&url).await.expect("document");
        let text = document.text();
        samples.push(started.elapsed());
        assert_eq!(text.len(), first_text.len());
    }
    samples.sort_unstable();

    // A marginal page: measured while the first one is still alive.
    let before_marginal = support::rss_bytes();
    let marginal = context.fetch(&url).await.expect("document");
    let _ = marginal.text();
    let after_marginal = support::rss_bytes();

    let p50 = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    let rss_delta_first = baseline
        .zip(after_first)
        .map(|(b, a)| a.saturating_sub(b))
        .unwrap_or(0);
    let rss_delta_marginal = before_marginal
        .zip(after_marginal)
        .map(|(b, a)| a.saturating_sub(b))
        .unwrap_or(0);

    println!(
        "S-static-text: first {:.2} ms, p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms \
         (budget 15 ms), text {} bytes, arena {:.2} MB",
        first_elapsed.as_secs_f64() * 1000.0,
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0,
        samples.last().expect("samples").as_secs_f64() * 1000.0,
        first_text.len(),
        first.memory_report().total() as f64 / 1e6,
    );
    support::report_rss(
        "S-static-text first page (budget 8 MB)",
        baseline,
        after_first,
    );
    support::report_rss(
        "S-static-text marginal page (budget 3 MB)",
        before_marginal,
        after_marginal,
    );

    // Assert budgets
    let budget_ms = 15.0;
    let p50_ms = p50.as_secs_f64() * 1000.0;
    let p95_ms = p95.as_secs_f64() * 1000.0;
    let rss_budget_first = 8 * 1024 * 1024;
    let rss_budget_marginal = 3 * 1024 * 1024;

    if is_strict_mode() {
        assert!(
            p50_ms <= budget_ms,
            "S-static-text p50 {:.2} ms exceeds budget {:.2} ms",
            p50_ms,
            budget_ms
        );
        assert!(
            p95_ms <= budget_ms * 3.0,
            "S-static-text p95 {:.2} ms exceeds 3× budget",
            p95_ms
        );
        assert!(
            rss_delta_first <= rss_budget_first,
            "S-static-text first page RSS delta {} B exceeds budget {} B",
            rss_delta_first,
            rss_budget_first
        );
        assert!(
            rss_delta_marginal <= rss_budget_marginal,
            "S-static-text marginal page RSS delta {} B exceeds budget {} B",
            rss_delta_marginal,
            rss_budget_marginal
        );
    } else {
        // 2× CI tolerance
        assert!(
            p50_ms <= budget_ms * 2.0,
            "S-static-text p50 {:.2} ms exceeds 2× budget {:.2} ms",
            p50_ms,
            budget_ms * 2.0
        );
        assert!(
            p95_ms <= budget_ms * 6.0,
            "S-static-text p95 {:.2} ms exceeds 6× budget",
            p95_ms
        );
        assert!(
            rss_delta_first <= rss_budget_first * 2,
            "S-static-text first page RSS delta {} B exceeds 2× budget {} B",
            rss_delta_first,
            rss_budget_first * 2
        );
        assert!(
            rss_delta_marginal <= rss_budget_marginal * 2,
            "S-static-text marginal page RSS delta {} B exceeds 2× budget {} B",
            rss_delta_marginal,
            rss_budget_marginal * 2
        );
    }

    drop(first);
    drop(marginal);
    engine.close();
}

#[derive(Default)]
struct FirstRequestClock {
    nanos: AtomicU64,
    origin: Option<Instant>,
}

impl Observer for FirstRequestClock {
    fn on_request(&self, _request: &Request) {
        if let Some(origin) = self.origin {
            let _ = self.nanos.compare_exchange(
                0,
                origin.elapsed().as_nanos() as u64,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
    }
}

/// The in-process part of `S-cold-fetch`: engine construction, context
/// construction and the first request reaching the wire. The CLI adds process
/// start and argument parsing on top; the whole scenario's budget is 8 ms.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measurement, not behaviour"]
async fn s_cold_fetch_engine_start() {
    let server = support::server().await;
    let url = server.url("/tiny/index.html");

    let origin = Instant::now();
    let engine = Engine::builder()
        .io_threads(1)
        .page_threads(2)
        .build()
        .expect("engine");
    let built = origin.elapsed();
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            ..ContextOptions::default()
        })
        .expect("context");
    let context_ready = origin.elapsed();

    let clock = Arc::new(FirstRequestClock {
        nanos: AtomicU64::new(0),
        origin: Some(origin),
    });
    context
        .observe(Arc::clone(&clock) as Arc<dyn Observer>)
        .expect("observer");

    let page = context.new_page(PageOptions::tier1()).await.expect("page");
    let page_ready = origin.elapsed();
    page.goto(&url, Goto::default().wait_until(Settle::Load))
        .await
        .expect("navigation");
    let settled = origin.elapsed();
    let first_request = Duration::from_nanos(clock.nanos.load(Ordering::Relaxed));

    // A second context in the same process, to separate one-off process cost
    // (the TLS root bundle is parsed once) from per-context cost.
    let second_started = Instant::now();
    let _second = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            ..ContextOptions::default()
        })
        .expect("context");
    let second_context = second_started.elapsed();

    let first_request_ms = first_request.as_secs_f64() * 1000.0;
    let settled_ms = settled.as_secs_f64() * 1000.0;

    println!(
        "S-cold-fetch (in process): engine {:.2} ms, +context {:.2} ms, +page {:.2} ms, \
         first request on the wire {:.2} ms (budget 8 ms), settled {:.2} ms",
        built.as_secs_f64() * 1000.0,
        context_ready.as_secs_f64() * 1000.0,
        page_ready.as_secs_f64() * 1000.0,
        first_request_ms,
        settled_ms,
    );
    println!(
        "S-cold-fetch: a second context in the same process costs {:.2} ms",
        second_context.as_secs_f64() * 1000.0
    );
    assert!(first_request > Duration::ZERO, "the observer saw a request");

    // Assert budget for first request (in-process part of S-cold-fetch)
    let budget_ms = 8.0;
    if is_strict_mode() {
        assert!(
            first_request_ms <= budget_ms,
            "S-cold-fetch first request {:.2} ms exceeds budget {:.2} ms",
            first_request_ms,
            budget_ms
        );
    } else {
        // 2× CI tolerance
        assert!(
            first_request_ms <= budget_ms * 2.0,
            "S-cold-fetch first request {:.2} ms exceeds 2× budget {:.2} ms",
            first_request_ms,
            budget_ms * 2.0
        );
    }

    page.close().await.expect("close");
    engine.close();
}
