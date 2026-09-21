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
    FetchCompletion, FetchDriver, FetchOutcome, FetchRequest, FetchStartError, FetchedResponse,
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
/// `fail_next` makes the *next* `start` refuse, which is how the tests tell a
/// driver refusal (a rejection) apart from a `fetch` that threw.
#[derive(Default)]
struct Recorder {
    requests: RefCell<Vec<FetchRequest>>,
    resolvers: RefCell<Vec<u64>>,
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
    FetchCompletion { resolver, outcome }
}

/// Runs `source` and returns its completion value, with no checkpoint.
fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("`{source}` should evaluate: {error}"))
}

/// Runs `source`, whose `run` may throw, and collects the constructor name of
/// whatever it throws.
fn caught(engine: &mut Engine, source: &str) -> String {
    eval(
        engine,
        &format!(
            r#"
            const attempt = run => {{
                try {{ run(); return 'no throw'; }}
                catch (e) {{ return e.constructor.name + ':' + e.name; }}
            }};
            const results = [];
            {source}
            results.join('|');
            "#
        ),
    )
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

    // A `try`/`catch` is the only thing that can observe a *throw*, and the
    // message names the member, so a page's own logging says which one.
    assert_eq!(
        caught(
            &mut engine,
            r#"
            results.push(attempt(() => fetch('/a', { mode: 'cors' })));
            // A non-object second argument is not a `RequestInit` at all, so it
            // is refused for the other reason.
            results.push(attempt(() => fetch('/a', 1)));
            "#,
        ),
        "TypeError:TypeError|TypeError:TypeError",
        "both are throws, and neither returned a promise"
    );
    assert_eq!(
        eval(
            &mut engine,
            r#"
            try { fetch('/a', { mode: 'cors' }); } catch (e) { e.message; }
            "#
        ),
        "Failed to execute 'fetch': the RequestInit member 'mode' is not implemented",
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
// boundary: the synchronous throw, the ignored undeclared key, and the
// presence test for each member a page can name.

/// The `name: message` pair of the `TypeError` `call` throws, or `"no throw"`.
///
/// `fetch`'s argument conversion is the one half of the call that throws rather
/// than rejects, so a `try`/`catch` is the only thing that can observe it — and
/// what it observes is the message, which is where this engine names the member
/// it refused.
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

/// A non-object second argument is a `TypeError` thrown **before** the promise
/// exists, and the promise is what makes that observable: a call that rejected
/// would have returned one, so the returned value is `undefined`.
///
/// The message is WebIDL's own for a dictionary conversion. It is kept even
/// though the generated conversion produces a different one, because it is the
/// text a browser puts on the same failure and the text a page's error handler
/// reads (`crates/tenta-js-v8/src/host.rs::request_init`).
#[test]
fn an_initializer_that_is_not_an_object_throws_and_returns_nothing() {
    let (mut engine, recorder) = engine();

    assert_eq!(
        threw(&mut engine, "fetch('/x', 3)"),
        "TypeError: Failed to execute 'fetch': the provided value is not of type 'RequestInit'"
    );
    // A call that threw returned no value at all, so there is no promise for a
    // page to attach a rejection to. Asserting `typeof` rather than "it threw"
    // is what makes this non-vacuous: if the engine made the promise first and
    // rejected it — which is what the standard does, by invoking the `Request`
    // constructor inside `fetch`'s steps — this would read `object`.
    assert_eq!(
        eval(
            &mut engine,
            "(function () {
               try { return typeof fetch('/x', 'POST'); } catch (e) { return 'threw'; }
             })()"
        ),
        "threw",
        "the call returned no promise"
    );
    assert_eq!(
        recorder.requests.borrow().len(),
        0,
        "nothing reached the driver"
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

/// Each of the twelve members this engine does not serve is refused *by name*,
/// with the message shape this engine has always used for the refusal.
///
/// The names come from the IDL, so this is the JS-side half of the presence
/// test: a member the page named is a member whether the value is `null`,
/// `false` or an object, because a dictionary member is what `Get` found and
/// not what it returned.
#[test]
fn every_unserved_member_is_refused_by_name() {
    let (mut engine, recorder) = engine();

    for (member, literal) in [
        ("referrer", "''"),
        ("referrerPolicy", "'no-referrer'"),
        ("mode", "'cors'"),
        ("credentials", "'omit'"),
        ("cache", "'no-store'"),
        ("redirect", "'follow'"),
        // The falsy and null-valued spellings are the ones a `Get`-and-truthy
        // test would get wrong.
        ("integrity", "''"),
        ("keepalive", "false"),
        ("signal", "null"),
        ("duplex", "'half'"),
        ("priority", "'low'"),
        ("window", "null"),
    ] {
        assert_eq!(
            threw(
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
