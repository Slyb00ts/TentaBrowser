//! Tests for the `fetch` global and the delivery seam (`fetch()` slice 4a-1a).
//!
//! The seam is the part of `fetch()` that has no network in it: a page asks for
//! a URL, a [`FetchDriver`] is handed the request and a resolver, and the answer
//! comes back as a [`FetchCompletion`] the engine settles a promise with. So
//! nothing here sends anything. The driver below is a recorder that never
//! answers on its own; a test drives the completion itself through
//! [`Engine::settle_fetches`], which is the same entry the page's settle loop
//! calls (`tenta_page::runtime`). 4a-1b is the slice that puts `tenta-net`
//! behind this trait.
//!
//! Two properties need their own observation rather than a single call:
//!
//! * a settlement is a **microtask**, not something applied where the
//!   completion is handed over, so a test that reads an answer has to checkpoint
//!   first (`Engine::evaluate` deliberately does not), and
//! * a `fetch` failure is a **rejection**, never a synchronous throw, so a test
//!   that cares has to look at the promise rather than at the call.
//!
//! The tests live here rather than under `crates/tenta-page/tests/` because they
//! need the V8 glue, and no V8 test may live there.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use tenta_core::Origin;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js::engine::PageEngine;
use tenta_js::fetch::{
    Credentials, FetchCompletion, FetchDriver, FetchOutcome, FetchRequest, FetchStartError,
    FetchedResponse, Mode, Redirect,
};
use tenta_js_v8::Engine;
use url::Url;

/// A document with `<html><body></body></html>` at
/// `http://fixture.test/dir/page.html`.
///
/// The URL is the point: a request for `/data.json` is resolved against it, so
/// the assertions below can tell a resolved URL from the string the page typed.
fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::with_url(Url::parse("http://fixture.test/dir/page.html").expect("a URL"));
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    d.mutate(Mutation::Append {
        parent: d.document_node(),
        child: html,
    })
    .unwrap();
    d.mutate(Mutation::Append {
        parent: html,
        child: body,
    })
    .unwrap();
    Rc::new(RefCell::new(d))
}

/// A driver that records what it was asked for and answers nothing on its own.
///
/// `fail_next` makes the *next* `start` refuse, so tests can distinguish a
/// driver rejection from an argument-conversion rejection before dispatch.
#[derive(Default)]
struct Recorder {
    requests: RefCell<Vec<FetchRequest>>,
    resolvers: RefCell<Vec<u64>>,
    /// The resolvers the engine asked the driver to end.
    aborted: RefCell<Vec<u64>>,
    fail_next: Cell<bool>,
}

impl Recorder {
    /// The one resolver the driver was handed, for a test that made exactly one
    /// request.
    fn only_resolver(&self) -> u64 {
        let resolvers = self.resolvers.borrow();
        assert_eq!(resolvers.len(), 1, "exactly one request was expected");
        resolvers[0]
    }

    /// Every resolver, in request order.
    fn resolvers(&self) -> Vec<u64> {
        self.resolvers.borrow().clone()
    }
}

impl FetchDriver for Recorder {
    fn start(&self, request: FetchRequest, resolver: u64) -> Result<(), FetchStartError> {
        self.requests.borrow_mut().push(request);
        self.resolvers.borrow_mut().push(resolver);
        match self.fail_next.replace(false) {
            true => Err(FetchStartError::new("the driver refuses")),
            false => Ok(()),
        }
    }

    fn abort(&self, resolver: u64) {
        self.aborted.borrow_mut().push(resolver);
    }
}

/// An engine over [`document`] with a [`Recorder`] installed. The handle is
/// returned too, because the requests only exist on it.
fn engine() -> (Engine, Rc<Recorder>) {
    let mut engine = Engine::with_document(document());
    let recorder = Rc::new(Recorder::default());
    // The cast is what makes the driver a trait object; `Rc::clone` would pick
    // the inherent `Rc<Recorder>` method and refuse to coerce.
    let driver = Rc::clone(&recorder) as Rc<dyn FetchDriver>;
    engine.set_fetch_driver(driver);
    (engine, recorder)
}

/// An engine over [`document`] with **no** driver, which is the state a page is
/// in before the network tier installs one.
fn engine_without_driver() -> Engine {
    Engine::with_document(document())
}

/// A completed response carrying `body` as JSON.
fn json_response(url: &str, body: &str) -> FetchOutcome {
    FetchOutcome::Response(FetchedResponse {
        url: url.to_owned(),
        status: 200,
        redirected: false,
        headers: vec![("Content-Type".to_owned(), b"application/json".to_vec())],
        body: Some(body.as_bytes().to_vec()),
    })
}

