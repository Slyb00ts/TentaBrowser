//! Unit tests for the engine-independent dispatch algorithm, driven by a
//! recording mock [`ScriptHost`] over a real `tenta_dom::Document` (no V8).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use tenta_core::NodeRef;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};

use super::*;
use crate::bindings::host::{HostContext, decode};
use crate::bindings::{AnyValue, Callback, Event, Handle};
use crate::event_loop::{Clock, MonoTime, SharedClock};
use crate::fetch::FetchStartError;
use crate::xhr::{XhrHandle, XhrRequest};

/// What a mock listener does when invoked, to exercise re-entrant paths.
#[derive(Clone)]
enum Action {
    None,
    StopPropagation,
    StopImmediate,
    /// Cancel the event (honouring cancelable + passive, like the binding would).
    Cancel,
    /// Remove another listener mid-dispatch.
    Remove {
        target: NodeRef,
        type_: String,
        cb: CallbackRef,
        capture: bool,
    },
}

/// A hook the mock runs at each invocation, to observe dispatch state that is
/// cleared before `dispatch` returns (`composedPath()`).
type PathProbe = Box<dyn Fn(EventKey)>;

/// The value a mock callback returns. The dispatcher acts only on an event
/// handler's return value, and only on an exact `false` (an `EventHandler`
/// returns `any`), so the mock models the value rather than the comparison: the
/// V8 host performs the same strict test with `Value::is_false()`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MockReturn {
    /// Exactly `false`, which cancels.
    False,
    /// `true`, which does not.
    True,
    /// `undefined`, a falsy value that is not `false`.
    Undefined,
    /// `0`, also falsy and also not `false`.
    Zero,
}

impl MockReturn {
    fn cancels(self) -> bool {
        matches!(self, MockReturn::False)
    }
}

struct MockHost {
    ctx: RefCell<Weak<HostContext>>,
    /// (callback bits, current_target bits, phase) in invocation order.
    calls: RefCell<Vec<(u64, u64, u16)>>,
    actions: RefCell<HashMap<u64, Action>>,
    /// What each callback returns when invoked; unlisted callbacks return
    /// `undefined`.
    returns: RefCell<HashMap<u64, MockReturn>>,
    /// Callbacks that observed `in_passive_listener == true` when invoked.
    passive_seen: RefCell<Vec<u64>>,
    /// Callbacks whose stored value was released.
    released: RefCell<Vec<u64>>,
    /// Called with the event key at each invocation.
    path_probe: RefCell<Option<PathProbe>>,
    /// Every [`ScriptHost::build_value`] request, as `(bits, description)`.
    built: RefCell<Vec<(u64, String)>>,
    /// The bits of every value [`ScriptHost::throw_value`] was asked to throw.
    thrown: RefCell<Vec<u64>>,
}

/// The bits the mock hands back for `undefined`. It is not `AnyValue::NULL`,
/// because the two must be distinguishable in an assertion — a `reason` that is
/// `undefined` and one that is `null` are different outcomes.
const MOCK_UNDEFINED: u64 = AnyValue::NULL.0 - 1;

/// The bits the mock hands back for a `DOMException`. Every call returns the
/// same ones, so a test can assert that the reason a signal stores and the
/// value `throwIfAborted` throws are the same by identity.
const MOCK_DOM_EXCEPTION: u64 = AnyValue::NULL.0 - 2;

/// The bits the mock hands back for a JavaScript string.
const MOCK_STRING: u64 = AnyValue::NULL.0 - 3;

/// The bits the mock hands back for a `TypeError`.
const MOCK_TYPE_ERROR: u64 = AnyValue::NULL.0 - 4;

impl MockHost {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            ctx: RefCell::new(Weak::new()),
            calls: RefCell::new(Vec::new()),
            actions: RefCell::new(HashMap::new()),
            returns: RefCell::new(HashMap::new()),
            passive_seen: RefCell::new(Vec::new()),
            released: RefCell::new(Vec::new()),
            path_probe: RefCell::new(None),
            built: RefCell::new(Vec::new()),
            thrown: RefCell::new(Vec::new()),
        })
    }

    fn set_action(&self, cb: CallbackRef, action: Action) {
        self.actions.borrow_mut().insert(cb.0, action);
    }

    fn set_return(&self, cb: CallbackRef, value: MockReturn) {
        self.returns.borrow_mut().insert(cb.0, value);
    }

    fn set_path_probe(&self, probe: impl Fn(EventKey) + 'static) {
        *self.path_probe.borrow_mut() = Some(Box::new(probe));
    }
}

impl ScriptHost for MockHost {
    fn invoke_listener(&self, callback: CallbackRef, current_target: u64, event: u64) -> bool {
        let ctx = self
            .ctx
            .borrow()
            .upgrade()
            .expect("ctx alive during dispatch");
        let key = decode(event).event_key();

        let (phase, passive) = {
            let events = ctx.events.borrow();
            let record = events.get(key).expect("record alive");
            (record.phase, record.flags.in_passive_listener)
        };
        self.calls
            .borrow_mut()
            .push((callback.0, current_target, phase));
        if passive {
            self.passive_seen.borrow_mut().push(callback.0);
        }
        if let Some(probe) = self.path_probe.borrow().as_ref() {
            probe(key);
        }

        let action = self
            .actions
            .borrow()
            .get(&callback.0)
            .cloned()
            .unwrap_or(Action::None);
        match action {
            Action::None => {}
            Action::StopPropagation => {
                if let Some(r) = ctx.events.borrow_mut().get_mut(key) {
                    r.flags.stop_propagation = true;
                }
            }
            Action::StopImmediate => {
                if let Some(r) = ctx.events.borrow_mut().get_mut(key) {
                    r.flags.stop_propagation = true;
                    r.flags.stop_immediate = true;
                }
            }
            Action::Cancel => {
                if let Some(r) = ctx.events.borrow_mut().get_mut(key)
                    && r.cancelable
                    && !r.flags.in_passive_listener
                {
                    r.flags.canceled = true;
                }
            }
            Action::Remove {
                target,
                type_,
                cb,
                capture,
            } => {
                ctx.listeners
                    .borrow_mut()
                    .remove(target, &type_, cb, capture);
            }
        }

        // The dispatcher asks only about an event handler's return value, and
        // acts on it only when it is exactly `false`.
        self.returns
            .borrow()
            .get(&callback.0)
            .copied()
            .unwrap_or(MockReturn::Undefined)
            .cancels()
    }

