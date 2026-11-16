//! `window.location` reports the navigated URL to the page's own scripts and
//! to an evaluation, on both backends, through the real page loop; `atob` and
//! `btoa` are there with it.
#![cfg(any(feature = "v8", feature = "quickjs"))]

use std::time::Duration;

use tenta_browser::{ContextOptions, Engine, Goto, JsEngine, PageOptions, Settle};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig};

/// The inline script runs during the parse, before `load`.
const PAGE: &str = "<!doctype html><title>unset</title><body><script>\
    document.title = [location.pathname, location.search, location.hash, \
    atob(btoa(location.pathname))].join(' ');\
    </script></body>";

async fn location_reports_the_navigated_url(backend: JsEngine) {
    let fixtures = tenta_testutil::temp_dir("location-js");
    fixtures.write("dir/page.html", PAGE);
    let server = FixtureServer::start(FixtureServerConfig::new(fixtures.path()))
        .await
        .expect("fixture server");
    let engine = Engine::builder()
        .js(backend)
        .max_tier(Tier::Js)
        .io_threads(1)
        .page_threads(1)
        .ephemeral(true)
        .build()
        .expect("browser");
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            ..ContextOptions::default()
        })
        .expect("context");
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("page");
    let url = server.url("/dir/page.html?x=1&y=two#frag");
    let (_, seen) = page
        .goto_with_eval(
            &url,
            Goto::default()
                .wait_until(Settle::Load)
                .timeout(Duration::from_secs(10)),
            "[document.title, location.href, location.href === document.URL, \
              location.origin + location.pathname + location.search + location.hash, \
              location.protocol, location.hostname, location.host === location.hostname + ':' + \
              location.port, String(location) === location.href].join('|')",
        )
        .await
        .expect("navigation and evaluation");
    let origin = server.url("");
    let origin = origin.trim_end_matches('/');
    assert_eq!(
        seen,
        format!(
            "/dir/page.html ?x=1&y=two #frag /dir/page.html|{url}|true|{url}|http:|127.0.0.1|\
             true|true"
        ),
        "origin {origin}"
    );
    page.close().await.expect("close page");
    context.close();
    engine.close();
}

#[cfg(feature = "v8")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v8_location_reports_the_navigated_url() {
    location_reports_the_navigated_url(JsEngine::V8).await;
}

#[cfg(feature = "quickjs")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quickjs_location_reports_the_navigated_url() {
    location_reports_the_navigated_url(JsEngine::QuickJs).await;
}
