//! Requests a page makes to another origin, end to end: V8 runs the script,
//! the `tenta-net` driver runs the CORS protocol over loopback against two
//! fixture servers, and the page loop settles the promise. The two servers are
//! two origins because they are two ports; `/api/cors` is an endpoint whose
//! CORS policy is its query string.
//!
//! Every navigation waits for `Settle::NetworkIdle`, for the reason
//! `fetch_js.rs` gives.

#![cfg(feature = "v8")]

mod support;

use std::time::{Duration, Instant};

use tenta_browser::{Document, Engine, Goto, PageOptions, Settle, SettleConfig};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

fn js_engine() -> Engine {
    Engine::builder()
        .io_threads(1)
        .page_threads(1)
        .max_tier(Tier::Js)
        .settle(SettleConfig::library())
        .build()
        .expect("engine")
}

async fn serve(files: &[(&str, &str)]) -> (FixtureServer, TempDir) {
    let temp = tenta_testutil::temp_dir("cors-js");
    for (relative, contents) in files {
        temp.write(relative, contents);
    }
    let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
        .await
        .expect("fixture server");
    (server, temp)
}

fn text_of(document: &Document, selector: &str) -> String {
    let node = document
        .query_selector(selector)
        .expect("selector parses")
        .unwrap_or_else(|| panic!("{selector} is in the document"));
    document.text_of(node).trim().to_owned()
}

/// Two origins and what the page's script left behind.
struct Run {
    /// What the script's `main()` returned, or the error it threw.
    result: String,
    /// Requests the page's own origin served, the document included.
    page_requests: u64,
    /// Requests the other origin served.
    api_requests: u64,
    elapsed: Duration,
}

/// Loads a page whose script is `async function main(api) { <body> }` and
/// reports the string it resolved to. `api` is the other origin, with no
/// trailing slash.
async fn run(body: &str) -> Run {
    let (api, _api_temp) = serve(&[("plain.json", "{\"secret\":\"the other origin's\"}")]).await;
    let api_origin = api.url("/");
    let api_origin = api_origin.trim_end_matches('/');
    let html = format!(
        "<!doctype html><html><head></head><body><div id=\"result\">unset</div>\
         <script>window.onerror = function (message) {{\
           document.getElementById('result').textContent = 'script error: ' + message; }};</script>\
         <script>\
         async function main(api) {{ {body} }}\
         main('{api_origin}').then(\
           function (value) {{ document.getElementById('result').textContent = String(value); }},\
           function (error) {{ document.getElementById('result').textContent = 'threw ' + error; }});\
         </script></body></html>"
    );
    let (server, _temp) = serve(&[("index.html", &html)]).await;
    let engine = js_engine();
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");
    let started = Instant::now();
    tokio::time::timeout(
        Duration::from_secs(20),
        page.goto(
            &server.url("/index.html"),
            Goto::default().wait_until(Settle::NetworkIdle),
        ),
    )
    .await
    .expect("the navigation settled instead of hanging")
    .expect("navigation");
    let elapsed = started.elapsed();
    let document = page.take_document().await.expect("document");
    let run = Run {
        result: text_of(&document, "#result"),
        page_requests: server.requests_served(),
        api_requests: api.requests_served(),
        elapsed,
    };
    assert!(
        !document.text().contains("the other origin's") || run.result.contains("shared"),
        "a body the other origin did not share is nowhere in the document"
    );
    engine.close();
    run
}

/// The server names the page's origin, so the response is the page's to read.
/// The request says where it came from and carries no cookie; of the response
/// headers, script sees the safelisted ones and the ones the server exposes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_response_that_names_the_origin_is_shared() {
    let run = run("var allow = encodeURIComponent(location.origin);\
         var r = await fetch(api + '/api/cors?origin=' + allow + '&expose=x-request-id');\
         var data = await r.json();\
         var hidden = await fetch(api + '/api/cors?origin=*');\
         return [r.status, data.method, data.origin === location.origin, data.cookie,\
                 r.headers.get('content-type'), r.headers.get('x-request-id'),\
                 hidden.headers.get('x-request-id'), hidden.ok].join('|');")
    .await;
    assert_eq!(run.result, "200|GET|true||application/json|cors-1||true");
    assert_eq!(run.api_requests, 2, "two simple requests, no preflight");
}