    fn release_value(&self, bits: u64) {
        self.released.borrow_mut().push(bits);
    }

    fn build_value(&self, value: HostValue<'_>) -> Option<u64> {
        let (bits, description) = match value {
            HostValue::Undefined => (MOCK_UNDEFINED, "undefined".to_owned()),
            HostValue::DomException { name, message } => {
                (MOCK_DOM_EXCEPTION, format!("{name}: {message}"))
            }
            HostValue::String(text) => (MOCK_STRING, format!("string:{text}")),
            HostValue::TypeError { message } => (MOCK_TYPE_ERROR, format!("TypeError: {message}")),
        };
        self.built.borrow_mut().push((bits, description));
        Some(bits)
    }

    fn throw_value(&self, bits: u64) {
        self.thrown.borrow_mut().push(bits);
    }

    /// The dispatcher never asks for a promise — its only members are listener
    /// invocation, and a listener's `return` value is discarded — so this mock
    /// models a world that cannot build one, which is the contract's `None`.
    fn new_promise(&self) -> Option<PromiseRef> {
        None
    }

    /// Unreachable for the same reason: nothing in this fixture can hold a
    /// [`PromiseRef`] to settle.
    fn settle_promise(&self, _resolver: u64, _outcome: PromiseOutcome<'_>) -> bool {
        false
    }

    /// Refused, because this fixture starts no requests: its dispatches are
    /// script's (`EventTarget.dispatchEvent`), and none of them is an
    /// `XMLHttpRequest` reaching a driver. It refuses rather than accepting, so
    /// a test that came to expect a request would observe the failure the
    /// contract names (["`Err` means no request was started for any
    /// reason"](ScriptHost::start_xhr)) instead of passing vacuously on a start
    /// nothing ever answered.
    fn start_xhr(&self, _request: XhrRequest, _handle: XhrHandle) -> Result<(), FetchStartError> {
        Err(FetchStartError::new(
            "the dispatcher mock starts no XMLHttpRequest",
        ))
    }

    /// `false` for the same reason [`Self::start_xhr`] refuses: this fixture
    /// never starts a request, so it never holds one to end.
    fn abort_xhr(&self, _handle: XhrHandle) -> bool {
        false
    }

    /// Unreachable, and by the refusal above rather than by luck: a timer is
    /// armed only for a request that started, and every start here is an `Err`.
    /// Panicking rather than dropping the task means a future caller cannot have
    /// its callback vanish silently.
    fn schedule_background_task(&self, _delay_ms: u64, _task: Box<dyn FnOnce()>) {
        unreachable!("the dispatcher mock starts no XMLHttpRequest, so it arms no timer");
    }
}

struct Fixture {
    ctx: Rc<HostContext>,
    mock: Rc<MockHost>,
    html: NodeRef,
    body: NodeRef,
    div: NodeRef,
}

/// `<html><body><div></div></body></html>`, with the mock host installed.
fn fixture() -> Fixture {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let div = document.create_html_element(atoms::DIV);
    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .unwrap();
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .unwrap();
    document
        .mutate(Mutation::Append {
            parent: body,
            child: div,
        })
        .unwrap();
    let html = document.node_ref(html);
    let body = document.node_ref(body);
    let div = document.node_ref(div);

    let clock = SharedClock::new(Clock::virtual_at(MonoTime::ZERO));
    let ctx = HostContext::new(
        Rc::new(RefCell::new(document)),
        clock,
        tenta_net::CookieJar::new(),
    );
    let mock = MockHost::new();
    *mock.ctx.borrow_mut() = Rc::downgrade(&ctx);
    *ctx.script.borrow_mut() = Some(mock.clone() as Rc<dyn ScriptHost>);
    Fixture {
        ctx,
        mock,
        html,
        body,
        div,
    }
}

/// Inserts an initialized event of the given type/bubbles into the slab.
fn make_event(ctx: &HostContext, type_: &str, bubbles: bool, cancelable: bool) -> EventKey {
    let mut record = EventRecord::new(type_.to_owned(), EventKind::Plain, 0.0);
    record.bubbles = bubbles;
    record.cancelable = cancelable;
    record.flags.initialized = true;
    ctx.events.borrow_mut().insert(record)
}

fn add(ctx: &HostContext, target: NodeRef, type_: &str, cb: u64, capture: bool) -> ListenerId {
    ctx.listeners
        .borrow_mut()
        .add(
            target,
            type_.to_owned(),
            CallbackRef(cb),
            capture,
            false,
            false,
        )
        .expect("listener added")
}

/// The kind is what picks the wrapper prototype, so every kind reports its own
/// interface name and no two share one.
#[test]
fn each_event_kind_reports_its_interface_name() {
    let mouse = EventKind::Mouse {
        detail: 0,
        screen_x: 0,
        screen_y: 0,
        client_x: 0,
        client_y: 0,
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
        button: 0,
        buttons: 0,
        related_target: None,
    };
    let cases = [
        (EventKind::Plain, "Event"),
        (EventKind::Custom { detail: None }, "CustomEvent"),
        (EventKind::Ui { detail: 0 }, "UIEvent"),
        (mouse, "MouseEvent"),
        (
            EventKind::Progress {
                length_computable: false,
                loaded: 0.0,
                total: 0.0,
            },
            "ProgressEvent",
        ),
    ];
    for (kind, name) in &cases {
        assert_eq!(kind.interface_name(), *name);
    }
    let mut names: Vec<&str> = cases.iter().map(|(_, name)| *name).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(names.len(), count, "two kinds would pick one prototype");
}

#[test]
fn capture_then_bubble_order_and_phases() {
    let f = fixture();
    // capture listeners on html, body; bubble listeners on body, html; target div.
    add(&f.ctx, f.html, "x", 1, true);
    add(&f.ctx, f.body, "x", 2, true);
    add(&f.ctx, f.div, "x", 3, true); // AT_TARGET, capture flag
    add(&f.ctx, f.div, "x", 4, false); // AT_TARGET, bubble flag
    add(&f.ctx, f.body, "x", 5, false);
    add(&f.ctx, f.html, "x", 6, false);

    let key = make_event(&f.ctx, "x", true, false);
    let result = dispatch(&f.ctx, f.div, key, false).unwrap();
    assert!(result); // not canceled

    let calls = f.mock.calls.borrow();
    let order: Vec<u64> = calls.iter().map(|c| c.0).collect();
    assert_eq!(order, vec![1, 2, 3, 4, 5, 6]);
    // phases: html/body capture = 1, div = 2 (AT_TARGET) for both its listeners,
    // body/html bubble = 3.
    let phases: Vec<u16> = calls.iter().map(|c| c.2).collect();
    assert_eq!(phases, vec![1, 1, 2, 2, 3, 3]);
}