/// A completed response with the given status and body.
fn response(url: &str, status: u16, body: Option<&str>) -> FetchOutcome {
    FetchOutcome::Response(FetchedResponse {
        url: url.to_owned(),
        status,
        redirected: false,
        headers: Vec::new(),
        body: body.map(|body| body.as_bytes().to_vec()),
    })
}

/// A completion that settles `resolver` with `outcome`.
fn completion(resolver: u64, outcome: FetchOutcome) -> FetchCompletion {
    FetchCompletion {
        resolver,
        outcome,
        origin: Origin::Tracked,
    }
}

/// Runs `source` and returns its completion value, with no checkpoint.
fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("`{source}` should evaluate: {error}"))
}

// ---------------------------------------------------------------------------
// The request the page made
// ---------------------------------------------------------------------------

#[test]
fn the_request_is_what_the_page_asked_for() {
    let (mut engine, recorder) = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            globalThis.p = fetch('/data.json');
            typeof p.then + ':' + (p instanceof Promise);
        "#
        ),
        "function:true",
        "the call returns a promise"
    );

    let requests = recorder.requests.borrow();
    assert_eq!(requests.len(), 1, "the driver was asked exactly once");
    assert_eq!(
        requests[0].url.as_str(),
        "http://fixture.test/data.json",
        "the URL is resolved against the document's"
    );
    assert_eq!(requests[0].method.as_str(), "GET");
    assert!(
        requests[0].headers.is_empty(),
        "the call passed no initializer, so there are no headers"
    );
    assert!(requests[0].body.is_none());
    assert_eq!(
        requests[0].initiator.as_str(),
        "http://fixture.test/dir/page.html",
        "the initiator is the document's URL, not the request's"
    );
    // `Engine::evaluate` is not a script the page handed out, so the task that
    // called `fetch` is background work — the asymmetry `PageEngine::run_script`
    // exists to create. A `fetch` from an inline script is `Tracked`, and that
    // is what makes it hold the settle open.
    assert_eq!(requests[0].origin, Origin::Background);
    assert_eq!(recorder.resolvers().len(), 1, "one promise, one resolver");
}

#[test]
fn a_relative_request_resolves_against_the_document_url() {
    // `resolve_url` joins, so the directory survives — which is the difference
    // between "/data.json" and "data.json" and the reason `initiator` is not
    // simply the request URL.
    let (mut engine, recorder) = engine();
    eval(&mut engine, "fetch('data.json'); fetch('/data.json');");
    let requests = recorder.requests.borrow();
    assert_eq!(
        requests[0].url.as_str(),
        "http://fixture.test/dir/data.json"
    );
    assert_eq!(requests[1].url.as_str(), "http://fixture.test/data.json");
}

// ---------------------------------------------------------------------------
// The settlement is a microtask
// ---------------------------------------------------------------------------

#[test]
fn a_completion_is_drained_as_a_microtask_and_not_applied_in_place() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        globalThis.m = 'pending';
        fetch('/a').then(
            () => { globalThis.m = 'resolved'; },
            () => { globalThis.m = 'rejected'; }
        );
        "#,
    );
    // The entry that ran the script owes a checkpoint of its own
    // (`HostState::microtasks_pending`), and this test is about the one the
    // *drain* owes. Pay it first, so the assertion below can only be about the
    // drain.
    engine.run_microtasks();
    assert_eq!(
        engine.next_wakeup(),
        None,
        "nothing is owed before the completion arrives"
    );

    engine.settle_fetches(vec![completion(recorder.only_resolver(), failed("gone"))]);
    assert_eq!(
        engine.next_wakeup(),
        Some(Duration::ZERO),
        "the drain queued a microtask, so the loop must not sleep"
    );
    assert_eq!(
        eval(&mut engine, "globalThis.m"),
        "pending",
        "the settlement is queued, not applied where it was handed over"
    );

    engine.run_microtasks();
    assert_eq!(eval(&mut engine, "globalThis.m"), "rejected");
}

#[test]
fn a_completion_for_an_unknown_resolver_is_dropped_rather_than_applied() {
    // A driver that answers twice, or answers after the promise is gone, must
    // not settle somebody else's promise. `resolver` is opaque bits, so this is
    // the state the engine has to survive: the registry simply does not have
    // it.
    let (mut engine, _recorder) = engine();
    let baseline = engine.stored_value_count();
    engine.settle_fetches(vec![completion(0xdead_beef, failed("gone"))]);
    assert_eq!(
        engine.stored_value_count(),
        baseline,
        "an unknown resolver stores nothing"
    );
    assert_eq!(
        engine.next_wakeup(),
        None,
        "and owes no checkpoint, because it queued no microtask"
    );
}

