//! Unit tests for the engine-independent dispatch algorithm, driven by a
//! recording mock [`ScriptHost`] over a real `tenta_dom::Document` (no V8).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use tenta_core::NodeRef;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};

use super::*;
use crate::bindings::AnyValue;
use crate::bindings::host::{HostContext, decode};
use crate::event_loop::{Clock, MonoTime, SharedClock};

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

struct MockHost {
    ctx: RefCell<Weak<HostContext>>,
    /// (callback bits, current_target bits, phase) in invocation order.
    calls: RefCell<Vec<(u64, u64, u16)>>,
    actions: RefCell<HashMap<u64, Action>>,
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

    fn set_path_probe(&self, probe: impl Fn(EventKey) + 'static) {
        *self.path_probe.borrow_mut() = Some(Box::new(probe));
    }
}

impl ScriptHost for MockHost {
    fn invoke_listener(&self, callback: CallbackRef, current_target: u64, event: u64) {
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
    let ctx = Rc::new(HostContext::new(Rc::new(RefCell::new(document)), clock));
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
    crate::bindings::host::encode_node(node)
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
    dispatch_at(&f.ctx, DispatchTarget::host(first), key, true).unwrap();

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
        Some(crate::bindings::host::encode_host(first)),
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
