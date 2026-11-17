//! Bodies over a real socket and on both engines.
//!
//! Read: `XMLHttpRequest`'s `json`, `arraybuffer` and `blob` response types,
//! and `Response.arrayBuffer()`, `blob()` and `clone()` on what `fetch()`
//! resolves with. Sent: a `FormData`, a `URLSearchParams` and a `Blob` as the
//! body of a `fetch()` and of a `send()`, checked against what the server says
//! it received.
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

const SENDING_PAGE: &str = r#"<!doctype html><html><body><script>
window.out = {};
function summary(reply) {
    var type = reply.headers['content-type'];
    var boundary = type.indexOf('boundary=') < 0 ? null : type.split('boundary=')[1];
    return [boundary ? type.split(';')[0] : type,
            boundary ? reply.body.split('--' + boundary).length - 1 : reply.body].join(',');
}
var form = new FormData();
form.append('name', 'value');
form.append('file', new File(['abc'], 'a.txt', {type: 'text/plain'}));
fetch('/api/echo', {method: 'POST', body: form})
    .then(function (response) { return response.json(); })
    .then(function (reply) {
        out.form = [summary(reply), reply.body.indexOf('filename="a.txt"') > 0,
                    reply.body.indexOf('\r\n\r\nabc\r\n') > 0].join(',');
    });
fetch('/api/echo', {method: 'POST', body: new URLSearchParams({q: 'a b'})})
    .then(function (response) { return response.json(); })
    .then(function (reply) { out.params = summary(reply); });
var xhr = new XMLHttpRequest();
xhr.open('POST', '/api/echo');
xhr.responseType = 'json';
xhr.onload = function () { out.blob = summary(xhr.response); };
xhr.send(new Blob(['raw'], {type: 'application/x-test'}));
</script></body></html>"#;

async fn read_bodies(js: JsEngine) -> String {
    evaluate_on(
        js,
        PAGE,
        "[out.json, out.buffer, out.blob, out.fetch].join('|')",
    )
    .await
}

async fn send_bodies(js: JsEngine) -> String {
    evaluate_on(
        js,
        SENDING_PAGE,
        "[out.form, out.params, out.blob].join('|')",
    )
    .await
}

/// Navigates a new page to `markup`, waits for the network, and evaluates
/// `expression` in it.
async fn evaluate_on(js: JsEngine, markup: &str, expression: &str) -> String {
    let fixtures = tenta_testutil::temp_dir("body-reads");
    fixtures.write("index.html", markup);
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
            expression,
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

/// The multipart body has three boundary lines for two entries (two openers and
/// the closer), carries the file's name and its bytes; the other two arrive as
/// the bytes and the type their arm gives them.
const SENT: &str = "multipart/form-data,3,true,true|\
                    application/x-www-form-urlencoded;charset=UTF-8,q=a+b|\
                    application/x-test,raw";

#[cfg(feature = "v8")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v8_sends_a_form_params_and_a_blob_as_a_body() {
    assert_eq!(send_bodies(JsEngine::V8).await, SENT);
}

#[cfg(feature = "quickjs")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quickjs_sends_a_form_params_and_a_blob_as_a_body() {
    assert_eq!(send_bodies(JsEngine::QuickJs).await, SENT);
}
