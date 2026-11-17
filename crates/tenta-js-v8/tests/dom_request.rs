//! `Request` and `fetch(request)`.
//!
//! `crates/tenta-js-v8/tests/dom_fetch.rs` covers `fetch(url, init)`; this file
//! covers the object: what its constructor stores and refuses, what a request
//! made from another request takes from it, `clone()`, the `Body` members, the
//! signal, and what a driver is handed when a request is fetched.
//!
//! The steps under test are engine-independent
//! (`crates/tenta-js/src/bindings/host/request.rs`); they are tested here
//! because every one of them is reached through a conversion that needs an
//! isolate.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js::engine::PageEngine;
use tenta_js::fetch::{Credentials, FetchDriver, FetchRequest, FetchStartError, Mode, Redirect};
use tenta_js_v8::Engine;
use url::Url;

/// A document with `<html><body></body></html>` at
/// `http://fixture.test/dir/page.html`, which a relative request URL is
/// resolved against.
fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::with_url(Url::parse("http://fixture.test/dir/page.html").expect("a URL"));
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    for (parent, child) in [(d.document_node(), html), (html, body)] {
        d.mutate(Mutation::Append { parent, child })
            .expect("append");
    }
    Rc::new(RefCell::new(d))
}

/// A driver that records what it was asked for and answers nothing.
#[derive(Default)]
struct Recorder {
    requests: RefCell<Vec<FetchRequest>>,
    resolvers: RefCell<Vec<u64>>,
    aborted: RefCell<Vec<u64>>,
}

impl FetchDriver for Recorder {
    fn start(&self, request: FetchRequest, resolver: u64) -> Result<(), FetchStartError> {
        self.requests.borrow_mut().push(request);
        self.resolvers.borrow_mut().push(resolver);
        Ok(())
    }

    fn abort(&self, resolver: u64) {
        self.aborted.borrow_mut().push(resolver);
    }
}

fn engine() -> (Engine, Rc<Recorder>) {
    let mut engine = Engine::with_document(document());
    let recorder = Rc::new(Recorder::default());
    let driver = Rc::clone(&recorder) as Rc<dyn FetchDriver>;
    engine.set_fetch_driver(driver);
    (engine, recorder)
}

/// Runs `source` and returns its completion value, with no checkpoint.
fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("`{source}` should evaluate: {error}"))
}

/// Runs `source`, drains the microtasks it queued, then reads `after`.
fn settled(engine: &mut Engine, source: &str, after: &str) -> String {
    eval(engine, source);
    engine.run_microtasks();
    eval(engine, after)
}

/// What each `run` in `source` returns, or the name of what it throws.
fn caught(engine: &mut Engine, source: &str) -> String {
    eval(
        engine,
        &format!(
            r#"
            {{
                const attempt = run => {{
                    try {{ return 'ok:' + run(); }}
                    catch (error) {{ return error.constructor.name; }}
                }};
                const results = [];
                {source}
                results.join('|');
            }}
            "#
        ),
    )
}

// ---------------------------------------------------------------------------
// The interface and what a new request holds
// ---------------------------------------------------------------------------

#[test]
fn request_is_installed_with_its_members() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            [
                typeof Request, Request.length, Request.name,
                Object.prototype.toString.call(new Request('/x')),
                Object.keys(Request.prototype).join(','),
            ].join('|');
            "#
        ),
        "function|1|Request|[object Request]|method,url,headers,destination,referrer,\
         referrerPolicy,mode,credentials,cache,redirect,integrity,keepalive,signal,duplex,clone,\
         bodyUsed,arrayBuffer,blob,bytes,json,text"
    );
}

#[test]
fn a_request_for_a_url_is_a_get_with_the_default_modes() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const request = new Request('data.json?x=1#part');
            [
                request.url, request.method, request.mode, request.credentials, request.cache,
                request.redirect, request.keepalive, request.bodyUsed,
                '[' + request.destination + ']', request.referrer,
                '[' + request.referrerPolicy + ']', '[' + request.integrity + ']', request.duplex,
                request.headers instanceof Headers, request.headers === request.headers,
                [...request.headers].length,
                request.signal instanceof AbortSignal, request.signal === request.signal,
                request.signal.aborted,
            ].join('|');
            "#
        ),
        "http://fixture.test/dir/data.json?x=1#part|GET|cors|same-origin|default|follow|false|\
         false|[]|about:client|[]|[]|half|true|true|0|true|true|false"
    );
}

