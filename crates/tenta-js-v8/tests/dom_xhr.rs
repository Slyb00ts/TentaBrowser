//! Tests for the XHR completion seam, the engine's native dispatch
//! (`tenta_js::xhr`, slice d-4b b-4), the script-visible `XMLHttpRequest`
//! object (slice d-4b b-5a), that object's response state machine and response
//! members (slice d-4b b-5a2), its request half — `send()`,
//! `setRequestHeader()` and the driver seam (slice d-4b b-5a3) — and the request
//! body `send()` accepts (slice d-4b b-9).
//!
//! XHR's completion is an **event dispatch**, not a promise settlement, so the
//! `fetch` seam (`dom_fetch.rs`) cannot be reused verbatim: a driver is handed a
//! request and the opaque handle bits of an event target, and the answer comes
//! back as an [`XhrCompletion`] the page's turn loop hands to
//! [`Engine::settle_xhr`]. Nothing here opens a socket: the driver below is a
//! recorder, so a `send()` reaches it and stops there.
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
//! The response tests rest on the same entry, reached the other way round: an
//! object the *script* created is handed a completion the *rust* side built, so
//! the test has to name that object's handle bits
//! ([`Engine::global_handle_bits`]) — the one thing a page cannot do for itself.
//!
//! The request tests drive the page's own `send()`, and the recorder refuses to
//! answer unless a test asks it to (`Recorder::hold`), so the object under test
//! is left in the state `send()` put it in rather than in the state a completion
//! would have moved it to.
//!
//! The tests live here rather than under `crates/tenta-page/tests/` because they
//! need the V8 glue, and no V8 test may live there.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use tenta_core::Origin;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js::bindings::XMLHttpRequest;
use tenta_js::bindings::host::encode_node;
use tenta_js::engine::PageEngine;
use tenta_js::fetch::{FetchOutcome, FetchRequest, FetchStartError, FetchedResponse};
use tenta_js::xhr::{XhrCompletion, XhrCompletions, XhrDriver, XhrHandle};
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

    let body_bits = encode_node(0, document.node_ref(body));
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
/// The XHR object itself reaches this through `send()` (slice b-5a3), so a
/// request test drives the page and then reads `starts`; the seam tests call
/// [`XhrDriver::start`] directly, which is what makes them a statement about the
/// boundary rather than about a surface. `fail` elects the outcome every request
/// is answered with, and `hold` withholds an answer altogether, which is what
/// leaves an object in the state `send()` put it in.
#[derive(Default)]
struct Recorder {
    /// Every `(request, handle)` the driver was started with, in order.
    ///
    /// The handle is echoed back with the answer, exactly as a real driver does
    /// — both halves of it. The `request` half is what a test uses to deliver a
    /// completion for a request the object has since abandoned without having to
    /// know the record's own counter.
    starts: RefCell<Vec<(FetchRequest, XhrHandle)>>,
    /// Every handle an `abort()` named, in order.
    ///
    /// The call is recorded whether or not it ended anything: the engine's
    /// `end_xhr_request` goes through this seam for every `open()` and every
    /// `abort()`, including the ones with no request in flight, so a test that
    /// wants to see the termination happen has to see the calls that ended
    /// nothing too.
    aborts: RefCell<Vec<XhrHandle>>,
    /// The table a completion is pushed into, exactly as a real driver would.
    table: XhrCompletions,
    /// Answer with a failure instead of a response.
    fail: Cell<bool>,
    /// Record the start and push nothing, so the request never completes.
    hold: Cell<bool>,
}

impl XhrDriver for Recorder {
    fn start(&self, request: FetchRequest, handle: XhrHandle) -> Result<(), FetchStartError> {
        self.starts.borrow_mut().push((request, handle));
        if self.hold.get() {
            return Ok(());
        }
        let outcome = match self.fail.replace(false) {
            true => FetchOutcome::Failed("the request failed".to_owned()),
            false => ok_outcome(),
        };
        self.table.push(handle, outcome);
        Ok(())
    }

    /// Records the name and ends nothing — **the table is left as it is**.
    ///
    /// That is the point of the racing-completion test: a driver which pushed an
    /// answer a moment before the abort reached it still has that answer in the
    /// table, and the engine's request id is what refuses it
    /// ([`deliver_xhr_completion`](tenta_js::bindings::host::deliver_xhr_completion)).
    /// A recorder that removed the completion here would be testing its own
    /// rollback rather than the seam's contract.
    ///
    /// The answer is `true` when a request was started under this handle and no
    /// `abort()` has named it yet. That is deliberately the *looser* of the two
    /// answers a real driver can give: [`start`](Self::start) answers
    /// synchronously, so this recorder has no notion of a request that has
    /// finished, while a real driver reports `false` once its task is done. The
    /// caller drops the answer either way — `abort()` is void and its steps raise
    /// nothing — so the difference cannot reach a page, and the tests that use it
    /// assert on *which handles were named* instead.
    fn abort(&self, handle: XhrHandle) -> bool {
        let started = self
            .starts
            .borrow()
            .iter()
            .any(|(_, started)| *started == handle);
        let mut aborts = self.aborts.borrow_mut();
        let already = aborts.contains(&handle);
        aborts.push(handle);
        started && !already
    }
}

/// Runs `source` and returns its completion value, with no checkpoint.
fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("`{source}` should evaluate: {error}"))
}

/// The request id of an object that has been `open()`ed and never sent.
///
/// `open()` bumps the record's id once (its step 10), so an object that has only
/// been opened holds `1`. That is the state of every object the response-half
/// tests deliver at — they never install a driver, so nothing is ever sent — and
/// a `send()` would bump it again, which is what makes the re-open test's
/// abandoned completion carry a *different* id.
const OPENED_ONLY: u64 = 1;

/// A completion that dispatches `outcome` at `target`, for the request an
/// `open()`ed-and-never-sent object is waiting for.
fn completion(target: u64, outcome: FetchOutcome) -> XhrCompletion {
    hand(target, OPENED_ONLY, outcome)
}

/// A completion for one specific request of an object, which is what a test needs
/// once an object has more than one in flight.
fn hand(target: u64, request: u64, outcome: FetchOutcome) -> XhrCompletion {
    XhrCompletion {
        handle: XhrHandle { target, request },
        outcome,
    }
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

/// The opaque handle bits of the global expression `expression`, which is what a
/// driver is handed as a completion's `target` for that object.
///
/// A page cannot produce these bits for itself — they are the wrapper world's
/// private identity for an object — so a test that has to deliver a *native*
/// completion at an object it created from script names it this way. The
/// expression is a *script* reference on purpose: an API read of the global
/// proxy does not reach the global object's own properties.
fn target_bits(engine: &mut Engine, expression: &str) -> u64 {
    engine
        .global_handle_bits::<XMLHttpRequest>(expression)
        .unwrap_or_else(|| panic!("`{expression}` should name a live interface object"))
        .bits()
}

/// Delivers one completion at the global `xhr`, as the page's turn loop would,
/// for the request an `open()`ed-and-never-sent object is waiting for
/// ([`OPENED_ONLY`]).
fn deliver(engine: &mut Engine, outcome: FetchOutcome) {
    let target = target_bits(engine, "xhr");
    engine.settle_xhr(vec![completion(target, outcome)]);
}

/// Delivers one completion for request `request` of the object a driver was
/// handed, which is what a test that sent a request needs: the id is the
/// driver's, not a constant.
fn deliver_at(engine: &mut Engine, handle: XhrHandle, outcome: FetchOutcome) {
    engine.settle_xhr(vec![hand(handle.target, handle.request, outcome)]);
}

/// A completed response carrying `status`, the driver's `headers` in the order
/// it reported them, and `body`.
fn response(status: u16, headers: &[(&str, &str)], body: Option<&[u8]>) -> FetchOutcome {
    FetchOutcome::Response(FetchedResponse {
        url: "http://fixture.test/data.json".to_owned(),
        status,
        redirected: false,
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.as_bytes().to_vec()))
            .collect(),
        body: body.map(<[u8]>::to_vec),
    })
}