#[test]
fn an_empty_batch_is_not_an_engine_entry() {
    // The page's `turn` calls `settle_fetches` every iteration, so the empty
    // case has to cost nothing. `discard_stray_termination` would clear a
    // termination the loop is about to report, which is the observable
    // difference.
    let (mut engine, recorder) = engine();
    eval(&mut engine, "fetch('/a');");
    engine.run_microtasks();
    let resolver = recorder.only_resolver();

    engine.settle_fetches(Vec::new());
    assert_eq!(
        engine.next_wakeup(),
        None,
        "an empty batch changes no state at all"
    );

    // And the promise it did not settle is still the one the batch below does.
    engine.settle_fetches(vec![completion(resolver, failed("gone"))]);
    engine.run_microtasks();
    assert_eq!(engine.stored_value_count(), 0, "the promise was settled");
}

/// A failing completion, the shape most of these tests need.
fn failed(message: &str) -> FetchOutcome {
    FetchOutcome::Failed(message.to_owned())
}

// ---------------------------------------------------------------------------
// A resolved response
// ---------------------------------------------------------------------------

#[test]
fn a_resolution_carries_a_real_response() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        globalThis.trace = [];
        fetch('/data.json').then(r => {
            trace.push(['fields', r.status, r.ok, r.url, r.type, r.redirected].join('|'));
            trace.push('content-type|' + r.headers.get('content-type'));
            trace.push('used0|' + r.bodyUsed);
            return r.json().then(value => {
                trace.push('json|' + value.a);
                trace.push('used1|' + r.bodyUsed);
                return r.json().then(
                    () => trace.push('second|resolved'),
                    error => trace.push('second|' + error.constructor.name)
                );
            });
        });
        "#,
    );
    engine.settle_fetches(vec![completion(
        recorder.only_resolver(),
        json_response("http://fixture.test/data.json", r#"{"a":1}"#),
    )]);
    engine.run_microtasks();

    assert_eq!(
        eval(&mut engine, "globalThis.trace.join(',')"),
        concat!(
            "fields|200|true|http://fixture.test/data.json|basic|false",
            ",content-type|application/json",
            ",used0|false",
            ",json|1",
            ",used1|true",
            ",second|TypeError",
        ),
    );
}

#[test]
fn a_null_body_status_has_no_body_and_says_so() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        globalThis.out = {};
        const read = (r, label) => r.text().then(text => {
            out[label] = [r.status, r.statusText, r.bodyUsed, text].join('|');
        });
        fetch('/empty').then(r => read(r, 'empty'));
        // A driver that hands over a body for a null-body status is wrong, and
        // the engine corrects it rather than trusting it.
        fetch('/ignored').then(r => read(r, 'ignored'));
        fetch('/missing').then(r => {
            // 299 is a status with no canonical reason, so the reason phrase is
            // empty rather than invented.
            out.missing = [r.status, r.statusText.length].join('|');
        });
        "#,
    );
    let resolvers = recorder.resolvers();
    assert_eq!(resolvers.len(), 3);
    engine.settle_fetches(vec![
        completion(
            resolvers[0],
            response("http://fixture.test/empty", 204, None),
        ),
        completion(
            resolvers[1],
            response("http://fixture.test/ignored", 204, Some("ignored")),
        ),
        completion(
            resolvers[2],
            response("http://fixture.test/missing", 299, None),
        ),
    ]);
    engine.run_microtasks();

    // Read per label rather than as one joined string: `text()` costs a
    // microtask hop that the 299 case does not take, so the three land in
    // whatever order their own chains finish in.
    assert_eq!(
        eval(
            &mut engine,
            "[out.empty, out.ignored, out.missing].join(',')"
        ),
        "204|No Content|false|,204|No Content|false|,299|0",
        "the reason phrase comes from the status, and a null-body status stays unread"
    );
}

#[test]
fn a_redirected_response_reports_where_it_came_from() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        fetch('/moved').then(r => {
            globalThis.trace = [r.status, r.url, r.redirected, r.type].join('|');
        });
        "#,
    );
    engine.settle_fetches(vec![completion(
        recorder.only_resolver(),
        FetchOutcome::Response(FetchedResponse {
            url: "http://fixture.test/final".to_owned(),
            status: 200,
            redirected: true,
            headers: Vec::new(),
            body: None,
        }),
    )]);
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "globalThis.trace"),
        "200|http://fixture.test/final|true|basic",
        "the response URL is the driver's, not the request's"
    );
}