#[test]
fn the_initializer_sets_the_method_the_headers_the_modes_and_the_body() {
    let (mut engine, _) = engine();
    assert_eq!(
        settled(
            &mut engine,
            r#"
            globalThis.request = new Request('/submit', {
                method: 'post',
                headers: { 'X-One': '1', 'Cookie': 'dropped=1' },
                body: 'payload',
                mode: 'same-origin',
                credentials: 'include',
                cache: 'no-store',
                redirect: 'error',
                keepalive: true,
            });
            globalThis.before = [
                request.method, request.headers.get('x-one'), String(request.headers.get('cookie')),
                request.headers.get('content-type'), request.mode, request.credentials,
                request.cache, request.redirect, request.keepalive, request.bodyUsed,
            ].join('|');
            request.text().then(text => { globalThis.body = text; });
            "#,
            "[before, body, request.bodyUsed].join('|')"
        ),
        "POST|1|null|text/plain;charset=UTF-8|same-origin|include|no-store|error|true|false|\
         payload|true",
        "the method is normalised, a forbidden request header is dropped by the guard, and the \
         body's type is inferred"
    );
}

#[test]
fn the_constructor_refuses_what_the_standard_refuses() {
    let (mut engine, _) = engine();
    assert_eq!(
        caught(
            &mut engine,
            r#"
            results.push(attempt(() => new Request()));
            results.push(attempt(() => new Request('http://')));
            results.push(attempt(() => new Request('/x', { method: 'TRACE' })));
            results.push(attempt(() => new Request('/x', { method: 'a b' })));
            results.push(attempt(() => new Request('/x', { body: 'x' })));
            results.push(attempt(() => new Request('/x', { method: 'HEAD', body: 'x' })));
            results.push(attempt(() => new Request('/x', { mode: 'navigate' })));
            results.push(attempt(() => new Request('/x', { mode: 'no-cors', method: 'PUT' })));
            results.push(attempt(() => new Request('/x', 1)));
            results.push(attempt(() => new Request('/x', { method: 'PUT', body: 'x' }).method));
            "#
        ),
        "TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|\
         TypeError|ok:PUT"
    );
}

#[test]
fn the_headers_of_a_request_are_guarded_as_request_headers() {
    let (mut engine, _) = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const request = new Request('/x');
            request.headers.set('X-Custom', 'yes');
            request.headers.set('Host', 'elsewhere.test');
            [request.headers.get('x-custom'), String(request.headers.get('host'))].join('|');
            "#
        ),
        "yes|null"
    );
}

// ---------------------------------------------------------------------------
// A request made from a request, and clone()
// ---------------------------------------------------------------------------

#[test]
fn a_request_made_from_a_request_copies_it_and_takes_its_body() {
    let (mut engine, _) = engine();
    assert_eq!(
        settled(
            &mut engine,
            r#"
            globalThis.first = new Request('/a', {
                method: 'POST', body: 'one', headers: { 'X-A': '1' }, credentials: 'omit',
            });
            globalThis.second = new Request(first);
            globalThis.shape = [
                second !== first, second.url === first.url, second.method, second.credentials,
                second.headers.get('x-a'), second.headers !== first.headers,
                first.bodyUsed, second.bodyUsed,
            ].join('|');
            second.headers.set('x-a', '2');
            second.text().then(text => { globalThis.body = text; });
            "#,
            "[shape, body, first.headers.get('x-a')].join('|')"
        ),
        "true|true|POST|omit|1|true|true|false|one|1",
        "the body moved to the new request, so the input is disturbed and the copy is not"
    );
}

#[test]
fn an_initializer_overrides_the_request_it_is_applied_to() {
    let (mut engine, _) = engine();
    assert_eq!(
        settled(
            &mut engine,
            r#"
            globalThis.first = new Request('/a', {
                method: 'POST', body: 'one', headers: { 'X-A': '1' },
            });
            globalThis.second = new Request(first, {
                method: 'PUT', body: 'two', headers: { 'X-B': '2' },
            });
            second.text().then(text => { globalThis.body = text; });
            "#,
            r#"
            [second.method, String(second.headers.get('x-a')), second.headers.get('x-b'), body,
             first.bodyUsed].join('|')
            "#
        ),
        "PUT|null|2|two|false",
        "the initializer's headers replace the copied list, and its body leaves the input's alone"
    );
}