#[test]
fn non_bubbling_event_only_hits_target_in_bubble_pass() {
    let f = fixture();
    add(&f.ctx, f.html, "x", 1, true); // capture still runs
    add(&f.ctx, f.div, "x", 2, false); // target bubble-flag runs
    add(&f.ctx, f.body, "x", 3, false); // ancestor bubble must NOT run
    let key = make_event(&f.ctx, "x", false, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();
    let order: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
    assert_eq!(order, vec![1, 2]);
}

#[test]
fn duplicate_listener_is_rejected() {
    let f = fixture();
    let first = f.ctx.listeners.borrow_mut().add(
        f.div,
        "x".to_owned(),
        CallbackRef(1),
        false,
        false,
        false,
    );
    let dup = f.ctx.listeners.borrow_mut().add(
        f.div,
        "x".to_owned(),
        CallbackRef(1),
        false,
        false,
        false,
    );
    assert!(first.is_some());
    assert!(dup.is_none());
    // But a different capture flag is a distinct listener.
    let cap =
        f.ctx
            .listeners
            .borrow_mut()
            .add(f.div, "x".to_owned(), CallbackRef(1), true, false, false);
    assert!(cap.is_some());
    assert_eq!(f.ctx.listeners.borrow().len(), 2);
}

#[test]
fn once_listener_fires_once_and_is_removed() {
    let f = fixture();
    let id = f
        .ctx
        .listeners
        .borrow_mut()
        .add(f.div, "x".to_owned(), CallbackRef(1), false, true, false)
        .unwrap();
    let key = make_event(&f.ctx, "x", false, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();
    assert_eq!(f.mock.calls.borrow().len(), 1);
    assert!(!f.ctx.listeners.borrow().contains(f.div, id));
    assert_eq!(f.mock.released.borrow().as_slice(), &[1]);

    // A second dispatch invokes nothing.
    let key2 = make_event(&f.ctx, "x", false, false);
    dispatch(&f.ctx, f.div, key2, false).unwrap();
    assert_eq!(f.mock.calls.borrow().len(), 1);
}

/// A listener registered on a node must die with it. The arena reuses a freed
/// slot for a different node under a bumped generation, so a dispatch whose
/// path contains that index must not match the stale registration and fire it
/// against the unrelated new node.
#[test]
fn a_listener_does_not_survive_into_a_node_that_reuses_its_slot() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);

    // Detach the div and sweep it: it has no wrapper and no Rust handle, so the
    // arena frees its slot, and the next allocation takes that index back.
    let recycled = {
        let mut doc = f.ctx.doc.borrow_mut();
        doc.mutate(Mutation::Remove { child: f.div.id }).unwrap();
        doc.sweep_detached();
        let span = doc.create_html_element(atoms::SPAN);
        assert_eq!(span, f.div.id, "the arena hands the freed slot back");
        assert_ne!(
            doc.node_ref(span).generation,
            f.div.generation,
            "the reused slot carries a bumped generation"
        );
        doc.node_ref(span)
    };

    let key = make_event(&f.ctx, "x", true, false);
    dispatch(&f.ctx, recycled, key, false).unwrap();

    assert!(
        f.mock.calls.borrow().is_empty(),
        "the stale listener must not fire for the node that reused its slot; \
         (callback, current_target, phase) calls: {:?}",
        f.mock.calls.borrow()
    );
    let events = f.ctx.events.borrow();
    let record = events.get(key).unwrap();
    assert_eq!(record.target, Some(encode_node_test(recycled)));
    assert_eq!(
        record.current_target, None,
        "no stale listener ran, so no current target was bound to the new node"
    );
}

#[test]
fn stop_propagation_halts_after_current_node() {
    let f = fixture();
    add(&f.ctx, f.html, "x", 1, true);
    add(&f.ctx, f.body, "x", 2, true);
    add(&f.ctx, f.div, "x", 3, false);
    f.mock.set_action(CallbackRef(2), Action::StopPropagation);
    let key = make_event(&f.ctx, "x", true, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();
    let order: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
    assert_eq!(order, vec![1, 2]); // div's listener never reached
}

#[test]
fn stop_immediate_halts_remaining_listeners_on_same_node() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);
    add(&f.ctx, f.div, "x", 2, false);
    add(&f.ctx, f.div, "x", 3, false);
    f.mock.set_action(CallbackRef(1), Action::StopImmediate);
    let key = make_event(&f.ctx, "x", true, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();
    let order: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
    assert_eq!(order, vec![1]);
}

#[test]
fn listener_removed_during_dispatch_is_skipped() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);
    add(&f.ctx, f.div, "x", 2, false);
    // Listener 1 removes listener 2 before it runs.
    f.mock.set_action(
        CallbackRef(1),
        Action::Remove {
            target: f.div,
            type_: "x".to_owned(),
            cb: CallbackRef(2),
            capture: false,
        },
    );
    let key = make_event(&f.ctx, "x", true, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();
    let order: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
    assert_eq!(order, vec![1]);
}

#[test]
fn prevent_default_cancels_a_cancelable_event() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);
    f.mock.set_action(CallbackRef(1), Action::Cancel);
    let key = make_event(&f.ctx, "x", true, true);
    let result = dispatch(&f.ctx, f.div, key, false).unwrap();
    assert!(!result);
    assert!(f.ctx.events.borrow().get(key).unwrap().flags.canceled);
}

#[test]
fn prevent_default_on_non_cancelable_event_is_ignored() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);
    f.mock.set_action(CallbackRef(1), Action::Cancel);
    let key = make_event(&f.ctx, "x", true, false);
    let result = dispatch(&f.ctx, f.div, key, false).unwrap();
    assert!(result); // stayed uncanceled
    assert!(!f.ctx.events.borrow().get(key).unwrap().flags.canceled);
}

