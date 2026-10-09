//! A structured-clone native workspace failure is an uncatchable page crash.

#![cfg(any(feature = "v8", feature = "quickjs"))]

use std::time::Duration;

use tenta_browser::{ContextOptions, Engine, Goto, JsEngine, PageOptions, Settle};
use tenta_core::{BudgetKind, Budgets, Error, Tier};
use tenta_testutil::{FixtureServer, FixtureServerConfig};

async fn fails_with_typed_page_memory_budget(js: JsEngine) {
    let fixtures = tenta_testutil::temp_dir("structured-clone-budget");
    fixtures.write("index.html", "<!doctype html><html><body></body></html>");
    let server = FixtureServer::start(FixtureServerConfig::new(fixtures.path()))
        .await
        .expect("fixture server");
    let engine = Engine::builder()
        .js(js)
        .max_tier(Tier::Js)
        .io_threads(1)
        .page_threads(1)
        .ephemeral(true)
        .build()
        .expect("browser with JavaScript");
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            budgets: Some(Budgets::desktop().page_memory_mb(1)),
            ..ContextOptions::default()
        })
        .expect("bounded context");
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("bounded page");
    let url = server.url("/index.html");
    let error = page
        .goto_with_eval(
            &url,
            Goto::default()
                .wait_until(Settle::Load)
                .timeout(Duration::from_secs(10)),
            "try { structuredClone('x'.repeat(70000)); 'resumed'; } catch (_) { 'caught'; }",
        )
        .await
        .expect_err("clone exceeds the page's structured-clone reservation");
    assert!(
        matches!(
            error,
            Error::BudgetExceeded {
                kind: BudgetKind::PageMemory,
                component: Some(ref component)
            } if component == "structured clone"
        ),
        "the triggering operation reports its typed first cause: {error:?}"
    );
    let later = page
        .goto(&url, Goto::default().wait_until(Settle::Load))
        .await;
    assert!(
        matches!(later, Err(Error::Crashed { .. })),
        "a second operation cannot run on the discarded page: {later:?}"
    );
    page.close().await.expect("close page");
    context.close();
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "quickjs")]
async fn quickjs_clone_memory_failure_is_typed_and_terminal() {
    fails_with_typed_page_memory_budget(JsEngine::QuickJs).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "v8")]
async fn v8_clone_memory_failure_is_typed_and_terminal() {
    fails_with_typed_page_memory_budget(JsEngine::V8).await;
}