#[test]
fn a_forbidden_response_header_never_reaches_script() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        fetch('/a').then(r => {
            // `String(..)` rather than the value itself: `join` renders `null`
            // as the empty string, which would make an absent header
            // indistinguishable from one that survived with an empty value.
            globalThis.trace = [
                String(r.headers.get('set-cookie')),
                String(r.headers.get('set-cookie2')),
                String(r.headers.get('x-visible')),
                [...r.headers.keys()].join(',')
            ].join('|');
        });
        "#,
    );
    engine.settle_fetches(vec![completion(
        recorder.only_resolver(),
        FetchOutcome::Response(FetchedResponse {
            url: "http://fixture.test/a".to_owned(),
            status: 200,
            redirected: false,
            headers: vec![
                ("Set-Cookie".to_owned(), b"a=1".to_vec()),
                ("Set-Cookie2".to_owned(), b"b=2".to_vec()),
                ("X-Visible".to_owned(), b"yes".to_vec()),
            ],
            body: None,
        }),
    )]);
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "globalThis.trace"),
        "null|null|yes|x-visible",
        "the two Set-Cookie spellings are dropped and the names are lowercased"
    );
}

// ---------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------

#[test]
fn a_failure_rejects_with_a_type_error() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        fetch('/a').then(
            value => { globalThis.trace = 'resolved|' + value; },
            error => {
                globalThis.trace = [
                    error.constructor.name, error.name, error.message,
                    error instanceof TypeError
                ].join('|');
            }
        );
        "#,
    );
    engine.settle_fetches(vec![completion(
        recorder.only_resolver(),
        failed("the network is gone"),
    )]);
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "globalThis.trace"),
        "TypeError|TypeError|the network is gone|true",
    );
}

#[test]
fn a_driver_refusal_rejects_instead_of_throwing() {
    // The whole reason `fetch` makes its promise first: a request that never
    // gets sent is still a settled promise, not an abrupt completion.
    let (mut engine, recorder) = engine();
    recorder.fail_next.set(true);
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const p = fetch('/a');
            p.then(
                () => { globalThis.trace = 'resolved'; },
                error => { globalThis.trace = 'rejected|' + error.constructor.name
                    + '|' + error.message; }
            );
            typeof p.then;
            "#
        ),
        "function",
        "the call returned a promise rather than throwing"
    );
    assert_eq!(recorder.requests.borrow().len(), 1, "the driver was asked");
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "globalThis.trace"),
        "rejected|TypeError|the driver refuses",
    );
}

#[test]
fn fetch_without_a_driver_rejects_rather_than_disappearing() {
    let mut engine = engine_without_driver();
    assert_eq!(eval(&mut engine, "typeof fetch"), "function");
    eval(
        &mut engine,
        r#"
        fetch('/a').then(
            () => { globalThis.trace = 'resolved'; },
            error => { globalThis.trace = error.constructor.name + '|' + error.message; }
        );
        "#,
    );
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "globalThis.trace"),
        "TypeError|fetch is not available",
        "a page with no network tier can still call fetch, and is told"
    );
}

#[test]
fn an_unresolvable_input_rejects_and_the_driver_is_never_asked() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        fetch('http://').then(
            () => { globalThis.trace = 'resolved'; },
            error => {
                globalThis.trace = [
                    error.constructor.name, error.message, error instanceof TypeError
                ].join('|');
            }
        );
        "#,
    );
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "globalThis.trace"),
        "TypeError|Failed to parse URL from http://|true",
    );
    assert!(
        recorder.requests.borrow().is_empty(),
        "a request with no URL is never handed over"
    );
}

// ---------------------------------------------------------------------------
// The initializer: the three members served, and the twelve refused
// ---------------------------------------------------------------------------