/// Registers on the global `xhr` one listener per event of the response machine
/// which appends `type@readyState:trusted:isProgressEvent:lengthComputable,loaded,total`
/// to the global `log`.
///
/// The ready state is read from the *object* rather than from the event, because
/// the two can disagree in the one way worth pinning: a listener runs after the
/// machine has already moved the state, so `progress` of a body is observed at
/// `LOADING` and the end-of-body one still is, while `load` is observed at
/// `DONE`.
///
/// `timeout` and `abort` are in the list even though the *response* machine
/// never fires either: they are the two events the *request error steps* fire
/// under another name (slice d-4b b-7), and a test that watches for them has to
/// have the listener already registered.
fn log_every_event(engine: &mut Engine) {
    eval(
        engine,
        r#"
        globalThis.log = [];
        for (const name of ['readystatechange', 'progress', 'load', 'error', 'timeout', 'abort', 'loadend']) {
            xhr.addEventListener(name, event => {
                const progress = event instanceof ProgressEvent;
                log.push([
                    event.type + '@' + xhr.readyState,
                    event.isTrusted,
                    progress,
                    progress ? [event.lengthComputable, event.loaded, event.total].join(',') : '',
                ].join(':'));
            });
        }
        "#,
    );
}

/// Creates the global `xhr`, opens it on `/x`, and logs every event the response
/// machine fires into the global `log`.
fn opened_xhr(engine: &mut Engine) {
    eval(
        engine,
        r#"
        globalThis.xhr = new XMLHttpRequest();
        xhr.open('GET', '/x');
        "#,
    );
    log_every_event(engine);
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

    let handle = XhrHandle {
        target: body,
        request: 5,
    };
    recorder
        .start(request.clone(), handle)
        .expect("the recorder never refuses");

    assert_eq!(
        recorder.starts.borrow().as_slice(),
        &[(request, handle)],
        "the driver was handed the request and the handle, unchanged"
    );
    assert_eq!(
        recorder.table.take_all(),
        vec![hand(body, 5, ok_outcome())],
        "and its answer is in the table against the same handle, both halves of it"
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
        .start(
            request(),
            XhrHandle {
                target: body,
                request: OPENED_ONLY,
            },
        )
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
/// This is one of the slice's four narrowings. Accepting the pair and dropping it
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

/// Every refusal the surface makes is synchronous and named, and the values it
/// can serve are stored.
///
/// `overrideMimeType` is in the list because it now succeeds where the state
/// allows it: it is not a narrowing any more, only the standard's own
/// `InvalidStateError` on a state that is loading or done. `timeout` is *not* in
/// the list any more either, for the opposite reason: it refuses nothing at all
/// in this engine, and [`the_timeout_setter_stores_any_value_and_arms_nothing`]
/// is where that is asserted.
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
                    attempt(() => { xhr.overrideMimeType('text/plain'); }),
                ];
                [results.join(','), xhr.responseType, xhr.withCredentials].join('|');
                "#
            ),
            "NotSupportedError,ok,TypeError,NotSupportedError,ok,ok|text|false",
            "a value the enumeration does not name is a `TypeError`, not a `NotSupportedError`"
        );
    });
}

/// `timeout` is a plain stored `unsigned long`: the setter refuses nothing and
/// throws nothing, and the value it stores is the one the getter reads back.
///
/// The standard's setter can throw exactly one thing — an `InvalidAccessError`
/// for a synchronous request on a `Window`, step 2 of
/// <https://xhr.spec.whatwg.org/#the-timeout-attribute> — and this engine cannot
/// reach it, because `open()` refuses `async = false` for every case. So there is
/// no value to narrow and nothing to refuse, which is what makes the member
/// `attribute unsigned long timeout;` with no `[Throws]` in
/// `idl/xhr/XMLHttpRequest.webidl`.
///
/// Storing is all this test can see: the timer the value describes is armed by
/// `send()` and by nothing else, which is the [next
/// test](a_request_that_outlives_its_timeout_is_reported_as_a_timeout).
#[test]
fn the_timeout_setter_stores_any_value_and_arms_nothing() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                const xhr = new XMLHttpRequest();
                const attempt = f => { try { f(); return 'ok'; } catch (error) { return error.name; } };
                const stored = [
                    attempt(() => { xhr.timeout = 0; }),
                    xhr.timeout,
                    attempt(() => { xhr.timeout = 1234; }),
                    xhr.timeout,
                    // The WebIDL `unsigned long` conversion: a negative number
                    // wraps and a fraction truncates, and neither throws.
                    attempt(() => { xhr.timeout = -1; }),
                    xhr.timeout,
                    attempt(() => { xhr.timeout = 1.9; }),
                    xhr.timeout,
                    // The getter ignores its argument and cannot throw either.
                    attempt(() => xhr.timeout),
                ];
                stored.join('|');
                "#
            ),
            "ok|0|ok|1234|ok|4294967295|ok|1|ok",
            "the setter stores what the WebIDL conversion produces, without refusing"
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

// ---------------------------------------------------------------------------
// The response half (slice d-4b b-5a2)
// ---------------------------------------------------------------------------

/// A body runs the whole machine: `HEADERS_RECEIVED`, `LOADING`, `DONE`, with
/// two `progress` events and the two events that close a request.
///
/// The two `progress` events are the standard's own sequence — one per chunk and
/// one from *handle response end-of-body* — and both land in this one turn,
/// because the completion arrives whole. Chromium fires the chunk's alone; the
/// engine follows the standard.
#[test]
fn a_response_with_a_body_runs_the_states_and_their_events_in_order() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(
            engine,
            response(200, &[("Content-Length", "5")], Some(b"hello")),
        );

        assert_eq!(
            eval(engine, "log.join('\\n')"),
            "readystatechange@2:true:false:\n\
             readystatechange@3:true:false:\n\
             progress@3:true:true:true,5,5\n\
             progress@3:true:true:true,5,5\n\
             readystatechange@4:true:false:\n\
             load@4:true:true:true,5,5\n\
             loadend@4:true:true:true,5,5",
            "a body must reach LOADING and fire the standard's seven events"
        );
    });
}

/// A response whose body is null goes from `HEADERS_RECEIVED` straight to
/// `DONE`: no `LOADING`, and one `progress` rather than two.
///
/// A null-body status carries no body whatever the driver reported, and the
/// machine keys on the bytes it was handed rather than on the status.
#[test]
fn a_null_body_skips_loading_and_fires_one_progress() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(engine, response(204, &[], None));

        assert_eq!(
            eval(engine, "log.join('\\n')"),
            "readystatechange@2:true:false:\n\
             progress@2:true:true:false,0,0\n\
             readystatechange@4:true:false:\n\
             load@4:true:true:false,0,0\n\
             loadend@4:true:true:false,0,0",
            "a null body must not pass through LOADING"
        );
    });
}

/// A failure goes to `DONE` and fires `error` and `loadend` — never `progress`,
/// and never a `LOADING` — with every response member already reading the
/// network error from inside the first listener.
#[test]
fn a_failed_request_reaches_done_with_error_and_no_progress() {
    with_page(|engine| {
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/x');
            globalThis.log = [];
            for (const name of ['readystatechange', 'progress', 'load', 'error', 'loadend']) {
                xhr.addEventListener(name, event => {
                    log.push([
                        event.type,
                        xhr.readyState,
                        xhr.status,
                        xhr.responseText.length,
                        xhr.getResponseHeader('content-type') === null,
                    ].join(':'));
                });
            }
            "#,
        );
        deliver(
            engine,
            FetchOutcome::Failed("the request failed".to_owned()),
        );

        assert_eq!(
            eval(engine, "log.join('\\n')"),
            "readystatechange:4:0:0:true\nerror:4:0:0:true\nloadend:4:0:0:true",
            "a failed request must fire neither `progress` nor a `load`"
        );
    });
}