/// Without the server's consent the request is still made, as a browser makes
/// it, and its answer is discarded: script learns only that the fetch failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_response_without_consent_is_a_network_error() {
    let run = run("var out = [];\
         for (var url of [api + '/plain.json', api + '/api/cors?origin=http://elsewhere.test']) {\
           try { var r = await fetch(url); out.push('shared ' + await r.text()); }\
           catch (e) { out.push(e.constructor.name + ':' + e.message); }\
         }\
         return out.join('|');")
    .await;
    assert_eq!(
        run.result,
        "TypeError:Failed to fetch|TypeError:Failed to fetch"
    );
    assert_eq!(
        run.api_requests, 2,
        "both requests reached the other origin"
    );
}

/// A method and headers a form could not have sent are asked about first. The
/// real request follows only when the preflight allows them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_that_is_not_simple_is_preflighted() {
    let allowed = run(
        "var allow = encodeURIComponent(location.origin);\
         var r = await fetch(api + '/api/cors?origin=' + allow + '&methods=PUT&headers=content-type,x-token', {\
           method: 'PUT', headers: {'Content-Type': 'application/json', 'X-Token': 't-1'}, body: '{}'});\
         var data = await r.json();\
         return [data.method, data.token, data.contentType].join('|');",
    )
    .await;
    assert_eq!(allowed.result, "PUT|t-1|application/json");
    assert_eq!(allowed.api_requests, 2, "the preflight, then the request");

    let refused = run("var allow = encodeURIComponent(location.origin);\
         try {\
           await fetch(api + '/api/cors?origin=' + allow + '&methods=PUT', {\
             method: 'PUT', headers: {'X-Token': 't-1'}, body: '{}'});\
           return 'sent';\
         } catch (e) { return e.constructor.name; }")
    .await;
    assert_eq!(refused.result, "TypeError");
    assert_eq!(
        refused.api_requests, 1,
        "only the preflight: the request it refused was never sent"
    );
}

/// Cookies stay home unless the page asks for them to travel, and then the
/// server has to allow that by name: the wildcard is not enough.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credentials_travel_only_when_asked_for_and_allowed() {
    let run = run(
        "await fetch('/api/cookie?set=' + encodeURIComponent('sid=1; Path=/'));\
         var allow = encodeURIComponent(location.origin);\
         var named = api + '/api/cors?origin=' + allow;\
         var out = [];\
         out.push((await (await fetch(named)).json()).cookie);\
         out.push((await (await fetch(named + '&credentials=1', {credentials: 'include'})).json()).cookie);\
         out.push((await (await fetch(named + '&credentials=1', {credentials: 'omit'})).json()).cookie);\
         for (var url of [named, api + '/api/cors?origin=*&credentials=1']) {\
           try { await fetch(url, {credentials: 'include'}); out.push('shared'); }\
           catch (e) { out.push(e.constructor.name); }\
         }\
         out.push((await (await fetch('/api/cookie')).json()).cookie);\
         return out.join('|');",
    )
    .await;
    assert_eq!(run.result, "|sid=1||TypeError|TypeError|sid=1");
}

