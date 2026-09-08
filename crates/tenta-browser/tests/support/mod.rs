//! Shared helpers for the fixture-server tests
//! (`docs/spec/15-testing-and-benchmarks.md` §1).

#![allow(dead_code)]

use std::path::PathBuf;

use tenta_browser::{BrowserContext, ContextOptions, Engine};
use tenta_testutil::server::{FixtureServer, FixtureServerConfig};

/// A fixture server on loopback, serving `tests/fixtures/`.
pub async fn server() -> FixtureServer {
    FixtureServer::start(FixtureServerConfig::default())
        .await
        .expect("fixture server")
}

/// An engine with a small, predictable pool.
pub fn engine(page_threads: usize) -> Engine {
    Engine::builder()
        .io_threads(1)
        .page_threads(page_threads)
        .build()
        .expect("engine")
}

/// A context that may reach loopback, which is what a fixture test needs
/// (`docs/spec/19-security.md` §2).
pub fn context(engine: &Engine) -> BrowserContext {
    engine
        .new_context(ContextOptions {
            allow_private_network: true,
            ..ContextOptions::default()
        })
        .expect("context")
}

/// Path of a fixture directory.
pub fn fixture_dir(name: &str) -> PathBuf {
    tenta_testutil::fixtures_root()
        .join(name)
        .canonicalize()
        .expect("fixture directory")
}

/// Resident set size of this process, in bytes.
///
/// `/proc/self/statm` on Linux, `ps` elsewhere; `None` when neither answers.
/// Used by the reported (not gated) memory numbers of
/// `docs/spec/14-memory.md` §1.
pub fn rss_bytes() -> Option<u64> {
    if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
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

/// Prints an RSS delta in the format the budget rows use.
pub fn report_rss(label: &str, before: Option<u64>, after: Option<u64>) {
    match (before, after) {
        (Some(before), Some(after)) => {
            let delta = after.saturating_sub(before);
            println!(
                "{label}: RSS {:.2} MB -> {:.2} MB (delta {:.2} MB)",
                before as f64 / 1e6,
                after as f64 / 1e6,
                delta as f64 / 1e6
            );
        }
        _ => println!("{label}: RSS unavailable on this platform"),
    }
}

/// Waits until the page has issued a request whose path ends with `path`,
/// which is when a navigation is provably in flight. Panics if the event
/// stream ends first.
pub async fn wait_for_request(events: &mut tenta_browser::PageEvents, path: &str) {
    loop {
        match events.next().await {
            Some(tenta_browser::PageEvent::Request { url, .. }) if url.path().ends_with(path) => {
                return;
            }
            Some(_) => {}
            None => panic!("the page's event stream closed before a request to {path}"),
        }
    }
}

/// Waits until the response head for `path` has been reported, so the
/// navigation is provably past the network head. Panics if the event stream
/// ends first.
pub async fn wait_for_response(events: &mut tenta_browser::PageEvents, path: &str) {
    loop {
        match events.next().await {
            Some(tenta_browser::PageEvent::Response { url, .. }) if url.path().ends_with(path) => {
                return;
            }
            Some(_) => {}
            None => panic!("the page's event stream closed before a response from {path}"),
        }
    }
}

/// Waits until a TierChanged event is emitted, indicating the DOM tier is
/// being entered. Panics if the event stream ends first.
pub async fn wait_for_tier_change(events: &mut tenta_browser::PageEvents) {
    loop {
        match events.next().await {
            Some(tenta_browser::PageEvent::TierChanged { .. }) => {
                return;
            }
            Some(_) => {}
            None => panic!("the page's event stream closed before a TierChanged event"),
        }
    }
}