/// The three members this engine serves reach the driver, and a fourth call that
/// names something it does not returns no promise at all.
///
/// The `body` call is the interesting one: `{body: 'x'}` names no method, so the
/// method is `GET` and the standard refuses the pair — as a *rejection*, which
/// is why that call reaches neither the driver nor this test's `try`/`catch`.
#[test]
fn the_served_members_reach_the_driver_and_a_refusal_does_not() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        fetch('/a', { method: 'POST' });
        fetch('/a', { method: 'POST', headers: {} });
        fetch('/a', { headers: { 'X-A': '1' } });
        fetch('/a', { body: 'x' });
        "#,
    );

    let requests = recorder.requests.borrow();
    assert_eq!(
        requests.len(),
        3,
        "the three served initializers reached the driver and the body one did not"
    );
    assert_eq!(requests[0].method.as_str(), "POST");
    assert_eq!(
        requests[1].method.as_str(),
        "POST",
        "an empty `headers` dictionary changes nothing"
    );
    assert_eq!(
        requests[2].headers,
        vec![("x-a".to_owned(), "1".to_owned())]
    );
    drop(requests);

    // Both conversion failures return rejected promises before the driver runs.
    assert_eq!(
        rejected(&mut engine, "fetch('/a', { mode: 'navigate' })"),
        "TypeError: Failed to execute 'fetch': the RequestInit member 'mode' is not implemented"
    );
    assert_eq!(
        rejected(&mut engine, "fetch('/a', 1)"),
        "TypeError: Failed to execute 'fetch': the provided value is not of type 'RequestInit'"
    );
    assert_eq!(
        recorder.requests.borrow().len(),
        3,
        "and neither of the refused calls reached the driver"
    );
}

#[test]
fn an_empty_initializer_is_a_get() {
    // `null`, `undefined` and `{}` are all the empty dictionary, which has no
    // members to refuse. `{}` is the case a page actually writes.
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        fetch('/a', {});
        fetch('/b', undefined);
        fetch('/c', null);
        fetch('/d');
        "#,
    );
    let requests = recorder.requests.borrow();
    assert_eq!(requests.len(), 4, "every one of them reached the driver");
    for request in requests.iter() {
        assert_eq!(request.method.as_str(), "GET");
        assert!(request.headers.is_empty());
        assert!(request.body.is_none());
    }
    assert_eq!(
        requests
            .iter()
            .map(|request| request.url.path())
            .collect::<Vec<_>>(),
        ["/a", "/b", "/c", "/d"],
    );
}

/// A dictionary member is read with `Get`, so how the property is *reached* does
/// not matter: an inherited one and a non-enumerable own one are both members,
/// and both requests are therefore `POST`s
/// (<https://webidl.spec.whatwg.org/#js-dictionary>: "Properties on the object
/// (or its prototype chain) correspond to dictionary members", and the
/// conversion's step is `? Get(jsDict, key)`).
///
/// This is the one behaviour of the old refusal that the dictionary conversion
/// changes beyond the named-members rule, and it is a change *towards* the
/// standard: the refusal enumerated own enumerable keys, so neither object was
/// a `RequestInit` member and both were a `GET`.
#[test]
fn a_member_is_read_through_the_prototype_chain_and_regardless_of_enumerability() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        r#"
        const inherited = Object.create({ method: 'POST' });
        const hidden = {};
        Object.defineProperty(hidden, 'method', { value: 'POST', enumerable: false });
        fetch('/inherited', inherited);
        fetch('/hidden', hidden);
        fetch('/plain', { method: 'POST' });
        "#,
    );
    let requests = recorder.requests.borrow();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.url.path(), request.method.as_str()))
            .collect::<Vec<_>>(),
        [
            ("/inherited", "POST"),
            ("/hidden", "POST"),
            ("/plain", "POST"),
        ],
        "a `Get` sees the inherited and the hidden `method` where an own-enumerable \
         enumeration would have seen neither"
    );
}

// ---------------------------------------------------------------------------
// Reclamation
// ---------------------------------------------------------------------------

#[test]
fn a_settled_fetch_settles_every_promise_and_frees_every_record() {
    // The drain is the only producer whose settlement arrives outside the call
    // that made the promise, so it is the one place a promise can be left
    // pending with its slots — and a `Response` record with its headers — still
    // held. A batch of twenty says whether both directions of that are clean.
    let (mut engine, recorder) = engine();
    let baseline_hosts = engine.host_record_count();
    eval(
        &mut engine,
        r#"
        globalThis.settled = 0;
        for (let i = 0; i < 20; i += 1) {
            fetch('/r' + i).then(r => { globalThis.settled += r.status + r.headers.get('x-i').length; });
        }
        "#,
    );
    let resolvers = recorder.resolvers();
    assert_eq!(resolvers.len(), 20);

    engine.settle_fetches(
        resolvers
            .iter()
            .enumerate()
            .map(|(i, resolver)| {
                completion(
                    *resolver,
                    FetchOutcome::Response(FetchedResponse {
                        url: format!("http://fixture.test/r{i}"),
                        status: 200,
                        redirected: false,
                        headers: vec![("X-I".to_owned(), i.to_string().into_bytes())],
                        body: None,
                    }),
                )
            })
            .collect(),
    );
    engine.run_microtasks();
    // 20 responses of 200, plus the length of their `X-I` value — "0".."9" is
    // ten one-byte values and "10".."19" is ten two-byte ones.
    assert_eq!(
        eval(&mut engine, "globalThis.settled"),
        (20 * 200 + 10 + 2 * 10).to_string(),
        "every promise in the batch was settled"
    );
    assert_eq!(
        engine.stored_value_count(),
        0,
        "no promise and no response outlives the batch"
    );

    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(
        engine.host_record_count(),
        baseline_hosts,
        "a collected response wrapper frees its record and its headers"
    );
}