/// `XMLHttpRequest` goes through the same protocol, with `withCredentials` as
/// its one switch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_xhr_to_another_origin_follows_the_same_rules() {
    let run = run(
        "await fetch('/api/cookie?set=' + encodeURIComponent('sid=1; Path=/'));\
         function xhr(url, withCredentials) {\
           return new Promise(function (resolve) {\
             var x = new XMLHttpRequest();\
             x.open('GET', url);\
             x.withCredentials = withCredentials;\
             x.onload = function () { resolve('load ' + JSON.parse(x.responseText).cookie + ' ' + x.getResponseHeader('x-request-id')); };\
             x.onerror = function () { resolve('error ' + x.status); };\
             x.send();\
           });\
         }\
         var allow = encodeURIComponent(location.origin);\
         var named = api + '/api/cors?origin=' + allow;\
         return [await xhr(named, false), await xhr(named + '&credentials=1&expose=x-request-id', true),\
                 await xhr(named, true), await xhr(api + '/plain.json', false)].join('|');",
    )
    .await;
    assert_eq!(
        run.result,
        "load null null|load sid=1 cors-1|error 0|error 0"
    );
}

/// `mode: 'same-origin'` never leaves the origin, and `redirect: 'error'`
/// never follows one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mode_and_redirect_members_are_honoured() {
    let run = run(
        "var out = [];\
         var tries = [\
           [api + '/api/cors?origin=*', {mode: 'same-origin'}],\
           ['/api/redirect?to=/api/status', {redirect: 'error'}],\
           ['/api/redirect?to=/api/status', {redirect: 'follow', mode: 'same-origin', cache: 'no-store'}],\
           ['/api/redirect?to=' + encodeURIComponent(api + '/api/cors?origin=*'), {mode: 'same-origin'}],\
           ['/api/redirect?to=' + encodeURIComponent(api + '/api/cors?origin=*'), {}],\
           [api + '/api/cors?origin=*', {mode: 'no-cors'}]];\
         for (var t of tries) {\
           try { var r = await fetch(t[0], t[1]); out.push(r.status + (r.redirected ? 'r' : '')); }\
           catch (e) { out.push(e.constructor.name); }\
         }\
         return out.join('|');",
    )
    .await;
    assert_eq!(run.result, "TypeError|TypeError|200r|TypeError|200r|0");
}

/// Aborting the signal a `fetch()` was given rejects its promise with the
/// signal's reason and ends the request, so the page does not wait for an
/// answer nobody wants. A signal that is already aborted makes no request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_aborted_signal_rejects_the_fetch_and_ends_the_request() {
    let run = run("var out = [];\
         var slow = new AbortController();\
         var pending = fetch('/api/slow?ms=15000', {signal: slow.signal});\
         slow.abort();\
         try { await pending; out.push('resolved'); }\
         catch (e) { out.push(e.name + ':' + (e === slow.signal.reason)); }\
         var custom = new AbortController();\
         var why = {why: 'changed my mind'};\
         var second = fetch('/api/slow?ms=15000', {signal: custom.signal});\
         custom.abort(why);\
         try { await second; out.push('resolved'); } catch (e) { out.push(e === why); }\
         var before = new AbortController();\
         before.abort();\
         try { await fetch('/api/echo', {signal: before.signal}); out.push('resolved'); }\
         catch (e) { out.push(e.name); }\
         var unused = new AbortController();\
         var r = await fetch('/api/status', {signal: unused.signal});\
         unused.abort();\
         out.push(r.status);\
         out.push((await fetch('/api/status', {signal: null})).status);\
         return out.join('|');")
    .await;
    assert_eq!(run.result, "AbortError:true|true|AbortError|200|200");
    assert!(
        run.elapsed < Duration::from_secs(10),
        "the aborted requests did not hold the page for their 15 s answers: {:?}",
        run.elapsed
    );
    assert!(
        run.page_requests >= 3,
        "the document and the answered requests"
    );
}

/// A request made after an `await` is still part of loading the page: the
/// continuation inherits the provenance of the task that awaited, through a
/// promise, a short timer or the answer to an earlier request, so the
/// navigation waits for it. Each of these used to settle the page with the
/// request still in flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_made_after_an_await_holds_the_settle() {
    for before in [
        "await Promise.resolve();",
        "await null; await null;",
        "try { await Promise.reject(1); } catch (e) {}",
        "await new Promise(function (ok) { setTimeout(ok, 5); });",
        "await (await fetch('/api/status')).text();",
        "try { await fetch('/api/redirect?to=/api/status', {redirect: 'error'}); } catch (e) {}",
    ] {
        let run = run(&format!(
            "{before} var r = await fetch('/api/slow?ms=200'); return 'answered ' + r.status;"
        ))
        .await;
        assert_eq!(run.result, "answered 200", "after `{before}`");
    }
}

