//! Tests for the XHR completion seam, the engine's native dispatch
//! (`tenta_js::xhr`, slice d-4b b-4) and the script-visible `XMLHttpRequest`
//! object (slice d-4b b-5a).
//!
//! XHR's completion is an **event dispatch**, not a promise settlement, so the
//! `fetch` seam (`dom_fetch.rs`) cannot be reused verbatim: a driver is handed a
//! request and the opaque handle bits of an event target, and the answer comes
//! back as an [`XhrCompletion`] the page's turn loop hands to
//! [`Engine::settle_xhr`]. Nothing here opens a socket — the driver below is a
//! recorder — and the b-5a tests below never reach one either: that slice brings
//! the object into existence and implements `UNSENT` and `OPENED`, and nothing
//! is sent until b-5a3.
//!
//! # The property this file exists for
//!
//! Every other `dispatch_at` caller in this tree runs inside a live DOM binding
//! call, so the listener invocation behind it bootstraps its V8 scope with a
//! `v8::CallbackScope` — legal only from a callback. This entry runs from the
//! turn loop with **no V8 callback anywhere on the stack**, where a
//! `CallbackScope` is undefined behaviour rather than a wrong value. So the
//! listener below is registered *from script* (`addEventListener`) and then
//! invoked by an entry that has no script on it at all. If the bootstrap were
//! wrong these tests would crash or corrupt a handle rather than fail an
//! assertion, which is why one of them drives two dispatches through a single
//! entry: a scope opened once per entry and misused per dispatch shows up there.
//!
//! The tests live here rather than under `crates/tenta-page/tests/` because they
//! need the V8 glue, and no V8 test may live there.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use tenta_core::Origin;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js::bindings::host::encode_node;
use tenta_js::engine::PageEngine;
use tenta_js::fetch::{FetchOutcome, FetchRequest, FetchStartError, FetchedResponse};
use tenta_js::xhr::{XhrCompletion, XhrCompletions, XhrDriver};
use tenta_js_v8::Engine;
use url::Url;

/// A document with `<html><body></body></html>` at
/// `http://fixture.test/dir/page.html`, and the body's handle bits.
///
/// The bits are the point: a completion carries an opaque target, and this is
/// what a real XHR object would have handed its driver. The URL is the one
/// `dom_fetch.rs` uses, so the two seams describe the same page.
fn document() -> (Rc<RefCell<Document>>, u64) {
    let mut document =
        Document::with_url(Url::parse("http://fixture.test/dir/page.html").expect("a URL"));
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append <html>");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append <body>");

    let body_bits = encode_node(document.node_ref(body));
    (Rc::new(RefCell::new(document)), body_bits)
}

/// The answer [`Recorder::start`] leaves for every request: a completed `200`.
fn ok_outcome() -> FetchOutcome {
    FetchOutcome::Response(FetchedResponse {
        url: "http://fixture.test/data.json".to_owned(),
        status: 200,
        redirected: false,
        headers: Vec::new(),
        body: None,
    })
}

/// A driver that records what it was handed and leaves the answer in a table the
/// test drains itself.
///
/// No surface calls [`XhrDriver::start`] in this slice — the XHR object that
/// would is b-5 — so the tests here call it directly, which is what makes them a
/// statement about the *seam* rather than about a surface that does not exist
/// yet. `fail` elects the outcome every request is answered with, because the
/// only decision this slice makes about an outcome is which event it becomes.
#[derive(Default)]
struct Recorder {
    /// Every `(request, target)` the driver was started with, in order.
    starts: RefCell<Vec<(FetchRequest, u64)>>,
    /// The table a completion is pushed into, exactly as a real driver would.
    table: XhrCompletions,
    /// Answer with a failure instead of a response.
    fail: Cell<bool>,
}

impl XhrDriver for Recorder {
    fn start(&self, request: FetchRequest, target: u64) -> Result<(), FetchStartError> {
        self.starts.borrow_mut().push((request, target));
        let outcome = match self.fail.replace(false) {
            true => FetchOutcome::Failed("the request failed".to_owned()),
            false => ok_outcome(),
        };
        self.table.push(target, outcome);
        Ok(())
    }
}

/// Runs `source` and returns its completion value, with no checkpoint.
fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("`{source}` should evaluate: {error}"))
}

