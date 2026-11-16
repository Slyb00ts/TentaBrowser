//! One timed load of a page with N linked stylesheets, then one screenshot.
//!
//! `S-css-load`: the page links `SHEETS` stylesheets of about 8 KiB each from a
//! fixture server that answers after `RTT_MS`, and has one script after them. The
//! example prints one JSON line (milliseconds for the load, the screenshot and
//! both together) and exits, so a driver can interleave runs of two builds and
//! read each process's peak RSS from `/usr/bin/time -l`.
//!
//!   cargo run --release -p tenta-browser --features v8,render --example css_load_bench
//!
//! Environment: `SHEETS` (default 10), `RTT_MS` (default 20), `SCRIPT` (1 puts a
//! script after the links, 0 none).

use std::time::{Duration, Instant};

use tenta_browser::{
    ContextOptions, Engine, Goto, JsEngine, PageOptions, ScreenshotOptions, Settle, Viewport,
};
use tenta_core::{FsPolicy, Tier};
use tenta_testutil::{FixtureServer, FixtureServerConfig};

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let sheets = env_number("SHEETS", 10);
    let rtt = Duration::from_millis(env_number("RTT_MS", 20));
    let script = env_number("SCRIPT", 1) != 0;
    let temp = tenta_testutil::temp_dir("css-load-bench");
    let mut links = String::new();
    for n in 0..sheets {
        let mut css = format!(".c{n}{{color:rgb({},{},{})}}", n % 255, 10, 20);
        for rule in 0..120 {
            css.push_str(&format!(
                ".r{n}x{rule}{{margin:{rule}px;padding:1px 2px}}\n"
            ));
        }
        temp.write(&format!("s{n}.css"), css);
        links.push_str(&format!("<link rel=stylesheet href=s{n}.css>"));
    }
    let tail = if script {
        "<script>window.ready = document.styleSheets.length;</script>"
    } else {
        ""
    };
    temp.write(
        "index.html",
        format!(
            "<!doctype html><html><head>{links}{tail}</head><body><div class=c0 style='height:50px'>x</div></body></html>"
        ),
    );
    let server = FixtureServer::start(FixtureServerConfig::new(temp.path()).rtt(rtt))
        .await
        .expect("fixture server");
    let engine = Engine::builder()
        .max_tier(Tier::Render)
        .io_threads(1)
        .page_threads(1)
        .fs_policy(FsPolicy::none().allow_read(temp.path()))
        .js(JsEngine::V8)
        .ephemeral(true)
        .build()
        .expect("engine");
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            ..ContextOptions::default()
        })
        .expect("context");
    let new_page = || async {
        context
            .new_page(PageOptions {
                javascript: true,
                viewport: Viewport {
                    width: 800,
                    height: 600,
                    device_scale_factor: 1.0,
                    mobile: false,
                },
                ..PageOptions::default()
            })
            .await
            .expect("page")
    };
    // A first page warms the engine, the connection pool and the fonts.
    let warm = new_page().await;
    warm.goto(
        &server.url("/index.html"),
        Goto::default().wait_until(Settle::Load),
    )
    .await
    .expect("warm-up");
    warm.screenshot(ScreenshotOptions::default())
        .await
        .expect("warm-up capture");
    warm.close().await.expect("close");

    let page = new_page().await;
    let started = Instant::now();
    page.goto(
        &server.url("/index.html"),
        Goto::default().wait_until(Settle::Load),
    )
    .await
    .expect("load");
    let loaded = started.elapsed();
    page.screenshot(ScreenshotOptions::default())
        .await
        .expect("capture");
    let total = started.elapsed();
    println!(
        "{{\"sheets\":{sheets},\"rtt_ms\":{},\"load_ms\":{:.2},\"screenshot_ms\":{:.2},\"total_ms\":{:.2}}}",
        rtt.as_millis(),
        loaded.as_secs_f64() * 1000.0,
        (total - loaded).as_secs_f64() * 1000.0,
        total.as_secs_f64() * 1000.0
    );
    page.close().await.expect("close");
    context.close();
    engine.close();
}