/// `mode: 'no-cors'` makes the request and tells script nothing about the
/// answer: the response is opaque. Only a method and headers a form could
/// have sent go out, and a same-origin request is an ordinary one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_no_cors_request_gets_an_opaque_response() {
    let run = run(
        "var out = [];\
         var r = await fetch(api + '/plain.json', {mode: 'no-cors'});\
         out.push([r.type, r.status, r.ok, r.url, Array.from(r.headers).length, (await r.text()).length].join(','));\
         try { await fetch(api + '/plain.json', {mode: 'no-cors', method: 'PUT'}); out.push('sent'); }\
         catch (e) { out.push(e.constructor.name); }\
         var same = await fetch('/api/cors', {mode: 'no-cors', method: 'POST', body: 'a=1',\
           headers: {'X-Token': 't', 'Content-Type': 'text/plain'}});\
         var seen = await same.json();\
         out.push([same.type, same.status, seen.method, seen.token, seen.contentType].join(','));\
         return out.join('|');",
    )
    .await;
    assert_eq!(
        run.result,
        "opaque,0,false,,0,0|TypeError|basic,200,POST,,text/plain"
    );
    assert_eq!(
        run.api_requests, 1,
        "the opaque request reached the other origin; the PUT was never sent"
    );
}

/// `AbortSignal.timeout()` and `AbortSignal.any()` end a request as a
/// controller's signal does, and `navigator.sendBeacon()` makes its request
/// and reports only that it was accepted: a form-typed body goes out without
/// a preflight, to another origin too, and the page does not see the answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timeout_signal_ends_a_request_and_a_beacon_is_sent() {
    let run = run("var out = [];\
         try { await fetch('/api/slow?ms=15000', {signal: AbortSignal.timeout(50)}); out.push('resolved'); }\
         catch (e) { out.push(e.name); }\
         var manual = new AbortController();\
         var any = AbortSignal.any([manual.signal, AbortSignal.timeout(50)]);\
         try { await fetch('/api/slow?ms=15000', {signal: any}); out.push('resolved'); }\
         catch (e) { out.push(e.name + ':' + (e === any.reason)); }\
         try { await fetch('/api/echo', {signal: AbortSignal.abort()}); out.push('resolved'); }\
         catch (e) { out.push(e.name); }\
         out.push(typeof navigator.sendBeacon);\
         out.push(navigator.sendBeacon('/api/echo', 'a=1'));\
         out.push(navigator.sendBeacon(api + '/plain.json', new URLSearchParams({b: '2'})));\
         out.push(navigator.sendBeacon('/api/echo'));\
         try { navigator.sendBeacon('http://['); out.push('sent'); } catch (e) { out.push(e.name); }\
         try { navigator.sendBeacon('ftp://example.com/'); out.push('sent'); } catch (e) { out.push(e.name); }\
         try { navigator.sendBeacon(); out.push('sent'); } catch (e) { out.push(e.name); }\
         return out.join('|');")
    .await;
    assert_eq!(
        run.result,
        "TimeoutError|TimeoutError:true|AbortError|function|true|true|true|TypeError|TypeError|TypeError"
    );
    assert!(
        run.elapsed < Duration::from_secs(10),
        "the timed-out requests did not hold the page for their 15 s answers: {:?}",
        run.elapsed
    );
    assert_eq!(
        run.api_requests, 1,
        "the beacon reached the other origin, once and without a preflight"
    );
    assert!(
        run.page_requests >= 3,
        "the document and the two same-origin beacons"
    );
}