/// `lengthComputable` and `total` come from `Content-Length`, and `loaded` from
/// the bytes handed over — the header here declares 9 and the body is 5, which
/// is what tells the two readings apart.
#[test]
fn a_content_length_makes_the_events_length_computable() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(
            engine,
            response(200, &[("Content-Length", "9")], Some(b"hello")),
        );

        assert_eq!(
            eval(engine, "log.join('\\n')"),
            "readystatechange@2:true:false:\n\
             readystatechange@3:true:false:\n\
             progress@3:true:true:true,5,9\n\
             progress@3:true:true:true,5,9\n\
             readystatechange@4:true:false:\n\
             load@4:true:true:true,5,9\n\
             loadend@4:true:true:true,5,9",
            "`total` is the header's length and `loaded` the body's own"
        );
    });
}

/// A declared length of zero is a length, and the standard's own rule makes the
/// events uncomputable for it: *extract a length* answers 0 and the progress
/// event sets `lengthComputable` and `total` only "if length is not 0"
/// (<https://xhr.spec.whatwg.org/#firing-events-using-the-progressevent-interface>).
///
/// So this is the case that tells the two readings apart, and the only one that
/// does: a response that declares no bytes reads exactly as one that declares
/// nothing does, where "computable because the header is present" would report
/// `true,0,0` here and is what the assertion below is written against.
#[test]
fn a_zero_content_length_is_not_length_computable() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(engine, response(200, &[("Content-Length", "0")], None));

        assert_eq!(
            eval(engine, "log.join('\\n')"),
            "readystatechange@2:true:false:\n\
             progress@2:true:true:false,0,0\n\
             readystatechange@4:true:false:\n\
             load@4:true:true:false,0,0\n\
             loadend@4:true:true:false,0,0",
            "a `Content-Length: 0` is a length of zero, not a computable length"
        );
    });
}

/// A body with no `Content-Length` still reports its own `loaded` count, and
/// `lengthComputable` is false because the *length* is what is missing.
#[test]
fn a_body_without_a_content_length_still_reports_its_own_loaded_count() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(engine, response(200, &[], Some(b"hello")));

        assert_eq!(
            eval(engine, "log.join('\\n')"),
            "readystatechange@2:true:false:\n\
             readystatechange@3:true:false:\n\
             progress@3:true:true:false,5,0\n\
             progress@3:true:true:false,5,0\n\
             readystatechange@4:true:false:\n\
             load@4:true:true:false,5,0\n\
             loadend@4:true:true:false,5,0",
            "an uncomputable length must not zero `loaded`"
        );
    });
}

/// The response members answer the standard's network error before a completion
/// and the response itself afterwards.
///
/// `OPENED` is not a special case in the code: a request that has stored nothing
/// *has* the network error as its response, and every member reads that value's
/// own fields.
#[test]
fn status_and_status_text_read_the_response_once_there_is_one() {
    with_page(|engine| {
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/x');
            globalThis.before = [
                xhr.readyState, xhr.status, xhr.statusText, xhr.responseText,
                xhr.getResponseHeader('content-type') === null,
                xhr.getAllResponseHeaders(),
            ].join('|');
            "#,
        );
        assert_eq!(
            eval(engine, "before"),
            "1|0|||true|",
            "an opened request without a response is the network error"
        );

        deliver(
            engine,
            response(404, &[("Content-Type", "text/plain")], Some(b"nope")),
        );

        assert_eq!(
            eval(
                engine,
                r#"
                [
                    xhr.readyState, xhr.status, xhr.statusText, xhr.responseText,
                    xhr.getResponseHeader('content-type'),
                    xhr.getAllResponseHeaders(),
                ].join('|')
                "#
            ),
            "4|404|Not Found|nope|text/plain|content-type: text/plain\r\n",
            "the stored response is what every member reads"
        );
    });
}

/// `getResponseHeader` is Fetch's *get*: the name matches byte-case
/// insensitively, every value of that name is joined with `", "`, and a name the
/// list does not hold — including one that is not a header name at all — is
/// `null` rather than the empty string.
#[test]
fn get_response_header_matches_case_insensitively_and_answers_null() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(
            engine,
            response(
                200,
                &[
                    ("Content-Type", "text/plain"),
                    ("X-Custom", "one"),
                    ("x-custom", "two"),
                ],
                None,
            ),
        );

        assert_eq!(
            eval(
                engine,
                r#"
                [
                    xhr.getResponseHeader('content-type'),
                    xhr.getResponseHeader('Content-Type'),
                    xhr.getResponseHeader('CONTENT-TYPE'),
                    xhr.getResponseHeader('x-custom'),
                    xhr.getResponseHeader('X-CUSTOM'),
                    xhr.getResponseHeader('x-missing') === null,
                    xhr.getResponseHeader('content-type ') === null,
                ].join('|')
                "#
            ),
            "text/plain|text/plain|text/plain|one, two|one, two|true|true",
            "a name the list does not hold is null, not the empty string"
        );
    });
}

/// `getAllResponseHeaders` is *sort and combine* followed by the legacy second
/// sort: names byte-lowercased and combined, `set-cookie` dropped, and the whole
/// view re-sorted by its byte-uppercased names.
///
/// The names are chosen so the two sorts disagree. Byte order ends `x_a`, `xa`,
/// because `_` sorts below a letter, while upper-casing first ends `xa`, `x_a`,
/// because `A` sorts below `_`. Getting the second sort wrong is invisible on
/// names that differ in their letters alone — and so is getting it right, which
/// is why the assertion is the whole view.
#[test]
fn get_all_response_headers_is_sorted_and_combined() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(
            engine,
            response(
                200,
                &[
                    ("X-Zebra", "z"),
                    ("X-Mix", "one"),
                    ("x-MIX", "two"),
                    ("Set-Cookie", "secret=1"),
                    ("X_A", "underscore"),
                    ("XA", "plain"),
                ],
                None,
            ),
        );

        assert_eq!(
            eval(engine, "xhr.getAllResponseHeaders()"),
            "x-mix: one, two\r\nx-zebra: z\r\nxa: plain\r\nx_a: underscore\r\n",
            "the view is combined, drop `set-cookie`, and legacy-sorted"
        );
    });
}

/// A body with no declared charset is decoded as UTF-8, and a malformed sequence
/// becomes U+FFFD rather than an error.
#[test]
fn response_text_decodes_the_body_as_utf_8_by_default() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(engine, response(200, &[], Some("h\u{e9}llo".as_bytes())));

        assert_eq!(eval(engine, "xhr.responseText"), "h\u{e9}llo");
    });
}

/// The `charset` of the response's `Content-Type` is what a text response
/// decodes with.
#[test]
fn response_text_decodes_with_the_charset_of_the_response() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(
            engine,
            response(
                200,
                &[("Content-Type", "text/plain;charset=windows-1252")],
                Some(b"caf\xe9"),
            ),
        );

        assert_eq!(eval(engine, "xhr.responseText"), "caf\u{e9}");
    });
}

/// A byte-order mark wins over the response's own label and is removed from the
/// output — the Encoding Standard's *decode*, which is what the member runs.
#[test]
fn response_text_lets_a_byte_order_mark_choose_the_encoding() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(
            engine,
            response(
                200,
                &[("Content-Type", "text/plain;charset=windows-1252")],
                Some(b"\xef\xbb\xbfhi"),
            ),
        );

        assert_eq!(
            eval(engine, "xhr.responseText"),
            "hi",
            "the BOM's encoding wins, and the BOM itself is not part of the text"
        );
    });
}

/// `overrideMimeType` replaces the label a text response decodes with, so the
/// same bytes decode differently.
///
/// The state is checked, not the reason: the override is stored while the
/// request is open, and the standard's own `InvalidStateError` is the only
/// refusal it makes (after DONE; the test below).
#[test]
fn override_mime_type_changes_the_encoding_the_text_response_decodes_with() {
    with_page(|engine| {
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/x');
            xhr.overrideMimeType('text/plain;charset=utf-8');
            "#,
        );
        deliver(
            engine,
            response(
                200,
                &[("Content-Type", "text/plain;charset=windows-1252")],
                Some(b"caf\xe9"),
            ),
        );

        assert_eq!(
            eval(engine, "xhr.responseText"),
            "caf\u{fffd}",
            "the override's charset replaced the response's, so 0xE9 is malformed UTF-8"
        );
    });
}

