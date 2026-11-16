//! `document.currentScript` through the real page loop: the page names the
//! `<script>` element while its classic script runs, whichever way the script
//! reached the page (inline, parser-blocking, `defer`, `async`), and nothing
//! outside one.
#![cfg(feature = "v8")]

use std::time::Duration;

use tenta_browser::{ContextOptions, Engine, Goto, JsEngine, PageOptions, Settle};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig};

const PAGE: &str = r#"<!doctype html><title>scripts</title><body>
<script>window.seen = [];
function note(where) {
  var script = document.currentScript;
  seen.push(where + '=' + (script ? script.id || script.getAttribute('src') : 'null'));
}</script>
<script id="inline">note('inline'); setTimeout(function () { note('timer'); }, 0);</script>
<script id="blocking" src="/note.js?blocking"></script>
<script id="deferred" defer src="/note.js?deferred"></script>
<script id="async" async src="/note.js?async"></script>
<script id="last">note('last');</script>
</body>"#;

const NOTE: &str = "note(document.currentScript.getAttribute('src').split('?')[1]);";

#[tokio::test(flavor = "multi_thread")]
async fn current_script_names_the_element_whose_script_is_running() {
    let fixtures = tenta_testutil::temp_dir("current-script");
    fixtures.write("page.html", PAGE);
    fixtures.write("note.js", NOTE);
    let server = FixtureServer::start(FixtureServerConfig::new(fixtures.path()))
        .await
        .expect("fixture server");
    let engine = Engine::builder()
        .js(JsEngine::V8)
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
    let (_, seen) = page
        .goto_with_eval(
            &server.url("/page.html"),
            Goto::default()
                .wait_until(Settle::NetworkIdle)
                .timeout(Duration::from_secs(10)),
            "note('evaluation'); seen.slice().sort().join(' ')",
        )
        .await
        .expect("navigation and evaluation");
    assert_eq!(
        seen,
        "async=async blocking=blocking deferred=deferred evaluation=null inline=inline \
         last=last timer=null",
        "each script saw its own element; the timer and the host's evaluation saw none"
    );
    page.close().await.expect("close page");
    context.close();
    engine.close();
}