#[test]
fn a_response_kept_by_script_survives_the_sweep() {
    // The other half of the accounting: the record belongs to the wrapper that
    // script holds, so keeping that wrapper keeps both.
    let (mut engine, recorder) = engine();
    let baseline_hosts = engine.host_record_count();
    eval(
        &mut engine,
        r#"
        globalThis.keep = null;
        fetch('/keep').then(r => { globalThis.keep = r; });
        "#,
    );
    engine.settle_fetches(vec![completion(
        recorder.only_resolver(),
        json_response("http://fixture.test/keep", r#"{"a":1}"#),
    )]);
    engine.run_microtasks();

    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(
        engine.host_record_count(),
        baseline_hosts + 2,
        "the live response and its headers both survive"
    );
    assert_eq!(
        eval(
            &mut engine,
            "[keep.status, keep.url, keep.headers.get('content-type')].join('|')"
        ),
        "200|http://fixture.test/keep|application/json",
    );

    eval(&mut engine, "globalThis.keep = null;");
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), baseline_hosts);
}

// ---------------------------------------------------------------------------
// The second argument, `RequestInit` (slice b-6.5)
// ---------------------------------------------------------------------------
//
// Everything here is about the *conversion*: what a V8 value becomes a
// `RequestInit` at all, and what a page sees when it does not. The rules the
// converted dictionary then runs — which members are served, what a method
// normalises to, whether a body is allowed on a `GET` — are asserted where they
// live, engine-independently, in `crates/tenta-js/src/bindings/mod.rs`'s
// `request_init` module. What can only be observed with an isolate is the
// boundary: the conversion rejection, the ignored undeclared key, and the
// presence test for each member a page can name.

/// Whether `call` throws synchronously, independently of its Promise result.
fn threw(engine: &mut Engine, call: &str) -> String {
    eval(
        engine,
        &format!(
            "{{ let out = 'no throw';
               try {{ {call} }} catch (e) {{ out = e.name + ': ' + e.message; }}
               out }}"
        ),
    )
}

/// The rejection a promise was settled with, as `name: message`.
fn rejected(engine: &mut Engine, call: &str) -> String {
    eval(
        engine,
        &format!(
            "globalThis.out = 'pending';
             ({call}).catch(e => {{ globalThis.out = e.name + ': ' + e.message; }});
             'started'"
        ),
    );
    engine.run_microtasks();
    eval(engine, "globalThis.out")
}

/// The one request the driver was handed, for a test that made exactly one.
fn only_request(recorder: &Recorder) -> FetchRequest {
    let requests = recorder.requests.borrow();
    assert_eq!(requests.len(), 1, "exactly one request was expected");
    requests[0].clone()
}

/// A non-object second argument rejects the returned promise with WebIDL's
/// dictionary TypeError and never reaches the driver.
#[test]
fn an_initializer_that_is_not_an_object_rejects_and_returns_a_promise() {
    let (mut engine, recorder) = engine();

    assert_eq!(
        rejected(&mut engine, "fetch('/x', 3)"),
        "TypeError: Failed to execute 'fetch': the provided value is not of type 'RequestInit'"
    );
    assert_eq!(
        eval(
            &mut engine,
            "const result = fetch('/x', 'POST'); result.catch(() => {}); typeof result"
        ),
        "object",
        "the call returned a Promise"
    );
    assert_eq!(
        recorder.requests.borrow().len(),
        0,
        "nothing reached the driver"
    );
}