#[test]
fn a_request_whose_body_was_read_cannot_be_copied_or_cloned() {
    let (mut engine, _) = engine();
    assert_eq!(
        caught(
            &mut engine,
            r#"
            const read = new Request('/a', { method: 'POST', body: 'x' });
            read.text();
            results.push(attempt(() => new Request(read)));
            results.push(attempt(() => read.clone()));
            // Naming a new body does not need the old one.
            results.push(attempt(() => new Request(read, { body: 'y' }).method));
            // A request with no body is never disturbed.
            const empty = new Request('/a');
            empty.text();
            results.push(attempt(() => new Request(empty).url.length > 0));
            results.push(attempt(() => empty.clone().method));
            "#
        ),
        "TypeError|TypeError|ok:POST|ok:true|ok:GET"
    );
}

#[test]
fn clone_copies_the_request_and_reads_separately() {
    let (mut engine, _) = engine();
    assert_eq!(
        settled(
            &mut engine,
            r#"
            globalThis.out = [];
            const original = new Request('/a', {
                method: 'POST', body: new Uint8Array([1, 2]), headers: { 'X-A': '1' },
            });
            const copy = original.clone();
            out.push(copy instanceof Request, copy !== original, copy.method, copy.url,
                     copy.headers.get('x-a'), copy.headers !== original.headers);
            original.arrayBuffer().then(buffer => {
                out.push(buffer.byteLength, original.bodyUsed, copy.bodyUsed);
                return copy.bytes();
            }).then(bytes => out.push(Array.from(bytes).join(',')));
            "#,
            "out.join('|')"
        ),
        "true|true|POST|http://fixture.test/a|1|true|2|true|false|1,2"
    );
}

// ---------------------------------------------------------------------------
// The signal
// ---------------------------------------------------------------------------

#[test]
fn a_request_given_a_signal_answers_with_it_and_keeps_it_alive() {
    let (mut engine, _) = engine();
    eval(
        &mut engine,
        r#"
        globalThis.controller = new AbortController();
        globalThis.request = new Request('/a', { signal: controller.signal });
        globalThis.copy = request.clone();
        globalThis.derived = new Request(request);
        globalThis.detached = new Request(request, { signal: null });
        "#,
    );
    assert_eq!(
        eval(
            &mut engine,
            r#"
            [request.signal === controller.signal, copy.signal === controller.signal,
             derived.signal === controller.signal, detached.signal !== controller.signal,
             request.signal.aborted].join('|')
            "#
        ),
        "true|true|true|true|false",
        "this engine has no signal that follows another, so the request's signal is the given one"
    );
    eval(&mut engine, "controller.abort('stop'); 'aborted'");
    assert_eq!(
        eval(
            &mut engine,
            "[request.signal.aborted, request.signal.reason, detached.signal.aborted].join('|')"
        ),
        "true|stop|false"
    );

    // The request is what keeps the signal once the page lets go of it.
    eval(
        &mut engine,
        "globalThis.kept = new Request('/b', { signal: new AbortController().signal }); 'kept'",
    );
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(
        eval(
            &mut engine,
            "[kept.signal instanceof AbortSignal, kept.signal.aborted].join('|')"
        ),
        "true|false"
    );
}

// ---------------------------------------------------------------------------
// fetch(request)
// ---------------------------------------------------------------------------

#[test]
fn a_fetched_request_reaches_the_driver_as_it_was_made() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        globalThis.request = new Request('/submit?x=1', {
            method: 'POST',
            headers: { 'X-One': '1' },
            body: new URLSearchParams({ a: 'b c' }),
            mode: 'same-origin',
            credentials: 'include',
            redirect: 'error',
            cache: 'reload',
        });
        fetch(request);
        'started'
        "#,
    );
    let requests = recorder.requests.borrow();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.url.as_str(), "http://fixture.test/submit?x=1");
    assert_eq!(request.method.as_str(), "POST");
    assert_eq!(
        request.headers,
        vec![
            ("x-one".to_owned(), "1".to_owned()),
            (
                "content-type".to_owned(),
                "application/x-www-form-urlencoded;charset=UTF-8".to_owned()
            ),
        ]
    );
    assert_eq!(request.body.as_deref(), Some(b"a=b+c".as_slice()));
    assert_eq!(
        request.initiator.as_str(),
        "http://fixture.test/dir/page.html"
    );
    assert_eq!(request.mode, Mode::SameOrigin);
    assert_eq!(request.credentials, Credentials::Include);
    assert_eq!(request.redirect, Redirect::Error);
    assert!(request.bypass_cache);
    drop(requests);
    assert_eq!(
        eval(&mut engine, "request.bodyUsed"),
        "true",
        "fetching a request reads its body"
    );
}