/// A completion that dispatches `outcome` at `target`.
fn completion(target: u64, outcome: FetchOutcome) -> XhrCompletion {
    XhrCompletion { target, outcome }
}

/// The request a page would have made for `/data.json` — the one the page above
/// has a URL for, so the driver's inputs are the ones a real page produces.
fn request() -> FetchRequest {
    FetchRequest::get(
        Url::parse("http://fixture.test/data.json").expect("a URL"),
        Url::parse("http://fixture.test/dir/page.html").expect("a URL"),
        Origin::Tracked,
    )
}

// ---------------------------------------------------------------------------
// The seam itself
// ---------------------------------------------------------------------------

#[test]
fn a_request_reaches_the_driver_verbatim_and_its_completion_keeps_the_target() {
    // The request type is `fetch`'s, reused verbatim rather than copied, because
    // the driver behind this trait is the same `tenta-net` code path (b-6). This
    // pins that the page's request crosses the boundary unchanged and that the
    // answer carries the target it was started with, since that target is the
    // only thing that says where the event goes.
    let (_document, body) = document();
    let recorder = Rc::new(Recorder::default());
    let request = request();

    recorder
        .start(request.clone(), body)
        .expect("the recorder never refuses");

    assert_eq!(
        recorder.starts.borrow().as_slice(),
        &[(request, body)],
        "the driver was handed the request and the target, unchanged"
    );
    assert_eq!(
        recorder.table.take_all(),
        vec![completion(body, ok_outcome())],
        "and its answer is in the table against the same target"
    );
}

#[test]
fn the_engine_holds_the_driver_it_was_given_and_releases_it_on_drop() {
    // The slot the driver lands in is write-only in this slice — nothing calls
    // it back (b-5) — so an install that silently dropped the driver would be
    // invisible to every other test here. The strong count is what makes it
    // observable: one handle is the test's and the second has to be the
    // engine's.
    let (document, _body) = document();
    let mut engine = Engine::with_document(document);
    let recorder = Rc::new(Recorder::default());
    assert_eq!(Rc::strong_count(&recorder), 1);

    // The cast is what makes the driver a trait object; `Rc::clone` alone would
    // pick the inherent `Rc<Recorder>` method and refuse to coerce.
    engine.set_xhr_driver(Rc::clone(&recorder) as Rc<dyn XhrDriver>);
    assert_eq!(
        Rc::strong_count(&recorder),
        2,
        "the engine kept the driver it was handed"
    );

    drop(engine);
    assert_eq!(
        Rc::strong_count(&recorder),
        1,
        "and released it with the engine rather than leaking the cycle"
    );
}

// ---------------------------------------------------------------------------
// The native dispatch
// ---------------------------------------------------------------------------

#[test]
fn a_completion_drained_by_the_engine_reaches_a_listener_registered_from_script() {
    let (document, body) = document();
    let mut engine = Engine::with_document(document);
    eval(
        &mut engine,
        r#"
        globalThis.seen = 'nothing';
        document.body.addEventListener('load', event => {
            globalThis.seen = [
                event.type,
                event.isTrusted,
                event.target === document.body,
                event instanceof ProgressEvent,
                event instanceof Event
            ].join('|');
        });
        "#,
    );
    assert_eq!(
        eval(&mut engine, "globalThis.seen"),
        "nothing",
        "the script only registered the listener"
    );

    // The entry the page's turn loop calls, with no script and no V8 callback
    // anywhere on the stack.
    engine.settle_xhr(vec![completion(body, ok_outcome())]);

    assert_eq!(
        eval(&mut engine, "globalThis.seen"),
        "load|true|true|true|true",
        "the listener ran, at the body, with a `load` the user agent fired"
    );
}

#[test]
fn one_drain_delivers_every_completion_in_its_batch() {
    // Two dispatches through one entry. A scope that the entry opened once and
    // the dispatcher then reused per dispatch would corrupt handles here rather
    // than fail an assertion, which is the reason this test exists beside the
    // single-completion one.
    let (document, body) = document();
    let mut engine = Engine::with_document(document);
    eval(
        &mut engine,
        r#"
        globalThis.log = [];
        document.body.addEventListener('load', () => log.push('load'));
        document.body.addEventListener('error', () => log.push('error'));
        "#,
    );

    engine.settle_xhr(vec![
        completion(body, ok_outcome()),
        completion(body, FetchOutcome::Failed("gone".to_owned())),
    ]);

    assert_eq!(
        eval(&mut engine, "globalThis.log.join(',')"),
        "load,error",
        "both completions of the batch were dispatched"
    );
}