/// Once the response is being received, `overrideMimeType` throws the standard's
/// `InvalidStateError` rather than storing anything.
#[test]
fn override_mime_type_is_refused_once_the_response_is_being_received() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(engine, response(200, &[], Some(b"body")));

        assert_eq!(
            eval(
                engine,
                r#"
                try { xhr.overrideMimeType('text/plain'); 'no throw'; }
                catch (error) { error.name; }
                "#
            ),
            "InvalidStateError",
            "the override is only settable before the response arrives"
        );
    });
}

/// `responseType`'s setter checks the state *before* it looks at the value, so
/// every value — even one it would accept, and even one that is not in the
/// enumeration — is the standard's `InvalidStateError` once the state is `DONE`.
#[test]
fn response_type_is_refused_once_the_response_is_being_received() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(engine, response(200, &[], Some(b"body")));

        assert_eq!(
            eval(
                engine,
                r#"
                const attempt = value => {
                    try { xhr.responseType = value; return 'ok'; }
                    catch (error) { return error.name; }
                };
                [attempt(''), attempt('text'), attempt('json')].join('|');
                "#
            ),
            "InvalidStateError|InvalidStateError|InvalidStateError",
            "the state check comes before the value is converted"
        );
    });
}

/// A second completion replaces the first response, because the response is
/// stored inline on the record rather than as an object of its own.
#[test]
fn a_second_completion_replaces_the_first_response() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(
            engine,
            response(200, &[("Content-Type", "text/plain")], Some(b"first")),
        );
        assert_eq!(
            eval(
                engine,
                "[xhr.status, xhr.statusText, xhr.responseText].join('|')"
            ),
            "200|OK|first"
        );

        deliver(engine, response(404, &[], Some(b"second")));

        assert_eq!(
            eval(
                engine,
                "[xhr.status, xhr.statusText, xhr.responseText, xhr.getResponseHeader('content-type') === null].join('|')"
            ),
            "404|Not Found|second|true",
            "the second response is what every member reads"
        );
    });
}

/// A second `open()` clears the stored response — the standard's step 11 — and
/// leaves the object at `OPENED` rather than `UNSENT`, whether or not it had
/// finished: its step 12 sets the state only if it is not already opened.
#[test]
fn a_second_open_clears_the_stored_response_and_leaves_the_state_opened() {
    with_page(|engine| {
        opened_xhr(engine);
        deliver(
            engine,
            response(200, &[("Content-Type", "text/plain")], Some(b"first")),
        );

        eval(
            engine,
            r#"
            globalThis.changes = 0;
            xhr.addEventListener('readystatechange', () => changes++);
            xhr.open('GET', '/again');
            "#,
        );

        assert_eq!(
            eval(
                engine,
                r#"
                [
                    xhr.readyState, changes,
                    xhr.status, xhr.statusText, xhr.responseText,
                    xhr.getResponseHeader('content-type') === null,
                    xhr.getAllResponseHeaders(),
                ].join('|')
                "#
            ),
            "1|1|0|||true|",
            "a re-opened request is the network error again, and is not UNSENT"
        );
    });
}

// ---------------------------------------------------------------------------
// The request half (slice d-4b b-5a3)
// ---------------------------------------------------------------------------

/// Installs a [`Recorder`] as the engine's XHR driver and hands it back, so a
/// test can read what a page's `send()` made it do.
fn install_recorder(engine: &mut Engine) -> Rc<Recorder> {
    let recorder = Rc::new(Recorder::default());
    engine.set_xhr_driver(Rc::clone(&recorder) as Rc<dyn XhrDriver>);
    recorder
}

/// `send()` on an object that was never opened is an `InvalidStateError`, and
/// the refusal happens before the driver is reached.
#[test]
fn send_before_open_is_an_invalid_state_error() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        assert_eq!(
            eval(
                engine,
                r#"
                globalThis.xhr = new XMLHttpRequest();
                let outcome;
                try { xhr.send(); outcome = 'no throw'; }
                catch (error) { outcome = error.name; }
                [outcome, xhr.readyState].join('|');
                "#
            ),
            "InvalidStateError|0",
            "a request that was never opened cannot be sent"
        );
        assert!(
            recorder.starts.borrow().is_empty(),
            "the refusal comes before anything is handed to a driver"
        );
    });
}

/// A second `send()` while the first request is still in flight is an
/// `InvalidStateError` — and it is the *send flag* that refuses it, not the
/// state, because the object is still open.
///
/// The driver is held open (`Recorder::hold`) so the first request never
/// completes; that is what makes the two checks distinguishable.
#[test]
fn a_second_send_before_the_first_completes_is_refused() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        assert_eq!(
            eval(
                engine,
                r#"
                globalThis.xhr = new XMLHttpRequest();
                xhr.open('GET', '/x');
                xhr.send();
                let outcome;
                try { xhr.send(); outcome = 'no throw'; }
                catch (error) { outcome = error.name; }
                [outcome, xhr.readyState].join('|');
                "#
            ),
            "InvalidStateError|1",
            "the flag is what refuses the second call, not the state"
        );
        assert_eq!(
            recorder.starts.borrow().len(),
            1,
            "only the first call reached the driver"
        );
    });
}

/// A completion moves the object to `DONE`, so a `send()` after one is the
/// standard's *state* refusal — step 1, before the send flag is even read.
///
/// The flag *is* cleared by a completion ([`deliver_xhr_completion`] does it for
/// both of its branches), but no page can tell: every completion ends at `DONE`,
/// so the state check always answers first. The clear is pinned at the record
/// level instead (`tenta_js::bindings::a_delivered_completion_clears_the_send_flag`),
/// which is the only place it is observable.
#[test]
fn send_after_a_completion_is_refused_by_the_state() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/x');
            xhr.send();
            "#,
        );
        // The request the driver was handed, not a constant: `send()` stamps the
        // id the completion has to carry to be delivered at all.
        let sent = recorder.starts.borrow()[0].1;
        deliver_at(engine, sent, ok_outcome());

        assert_eq!(
            eval(
                engine,
                r#"
                let outcome;
                try { xhr.send(); outcome = 'no throw'; }
                catch (error) { outcome = error.name; }
                [outcome, xhr.readyState].join('|');
                "#
            ),
            "InvalidStateError|4",
            "a finished request cannot be sent again without being re-opened"
        );
    });
}

/// `send()` fires exactly one event of its own — `loadstart`, a trusted
/// `ProgressEvent` with no length — and the request it hands the driver carries
/// the method and URL `open()` set up, the headers `setRequestHeader`
/// combined, and the document as the initiator.
#[test]
fn send_fires_loadstart_and_hands_the_driver_the_request_it_built() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        assert_eq!(
            eval(
                engine,
                r#"
                globalThis.xhr = new XMLHttpRequest();
                xhr.open('POST', '/submit?x=1');
                // Registered after `open()` so the `open`'s own
                // `readystatechange` is not in the log: this test is about what
                // `send()` fires.
                globalThis.log = [];
                for (const name of ['loadstart', 'readystatechange', 'progress',
                                    'load', 'error', 'loadend']) {
                    xhr.addEventListener(name, event => log.push([
                        event.type, event.isTrusted, event.target === xhr,
                        event instanceof ProgressEvent,
                        event.lengthComputable, event.loaded, event.total,
                    ].join(':')));
                }
                xhr.setRequestHeader('X-Token', 'abc');
                xhr.send();
                [log.join(','), xhr.readyState].join('|');
                "#
            ),
            "loadstart:true:true:true:false:0:0|1",
            "`loadstart` is the only event `send()` fires, and nothing else has happened yet"
        );

        let starts = recorder.starts.borrow();
        assert_eq!(starts.len(), 1, "one `send()` makes one request");
        let (request, handle) = &starts[0];
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url.as_str(), "http://fixture.test/submit?x=1");
        assert_eq!(
            request.headers,
            vec![("x-token".to_owned(), "abc".to_owned())],
            "the author request headers reach the driver"
        );
        assert_eq!(
            request.initiator.as_str(),
            "http://fixture.test/dir/page.html",
            "the document is the initiator, which is what a same-origin test compares"
        );
        assert_eq!(
            handle.target,
            target_bits(engine, "xhr"),
            "the completion is addressed to the object that made the request"
        );
        assert_eq!(
            handle.request, 2,
            "and to this object's *first* request: `open()` took 1 and `send()` 2"
        );
    });
}