#[test]
fn passive_flag_is_visible_to_the_listener() {
    let f = fixture();
    f.ctx
        .listeners
        .borrow_mut()
        .add(f.div, "x".to_owned(), CallbackRef(1), false, false, true)
        .unwrap();
    let key = make_event(&f.ctx, "x", true, true);
    dispatch(&f.ctx, f.div, key, false).unwrap();
    assert_eq!(f.mock.passive_seen.borrow().as_slice(), &[1]);
    // And after dispatch the flag is cleared.
    assert!(
        !f.ctx
            .events
            .borrow()
            .get(key)
            .unwrap()
            .flags
            .in_passive_listener
    );
}

#[test]
fn dispatching_uninitialized_event_throws_invalid_state() {
    let f = fixture();
    let mut record = EventRecord::new("x".to_owned(), EventKind::Plain, 0.0);
    record.flags.initialized = false;
    let key = f.ctx.events.borrow_mut().insert(record);
    let err = dispatch(&f.ctx, f.div, key, false).unwrap_err();
    assert_eq!(err.dom_name(), Some("InvalidStateError"));
}

#[test]
fn target_and_current_target_are_set_then_cleared() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);
    let key = make_event(&f.ctx, "x", true, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();
    let record = f.ctx.events.borrow();
    let record = record.get(key).unwrap();
    assert_eq!(record.target, Some(encode_node_test(f.div)));
    assert_eq!(record.current_target, None);
    assert_eq!(record.phase, PHASE_NONE);
    // current_target was the div while the listener ran.
    assert_eq!(f.mock.calls.borrow()[0].1, encode_node_test(f.div));
}

fn encode_node_test(node: NodeRef) -> u64 {
    crate::bindings::host::encode_node(0, node)
}

// ---------------------------------------------------------------------------
// The window as a target
// ---------------------------------------------------------------------------

/// Registers a listener on the window.
fn add_window(ctx: &HostContext, type_: &str, cb: u64, capture: bool) -> ListenerId {
    ctx.listeners
        .borrow_mut()
        .add_at(
            TargetKey::Window,
            type_.to_owned(),
            CallbackRef(cb),
            capture,
            false,
            false,
        )
        .expect("listener added")
}

/// The window is not an ancestor in the tree, but it is the outermost entry of
/// the event path: this is the whole reason `window.addEventListener` is worth
/// having, since a page's own listeners are almost never on `document`.
#[test]
fn a_window_listener_fires_for_an_event_dispatched_at_a_descendant() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);
    add_window(&f.ctx, "x", 2, false);

    let key = make_event(&f.ctx, "x", true, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();

    let calls = f.mock.calls.borrow();
    let order: Vec<u64> = calls.iter().map(|c| c.0).collect();
    assert_eq!(order, vec![1, 2], "the window runs last in the bubble pass");
    // The window entry is the outermost, so it is visited first in the capture
    // pass — nothing else in this fixture captures.
    assert_eq!(calls[1].1, encode_window());
    assert_eq!(calls[1].2, PHASE_BUBBLING);
}