#[test]
fn a_failed_outcome_dispatches_an_error_rather_than_a_load() {
    // The outcome-to-event mapping is made in exactly one place, so b-5/b-6 can
    // rely on a failure never arriving as a `load`. It is driven through the
    // recorder rather than handed over directly, which is also what pins the
    // push-then-drain half of the driver contract for a failure.
    let (document, body) = document();
    let mut engine = Engine::with_document(document);
    let recorder = Rc::new(Recorder::default());
    engine.set_xhr_driver(Rc::clone(&recorder) as Rc<dyn XhrDriver>);
    eval(
        &mut engine,
        r#"
        globalThis.log = [];
        document.body.addEventListener('load', () => log.push('load'));
        document.body.addEventListener('error', event => {
            globalThis.log.push('error|' + event.type + '|' + (event instanceof ProgressEvent));
        });
        "#,
    );

    recorder.fail.set(true);
    recorder
        .start(request(), body)
        .expect("the recorder never refuses");
    engine.settle_xhr(recorder.table.take_all());

    assert_eq!(
        eval(&mut engine, "globalThis.log.join(',')"),
        "error|error|true",
        "a failure is an `error` event, and a `ProgressEvent` like the others"
    );
}

#[test]
fn a_completion_at_the_window_dispatches_at_the_window() {
    // `target` is opaque bits, so the engine has to accept every kind of event
    // target the bits can name — and the window is the one target that is not a
    // node, so it has its own arm of the decode.
    let (document, _body) = document();
    let mut engine = Engine::with_document(document);
    eval(
        &mut engine,
        r#"
        globalThis.seen = 'nothing';
        addEventListener('load', event => {
            globalThis.seen = [event.target === window, event.type].join('|');
        });
        "#,
    );

    engine.settle_xhr(vec![completion(
        tenta_js::bindings::host::encode_window(),
        ok_outcome(),
    )]);

    assert_eq!(eval(&mut engine, "globalThis.seen"), "true|load");
}

#[test]
fn a_target_that_is_not_an_event_target_is_skipped_without_taking_the_batch_with_it() {
    // A handle the engine cannot dispatch at is a driver's mistake, not the
    // page's, and the completions beside it did nothing wrong. `u64::MAX` has a
    // tag no handle uses, so the engine refuses it before building anything.
    let (document, body) = document();
    let mut engine = Engine::with_document(document);
    eval(
        &mut engine,
        r#"
        globalThis.log = [];
        document.body.addEventListener('load', () => log.push('load'));
        "#,
    );

    engine.settle_xhr(vec![
        completion(u64::MAX, ok_outcome()),
        completion(body, ok_outcome()),
    ]);

    assert_eq!(
        eval(&mut engine, "globalThis.log.join(',')"),
        "load",
        "the undispatchable target was skipped, not the rest of the batch"
    );
}

#[test]
fn dropping_an_engine_after_a_native_dispatch_is_clean() {
    // The drain leaves a script activation behind it (`HostState::enter_script`
    // records the microtask debt the listener's own work owes), and the event
    // record it built may still have a wrapper script kept. Both have to be
    // released while the isolate is alive, so this drop neither leaks nor
    // faults.
    let (document, body) = document();
    let mut engine = Engine::with_document(document);
    eval(
        &mut engine,
        r#"
        globalThis.kept = null;
        document.body.addEventListener('load', event => { globalThis.kept = event; });
        "#,
    );
    engine.settle_xhr(vec![completion(body, ok_outcome())]);
    assert_eq!(
        eval(&mut engine, "globalThis.kept.type"),
        "load",
        "the listener kept the event, so the record outlives the dispatch"
    );
    drop(engine);
}

// ---------------------------------------------------------------------------
// The script-visible surface (slice d-4b b-5a)
// ---------------------------------------------------------------------------