/// A `USVString` body arrives as its UTF-8 bytes and the request carries
/// `text/plain;charset=UTF-8` — Fetch's *extract a body with type* for the string
/// arm, written into the author request headers by `send()` step 4's sub-step 6
/// — its `Otherwise`
/// (<https://xhr.spec.whatwg.org/#the-send()-method>).
#[test]
fn a_string_body_becomes_utf_8_bytes_and_infers_the_text_plain_content_type() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            const xhr = new XMLHttpRequest();
            xhr.open('POST', '/submit');
            xhr.send('a=1&café');
            "#,
        );

        let starts = recorder.starts.borrow();
        assert_eq!(starts.len(), 1, "a body is sent, not refused");
        let request = &starts[0].0;
        assert_eq!(
            request.body.as_deref(),
            Some("a=1&café".as_bytes()),
            "the string crosses as its UTF-8 encoding, not as a lossy form"
        );
        assert_eq!(
            request.headers,
            vec![(
                "content-type".to_owned(),
                "text/plain;charset=UTF-8".to_owned()
            )],
            "the type the string arm infers is written into the request's headers"
        );
    });
}

/// A buffer-source body crosses byte for byte and infers no `Content-Type` at
/// all: Fetch's `BufferSource` arm produces bytes and a null type, and `send()`
/// step 4's sub-step 6 writes nothing for a null `extractedContentType`.
#[test]
fn a_buffer_source_body_carries_its_bytes_and_infers_no_content_type() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            const xhr = new XMLHttpRequest();
            xhr.open('PUT', '/upload');
            xhr.send(new Uint8Array([0x00, 0xFF, 0x80, 0x41]));
            "#,
        );

        let starts = recorder.starts.borrow();
        assert_eq!(starts.len(), 1);
        let request = &starts[0].0;
        assert_eq!(
            request.body.as_deref(),
            Some([0x00u8, 0xFF, 0x80, 0x41].as_slice()),
            "every byte survives, including the ones that are not UTF-8"
        );
        assert!(
            request.headers.is_empty(),
            "a buffer source infers no type, so no header is written"
        );
    });
}

/// An author `Content-Type` survives the body step, under any casing: the
/// inferred-type write, step 4's sub-step 6, is the `Otherwise` of sub-step 5's
/// "If originalAuthorContentType is non-null", so it is only reached when the
/// author set none. `setRequestHeader` byte-lowercases the name, which is why the
/// survival does not depend on how the page spelled it.
#[test]
fn an_author_content_type_survives_the_inferred_one() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            const xhr = new XMLHttpRequest();
            xhr.open('POST', '/submit');
            xhr.setRequestHeader('Content-Type', 'application/json');
            xhr.send('{"a":1}');
            "#,
        );

        let starts = recorder.starts.borrow();
        let request = &starts[0].0;
        assert_eq!(
            request.headers,
            vec![("content-type".to_owned(), "application/json".to_owned())],
            "the author's value is kept and the inferred one is not written"
        );
        assert_eq!(request.body.as_deref(), Some(b"{\"a\":1}".as_slice()));
    });
}

/// A `URLSearchParams` body falls to the `USVString` arm, because
/// `XMLHttpRequestBodyInit` declares no arm for it: the arm cannot be declared
/// until the generator's `union_test` can brand-check a platform object rather
/// than testing `value.is_object()` for every object.
///
/// **The bytes are right and the inferred type is not**, and the assertion below
/// pins the engine's *actual* behaviour so that a slice which adds the brand
/// check is forced back here. `URLSearchParams`'s stringifier *is* the
/// `application/x-www-form-urlencoded` serializer, so the body is exactly what a
/// browser sends; the type is `text/plain;charset=UTF-8`, where the standard's
/// own `URLSearchParams` branch of *extract a body with type* sets
/// `application/x-www-form-urlencoded;charset=UTF-8`. The deviation is recorded
/// in full in `idl/xhr/XMLHttpRequest.webidl`.
#[test]
fn a_url_search_params_body_serializes_but_infers_the_plain_text_type() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            const xhr = new XMLHttpRequest();
            xhr.open('POST', '/submit');
            xhr.send(new URLSearchParams('a=1&b=two'));
            "#,
        );

        let starts = recorder.starts.borrow();
        let request = &starts[0].0;
        assert_eq!(
            request.body.as_deref(),
            Some(b"a=1&b=two".as_slice()),
            "the stringifier is the serializer, so the bytes are the standard's"
        );
        assert_eq!(
            request.headers,
            vec![(
                "content-type".to_owned(),
                "text/plain;charset=UTF-8".to_owned()
            )],
            "the recorded deviation: the standard says \
             `application/x-www-form-urlencoded;charset=UTF-8` here, and saying \
             so needs a branded `URLSearchParams` arm that this engine has not got"
        );
    });
}

/// `send('x')` on a `GET` and on a `HEAD` is not a refusal and not a body: step 3
/// ("If this's request method is `GET` or `HEAD`, then set body to null") drops it
/// before step 4 can convert it, so the request the driver is handed is the one
/// the same call would have made with no argument at all.
#[test]
fn a_get_and_a_head_drop_the_body_before_it_reaches_the_step_that_converts_it() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            for (const method of ['GET', 'HEAD']) {
                const xhr = new XMLHttpRequest();
                xhr.open(method, '/search?q=1');
                xhr.send('q=1');
            }
            "#,
        );

        let starts = recorder.starts.borrow();
        assert_eq!(starts.len(), 2, "both requests were made");
        for (request, method) in starts.iter().zip(["GET", "HEAD"]) {
            assert_eq!(request.0.method.as_str(), method);
            assert!(
                request.0.body.is_none(),
                "{method} dropped the body at step 3"
            );
            assert!(
                request.0.headers.is_empty(),
                "and step 4 never ran, so no `Content-Type` was inferred"
            );
        }
    });
}

/// The three ways a page spells the default body — an omitted argument,
/// `undefined` and `null` — are all the null body, and none of them is a refusal
/// or an empty body. The declared union's `null` is converted by the generated
/// glue into the same `None` an omitted argument produces.
#[test]
fn an_omitted_undefined_and_null_body_are_all_the_null_body() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        assert_eq!(
            eval(
                engine,
                r#"
                const attempt = send => {
                    const xhr = new XMLHttpRequest();
                    xhr.open('POST', '/x');
                    try { send(xhr); return 'ok'; }
                    catch (error) { return error.name; }
                };
                [
                    attempt(xhr => xhr.send()),
                    attempt(xhr => xhr.send(undefined)),
                    attempt(xhr => xhr.send(null)),
                ].join('|');
                "#
            ),
            "ok|ok|ok",
            "the default body is accepted in all three of its spellings"
        );

        let starts = recorder.starts.borrow();
        assert_eq!(starts.len(), 3, "each call reached the driver");
        for (request, _) in starts.iter() {
            assert!(request.body.is_none(), "the null body is not an empty body");
            assert!(
                request.headers.is_empty(),
                "and a null body infers no `Content-Type`"
            );
        }
    });
}