/// A page-thrown argument conversion rejects with that exact value, before
/// any native request or resolver reaches the driver.
#[test]
fn throwing_input_and_initializer_getters_reject_with_the_original_value() {
    let (mut engine, recorder) = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const inputMarker = { error: 'input' };
            const initMarker = { error: 'init' };
            globalThis.inputSame = false;
            globalThis.initSame = false;
            const input = fetch({ [Symbol.toPrimitive]() { throw inputMarker; } });
            const init = fetch('/x', { get method() { throw initMarker; } });
            input.catch(error => { globalThis.inputSame = error === inputMarker; });
            init.catch(error => { globalThis.initSame = error === initMarker; });
            [input instanceof Promise, init instanceof Promise].join('|');
            "#,
        ),
        "true|true"
    );
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "[inputSame, initSame].join('|')"),
        "true|true"
    );
    assert_eq!(
        recorder.requests.borrow().len(),
        0,
        "argument conversion never starts a request"
    );
}

/// A key the dictionary does not name is **ignored**, because that is what a
/// WebIDL dictionary conversion does: it reads the members it declares and
/// nothing else.
///
/// This is the deliberate behaviour change of this slice and the reason the
/// blanket refusal was allowed to go. Until it, `fetch(url, {foo: 1})` threw
/// "the RequestInit member 'foo' is not implemented"; a key that names nothing
/// is not a member of any dictionary, so there is nothing for the engine to be
/// refusing.
#[test]
fn a_key_the_dictionary_does_not_name_is_ignored() {
    let (mut engine, recorder) = engine();
    assert_eq!(
        threw(&mut engine, "fetch('/x', {foo: 1, bar: 'baz'})"),
        "no throw",
        "an undeclared key is not a member, so there is nothing to refuse"
    );

    let request = only_request(&recorder);
    assert_eq!(request.method.as_str(), "GET");
    assert!(
        request.headers.is_empty(),
        "and it reaches no header list either"
    );
}

/// A member whose value this engine cannot serve is refused *by name*, with the
/// message shape this engine has always used for the refusal.
#[test]
fn every_unserved_member_is_refused_by_name() {
    let (mut engine, recorder) = engine();

    for (member, literal) in [
        ("referrer", "'https://elsewhere.test/'"),
        ("referrerPolicy", "'no-referrer'"),
        ("mode", "'navigate'"),
        ("cache", "'only-if-cached'"),
        ("redirect", "'manual'"),
        ("integrity", "'sha256-abc'"),
        ("window", "{}"),
    ] {
        assert_eq!(
            rejected(
                &mut engine,
                &format!("fetch('/x', {{{member}: {literal}}})")
            ),
            format!(
                "TypeError: Failed to execute 'fetch': the RequestInit member '{member}' is not implemented"
            ),
            "the member `{member}` is not served by this engine"
        );
    }

    assert_eq!(
        recorder.requests.borrow().len(),
        0,
        "a refused initializer never reaches the driver"
    );
}

/// The modes the page names reach the driver, and a member written out with
/// its default is the request the engine sends anyway.
#[test]
fn the_modes_of_an_initializer_reach_the_driver() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        "fetch('/x', {credentials: 'include', mode: 'same-origin', redirect: 'error', cache: 'no-store'});
         fetch('/y', {credentials: 'omit', mode: 'cors', redirect: 'follow', cache: 'default',
                      integrity: '', keepalive: false, signal: null, duplex: 'half',
                      priority: 'low', window: null, referrer: 'about:client', referrerPolicy: ''});
         fetch('/z');
         'started'",
    );
    let requests = recorder.requests.borrow();
    assert_eq!(requests.len(), 3);
    let modes = |request: &FetchRequest| {
        (
            request.credentials,
            request.mode,
            request.redirect,
            request.bypass_cache,
        )
    };
    assert_eq!(
        modes(&requests[0]),
        (
            Credentials::Include,
            Mode::SameOrigin,
            Redirect::Error,
            true
        )
    );
    assert_eq!(
        modes(&requests[1]),
        (Credentials::Omit, Mode::Cors, Redirect::Follow, false)
    );
    assert_eq!(
        modes(&requests[2]),
        (Credentials::SameOrigin, Mode::Cors, Redirect::Follow, false)
    );
}