/// Runs `check` on a fresh engine over the page `document()` builds, which is the
/// one with a URL: `open()` resolves its `url` argument against it.
fn with_page(check: impl FnOnce(&mut Engine)) {
    let (document, _body) = document();
    let mut engine = Engine::with_document(document);
    check(&mut engine);
}

/// `new XMLHttpRequest()` is the object itself, and the interface's two-level
/// prototype chain is intact.
///
/// This is the riskiest part of the slice: `XMLHttpRequest` is the first
/// interface in this product whose chain is three deep
/// (`XMLHttpRequest -> XMLHttpRequestEventTarget -> EventTarget`), and a flat
/// install would leave `instanceof` and `Object.getPrototypeOf` disagreeing with
/// the IDL while every other test here still passed.
#[test]
fn a_new_request_sits_on_the_two_level_event_target_prototype_chain() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                [
                    typeof XMLHttpRequest,
                    typeof XMLHttpRequestEventTarget,
                    xhr instanceof XMLHttpRequest,
                    xhr instanceof XMLHttpRequestEventTarget,
                    xhr instanceof EventTarget,
                    Object.getPrototypeOf(XMLHttpRequest.prototype) === XMLHttpRequestEventTarget.prototype,
                    Object.getPrototypeOf(XMLHttpRequestEventTarget.prototype) === EventTarget.prototype,
                    xhr.constructor.name,
                    Object.prototype.toString.call(xhr),
                ].join('|');
                "#
            ),
            "function|function|true|true|true|true|true|XMLHttpRequest|[object XMLHttpRequest]",
            "the chain is flat, or a name resolves to the wrong interface"
        );
    });
}

/// A fresh request is `UNSENT`, and the five state constants are readable on the
/// instance as well as on the interface.
#[test]
fn a_fresh_request_reports_unsent_and_all_five_state_constants() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                [
                    xhr.readyState,
                    xhr.UNSENT, xhr.OPENED, xhr.HEADERS_RECEIVED, xhr.LOADING, xhr.DONE,
                    XMLHttpRequest.UNSENT, XMLHttpRequest.OPENED,
                    XMLHttpRequest.HEADERS_RECEIVED, XMLHttpRequest.LOADING, XMLHttpRequest.DONE,
                    xhr.readyState === xhr.UNSENT,
                ].join('|');
                "#
            ),
            "0|0|1|2|3|4|0|1|2|3|4|true",
            "the constants are installed on the prototype and the interface"
        );
    });
}

/// `open()` moves the state to `OPENED` and fires exactly one
/// `readystatechange`; a second `open()` on the already-opened object fires
/// none.
#[test]
fn open_opens_the_request_and_fires_one_readystatechange_only_once() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                let count = 0;
                xhr.addEventListener('readystatechange', () => count++);
                xhr.open('GET', '/first');
                const afterFirst = count;
                xhr.open('GET', '/second');
                [xhr.readyState, afterFirst, count].join('|');
                "#
            ),
            "1|1|1",
            "the second `open()` re-opened an open request, which fires nothing"
        );
    });
}

/// The `readystatechange` a UA fires from `open()` is a real dispatch: a
/// listener and an `onreadystatechange` handler both run synchronously inside
/// the call, the event is trusted, and `event.target` is the same object the
/// page holds.
///
/// The handler attribute is the half that pins the event *name*: the host names
/// a handler's listener by stripping its `on` prefix, so the handler only
/// receives the event if `open()` fires the type that rule produces.
#[test]
fn open_fires_a_trusted_readystatechange_at_the_request_itself() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                const describe = event => [
                    event.type, event.isTrusted,
                    event.target === xhr, event.currentTarget === xhr,
                    event instanceof Event, event instanceof ProgressEvent,
                ].join('|');
                globalThis.fromListener = 'nothing';
                globalThis.fromHandler = 'nothing';
                xhr.addEventListener('readystatechange', event => {
                    globalThis.fromListener = describe(event);
                });
                xhr.onreadystatechange = function (event) {
                    globalThis.fromHandler = describe(event);
                };
                xhr.open('GET', '/x');
                [globalThis.fromListener, globalThis.fromHandler].join('#');
                "#
            ),
            "readystatechange|true|true|true|true|false\
             #readystatechange|true|true|true|true|false",
            "the completion is dispatched at the request, and is a plain `Event`"
        );
    });
}