/// The `USVString` arm converts by WebIDL's `ToString`, which is why a number is
/// stringified rather than refused: `send(5)` sends the byte `5`.
///
/// A `Symbol` is the one value `ToString` refuses, and what this engine does with
/// it is **two things at once**, which is a recorded deviation rather than the
/// standard's single throw. The standard converts an operation's arguments before
/// its steps run, so a body whose conversion throws leaves `send()` with nothing
/// to do; here the conversion is
/// [`string_from_v8`](../../../tenta-idl/src/codegen/prelude.rs), which is
/// infallible — it calls V8's `to_rust_string_lossy`, so a failing `to_string`
/// yields `""` and leaves the exception pending. `send_request` therefore *runs*,
/// starts a request with an empty body, and the `TypeError` V8 is still holding is
/// thrown to the page when the binding returns.
///
/// The assertion below pins that pair, and the defect is deliberately not fixed
/// here: it is the shared `DOMString`/`USVString` conversion rather than `send()`
/// — `document.title = Symbol()` throws its `TypeError` and stores the empty
/// string for the same reason — so the fix is `string_from_v8` becoming fallible,
/// which rewrites every generated call site and therefore needs a generator slice
/// with its own goldens.
#[test]
fn a_number_body_is_stringified_and_a_symbol_starts_a_request_before_it_throws() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        assert_eq!(
            eval(
                engine,
                r#"
                const number = new XMLHttpRequest();
                number.open('POST', '/x');
                number.send(5);

                const symbol = new XMLHttpRequest();
                symbol.open('POST', '/x');
                let thrown;
                try { symbol.send(Symbol('s')); thrown = 'no throw'; }
                catch (error) { thrown = error.name; }
                [thrown, symbol.readyState].join('|');
                "#
            ),
            "TypeError|1",
            "the symbol's `ToString` fails, and the state is the one `send()` left"
        );

        let starts = recorder.starts.borrow();
        assert_eq!(
            starts.len(),
            2,
            "the recorded deviation: the refused call started a request anyway"
        );
        assert_eq!(
            starts[0].0.body.as_deref(),
            Some(b"5".as_slice()),
            "`5` is stringified per WebIDL, not dropped"
        );
        assert_eq!(
            starts[1].0.body.as_deref(),
            Some(b"".as_slice()),
            "the symbol's failed conversion reached `send_request` as the empty \
             string, which is the half of the deviation a page can see on the wire"
        );
    });
}

/// A page with no driver installed gets the standard's failure sequence
/// synchronously out of `send()`, rather than an object that waits forever.
///
/// `fetch`'s precedent treats a missing driver as a state a page can be in
/// ("fetch is not available"), and XHR's translation of it is the *request error
/// steps* — there is no promise to reject.
#[test]
fn send_without_a_driver_reports_the_failure_sequence() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                globalThis.xhr = new XMLHttpRequest();
                xhr.open('GET', '/x');
                // Registered after `open()` so its `readystatechange` is not in
                // the log.
                globalThis.log = [];
                for (const name of ['loadstart', 'readystatechange', 'progress',
                                    'load', 'error', 'loadend']) {
                    xhr.addEventListener(name, event => log.push(event.type + '@' + xhr.readyState));
                }
                xhr.send();
                [log.join(','), xhr.readyState, xhr.status === 0].join('|');
                "#
            ),
            "loadstart@1,readystatechange@4,error@4,loadend@4|4|true",
            "no driver is a failed request, reported before `send()` returns"
        );
    });
}

/// `setRequestHeader` checks the state and the send flag *before* it looks at
/// either argument, so a bogus pair on an unopened object is an
/// `InvalidStateError` rather than the `SyntaxError` the pair itself earns.
#[test]
fn set_request_header_checks_the_state_before_the_arguments() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                globalThis.xhr = new XMLHttpRequest();
                let outcome;
                try { xhr.setRequestHeader('bad name', 'a\u0000b'); outcome = 'no throw'; }
                catch (error) { outcome = error.name; }
                [outcome, xhr.readyState].join('|');
                "#
            ),
            "InvalidStateError|0",
            "the state check comes first"
        );
    });
}

/// A name that is not an HTTP `token` and a value holding a NUL, CR or LF are
/// `SyntaxError`s — WebIDL's exception, where the `Headers` mutators over the
/// same two checks throw a `TypeError`.
#[test]
fn set_request_header_refuses_a_bad_name_or_value_with_a_syntax_error() {
    with_page(|engine| {
        assert_eq!(
            eval(
                engine,
                r#"
                globalThis.xhr = new XMLHttpRequest();
                xhr.open('GET', '/x');
                const attempt = (name, value) => {
                    try { xhr.setRequestHeader(name, value); return 'ok'; }
                    catch (error) { return error.name; }
                };
                [
                    attempt('X-Token', 'abc'),
                    attempt('bad name', 'v'),
                    attempt('x:', 'v'),
                    attempt('', 'v'),
                    attempt('x', 'a\u0000b'),
                    attempt('x', 'a\nb'),
                    attempt('x', 'a\rb'),
                ].join('|');
                "#
            ),
            "ok|SyntaxError|SyntaxError|SyntaxError|SyntaxError|SyntaxError|SyntaxError",
            "a bad name is reported before a bad value, and both are SyntaxErrors"
        );
    });
}

/// A *forbidden request-header* is dropped silently — no exception, and nothing
/// stored — and the predicate is over the **(name, value) pair**: `X-HTTP-Method`
/// is forbidden only when one of its values names a forbidden method.
#[test]
fn forbidden_request_headers_are_dropped_silently() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        assert_eq!(
            eval(
                engine,
                r#"
                globalThis.xhr = new XMLHttpRequest();
                xhr.open('GET', '/x');
                const attempt = (name, value) => {
                    try { xhr.setRequestHeader(name, value); return 'ok'; }
                    catch (error) { return error.name; }
                };
                const outcomes = [
                    attempt('Cookie', 'a=b'),
                    attempt('HOST', 'example.test'),
                    attempt('Content-Length', '5'),
                    attempt('Proxy-Authorization', 'v'),
                    attempt('Sec-Fetch-Site', 'same-origin'),
                    attempt('X-HTTP-Method', 'TRACE'),
                    attempt('X-HTTP-Method-Override', 'PATCH, track'),
                    attempt('x-method-override', 'CONNECT'),
                    attempt('X-HTTP-Method', 'PATCH, PATCH'),
                    attempt('X-Token', 'kept'),
                ];
                xhr.send();
                outcomes.join('|');
                "#
            ),
            "ok|ok|ok|ok|ok|ok|ok|ok|ok|ok",
            "a forbidden name is dropped rather than thrown"
        );

        let starts = recorder.starts.borrow();
        assert_eq!(starts.len(), 1, "the request still went out");
        assert_eq!(
            starts[0].0.headers,
            vec![
                ("x-http-method".to_owned(), "PATCH, PATCH".to_owned()),
                ("x-token".to_owned(), "kept".to_owned()),
            ],
            "only the two non-forbidden pairs were stored, in the order they were authored"
        );
    });
}

/// The value's leading and trailing HTTP whitespace is stripped before the
/// driver sees it, and a name already present is *combined* with `", "` — the
/// comparison being byte-case-insensitive, which is why the two spellings here
/// land on one entry.
#[test]
fn set_request_header_strips_whitespace_and_combines_case_insensitively() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/x');
            xhr.setRequestHeader('X-Token', '  one  ');
            xhr.setRequestHeader('x-token', '\ttwo\t');
            xhr.setRequestHeader('X-Other', 'v');
            xhr.send();
            "#,
        );

        let starts = recorder.starts.borrow();
        assert_eq!(
            starts[0].0.headers,
            vec![
                ("x-token".to_owned(), "one, two".to_owned()),
                ("x-other".to_owned(), "v".to_owned()),
            ],
            "the halves are normalized and the repeated name is combined"
        );
    });
}

/// `setRequestHeader` is refused once `send()` has been invoked, because the
/// request is already under way.
#[test]
fn set_request_header_is_refused_once_send_has_been_invoked() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        assert_eq!(
            eval(
                engine,
                r#"
                globalThis.xhr = new XMLHttpRequest();
                xhr.open('GET', '/x');
                xhr.send();
                let outcome;
                try { xhr.setRequestHeader('X-Token', 'abc'); outcome = 'no throw'; }
                catch (error) { outcome = error.name; }
                [outcome, xhr.readyState].join('|');
                "#
            ),
            "InvalidStateError|1",
            "the author request headers are frozen once the request is under way"
        );
    });
}

