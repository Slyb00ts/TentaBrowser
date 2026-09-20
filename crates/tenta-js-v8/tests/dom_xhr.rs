//! Tests for the XHR completion seam and the engine's native dispatch
//! (`tenta_js::xhr`, slice d-4b b-4).
//!
//! XHR's completion is an **event dispatch**, not a promise settlement, so the
//! `fetch` seam (`dom_fetch.rs`) cannot be reused verbatim: a driver is handed a
//! request and the opaque handle bits of an event target, and the answer comes
//! back as an [`XhrCompletion`] the page's turn loop hands to
//! [`Engine::settle_xhr`]. Nothing here opens a socket — the driver below is a
//! recorder — and nothing here is script-visible: b-5 adds the
//! `XMLHttpRequest` surface that starts a request from script, and b-6 puts
//! `tenta-net` behind this trait.
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