#[test]
fn a_capturing_window_listener_runs_before_every_node() {
    let f = fixture();
    add(&f.ctx, f.html, "x", 1, true);
    add_window(&f.ctx, "x", 2, true);

    let key = make_event(&f.ctx, "x", true, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();

    let calls = f.mock.calls.borrow();
    let order: Vec<u64> = calls.iter().map(|c| c.0).collect();
    assert_eq!(order, vec![2, 1], "the window captures from the outside in");
    assert_eq!(calls[0].2, PHASE_CAPTURING);
}

/// A non-bubbling event stops at the target, so the window is reached only by a
/// capture listener. This is what keeps `focus`-style events from running every
/// bubble-phase window listener in the page on every dispatch.
#[test]
fn a_non_bubbling_event_reaches_the_window_only_in_capture() {
    let f = fixture();
    add_window(&f.ctx, "x", 1, false);
    let key = make_event(&f.ctx, "x", false, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();
    assert!(f.mock.calls.borrow().is_empty());

    add_window(&f.ctx, "x", 2, true);
    let key = make_event(&f.ctx, "x", false, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();
    let order: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
    assert_eq!(order, vec![2]);
}

/// The same callback registered on a node and on the window is two registrations
/// on two targets: removing one must not remove the other.
#[test]
fn a_node_and_the_window_are_independent_targets_for_one_callback() {
    let f = fixture();
    let on_div = add(&f.ctx, f.div, "x", 7, false);
    let on_window = add_window(&f.ctx, "x", 7, false);

    let removed = f
        .ctx
        .listeners
        .borrow_mut()
        .remove_by_id_at(TargetKey::Window, on_window);
    assert_eq!(removed, Some(CallbackRef(7)));
    assert!(
        f.ctx
            .listeners
            .borrow()
            .contains_at(TargetKey::Node(f.div), on_div)
    );
    assert!(
        !f.ctx
            .listeners
            .borrow()
            .contains_at(TargetKey::Window, on_window)
    );

    let key = make_event(&f.ctx, "x", true, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();
    let order: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
    assert_eq!(order, vec![7], "only the node's registration is left");
}

/// A window listener removed by an earlier listener in the same dispatch is
/// skipped, exactly as a node's is: the removal check is on the target, not on
/// the kind of target.
#[test]
fn a_window_listener_removed_during_dispatch_is_skipped() {
    let f = fixture();
    add_window(&f.ctx, "x", 1, false);
    let second = add_window(&f.ctx, "x", 2, false);

    // The first callback removes the second while the dispatch is running.
    let weak = Rc::downgrade(&f.ctx);
    f.mock.set_path_probe(move |_| {
        if let Some(ctx) = weak.upgrade() {
            ctx.listeners
                .borrow_mut()
                .remove_by_id_at(TargetKey::Window, second);
        }
    });

    let key = make_event(&f.ctx, "x", true, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();

    let order: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
    assert_eq!(order, vec![1], "the second window listener was removed");
    assert!(
        !f.ctx
            .listeners
            .borrow()
            .contains_at(TargetKey::Window, second)
    );
}

/// `composedPath()` is the path the dispatcher walked, window included and last.
#[test]
fn the_recorded_path_ends_with_the_window() {
    let f = fixture();
    add_window(&f.ctx, "x", 1, false);

    let path_seen: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = path_seen.clone();
    // A `Weak` here: the probe lives inside the mock, which the context holds,
    // so a strong handle would be a cycle.
    let weak = Rc::downgrade(&f.ctx);
    f.mock.set_path_probe(move |k| {
        if let Some(ctx) = weak.upgrade() {
            let events = ctx.events.borrow();
            if let Some(record) = events.get(k) {
                *seen.borrow_mut() = record.path.clone();
            }
        }
    });

    let key = make_event(&f.ctx, "x", true, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();

    let path = path_seen.borrow();
    assert_eq!(path.len(), 5, "div, body, html, document, window");
    assert_eq!(*path.last().unwrap(), encode_window());
    assert_eq!(path[0], encode_node_test(f.div));
    let document_node = {
        let doc = f.ctx.doc.borrow();
        doc.node_ref(doc.document_node())
    };
    assert_eq!(
        path[3],
        encode_node_test(document_node),
        "the document is the last node, and the window sits outside it"
    );

    // Cleared outside a dispatch, so `composedPath()` is empty then.
    let events = f.ctx.events.borrow();
    assert!(events.get(key).expect("record alive").path.is_empty());
}

/// A dispatch *at* the window: no ancestors to walk and nothing above it, so
/// the path is the window alone. This is what `window.dispatchEvent(...)` and
/// the runtime's own `load` dispatch do, and it is why the window's path cannot
/// be built by walking the tree.
#[test]
fn a_dispatch_at_the_window_has_the_window_as_its_whole_path() {
    let f = fixture();
    add_window(&f.ctx, "x", 1, false);
    // A listener on an element must not see a window-targeted event, even
    // though the window is on every element's path.
    add(&f.ctx, f.div, "x", 2, false);

    let path_seen: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = path_seen.clone();
    let weak = Rc::downgrade(&f.ctx);
    f.mock.set_path_probe(move |k| {
        if let Some(ctx) = weak.upgrade() {
            let events = ctx.events.borrow();
            if let Some(record) = events.get(k) {
                *seen.borrow_mut() = record.path.clone();
            }
        }
    });

    let key = make_event(&f.ctx, "x", true, false);
    dispatch_at(&f.ctx, DispatchTarget::window(), key, false).unwrap();

    let calls = f.mock.calls.borrow();
    let order: Vec<u64> = calls.iter().map(|c| c.0).collect();
    assert_eq!(order, vec![1], "only the window's own listener ran");
    assert_eq!(calls[0].1, encode_window());
    // The window is the target, so both passes see it AT_TARGET — not once as
    // a capturing ancestor and once as a bubbling one.
    assert_eq!(calls[0].2, PHASE_AT_TARGET);

    assert_eq!(*path_seen.borrow(), vec![encode_window()]);
    // The target the record reports is the window, and it is cleared after.
    assert_eq!(
        f.ctx.events.borrow().get(key).unwrap().current_target,
        None,
        "the transient dispatch state is cleared"
    );
}

/// DOM's *legacy target override* is what lets a dispatch be addressed to one
/// object while `event.target` reports another, and HTML's `load` at the window
/// is its only user: `target` has to be the window's `Document`, but the
/// listener table and the propagation path still have to be the window's, or the
/// page's `window.addEventListener('load', …)` would never run.
#[test]
fn a_legacy_target_override_changes_event_target_but_not_the_path_or_the_listeners() {
    let f = fixture();
    add_window(&f.ctx, "x", 1, false);
    // A listener on an element must not see a window-targeted event, override
    // or not: the override is script-visible identity, not a second path.
    add(&f.ctx, f.div, "x", 2, false);

    let path_seen: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let target_seen: Rc<RefCell<Option<u64>>> = Rc::new(RefCell::new(None));
    let seen = (path_seen.clone(), target_seen.clone());
    let weak = Rc::downgrade(&f.ctx);
    f.mock.set_path_probe(move |k| {
        if let Some(ctx) = weak.upgrade() {
            let events = ctx.events.borrow();
            if let Some(record) = events.get(k) {
                *seen.0.borrow_mut() = record.path.clone();
                *seen.1.borrow_mut() = record.target;
            }
        }
    });

    let override_bits = encode_node_test(f.div);
    let key = make_event(&f.ctx, "x", false, false);
    dispatch_at(
        &f.ctx,
        DispatchTarget::window().with_target_override(override_bits),
        key,
        false,
    )
    .unwrap();

    assert_eq!(
        *target_seen.borrow(),
        Some(override_bits),
        "the override is what script sees as `event.target`"
    );
    assert_eq!(
        *path_seen.borrow(),
        vec![encode_window()],
        "the path still leads with the window, and the override adds no entry"
    );
    // The listener ran, and every listener on the path was handed the override
    // as its current target rather than the element the override names.
    let calls = f.mock.calls.borrow();
    assert_eq!(calls.len(), 1, "only the window's own listener ran");
    assert_eq!(calls[0].0, 1);
    assert_eq!(calls[0].1, encode_window());
}

// ---------------------------------------------------------------------------
// Draining the listeners of nodes a sweep freed
// ---------------------------------------------------------------------------

/// A sweep records the nodes it freed on the document; draining them releases
/// the entries keyed by those references and the stored callback each owned. A
/// `CallbackRef` has no `Drop`, so a drain that only removed the entries would
/// leak a V8 reference per listener — the mock host records what was released.
#[test]
fn draining_the_sweeps_freed_nodes_removes_and_releases_their_listeners() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);
    add(&f.ctx, f.div, "y", 2, false);
    let on_body = add(&f.ctx, f.body, "x", 3, false);

    // Detach the div and sweep it: no wrapper and no handle, so it is freed and
    // its reference is what the engine has to drain.
    {
        let mut doc = f.ctx.doc.borrow_mut();
        doc.mutate(Mutation::Remove { child: f.div.id })
            .expect("detach the div");
        assert_eq!(doc.sweep_detached().nodes_freed, 1);
        assert_eq!(
            doc.sweep_freed_nodes(),
            &[f.div],
            "the sweep records the freed node for the drain"
        );
    }
    assert_eq!(
        f.ctx.listeners.borrow().len(),
        3,
        "the sweep itself holds no stored reference and releases none"
    );
    assert!(f.mock.released.borrow().is_empty());

    f.ctx.drain_swept_listeners();

    assert_eq!(
        f.ctx.listeners.borrow().len(),
        1,
        "both of the freed div's entries are gone"
    );
    assert!(
        f.ctx.listeners.borrow().contains(f.body, on_body),
        "a live node's listener is not touched by the drain"
    );
    let mut released = f.mock.released.borrow().clone();
    released.sort_unstable();
    assert_eq!(
        released,
        vec![1, 2],
        "each drained callback's stored reference is released"
    );

    // The references stay recorded until the next sweep, so a trim that runs
    // again before then must not release the same references a second time.
    f.ctx.drain_swept_listeners();
    assert_eq!(
        f.mock.released.borrow().as_slice(),
        &[1, 2],
        "a second drain over the same references releases nothing"
    );
}

/// The drain of a freed node must not remove a listener registered on the node
/// that reuses its slot. The slot keeps the freed node's id, so a drain that
/// matched on the id alone would take the live listener with it; the recorded
/// reference carries the generation the freed node lived under, so it matches
/// only that node's own entries.
#[test]
fn a_reused_slots_listener_survives_a_drain_of_the_freed_nodes_ref() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);

    // Detach and sweep the div: its slot is freed, so the next allocation
    // takes the id back under a bumped generation.
    let reused = {
        let mut doc = f.ctx.doc.borrow_mut();
        doc.mutate(Mutation::Remove { child: f.div.id })
            .expect("detach the div");
        assert_eq!(doc.sweep_detached().nodes_freed, 1);
        assert_eq!(
            doc.sweep_freed_nodes(),
            &[f.div],
            "the sweep records the reference the div lived under"
        );
        let span = doc.create_html_element(atoms::SPAN);
        let node = doc.node_ref(span);
        assert_eq!(node.id, f.div.id, "the arena reuses the freed slot");
        assert_ne!(
            node.generation, f.div.generation,
            "the reused slot carries a bumped generation"
        );
        node
    };

    // Script registers a listener on the node that took the slot; the drain
    // that follows must leave it alone.
    let on_reused = add(&f.ctx, reused, "x", 2, false);

    f.ctx.drain_swept_listeners();

    assert!(
        f.ctx.listeners.borrow().contains(reused, on_reused),
        "the listener on the reused slot survives the drain"
    );
    assert_eq!(
        f.ctx.listeners.borrow().len(),
        1,
        "only the freed node's own entry was drained"
    );
    assert_eq!(
        f.mock.released.borrow().as_slice(),
        &[1],
        "and only the freed node's callback was released"
    );
}

// ---------------------------------------------------------------------------
// A JS-owned host object as a target
// ---------------------------------------------------------------------------

/// Inserts a fresh host record into the context's host slab and returns its key.
///
/// The record's payload is irrelevant to the dispatch algorithm — only the key
/// is, because a host target's propagation path is the target alone — so the
/// cheapest record stands in for an `AbortSignal`.
fn host_key(ctx: &HostContext) -> HostKey {
    ctx.host_records
        .borrow_mut()
        .insert(crate::bindings::host::HostRecord::Headers(
            crate::bindings::host::HeadersRecord {
                list: Vec::new(),
                guard: crate::bindings::host::HeadersGuard::None,
                response: None,
                has_wrapper: false,
            },
        ))
}

#[test]
fn a_host_target_is_its_own_listener_bucket() {
    let f = fixture();
    let first = host_key(&f.ctx);
    let second = host_key(&f.ctx);
    assert_ne!(first, second, "two records are two keys");

    let on_first = f
        .ctx
        .listeners
        .borrow_mut()
        .add_at(
            TargetKey::Host(first),
            "abort".to_owned(),
            CallbackRef(1),
            false,
            false,
            false,
        )
        .expect("listener added");
    let on_second = f
        .ctx
        .listeners
        .borrow_mut()
        .add_at(
            TargetKey::Host(second),
            "abort".to_owned(),
            CallbackRef(2),
            false,
            false,
            false,
        )
        .expect("listener added");

    let key = make_event(&f.ctx, "abort", false, false);
    dispatch_at(&f.ctx, DispatchTarget::host(f.ctx.slot, first), key, true).unwrap();

    assert_eq!(
        f.mock
            .calls
            .borrow()
            .iter()
            .map(|c| c.0)
            .collect::<Vec<_>>(),
        [1],
        "only the dispatched-at key's listener runs"
    );
    assert_eq!(
        f.ctx.events.borrow().get(key).unwrap().target,
        Some(crate::bindings::host::encode_host(f.ctx.slot, first)),
        "script sees the host's own handle as the target"
    );

    let drained = f.ctx.drain_host_listeners(&[first]);
    assert_eq!(
        drained.iter().map(|cb| cb.0).collect::<Vec<_>>(),
        [1],
        "the drain reaches exactly the drained key's listener"
    );
    assert!(
        !f.ctx
            .listeners
            .borrow()
            .contains_at(TargetKey::Host(first), on_first)
    );
    assert!(
        f.ctx
            .listeners
            .borrow()
            .contains_at(TargetKey::Host(second), on_second),
        "a different host key is untouched"
    );
    assert!(
        !f.ctx
            .listeners
            .borrow()
            .contains_at(TargetKey::Host(second), on_first),
        "the drain of one key never reaches another key's bucket"
    );
}

// ---------------------------------------------------------------------------
// Event handler IDL attributes (HTML §8.1.8.1)
// ---------------------------------------------------------------------------

/// The value an `onX` setter receives: WebIDL's `EventHandlerNonNull`, which is
/// `Callback<(Handle<Event>,), AnyValue>`.
fn handler(bits: u64) -> Callback<(Handle<Event>,), AnyValue> {
    Callback::from_bits(bits)
}

/// The handler name these tests use. Its event type is `x`, which is what the
/// fixture's events are dispatched with.
fn on_x() -> EventHandlerName {
    EventHandlerName::new("onx")
}

/// Sets the fixture div's `onx` handler, or deactivates it when `bits` is `None`
/// — the setter of an event handler IDL attribute.
fn set_onx(f: &Fixture, bits: Option<u64>) {
    f.ctx
        .set_event_handler(TargetKey::Node(f.div), on_x(), bits.map(handler));
}

/// Reads the fixture div's `onx` handler as the bits the page would see.
fn get_onx(f: &Fixture) -> Option<u64> {
    f.ctx
        .event_handler(TargetKey::Node(f.div), on_x())
        .map(|callback| callback.bits())
}

/// The handler's name is an `on`-prefixed string and its listener's type is what
/// follows. The legacy-prefixed names are the documented exception: their type
/// is not the stripped remainder, which is why nothing strips the prefix
/// silently.
#[test]
fn a_handler_name_derives_its_event_type_by_stripping_on() {
    assert_eq!(EventHandlerName::new("onload").as_str(), "onload");
    assert_eq!(EventHandlerName::new("onload").event_type(), "load");
    assert_eq!(on_x().event_type(), "x");
    assert_ne!(
        EventHandlerName::new("onwebkitanimationend").event_type(),
        "webkitAnimationEnd",
        "a legacy-prefixed handler's type is not the stripped remainder"
    );
}

/// The discriminating case. A handler registered by an assignment is an ordinary
/// listener, added where the assignment happened — *between* the listeners the
/// page registered before and after it. A design that ran the handler after all
/// of the target's listeners in the at-target phase would answer A, C, B.
#[test]
fn a_handler_takes_its_place_where_the_assignment_happens() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);
    set_onx(&f, Some(2));
    add(&f.ctx, f.div, "x", 3, false);

    let key = make_event(&f.ctx, "x", false, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();

    let order: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
    assert_eq!(order, vec![1, 2, 3]);
}

/// A reassignment changes the value in place: the listener the first non-null
/// assignment registered is reused, so the handler keeps its position and no
/// second listener is added.
#[test]
fn reassigning_a_handler_reuses_its_listener_in_place() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);
    set_onx(&f, Some(2));
    add(&f.ctx, f.div, "x", 3, false);
    set_onx(&f, Some(4));

    let key = make_event(&f.ctx, "x", false, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();

    let order: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
    assert_eq!(
        order,
        vec![1, 4, 3],
        "the handler fires where it first registered"
    );
    assert_eq!(f.ctx.listeners.borrow().len(), 3, "no listener was added");
    assert_eq!(
        f.mock.released.borrow().as_slice(),
        &[2],
        "the value the reassignment displaced is released exactly once"
    );
}