/// `open()` clears both the send flag and the author request headers, so
/// re-opening an object whose request was started leaves it able to make a fresh
/// one that carries nothing of the old.
#[test]
fn open_clears_the_send_flag_and_the_author_request_headers() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        assert_eq!(
            eval(
                engine,
                r#"
                globalThis.xhr = new XMLHttpRequest();
                const attempt = send => {
                    try { send(); return 'ok'; }
                    catch (error) { return error.name; }
                };
                xhr.open('GET', '/first');
                xhr.setRequestHeader('X-Token', 'abc');
                xhr.send();
                const refused = attempt(() => xhr.send());
                xhr.open('GET', '/second');
                const accepted = attempt(() => xhr.send());
                [refused, accepted, xhr.readyState].join('|');
                "#
            ),
            "InvalidStateError|ok|1",
            "re-opening must leave the object able to start a fresh request"
        );

        let starts = recorder.starts.borrow();
        assert_eq!(starts.len(), 2, "the re-opened object made its own request");
        assert_eq!(
            starts[0].0.headers,
            vec![("x-token".to_owned(), "abc".to_owned())]
        );
        assert_eq!(starts[1].0.url.as_str(), "http://fixture.test/second");
        assert!(
            starts[1].0.headers.is_empty(),
            "the previous request's headers must not carry into the new one"
        );
    });
}

/// A completion for a request its object has abandoned is discarded: none of its
/// events fire and none of its response lands.
///
/// This is `open()`'s step 10 as this product can serve it. `send()` does not
/// change the state — the object stays at `OPENED` for as long as its request is
/// unanswered — so with a driver installed, `open()` is reachable with a request
/// in flight and the object can carry two. The standard's answer is to terminate
/// the first; the host layer cannot, because the driver is on the other side of
/// the seam, so the record's request id is what the abandonment is expressed
/// with (b-6) and this is the test that it holds.
///
/// The second `send()` is what makes the two requests distinguishable from the
/// outside: without an id, both completions would be addressed to one object and
/// the page would see two interleaved event sequences, with `response` ending up
/// whichever arrived last. That is the failure this pins.
#[test]
fn a_completion_for_a_request_the_object_abandoned_is_discarded() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/first');
            xhr.send();
            // The re-open with the first request still in flight: this is the
            // step-10 case. The state is already `OPENED`, so the second
            // `open()` fires no `readystatechange`.
            xhr.open('GET', '/second');
            xhr.send();
            // Registered after both, so the log holds the completions' events
            // and nothing of the setup's.
            globalThis.log = [];
            for (const name of ['readystatechange', 'progress', 'load', 'error', 'loadend']) {
                xhr.addEventListener(name, event => log.push(event.type + '@' + xhr.readyState));
            }
            "#,
        );

        let (abandoned, live) = {
            let starts = recorder.starts.borrow();
            assert_eq!(
                starts.len(),
                2,
                "the re-opened object made a second request"
            );
            assert_eq!(
                starts[0].1.target, starts[1].1.target,
                "one object made both requests, so both answers are addressed to it"
            );
            assert_ne!(
                starts[0].1.request, starts[1].1.request,
                "and the two requests are told apart by their ids alone"
            );
            (starts[0].1, starts[1].1)
        };

        // The abandoned request's answer arrives first, and it is a *response*
        // with a body: delivered, it would move the object to `DONE` and store
        // its body.
        engine.settle_xhr(vec![hand(
            abandoned.target,
            abandoned.request,
            response(200, &[("Content-Type", "text/plain")], Some(b"abandoned")),
        )]);

        assert_eq!(
            eval(engine, "log.join(',')"),
            "",
            "an abandoned request's completion fires no event at all"
        );
        assert_eq!(
            eval(
                engine,
                "[xhr.readyState, xhr.status, xhr.responseText.length, \
                  xhr.getResponseHeader('content-type') === null].join('|')"
            ),
            "1|0|0|true",
            "and it neither moves the state nor stores anything"
        );

        // The live request's answer still lands, so the discard is not a machine
        // that has stopped working.
        engine.settle_xhr(vec![hand(
            live.target,
            live.request,
            response(200, &[("Content-Type", "text/plain")], Some(b"live")),
        )]);

        assert_eq!(
            eval(
                engine,
                "[xhr.readyState, xhr.status, xhr.responseText].join('|')"
            ),
            "4|200|live"
        );
        assert_eq!(
            eval(engine, "log.join(',')"),
            "readystatechange@2,readystatechange@3,progress@3,progress@3,\
             readystatechange@4,load@4,loadend@4",
            "the live request runs the whole response machine"
        );
    });
}

// ---------------------------------------------------------------------------
// Ending a request early: `abort()` and `timeout` (slice d-4b b-7)
// ---------------------------------------------------------------------------

/// `abort()` on a request that is in flight ends it at the driver and reports it
/// through the standard's *request error steps* with `abort` — synchronously, and
/// with the state reset behind the events.
///
/// The steps are <https://xhr.spec.whatwg.org/#the-abort()-method> steps 1 to 3:
/// the driver is told to stop the request, the error steps run (`DONE`,
/// `readystatechange`, `abort`, `loadend`, all with no length), and then the
/// object is reset to `UNSENT` because step 3's condition is read *after* step 2
/// has set the state to `DONE`. That last part is what the WPT
/// `abort-after-send.any.js` asserts when it checks `readyState === 0` after the
/// event sequence, and it is why `4|...` inside the log and `0` afterwards are
/// not a contradiction.
///
/// The driver's half is asserted separately, because it is the half a page can
/// never see: the handle the request was started under is the handle `abort()`
/// named through the seam.
#[test]
fn abort_ends_an_in_flight_request_and_fires_the_abort_sequence() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/never');
            xhr.send();
            "#,
        );
        log_every_event(engine);

        assert_eq!(
            eval(
                engine,
                r#"
                xhr.abort();
                [log.join(','), xhr.readyState, xhr.status, xhr.responseText].join('|')
                "#
            ),
            "readystatechange@4:true:false:,\
             abort@4:true:true:false,0,0,\
             loadend@4:true:true:false,0,0|\
             0|0|",
            "the events see DONE and the state a caller reads afterwards is UNSENT"
        );

        let starts = recorder.starts.borrow();
        assert_eq!(starts.len(), 1, "one request was handed over");
        assert!(
            recorder.aborts.borrow().contains(&starts[0].1),
            "the seam was given the handle the request was started under"
        );
    });
}

/// `abort()` on an object that was opened but never sent does nothing observable:
/// neither of the standard's two conditions holds, so no event fires and the
/// state stays `OPENED` — the WPT `abort-during-unsent.any.js` case.
///
/// The seam is still crossed, and the test asserts that too: ending a request is
/// `end_xhr_request`'s job whether or not one is in flight, and a handle nothing
/// was started under is exactly what the driver answers `false` for. What must
/// *not* happen is an event, a state change or a start.
#[test]
fn abort_on_an_opened_request_that_was_never_sent_does_nothing() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/x');
            "#,
        );
        log_every_event(engine);

        assert_eq!(
            eval(
                engine,
                r#"
                xhr.abort();
                [log.length, xhr.readyState].join('|')
                "#
            ),
            "0|1",
            "an opened-but-unsent object is neither in flight nor done"
        );
        assert!(
            recorder.starts.borrow().is_empty(),
            "no request was ever handed to the driver, so there was none to end"
        );
        assert!(
            !recorder.aborts.borrow().is_empty(),
            "the seam is still the way a request ends, even when none is in flight"
        );
    });
}

/// `abort()` after the answer has arrived resets the object to `UNSENT` and fires
/// nothing at all — the WPT `abort-during-done.window.js` case, and the standard's
/// note on step 3: "No readystatechange event is dispatched."
///
/// The response goes with the state, so every response member reads the network
/// error again: `status` 0, `statusText` empty, `responseText` empty. That is the
/// observable difference between this case and a bare no-op, and it is the one a
/// page relies on when it aborts from a `load` handler.
#[test]
fn abort_after_the_answer_arrived_resets_the_object_to_unsent_without_events() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/x');
            xhr.send();
            "#,
        );
        let handle = recorder.starts.borrow()[0].1;
        deliver_at(
            engine,
            handle,
            response(200, &[("Content-Type", "text/plain")], Some(b"body")),
        );
        assert_eq!(eval(engine, "xhr.readyState"), "4", "the answer arrived");
        log_every_event(engine);

        assert_eq!(
            eval(
                engine,
                r#"
                xhr.abort();
                [log.length, xhr.readyState, xhr.status, xhr.statusText, xhr.responseText].join('|')
                "#
            ),
            "0|0|0||",
            "step 3 resets the state and the response, and fires no event"
        );
    });
}

