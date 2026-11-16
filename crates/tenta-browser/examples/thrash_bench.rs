//! `S-style-thrash`: 200 write-then-read iterations on a page of about 6,000
//! elements, the loop a script writes when it reads layout or style between
//! mutations. Prints one JSON line with the milliseconds per iteration.
//!
//!   cargo run --release -p tenta-browser --features v8,render --example thrash_bench
//!
//! Environment: `READ` is `color` (`getComputedStyle(e).color`), `width`
//! (`getComputedStyle(e).width`) or `offset` (`e.offsetWidth`); `ITERATIONS`
//! defaults to 200.

use std::time::Duration;

use tenta_browser::{
    ContextOptions, Engine, EvalOptions, Goto, JsEngine, PageOptions, Settle, Viewport,
};
use tenta_core::{FsPolicy, Tier};
use tenta_testutil::{FixtureServer, FixtureServerConfig};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let read = std::env::var("READ").unwrap_or_else(|_| "color".to_owned());
    let iterations: u32 = std::env::var("ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let temp = tenta_testutil::temp_dir("thrash-bench");
    let mut body = String::from("<div id=root>");
    for n in 0..1500 {
        body.push_str(&format!(
            "<div class=row><span>a{n}</span><b>b</b><i>c</i><p>d</p></div>"
        ));
    }
    body.push_str("</div>");
    temp.write(
        "index.html",
        format!("<!doctype html><html><head><style>.row{{margin:1px;padding:1px}}p{{width:30px}}</style></head><body>{body}</body></html>"),
    );
    let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
        .await
        .expect("server");
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
    let page = context
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
        .expect("page");
    page.goto(
        &server.url("/index.html"),
        Goto::default()
            .wait_until(Settle::Load)
            .timeout(Duration::from_secs(60)),
    )
    .await
    .expect("load");
    let reader = match read.as_str() {
        "width" => "getComputedStyle(el).width.length",
        "offset" => "el.offsetWidth",
        _ => "getComputedStyle(el).color.length",
    };
    let script = format!(
        "(function () {{ var all = document.querySelectorAll('*'); var el = all[all.length >> 1]; var sink = 0; \
         var warm = {reader}; var t0 = performance.now(); \
         for (var i = 0; i < {iterations}; i++) {{ el.style.color = 'rgb(' + (i % 250) + ',0,0)'; sink += {reader}; }} \
         return JSON.stringify({{ elements: all.length, read: '{read}', iterations: {iterations}, \
           ms_per_iteration: (performance.now() - t0) / {iterations}, sink: sink }}); }})()"
    );
    let out = page
        .evaluate_with(&script, EvalOptions::default())
        .await
        .expect("ran")
        .into_value::<String>()
        .expect("a string");
    println!("{out}");
    page.close().await.expect("close");
    context.close();
    engine.close();
}