/// The spec's second worked example. A deactivation removes the handler's
/// listener outright, and a later assignment registers a fresh one at the end of
/// the list — so a handler re-activated after a deactivation fires after every
/// listener registered while it was unset.
#[test]
fn a_deactivated_handler_is_removed_and_re_registers_last() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false); // A
    set_onx(&f, Some(2)); // the handler's listener, activated
    add(&f.ctx, f.div, "x", 3, false); // C, registered after the activation
    set_onx(&f, None); // deactivated: its listener is removed
    add(&f.ctx, f.div, "x", 4, false); // D, registered while it is unset
    set_onx(&f, Some(5)); // re-activated, at the end of the list

    let key = make_event(&f.ctx, "x", false, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();

    let order: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
    assert_eq!(
        order,
        vec![1, 3, 4, 5],
        "the deactivated handler does not fire, and the re-activated one is last"
    );
}

/// A handler that returns exactly `false` cancels a cancelable event. `true`,
/// `undefined` and `0` do not: the comparison is strict, because an
/// `EventHandler` returns `any` and the standard's test is `=== false`.
#[test]
fn a_handler_returning_false_cancels_a_cancelable_event() {
    for (returned, cancels) in [
        (MockReturn::False, true),
        (MockReturn::True, false),
        (MockReturn::Undefined, false),
        (MockReturn::Zero, false),
    ] {
        let f = fixture();
        set_onx(&f, Some(1));
        f.mock.set_return(CallbackRef(1), returned);

        let key = make_event(&f.ctx, "x", false, true);
        let not_canceled = dispatch(&f.ctx, f.div, key, false).unwrap();

        assert_eq!(
            !not_canceled,
            cancels,
            "a handler returning {returned:?} must {} the event",
            if cancels { "cancel" } else { "not cancel" }
        );
        assert_eq!(
            f.ctx.events.borrow().get(key).unwrap().flags.canceled,
            cancels,
            "the canceled flag follows the same rule"
        );
    }
}