/// A completion that **races** an `abort()` is discarded: the driver answered
/// before the termination reached it, so its answer is still in the table when
/// the abort lands, and nothing of it reaches the page.
///
/// This is the reason the engine bumps the request id in `end_xhr_request` as
/// well as calling the driver: the driver's own termination cannot recall a
/// completion it has already pushed, and the id is the only thing that can refuse
/// one. Without the bump this test's drained batch would run the whole response
/// machine on an object that had already reported the request over.
///
/// The abort itself fires its own sequence first, which is what makes the
/// ordering a real race rather than a hypothetical one: `abort()` has already
/// moved the state to `UNSENT` by the time the stale completion is drained, so a
/// delivery here would move it to `DONE` and store a body for the request the
/// page walked away from.
#[test]
fn a_completion_that_races_an_abort_is_discarded() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        // No `hold`: the driver answers at `start`, exactly as one that had
        // already finished its work when the abort arrived.
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/x');
            xhr.send();
            "#,
        );
        log_every_event(engine);

        eval(engine, "xhr.abort()");
        assert_eq!(
            eval(engine, "[log.length, xhr.readyState].join('|')"),
            "3|0",
            "the abort's own sequence fired, and step 3 reset the object"
        );

        // The driver's answer was pushed before the abort and the abort did not
        // remove it, so this is exactly the batch the page's turn loop would
        // drain next.
        let raced = recorder.table.take_all();
        assert_eq!(raced.len(), 1, "the driver had answered already");
        engine.settle_xhr(raced);

        assert_eq!(
            eval(
                engine,
                "[log.length, xhr.readyState, xhr.status, xhr.responseText].join('|')"
            ),
            "3|0|0|",
            "the stale completion fires nothing, stores nothing and moves nothing"
        );
    });
}

/// A request that outlives its `timeout` is reported through the same *request
/// error steps* as a network failure, under the name `timeout`.
///
/// The standard's `send()` arms this from its parallel branch — "Wait until
/// either req's done flag is set or this's timeout is not 0 and this's timeout
/// milliseconds have passed since now. If req's done flag is unset, then set
/// this's timed out to true and terminate this's fetch controller"
/// (<https://xhr.spec.whatwg.org/#the-send()-method>) — and the events are the
/// ones `event-timeout-order.any.js` pins, minus its upload half: `DONE`, then
/// `timeout`, then `loadend`, with no `progress` of any kind.
///
/// The state is **not** reset to `UNSENT`, unlike `abort()`: the standard's
/// timeout runs the *request error steps* alone, with no step 3 after them, so a
/// page that reads `readyState` from its `timeout` handler — or after it — sees
/// `DONE`.
///
/// The clock is the engine's own, and `run_event_loop` is what moves it: the
/// timer is armed as page activity
/// ([`ScriptHost::schedule_background_task`](tenta_js::dom::events::ScriptHost::schedule_background_task)),
/// so no test here sleeps and the deadline is exact rather than approximate.
///
/// Two halves of the discard are asserted at the end, because they are the two
/// halves of `end_xhr_request`: the driver was told to end the request, and a
/// completion that arrives after the timeout anyway is refused.
#[test]
fn a_request_that_outlives_its_timeout_is_reported_as_a_timeout() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/never');
            xhr.timeout = 5;
            xhr.send();
            "#,
        );
        log_every_event(engine);
        assert_eq!(
            eval(engine, "[log.length, xhr.readyState].join('|')"),
            "0|1",
            "nothing has fired: the deadline has not been reached"
        );

        engine.run_event_loop(8);

        assert_eq!(
            eval(
                engine,
                "[log.join(','), xhr.readyState, xhr.status, xhr.responseText].join('|')"
            ),
            "readystatechange@4:true:false:,\
             timeout@4:true:true:false,0,0,\
             loadend@4:true:true:false,0,0|\
             4|0|",
            "a timeout is the error sequence under another name, and leaves the state DONE"
        );

        let handle = recorder.starts.borrow()[0].1;
        assert!(
            recorder.aborts.borrow().contains(&handle),
            "the timer ends the request at the driver as well as reporting it"
        );

        // The driver answers anyway, for the request the timeout has already
        // ended. It is refused, and the page sees nothing.
        engine.settle_xhr(vec![hand(
            handle.target,
            handle.request,
            response(200, &[], Some(b"too late")),
        )]);
        assert_eq!(
            eval(
                engine,
                "[log.length, xhr.status, xhr.responseText].join('|')"
            ),
            "3|0|",
            "a completion that arrives after the timeout is discarded"
        );
    });
}

/// The other order: a request that answers before its deadline fires no
/// `timeout` event at all, and the timer that fires afterwards does nothing.
///
/// The timer is never cancelled when its request ends early — the standard
/// describes a wait, not a cancellation — so the callback has to recognise that
/// its request is over and return. The record is what it reads: the send flag is
/// cleared by every way a request ends, so a timer whose request has finished
/// finds no request in flight. This is also what makes a `timeout` that fires
/// *between* two requests on one object harmless, since the id is checked against
/// the same read.
///
/// `run_event_loop` is called after the delivery, so the timer is reached with
/// the object already at `DONE` — the state a page would be in if its server had
/// answered inside the deadline.
#[test]
fn a_timer_that_fires_after_the_answer_has_arrived_reports_nothing() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/fast');
            xhr.timeout = 1000;
            xhr.send();
            "#,
        );
        log_every_event(engine);

        let handle = recorder.starts.borrow()[0].1;
        deliver_at(engine, handle, response(200, &[], None));
        assert_eq!(eval(engine, "xhr.readyState"), "4", "the answer arrived");

        engine.run_event_loop(8);

        assert_eq!(
            eval(engine, "log.join(',')"),
            "readystatechange@2:true:false:,\
             progress@2:true:true:false,0,0,\
             readystatechange@4:true:false:,\
             load@4:true:true:false,0,0,\
             loadend@4:true:true:false,0,0",
            "the response machine's events alone: the timer found nothing to report"
        );
        assert_eq!(
            eval(
                engine,
                "[xhr.readyState, xhr.status, xhr.responseText].join('|')"
            ),
            "4|200|",
            "and the delivered response is untouched"
        );
        assert!(
            !recorder.aborts.borrow().contains(&handle),
            "nothing was ended: the request had already finished"
        );
    });
}

/// A `timeout` of 0 arms nothing, so a request with one runs to its own end
/// whatever the driver's pace.
///
/// This is the other half of `the_timeout_setter_stores_any_value_and_arms_nothing`:
/// the setter stores the 0, and `send()` reads it and arms no timer. The
/// distinction matters because the standard's condition is "this's timeout is not
/// 0", so a page takes a deadline *off* a request by setting 0, and a page that
/// never touched the attribute has exactly the same behaviour.
#[test]
fn a_timeout_of_zero_arms_no_timer() {
    with_page(|engine| {
        let recorder = install_recorder(engine);
        recorder.hold.set(true);
        eval(
            engine,
            r#"
            globalThis.xhr = new XMLHttpRequest();
            xhr.open('GET', '/never');
            xhr.timeout = 0;
            xhr.send();
            "#,
        );
        log_every_event(engine);

        engine.run_event_loop(64);

        assert_eq!(
            eval(engine, "[log.length, xhr.readyState].join('|')"),
            "0|1",
            "no deadline means no timer, so the request is still waiting"
        );
        assert!(
            recorder
                .aborts
                .borrow()
                .iter()
                .all(|handle| handle.request == 0),
            "no timer fired, so only `open()`'s step 10 ever named a request"
        );
    });
}
