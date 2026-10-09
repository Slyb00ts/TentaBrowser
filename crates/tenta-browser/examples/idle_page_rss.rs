//! Resident memory of idle pages that keep their engine after `goto`.
//!
//! `idle_page_rss <v8|quickjs|none> [pages]` opens `pages` (default 16) pages on
//! one page thread, each loading a small page with an inline script, and prints
//! the process RSS before, with the pages idle, and after they close. `none`
//! loads the same page with JavaScript off, which is the DOM-only baseline an
//! engine's cost is read against. Run it in release mode, one engine per
//! process: the numbers are a process-level delta, not an allocator profile.

use std::process::Command;
use std::time::Duration;

use tenta_browser::{ContextOptions, Engine, Goto, JsEngine, PageOptions, Settle};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig};

const PAGE: &str = "<!doctype html><html><head><title>idle</title></head><body>\
    <div id=\"m\">before</div>\
    <script>document.getElementById('m').textContent = 'after';</script>\
    </body></html>";

fn rss_bytes() -> u64 {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs");
    let kilobytes: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("ps prints the resident set size in KiB");
    kilobytes * 1024
}

fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let kind = args.next().unwrap_or_else(|| "v8".to_owned());
    let pages: usize = args.next().map_or(16, |count| {
        count.parse().expect("the page count is a number")
    });
    let (js, javascript) = match kind.as_str() {
        "v8" => (JsEngine::V8, true),
        "quickjs" => (JsEngine::QuickJs, true),
        "none" => (JsEngine::Auto, false),
        other => return Err(format!("unknown engine {other:?}").into()),
    };

    let fixtures = tenta_testutil::temp_dir("idle-page-rss");
    fixtures.write("index.html", PAGE);
    let server = FixtureServer::start(FixtureServerConfig::new(fixtures.path())).await?;
    let engine = Engine::builder()
        .js(js)
        .max_tier(if javascript { Tier::Js } else { Tier::Dom })
        .io_threads(1)
        .page_threads(1)
        .ephemeral(true)
        .build()?;
    let context = engine.new_context(ContextOptions {
        allow_private_network: true,
        ..ContextOptions::default()
    })?;
    let options = || PageOptions {
        javascript,
        ..PageOptions::default()
    };
    let url = server.url("/index.html");
    let goto = || {
        Goto::default()
            .wait_until(Settle::Load)
            .timeout(Duration::from_secs(10))
    };

    // One page opened and closed first, so the process-wide costs (the engine
    // platform, the first isolate's snapshot, the page thread) are not charged
    // to the pages being measured.
    let warm = context.new_page(options()).await?;
    warm.goto(&url, goto()).await?;
    warm.close().await?;
    let before = rss_bytes();

    let mut open = Vec::with_capacity(pages);
    for _ in 0..pages {
        let page = context.new_page(options()).await?;
        page.goto(&url, goto()).await?;
        open.push(page);
    }
    let idle = rss_bytes();

    for page in &open {
        page.close().await?;
    }
    drop(open);
    let closed = rss_bytes();

    println!(
        "{kind}: {pages} idle pages  RSS {:.1} MiB -> {:.1} MiB  delta {:.2} MiB/page  after close {:.1} MiB",
        megabytes(before),
        megabytes(idle),
        megabytes(idle.saturating_sub(before)) / pages as f64,
        megabytes(closed),
    );
    context.close();
    engine.close();
    Ok(())
}