/// `open(method, url, false)` is refused, and the refusal leaves the object
/// unsent.
///
/// The standard's synchronous request is the one deviation this slice adds: the
/// network driver is asynchronous all the way down, so `false` cannot be
/// honoured. The state check is what pins that the refusal happens *before* the
/// state transition rather than halfway through it.
#[test]
fn a_synchronous_open_is_refused_and_leaves_the_request_unsent() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                let outcome;
                try { xhr.open('GET', '/x', false); outcome = 'no throw'; }
                catch (error) { outcome = error.name; }
                [outcome, xhr.readyState, xhr.UNSENT].join('|');
                "#
            ),
            "NotSupportedError|0|0",
            "a synchronous request must be refused, not silently made asynchronous"
        );
    });
}

/// The method is validated before it is stored: a byte outside HTTP's `token`
/// production is a `SyntaxError`, and a forbidden method is a `SecurityError`
/// whatever case it is spelled in.
#[test]
fn open_validates_the_method_token_and_the_forbidden_methods() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                const attempt = method => {
                    try { xhr.open(method, '/x'); return 'ok'; }
                    catch (error) { return error.name; }
                };
                [
                    attempt('GET'), attempt('get'), attempt('BAD METHOD'), attempt(''),
                    attempt('TRACE'), attempt('trace'), attempt('CONNECT'), attempt('TRACK'),
                    xhr.readyState,
                ].join('|');
                "#
            ),
            "ok|ok|SyntaxError|SyntaxError|SecurityError|SecurityError|SecurityError|SecurityError|1",
            "a method must be a token, and the three forbidden names are case-insensitive"
        );
    });
}

/// An `url` that cannot be parsed is a `SyntaxError`, and nothing is opened.
#[test]
fn open_refuses_a_url_that_cannot_be_parsed() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                let outcome;
                try { xhr.open('GET', 'http://'); outcome = 'no throw'; }
                catch (error) { outcome = error.name; }
                [outcome, xhr.readyState].join('|');
                "#
            ),
            "SyntaxError|0",
            "an unparseable URL must not leave a half-opened request behind"
        );
    });
}

/// A non-null `username` or `password` is refused rather than stored, and
/// passing `null` for both is not a refusal.
///
/// This is one of the slice's five narrowings. Accepting the pair and dropping it
/// would be the silent version of the same lie `withCredentials = true` refuses
/// to tell: the page would believe a credential travelled when none did. The
/// state check is what pins that the refusal happens before the transition.
#[test]
fn open_refuses_credentials_and_leaves_the_request_untouched() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                const attempt = args => {
                    try { xhr.open(...args); return 'ok'; }
                    catch (error) { return error.name; }
                };
                const refused = [
                    attempt(['GET', '/x', true, 'user', 'pass']),
                    attempt(['GET', '/x', true, 'user']),
                    attempt(['GET', '/x', true, null, 'pass']),
                    attempt(['GET', '/x', true, '']),
                ];
                const state = xhr.readyState;
                const accepted = attempt(['GET', '/x', true, null, null]);
                [refused.join(','), state, accepted, xhr.readyState].join('|');
                "#
            ),
            "NotSupportedError,NotSupportedError,NotSupportedError,NotSupportedError|0|ok|1",
            "credentials must be refused, and an explicit `null` must not be"
        );
    });
}

/// Assigning a non-object to an `on*` attribute reads back as `null`, which is
/// WebIDL §3.4.8 `[LegacyTreatNonObjectAsNull]` reached end to end: the IDL, the
/// generated setter glue and the host's event handler map.
///
/// This is the runtime half of the generator slice that came before this one,
/// which could only assert the emitted text.
#[test]
fn a_non_object_event_handler_reads_back_as_null() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                const never = xhr.onload === null;
                const attempts = [5, 'text', true, undefined, Symbol('s')].map(value => {
                    xhr.onload = value;
                    return xhr.onload;
                });
                [never, ...attempts.map(handler => handler === null)].join('|');
                "#
            ),
            "true|true|true|true|true|true",
            "a non-object assignment must deactivate the handler rather than store it"
        );
    });
}