#[test]
fn a_request_can_be_fetched_once_with_its_body_and_again_without_one() {
    let (mut engine, recorder) = engine();
    assert_eq!(
        settled(
            &mut engine,
            r#"
            globalThis.out = [];
            const posted = new Request('/a', { method: 'POST', body: 'x' });
            fetch(posted);
            fetch(posted).catch(error => out.push(error.constructor.name));
            const plain = new Request('/b');
            fetch(plain);
            fetch(plain);
            "#,
            "out.join('|')"
        ),
        "TypeError",
        "a second fetch of a request whose body was read is rejected"
    );
    let urls: Vec<String> = recorder
        .requests
        .borrow()
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect();
    assert_eq!(urls, ["/a", "/b", "/b"]);
}

#[test]
fn an_initializer_given_to_fetch_is_applied_over_the_request() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        const request = new Request('/a', { headers: { 'X-A': '1' } });
        fetch(request, { method: 'DELETE', headers: { 'X-B': '2' } });
        fetch(new Request(request, { method: 'PATCH', body: '{}' }));
        'started'
        "#,
    );
    let requests = recorder.requests.borrow();
    assert_eq!(requests[0].method.as_str(), "DELETE");
    assert_eq!(
        requests[0].headers,
        vec![("x-b".to_owned(), "2".to_owned())]
    );
    assert_eq!(requests[1].method.as_str(), "PATCH");
    assert_eq!(requests[1].body.as_deref(), Some(b"{}".as_slice()));
    assert_eq!(
        requests[1].headers,
        vec![
            ("x-a".to_owned(), "1".to_owned()),
            (
                "content-type".to_owned(),
                "text/plain;charset=UTF-8".to_owned()
            ),
        ]
    );
}

#[test]
fn the_signal_of_a_fetched_request_aborts_the_fetch() {
    let (mut engine, recorder) = engine();
    assert_eq!(
        settled(
            &mut engine,
            r#"
            globalThis.out = [];
            globalThis.live = new AbortController();
            fetch(new Request('/a', { signal: live.signal }))
                .catch(error => out.push('live:' + error.name));
            const done = new AbortController();
            done.abort();
            fetch(new Request('/b', { signal: done.signal }))
                .catch(error => out.push('done:' + error.name));
            "#,
            "out.join('|')"
        ),
        "done:AbortError",
        "a request whose signal is already aborted is rejected and never sent"
    );
    assert_eq!(recorder.requests.borrow().len(), 1);
    assert!(recorder.aborted.borrow().is_empty());

    eval(&mut engine, "live.abort(); 'aborted'");
    engine.run_microtasks();
    assert_eq!(
        *recorder.aborted.borrow(),
        *recorder.resolvers.borrow(),
        "aborting the signal ends the request the driver holds"
    );
    assert_eq!(
        eval(&mut engine, "out.join('|')"),
        "done:AbortError|live:AbortError"
    );
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

#[test]
fn a_collected_request_frees_its_record_its_headers_and_its_signal() {
    let (mut engine, _) = engine();
    let baseline = engine.host_record_count();
    eval(
        &mut engine,
        r#"
        globalThis.kept = new Request('/a', { method: 'POST', body: 'x' });
        globalThis.signalled = new Request('/b', { signal: new AbortController().signal });
        'made'
        "#,
    );
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(
        engine.host_record_count(),
        baseline + 6,
        "each request with its headers; the first owns a signal, the second keeps the one it \
         was given, whose controller is gone"
    );

    eval(
        &mut engine,
        "globalThis.kept = null; globalThis.signalled = null; 'released'",
    );
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), baseline);
}
