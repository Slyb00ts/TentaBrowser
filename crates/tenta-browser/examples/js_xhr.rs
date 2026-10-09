//! One real in-process S-js-xhr sample, with correctness checks outside the
//! navigation timer. Build in release mode and interleave separate subjects.
//! The timer includes page runtime creation during goto, but excludes browser,
//! context, page and fixture-server construction. This does not measure the
//! last-mutation-to-settle gap or establish the complete P2 performance gate.

use std::time::{Duration, Instant};

use tenta_browser::{
    ContextOptions, Document, Engine, Goto, JsEngine, PageOptions, Settle, SettleState,
};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig};

fn text_of(document: &Document, selector: &str) -> String {
    let node = document
        .query_selector(selector)
        .expect("valid fixture selector")
        .expect("fixture element exists");
    document.text_of(node).trim().to_owned()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = FixtureServer::start(FixtureServerConfig::default()).await?;
    let started = Instant::now();
    let engine = Engine::builder()
        .js(JsEngine::V8)
        .max_tier(Tier::Js)
        .io_threads(1)
        .page_threads(1)
        .ephemeral(true)
        .build()?;
    let engine_init_ns = started.elapsed().as_nanos();
    let context = engine.new_context(ContextOptions {
        allow_private_network: true,
        ..ContextOptions::default()
    })?;
    let page = context.new_page(PageOptions::default()).await?;
    let url = server.url("/js-xhr/index.html");

    let started = Instant::now();
    let navigation = page
        .goto(
            &url,
            Goto::default()
                .wait_until(Settle::NetworkIdle)
                .timeout(Duration::from_secs(10)),
        )
        .await?;
    let navigation_ns = started.elapsed().as_nanos();

    assert_eq!(navigation.status, 200, "fixture navigation succeeded");
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });
    let document = page
        .take_document()
        .expect("navigation produced a document");
    // Error handlers also set the done marker: assert both successful responses.
    assert_eq!(text_of(&document, "#xhr"), "xhr: item=alpha&via=xhr");
    assert_eq!(text_of(&document, "#fetch"), "fetch: beta via fetch");
    assert_eq!(text_of(&document, "#status"), "js-xhr: done (2 requests)");
    assert_eq!(server.requests_served(), 3, "document, XHR and fetch");

    page.close().await?;
    context.close();
    engine.close();
    println!(
        "{}",
        serde_json::json!({
            "scenario": "S-js-xhr",
            "backend": "v8",
            "navigation_ns": navigation_ns,
            "engine_init_ns": engine_init_ns,
            "requests": server.requests_served(),
            "verified": true,
            "settle": "networkidle",
            "rtt_ms": 0,
        })
    );
    Ok(())
}