/// An *object* assigned to an `on*` attribute is stored as itself, callable or
/// not, and reads back by identity.
#[test]
fn an_object_event_handler_is_stored_by_identity() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                const plain = {};
                xhr.onload = plain;
                const keptPlain = xhr.onload === plain;
                const fn = function () {};
                xhr.onload = fn;
                const keptFunction = xhr.onload === fn;
                xhr.onload = null;
                [keptPlain, keptFunction, xhr.onload === null].join('|');
                "#
            ),
            "true|true|true",
            "the loose rule stores any object, and deactivation still clears it"
        );
    });
}

/// A handler and a listener for the same function are two independent
/// registrations, whichever direction they are matched from.
///
/// HTML §8.1.8.1 registers a handler's listener with an internal callback the
/// page cannot name, so `addEventListener('load', fn)` followed by
/// `xhr.onload = fn` leaves two listeners — the function fires twice — and
/// `removeEventListener('load', fn)` takes only the page's own. That
/// handler-blindness is what keeps a page from colliding with its own handler.
#[test]
fn a_handler_and_a_listener_for_one_function_stay_two_registrations() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                globalThis.log = [];
                const xhr = new XMLHttpRequest();
                const fn = function () { log.push('fired'); };
                xhr.addEventListener('load', fn);
                xhr.onload = fn;
                const kept = xhr.onload === fn;
                xhr.dispatchEvent(new Event('load'));
                const bothFired = log.length;
                xhr.removeEventListener('load', fn);
                log.length = 0;
                xhr.dispatchEvent(new Event('load'));
                [kept, bothFired, log.length].join('|');
                "#
            ),
            "true|2|1",
            "`removeEventListener` took the page's listener and left the handler's"
        );
    });
}

/// Every refusal the surface makes is synchronous and named, and the two values
/// it can serve are stored.
#[test]
fn the_refusing_setters_accept_only_what_this_engine_can_serve() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                const attempt = f => { try { f(); return 'ok'; } catch (error) { return error.name; } };
                const results = [
                    attempt(() => { xhr.responseType = 'json'; }),
                    attempt(() => { xhr.responseType = 'text'; }),
                    attempt(() => { xhr.responseType = 'bogus'; }),
                    attempt(() => { xhr.withCredentials = true; }),
                    attempt(() => { xhr.withCredentials = false; }),
                    attempt(() => { xhr.timeout = 1; }),
                    attempt(() => { xhr.timeout = 0; }),
                    attempt(() => { xhr.overrideMimeType('text/plain'); }),
                ];
                [results.join(','), xhr.responseType, xhr.withCredentials, xhr.timeout].join('|');
                "#
            ),
            "NotSupportedError,ok,TypeError,NotSupportedError,ok,NotSupportedError,ok,\
             NotSupportedError|text|false|0",
            "a value the enumeration does not name is a `TypeError`, not a `NotSupportedError`"
        );
    });
}

/// Before any response, the read-only members answer what the standard says
/// they answer in the `opened` state — which is not a stub but the spec's own
/// rule for a state this slice can reach.
#[test]
fn an_opened_request_without_a_response_reports_the_standard_defaults() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                xhr.open('GET', '/x');
                [
                    xhr.status,
                    xhr.statusText,
                    xhr.responseText,
                    xhr.getResponseHeader('content-type') === null,
                    xhr.getAllResponseHeaders(),
                    xhr.responseType,
                ].join('|');
                "#
            ),
            "0|||true||",
            "the unsent-and-opened defaults are 0, the empty string and null"
        );
    });
}

/// Two requests get two records and two listener tables: a dispatch at one never
/// reaches the other.
#[test]
fn two_requests_have_independent_listener_tables() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const first = new XMLHttpRequest();
                const second = new XMLHttpRequest();
                globalThis.log = [];
                first.addEventListener('load', () => log.push('first'));
                second.addEventListener('load', () => log.push('second'));
                second.dispatchEvent(new Event('load'));
                const afterSecond = log.join(',');
                first.dispatchEvent(new Event('load'));
                [first !== second, afterSecond, log.join(',')].join('|');
                "#
            ),
            "true|second|second,first",
            "the two objects share a record, so one's listener fires for the other"
        );
    });
}