/// Aborting the signal a `fetch()` was given rejects its promise with the
/// signal's reason — the same value, not a copy — and tells the driver to end
/// the request. A request that was answered first is no longer the signal's
/// to abort, and a signal that is already aborted makes no request at all.
#[test]
fn a_signal_aborts_the_requests_it_was_given_to() {
    let (mut engine, recorder) = engine();
    eval(
        &mut engine,
        "globalThis.log = [];
         globalThis.first = new AbortController();
         globalThis.second = new AbortController();
         globalThis.why = {why: 'changed my mind'};
         fetch('/a', {signal: first.signal}).catch(e => { log.push('a ' + e.name + ' ' + (e === first.signal.reason)); });
         fetch('/b', {signal: first.signal}).catch(e => { log.push('b ' + e.name); });
         fetch('/c', {signal: second.signal}).then(r => { log.push('c ' + r.status); }, e => { log.push('c rejected'); });
         fetch('/d', {signal: second.signal}).catch(e => { log.push('d ' + (e === why)); });
         'started'",
    );
    let resolvers = recorder.resolvers();
    assert_eq!(resolvers.len(), 4);

    eval(&mut engine, "first.abort(); 'aborted'");
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "log.join('|')"),
        "a AbortError true|b AbortError"
    );
    let mut aborted = recorder.aborted.borrow().clone();
    aborted.sort_unstable();
    let mut expected = vec![resolvers[0], resolvers[1]];
    expected.sort_unstable();
    assert_eq!(
        aborted, expected,
        "the driver was told to end both requests"
    );

    // `/c` is answered before its signal aborts; `/d` is still in flight.
    engine.settle_fetches(vec![completion(
        resolvers[2],
        response("http://fixture.test/c", 200, Some("ok")),
    )]);
    engine.run_microtasks();
    eval(&mut engine, "second.abort(why); 'aborted'");
    engine.run_microtasks();
    assert_eq!(eval(&mut engine, "log.slice(2).join('|')"), "c 200|d true");
    assert_eq!(
        recorder.aborted.borrow().len(),
        3,
        "only `/d` was left for the second signal to end"
    );

    // A late answer for an aborted request finds its promise settled.
    engine.settle_fetches(vec![completion(
        resolvers[0],
        response("http://fixture.test/a", 200, Some("late")),
    )]);
    engine.run_microtasks();
    assert_eq!(eval(&mut engine, "log.length"), "4");

    assert_eq!(
        rejected(&mut engine, "fetch('/e', {signal: first.signal})"),
        "AbortError: The operation was aborted."
    );
    assert_eq!(
        recorder.requests.borrow().len(),
        4,
        "an aborted signal makes no request"
    );
}

/// A forbidden method is a `TypeError` — but a *rejection*, not a throw, because
/// the standard runs that check inside the `Request` constructor and `fetch`
/// converts the constructor's throw to a rejection
/// (<https://fetch.spec.whatwg.org/#dom-global-fetch>, step 2).
///
/// Both halves are asserted: the message says which rule refused, and the return
/// value says the call got past its argument conversion and made a promise.
#[test]
fn a_forbidden_method_rejects_rather_than_throwing() {
    let (mut engine, recorder) = engine();

    assert_eq!(
        eval(
            &mut engine,
            "(function () {
               try {
                 const p = fetch('/x', {method: 'CONNECT'});
                 p.catch(function () {});
                 return typeof p.then;
               } catch (e) {
                 return 'threw: ' + e.message;
               }
             })()"
        ),
        "function",
        "the call returned a promise, so nothing about it threw"
    );
    assert_eq!(
        rejected(&mut engine, "fetch('/x', {method: 'CONNECT'})"),
        "TypeError: Failed to execute 'fetch': the method is forbidden"
    );
    assert_eq!(
        recorder.requests.borrow().len(),
        0,
        "a refused method never reaches the driver"
    );
}

/// The `GET`/`HEAD` body rule rejects too, and `method` normalisation is
/// observable through what the driver is handed: the six methods the standard
/// uppercases arrive uppercased and every other method keeps its case.
#[test]
fn the_method_rules_reach_the_driver_as_the_standard_leaves_them() {
    let (mut engine, recorder) = engine();

    assert_eq!(
        rejected(&mut engine, "fetch('/x', {method: 'GET', body: 'hi'})"),
        "TypeError: Failed to execute 'fetch': a request with a body cannot be a GET or a HEAD"
    );
    assert_eq!(
        rejected(&mut engine, "fetch('/x', {method: 'head', body: 'hi'})"),
        "TypeError: Failed to execute 'fetch': a request with a body cannot be a GET or a HEAD",
        "the rule is about the method, and `head` is a `HEAD`"
    );

    eval(
        &mut engine,
        "fetch('/x', {method: 'post', body: 'hi'});
         fetch('/x', {method: 'patch'});
         fetch('/x', {method: 'CHICKEN'});",
    );
    let methods: Vec<String> = recorder
        .requests
        .borrow()
        .iter()
        .map(|request| request.method.as_str().to_owned())
        .collect();
    assert_eq!(
        methods,
        ["POST", "patch", "CHICKEN"],
        "the six the standard uppercases arrive uppercased, the rest keep the page's case"
    );
}
