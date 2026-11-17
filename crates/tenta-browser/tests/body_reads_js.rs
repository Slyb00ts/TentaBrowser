//! The body reads that are not text, over a real socket and on both engines:
//! `XMLHttpRequest`'s `json`, `arraybuffer` and `blob` response types, and
//! `Response.arrayBuffer()`, `blob()` and `clone()` on what `fetch()` resolves
//! with.
//!
//! `crates/tenta-js-v8/tests/dom_xhr.rs` and `dom_response.rs` cover each member
//! against a recording driver and a constructed response; QuickJS has no such
//! files for `XMLHttpRequest`, so this is where its half is exercised.

#![cfg(any(feature = "v8", feature = "quickjs"))]

use std::time::Duration;

use tenta_browser::{ContextOptions, Engine, Goto, JsEngine, PageOptions, Settle};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig};

const PAGE: &str = r#"<!doctype html><html><body><script>
window.out = {};
function request(type, done) {
    var xhr = new XMLHttpRequest();
    xhr.open('GET', '/api/echo?item=alpha');
    xhr.responseType = type;
    xhr.onload = function () { done(xhr); };
    xhr.send();
}
request('json', function (xhr) {
    out.json = [xhr.response.query, xhr.response === xhr.response,
                xhr.responseURL.split('/').slice(3).join('/')].join(',');
});
request('arraybuffer', function (xhr) {
    var text = String.fromCharCode.apply(null, new Uint8Array(xhr.response));
    out.buffer = [xhr.response instanceof ArrayBuffer, JSON.parse(text).query].join(',');
});
request('blob', function (xhr) {
    out.blob = [xhr.response instanceof Blob, xhr.response.size > 0, xhr.response.type].join(',');
});
fetch('/api/echo?item=beta').then(function (response) {
    var copy = response.clone();
    return Promise.all([response.arrayBuffer(), copy.blob(), copy.bodyUsed]);
}).then(function (read) {
    out.fetch = [read[0].byteLength === read[1].size, read[1].type, read[2]].join(',');
});
</script></body></html>"#;

async fn read_bodies(js: JsEngine) -> String {
    let fixtures = tenta_testutil::temp_dir("body-reads");
    fixtures.write("index.html", PAGE);
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
            "[out.json, out.buffer, out.blob, out.fetch].join('|')",
        )
        .await
        .expect("navigation and evaluation");
    page.close().await.expect("close page");
    context.close();
    engine.close();
    completion
}

const EXPECTED: &str = "item=alpha,true,api/echo?item=alpha|true,item=alpha|\
                        true,true,application/json|true,application/json,true";

#[cfg(feature = "v8")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v8_reads_bodies_as_json_buffers_and_blobs() {
    assert_eq!(read_bodies(JsEngine::V8).await, EXPECTED);
}

#[cfg(feature = "quickjs")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quickjs_reads_bodies_as_json_buffers_and_blobs() {
    assert_eq!(read_bodies(JsEngine::QuickJs).await, EXPECTED);
}
