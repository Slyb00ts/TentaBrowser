//! Unit tests for the engine-independent dispatch algorithm, driven by a
//! recording mock [`ScriptHost`] over a real `tenta_dom::Document` (no V8).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use tenta_core::atom::atoms;
use tenta_core::{NodeId, NodeRef};
use tenta_dom::{Document, Mutation};

use super::*;
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
        target: NodeId,
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
}

impl MockHost {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            ctx: RefCell::new(Weak::new()),
            calls: RefCell::new(Vec::new()),
            actions: RefCell::new(HashMap::new()),
            passive_seen: RefCell::new(Vec::new()),
            released: RefCell::new(Vec::new()),
            path_probe: RefCell::new(None),
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
            target.id,
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
        f.div.id,
        "x".to_owned(),
        CallbackRef(1),
        false,
        false,
        false,
    );
    let dup = f.ctx.listeners.borrow_mut().add(
        f.div.id,
        "x".to_owned(),
        CallbackRef(1),
        false,
        false,
        false,
    );
    assert!(first.is_some());
    assert!(dup.is_none());
    // But a different capture flag is a distinct listener.
    let cap = f.ctx.listeners.borrow_mut().add(
        f.div.id,
        "x".to_owned(),
        CallbackRef(1),
        true,
        false,
        false,
    );
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
        .add(f.div.id, "x".to_owned(), CallbackRef(1), false, true, false)
        .unwrap();
    let key = make_event(&f.ctx, "x", false, false);
    dispatch(&f.ctx, f.div, key, false).unwrap();
    assert_eq!(f.mock.calls.borrow().len(), 1);
    assert!(!f.ctx.listeners.borrow().contains(f.div.id, id));
    assert_eq!(f.mock.released.borrow().as_slice(), &[1]);

    // A second dispatch invokes nothing.
    let key2 = make_event(&f.ctx, "x", false, false);
    dispatch(&f.ctx, f.div, key2, false).unwrap();
    assert_eq!(f.mock.calls.borrow().len(), 1);
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
            target: f.div.id,
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
        .add(f.div.id, "x".to_owned(), CallbackRef(1), false, false, true)
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
            .contains_at(TargetKey::Node(f.div.id), on_div)
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
