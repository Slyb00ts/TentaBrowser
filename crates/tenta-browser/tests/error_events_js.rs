//! The `error` event of a real page: an inline script, a timer and a listener
//! each throw, and the page's own `window.onerror` hears all three.
//!
//! `crates/tenta-js-v8/tests/dom_error_event.rs` covers the event and the
//! handler against an engine; this is the page loop's half, where the three
//! exceptions come from three different engine entries.
//!
//! The source and the line are asserted only by their kind. A script is
//! compiled without an origin, so the source is the empty string and the line
//! is counted from the start of the script rather than of the document.

#![cfg(feature = "v8")]

use std::time::Duration;

use tenta_browser::{ContextOptions, Engine, Goto, JsEngine, PageOptions, Settle};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig};

const PAGE: &str = r#"<!doctype html><html><body>
<script>
window.heard = [];
window.onerror = function (message, source, lineno, colno, error) {
    heard.push(message + ' ' + (typeof source) + ' ' + (lineno > 0) + ' ' +
               (error instanceof Error));
    return true;
};
</script>
<script>
throw new TypeError('inline');
</script>
<script>
setTimeout(function () { throw new Error('timer'); }, 0);
document.addEventListener('DOMContentLoaded', function () { throw new Error('listener'); });
window.after = 'the script after the one that threw still ran';
</script>
</body></html>"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_hears_its_uncaught_exceptions_through_onerror() {
    let fixtures = tenta_testutil::temp_dir("error-events");
    fixtures.write("index.html", PAGE);
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
        .expect("engine");
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
    let (_, completion) = page
        .goto_with_eval(
            &server.url("/index.html"),
            Goto::default()
                .wait_until(Settle::NetworkIdle)
                .timeout(Duration::from_secs(10)),
            "heard.slice().sort().join('|') + '|' + after",
        )
        .await
        .expect("navigation and evaluation");
    assert_eq!(
        completion,
        "Uncaught Error: listener string true true|\
         Uncaught Error: timer string true true|\
         Uncaught TypeError: inline string true true|\
         the script after the one that threw still ran"
    );
    page.close().await.expect("close page");
    context.close();
    engine.close();
}