/// The canceling return is not gated on `cancelable`: the processing algorithm
/// sets the flag unconditionally, and a non-cancelable event's flag therefore
/// ends up set too. The two observations agree — `dispatch` reports canceled and
/// the flag is set — because `Event.defaultPrevented` reads the flag itself;
/// what the `cancelable` test gates is `preventDefault`, which a plain listener
/// on this same event calls without effect.
#[test]
fn a_handler_returning_false_sets_the_flag_on_a_non_cancelable_event() {
    let f = fixture();
    set_onx(&f, Some(1));
    f.mock.set_return(CallbackRef(1), MockReturn::False);

    let key = make_event(&f.ctx, "x", false, false);
    let not_canceled = dispatch(&f.ctx, f.div, key, false).unwrap();

    assert!(!not_canceled, "dispatch reports the event canceled");
    assert!(
        f.ctx.events.borrow().get(key).unwrap().flags.canceled,
        "so does the flag `Event.defaultPrevented` reads"
    );
}

/// The negative control for the flag: only a handler's return value is acted on,
/// so a listener the page registered with `addEventListener` that returns
/// `false` cancels nothing.
#[test]
fn a_plain_listeners_false_return_does_not_cancel() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);
    f.mock.set_return(CallbackRef(1), MockReturn::False);

    let key = make_event(&f.ctx, "x", false, true);
    let not_canceled = dispatch(&f.ctx, f.div, key, false).unwrap();

    assert!(
        not_canceled,
        "a plain listener's return value is meaningless"
    );
    assert!(!f.ctx.events.borrow().get(key).unwrap().flags.canceled);
}

/// The getter returns the value the page last assigned, and the same value
/// across two reads — the stored bits are handed back as they are, non-owning,
/// so `el.onx === el.onx` holds by identity rather than by construction.
#[test]
fn the_handler_getter_returns_the_assigned_value() {
    let f = fixture();
    assert_eq!(get_onx(&f), None, "an unset handler reads as null");

    set_onx(&f, Some(7));
    let first = f
        .ctx
        .event_handler(TargetKey::Node(f.div), on_x())
        .expect("the handler is set");
    assert_eq!(first.bits(), 7);
    assert_eq!(
        f.ctx.event_handler(TargetKey::Node(f.div), on_x()),
        Some(first),
        "two reads are the same value"
    );

    set_onx(&f, Some(8));
    assert_eq!(get_onx(&f), Some(8), "a reassignment is what reads back");

    set_onx(&f, None);
    assert_eq!(get_onx(&f), None, "a deactivation unsets the handler");
}

