//! Real QuickJS navigation through the browser, shared page loop and network.
#![cfg(feature = "quickjs")]

use std::time::Duration;
use tenta_browser::{ContextOptions, Engine, Goto, JsEngine, PageOptions, Settle, SettleState};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quickjs_navigation_settles_fetch_xhr_timers_and_native_dom_changes() {
    let fixtures = tenta_testutil::temp_dir("quickjs-page");
    fixtures.write(
        "index.html",
        format!(
            "{}<script>setTimeout(function(){{document.body.setAttribute('data-timer','fired');Promise.resolve().then(function(){{document.body.setAttribute('data-microtask','done')}})}},2)</script>",
            include_str!("../../../tests/fixtures/js-xhr/index.html")
        ),
    );
    let server = FixtureServer::start(FixtureServerConfig::new(fixtures.path()))
        .await
        .expect("fixture server");
    let engine = Engine::builder()
        .js(JsEngine::QuickJs)
        .max_tier(Tier::Js)
        .io_threads(1)
        .page_threads(1)
        .ephemeral(true)
        .build()
        .expect("QuickJS browser");
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
    let (navigation, completion) = page.goto_with_eval(
        &server.url("/index.html"),
        Goto::default().wait_until(Settle::NetworkIdle).timeout(Duration::from_secs(10)),
        "[document.getElementById('xhr').textContent,document.getElementById('fetch').textContent,document.getElementById('status').textContent,document.body.getAttribute('data-timer'),document.body.getAttribute('data-microtask')].join('|')",
    ).await.expect("real QuickJS navigation and evaluation");
    assert_eq!(navigation.status, 200);
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });
    assert_eq!(
        completion,
        "xhr: item=alpha&via=xhr|fetch: beta via fetch|js-xhr: done (2 requests)|fired|done"
    );
    assert_eq!(server.requests_served(), 3);
    page.close().await.expect("close page");
    context.close();
    engine.close();
}