/// Deactivation is once-only: the listener leaves the table and the one stored
/// reference the entry owned is given back, so a second `= null` finds nothing
/// to release and a registration is what owns the reference until then.
#[test]
fn deactivating_a_handler_removes_its_listener_and_releases_once() {
    let f = fixture();
    add(&f.ctx, f.div, "x", 1, false);
    set_onx(&f, Some(2));
    assert_eq!(f.ctx.listeners.borrow().len(), 2);
    assert!(
        f.mock.released.borrow().is_empty(),
        "registering the handler owns its reference"
    );

    set_onx(&f, None);
    assert_eq!(
        f.ctx.listeners.borrow().len(),
        1,
        "the handler's listener is gone"
    );
    assert_eq!(f.mock.released.borrow().as_slice(), &[2]);

    set_onx(&f, None);
    assert_eq!(
        f.mock.released.borrow().as_slice(),
        &[2],
        "a second deactivation releases nothing"
    );
    assert_eq!(get_onx(&f), None);
}

/// The handler map can name a listener that is no longer registered: only the
/// setter writes the map, so a removal the setter does not perform — a sweep's
/// drain, a host reclamation — leaves the entry behind. A reassignment must
/// then register a fresh listener instead of writing the new value into the
/// dead entry, which would lose both the value and its reference.
///
/// `removeEventListener` is not such a removal: see
/// [`remove_event_listener_does_not_remove_a_handler`]. This is the real sweep
/// drain the fixture's other drain test performs, because that is the one path
/// that takes a target's listeners away without the setter's knowledge.
#[test]
fn a_reassignment_after_its_listener_was_drained_registers_a_fresh_one() {
    let f = fixture();
    set_onx(&f, Some(2));

    // Detach the div and sweep it: no wrapper and no handle, so it is freed and
    // its listeners are what the engine has to drain.
    {
        let mut doc = f.ctx.doc.borrow_mut();
        doc.mutate(Mutation::Remove { child: f.div.id })
            .expect("detach the div");
        assert_eq!(doc.sweep_detached().nodes_freed, 1);
        assert_eq!(doc.sweep_freed_nodes(), &[f.div]);
    }
    f.ctx.drain_swept_listeners();

    assert_eq!(
        f.mock.released.borrow().as_slice(),
        &[2],
        "the drain releases the handler's reference"
    );
    assert_eq!(
        get_onx(&f),
        None,
        "the getter does not resurrect a drained handler"
    );

    set_onx(&f, Some(9));

    assert_eq!(
        get_onx(&f),
        Some(9),
        "the new value is stored, not written into the dead entry"
    );
    assert_eq!(
        f.mock.released.borrow().as_slice(),
        &[2],
        "the drained entry's reference is not released twice"
    );
    assert_eq!(
        f.ctx.listeners.borrow().len(),
        1,
        "a fresh listener carries the handler"
    );
}

/// A handler and a plain listener are two entries even when the page passes one
/// function to both: HTML registers a handler's listener with an internal
/// callback the page cannot name, so the DOM duplicate rule that makes
/// `addEventListener('x', f)` twice a no-op does not apply between the two.
/// Both run, and the getter still returns the function the page assigned.
///
/// The order matters, because the duplicate rule is evaluated by whichever
/// registration comes second: a table that matched on callback identity alone
/// would answer one listener for one of these orders and two for the other.
#[test]
fn a_plain_listener_and_a_handler_with_the_same_function_are_two_listeners() {
    for handler_first in [false, true] {
        let f = fixture();
        let order = if handler_first {
            set_onx(&f, Some(5));
            add(&f.ctx, f.div, "x", 5, false);
            "handler first"
        } else {
            add(&f.ctx, f.div, "x", 5, false);
            set_onx(&f, Some(5));
            "plain listener first"
        };

        assert_eq!(
            f.ctx.listeners.borrow().len(),
            2,
            "{order}: two listeners, neither the other's duplicate"
        );
        assert_eq!(
            get_onx(&f),
            Some(5),
            "{order}: the handler map is written, so the getter opens"
        );
        assert!(
            f.mock.released.borrow().is_empty(),
            "{order}: each entry owns its own reference"
        );

        let key = make_event(&f.ctx, "x", false, false);
        dispatch(&f.ctx, f.div, key, false).unwrap();

        let calls: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
        assert_eq!(calls, vec![5, 5], "{order}: both listeners are invoked");
    }
}

/// `removeEventListener` cannot unhook a handler: it can only remove a listener
/// it registered, and a handler's listener is HTML's own internal registration.
/// The removal finds nothing, the handler keeps firing, and the argument's
/// reference is still released, because that is what the binding does with
/// every callback argument it is handed.
#[test]
fn remove_event_listener_does_not_remove_a_handler() {
    let f = fixture();
    set_onx(&f, Some(5));

    // `EventTargetOps::remove_event_listener`'s body, which the fixture cannot
    // call directly: remove by `(type, callback, capture)`, then release the
    // argument's stored reference, then any removed entry's.
    let removed =
        f.ctx
            .listeners
            .borrow_mut()
            .remove_at(TargetKey::Node(f.div), "x", CallbackRef(5), false);
    f.ctx
        .script
        .borrow()
        .as_ref()
        .expect("the fixture installs a script host")
        .release_value(5);

    assert!(removed.is_none(), "nothing was removed");
    assert_eq!(
        f.ctx.listeners.borrow().len(),
        1,
        "the handler's listener is still registered"
    );
    assert_eq!(get_onx(&f), Some(5), "and the handler is still set");
    assert_eq!(
        f.mock.released.borrow().as_slice(),
        &[5],
        "the argument's own reference is released even though nothing matched"
    );

    let key = make_event(&f.ctx, "x", false, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();

    let calls: Vec<u64> = f.mock.calls.borrow().iter().map(|c| c.0).collect();
    assert_eq!(calls, vec![5], "the handler still fires");
}
