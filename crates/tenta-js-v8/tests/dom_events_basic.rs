//! Tests for the DOM event model wired through V8 (Phase 3a): EventTarget
//! (`addEventListener`/`removeEventListener`/`dispatchEvent`), Event
//! (`initEvent`, phase/target/flag getters, `preventDefault`,
//! `stopImmediatePropagation`) and `Document.createEvent`.
//!
//! These exercise the full DOM→JS callback path: a listener registered from
//! script is invoked by the engine-independent dispatcher through the
//! `V8ScriptHost` boundary, with the event and current target resolved back to
//! their wrappers.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// `<html><body><div id="target"></div></body></html>`
fn test_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let div = document.create_html_element(atoms::DIV);

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
    document
        .mutate(Mutation::Append {
            parent: body,
            child: div,
        })
        .expect("append <div>");
    document
        .mutate(Mutation::SetAttribute {
            element: div,
            name: tenta_html::QualName::new(atoms::EMPTY, atoms::ID),
            value: "target".to_owned(),
        })
        .expect("set id attribute");

    Rc::new(RefCell::new(document))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn dispatch_invokes_listener_with_target_and_phase() {
    // A single listener sees the correct type, target, currentTarget, phase and
    // init flags; the return value is `true` when nothing cancels the event.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const out = [];
            function f(e) {
                out.push(e.type);
                out.push(e.target === el);
                out.push(e.currentTarget === el);
                out.push(e.eventPhase === Event.AT_TARGET);
                out.push(e.bubbles);
                out.push(e.cancelable);
            }
            el.addEventListener('x', f);
            const ev = document.createEvent('Event');
            ev.initEvent('x', true, true);
            const returned = el.dispatchEvent(ev);
            out.push(returned);
            out.push(ev.defaultPrevented);
            out.join(',');
        "#
        ),
        "x,true,true,true,true,true,true,false"
    );
}

#[test]
fn duplicate_registration_is_ignored() {
    // Registering the same (type, callback, capture) twice fires it once.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            let count = 0;
            function f() { count++; }
            el.addEventListener('x', f);
            el.addEventListener('x', f);
            const ev = document.createEvent('Event');
            ev.initEvent('x', false, false);
            el.dispatchEvent(ev);
            String(count);
        "#
        ),
        "1"
    );
}

#[test]
fn removed_listener_does_not_fire() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            let count = 0;
            function f() { count++; }
            el.addEventListener('x', f);
            el.removeEventListener('x', f);
            const ev = document.createEvent('Event');
            ev.initEvent('x', false, false);
            el.dispatchEvent(ev);
            String(count);
        "#
        ),
        "0"
    );
}

#[test]
fn once_listener_fires_a_single_time() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            let count = 0;
            el.addEventListener('x', () => { count++; }, { once: true });
            for (let i = 0; i < 3; i++) {
                const ev = document.createEvent('Event');
                ev.initEvent('x', false, false);
                el.dispatchEvent(ev);
            }
            String(count);
        "#
        ),
        "1"
    );
}

#[test]
fn throwing_listener_is_swallowed_and_later_listeners_run() {
    // A listener that throws does not abort dispatch: the next listener still
    // runs and dispatchEvent returns normally.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const out = [];
            el.addEventListener('x', () => { out.push('a'); throw new Error('boom'); });
            el.addEventListener('x', () => { out.push('b'); });
            const ev = document.createEvent('Event');
            ev.initEvent('x', false, false);
            const r = el.dispatchEvent(ev);
            out.join(',') + '/' + r;
        "#
        ),
        "a,b/true"
    );
}

#[test]
fn handle_event_object_listener_is_invoked() {
    // A non-callable object listener is dispatched through its `handleEvent`
    // method, with `this` bound to the object.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const handler = {
                seen: 0,
                handleEvent(e) { this.seen++; return e.type; },
            };
            el.addEventListener('x', handler);
            const ev = document.createEvent('Event');
            ev.initEvent('x', false, false);
            el.dispatchEvent(ev);
            String(handler.seen);
        "#
        ),
        "1"
    );
}

#[test]
fn prevent_default_cancels_a_cancelable_event() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            el.addEventListener('x', (e) => { e.preventDefault(); });
            const ev = document.createEvent('Event');
            ev.initEvent('x', false, true);
            const r = el.dispatchEvent(ev);
            r + '/' + ev.defaultPrevented;
        "#
        ),
        "false/true"
    );
}

#[test]
fn prevent_default_ignored_on_non_cancelable_event() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            el.addEventListener('x', (e) => { e.preventDefault(); });
            const ev = document.createEvent('Event');
            ev.initEvent('x', false, false);
            const r = el.dispatchEvent(ev);
            r + '/' + ev.defaultPrevented;
        "#
        ),
        "true/false"
    );
}

#[test]
fn stop_immediate_propagation_skips_later_listeners() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const out = [];
            el.addEventListener('x', (e) => { out.push('a'); e.stopImmediatePropagation(); });
            el.addEventListener('x', () => { out.push('b'); });
            const ev = document.createEvent('Event');
            ev.initEvent('x', false, false);
            el.dispatchEvent(ev);
            out.join(',');
        "#
        ),
        "a"
    );
}

#[test]
fn dispatching_uninitialized_event_throws_invalid_state() {
    // An event fresh from createEvent (never initialized) cannot be dispatched.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const ev = document.createEvent('Event');
            try {
                el.dispatchEvent(ev);
                'no throw';
            } catch (e) {
                (e instanceof DOMException) + '/' + e.name;
            }
        "#
        ),
        "true/InvalidStateError"
    );
}

#[test]
fn create_event_rejects_names_outside_the_legacy_table() {
    // `ProgressEvent` is the discriminating case: the tree implements the
    // interface, but the standard's `createEvent` table does not list it, so
    // only the constructor can reach it.
    assert_eq!(
        eval(
            r#"
            ['NoSuchEvent', 'ProgressEvent'].map((name) => {
                try {
                    document.createEvent(name);
                    return 'no throw';
                } catch (e) {
                    return (e instanceof DOMException) + '/' + e.name;
                }
            }).join(' ');
        "#
        ),
        "true/NotSupportedError true/NotSupportedError"
    );
}

#[test]
fn node_prototype_chains_through_event_target() {
    // Node inherits EventTarget, so its prototype's prototype is
    // EventTarget.prototype and instances see the inherited methods.
    assert_eq!(
        eval(
            r#"
            const chained =
                Object.getPrototypeOf(Node.prototype) === EventTarget.prototype;
            const el = document.querySelector('#target');
            const inherits = typeof el.addEventListener === 'function';
            chained + '/' + inherits;
        "#
        ),
        "true/true"
    );
}

#[test]
fn listener_removing_itself_during_dispatch_is_reusable() {
    // A non-`once` listener that removes itself mid-dispatch must not corrupt
    // the value store: re-registering the same function and dispatching again
    // fires it once more. A double-free or premature free of the callback slot
    // would surface here as a wrong count, a missed invocation, or a crash.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            let count = 0;
            function f() { count++; el.removeEventListener('x', f); }
            function fire() {
                const ev = document.createEvent('Event');
                ev.initEvent('x', false, false);
                el.dispatchEvent(ev);
            }
            el.addEventListener('x', f);
            fire();          // fires, then removes itself
            fire();          // no listener registered -> no fire
            el.addEventListener('x', f);  // re-register the same function
            fire();          // fires again
            String(count);
        "#
        ),
        "2"
    );
}

#[test]
fn same_callback_on_two_types_removes_independently() {
    // The same function registered for two event types interns to one refcounted
    // slot with two owners. Removing one registration must leave the other live.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            let count = 0;
            function f() { count++; }
            el.addEventListener('x', f);
            el.addEventListener('y', f);
            el.removeEventListener('x', f);   // drops one owner; 'y' still live
            for (const t of ['x', 'y']) {
                const ev = document.createEvent('Event');
                ev.initEvent(t, false, false);
                el.dispatchEvent(ev);
            }
            String(count);   // only 'y' fires
        "#
        ),
        "1"
    );
}

#[test]
fn src_element_aliases_target() {
    // srcElement is the legacy alias of target.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            let out = '';
            el.addEventListener('x', (e) => {
                out = (e.srcElement === e.target) + '/' + (e.srcElement === el);
            });
            const ev = document.createEvent('Event');
            ev.initEvent('x', false, false);
            el.dispatchEvent(ev);
            out;
        "#
        ),
        "true/true"
    );
}

#[test]
fn composed_path_is_the_propagation_chain_during_dispatch_only() {
    // composedPath() returns [target, ...ancestors, window] while dispatching
    // and an empty array otherwise. The window is not a link in `parentNode`,
    // so it is appended explicitly: it is the outermost entry of the event
    // path without being an ancestor in the tree.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            function chain(n) { const a = []; for (; n; n = n.parentNode) a.push(n); return a; }
            let result = 'not-run';
            el.addEventListener('x', (e) => {
                const cp = e.composedPath();
                const expected = chain(el).concat([window]);
                let ok = cp.length === expected.length;
                for (let i = 0; i < cp.length; i++) if (cp[i] !== expected[i]) ok = false;
                result = ok + '/' + cp.length + '/' + (cp[0] === el)
                    + '/' + (cp[cp.length - 2] === document)
                    + '/' + (cp[cp.length - 1] === window);
            });
            const ev = document.createEvent('Event');
            ev.initEvent('x', true, true);
            el.dispatchEvent(ev);
            result + '/outside:' + ev.composedPath().length;
        "#
        ),
        "true/5/true/true/true/outside:0"
    );
}

#[test]
fn cancel_bubble_stops_propagation_to_ancestors() {
    // Setting cancelBubble = true stops the event bubbling to ancestor targets;
    // the getter reflects the "stop propagation" flag.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const body = el.parentNode;
            const out = [];
            body.addEventListener('x', () => out.push('body'));
            el.addEventListener('x', (e) => {
                out.push('el:' + e.cancelBubble);
                e.cancelBubble = true;
                out.push('el:' + e.cancelBubble);
            });
            const ev = document.createEvent('Event');
            ev.initEvent('x', true, false);
            el.dispatchEvent(ev);
            out.join(',');
        "#
        ),
        "el:false,el:true"
    );
}

#[test]
fn return_value_false_cancels_a_cancelable_event() {
    // returnValue is the legacy inverse of defaultPrevented; setting it false
    // cancels a cancelable event.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            let seen;
            el.addEventListener('x', (e) => { seen = e.returnValue; e.returnValue = false; });
            const ev = document.createEvent('Event');
            ev.initEvent('x', false, true);
            const r = el.dispatchEvent(ev);
            seen + '/' + r + '/' + ev.returnValue + '/' + ev.defaultPrevented;
        "#
        ),
        "true/false/false/true"
    );
}

// ---------------------------------------------------------------------------
// The window as a target
// ---------------------------------------------------------------------------

#[test]
fn the_window_is_the_global_object_and_an_event_target() {
    // `window`, `self`, `top` and `globalThis` name one object, and that object
    // is an event target: every alias reaches the same listener table. A page's
    // own listeners are almost always here rather than on `document`.
    assert_eq!(
        eval(
            r#"
            const same = (window === globalThis) && (window === self)
                && (window === top) && (window === window.window);
            const has = typeof window.addEventListener + '/'
                + typeof window.removeEventListener + '/'
                + typeof window.dispatchEvent;
            same + '/' + has;
        "#
        ),
        "true/function/function/function"
    );
}

#[test]
fn a_window_listener_sees_an_event_dispatched_at_a_descendant() {
    // The window is the outermost entry of every event path, so a listener
    // registered on it fires for an event dispatched at an element — with the
    // window as `currentTarget` and the element still as `target`.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            let out = [];
            window.addEventListener('x', (e) => {
                out.push('window:' + (e.currentTarget === window)
                    + '/' + (e.target === el) + '/' + e.eventPhase);
            });
            el.addEventListener('x', (e) => { out.push('el:' + e.eventPhase); });
            const ev = document.createEvent('Event');
            ev.initEvent('x', true, false);
            el.dispatchEvent(ev);
            out.join(',');
        "#
        ),
        "el:2,window:true/true/3"
    );
}

#[test]
fn a_window_listener_does_not_see_a_non_bubbling_event() {
    // Without bubbling the path stops at the target, so an ancestor — and the
    // window above it — is reached only by a capture listener.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            let out = [];
            window.addEventListener('x', () => { out.push('bubble'); });
            window.addEventListener('x', () => { out.push('capture'); }, true);
            const ev = document.createEvent('Event');
            ev.initEvent('x', false, false);
            el.dispatchEvent(ev);
            out.join(',');
        "#
        ),
        "capture"
    );
}

#[test]
fn dispatching_at_the_window_gives_the_window_as_the_target() {
    // `window.dispatchEvent` has no node to name, so its path is the window
    // alone: a listener on an element must not see it.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            let out = [];
            window.addEventListener('x', (e) => {
                out.push('window:' + (e.target === window)
                    + '/' + (e.currentTarget === window) + '/' + e.eventPhase
                    + '/' + e.composedPath().length);
            });
            el.addEventListener('x', () => { out.push('el'); });
            const ev = document.createEvent('Event');
            ev.initEvent('x', true, false);
            const r = window.dispatchEvent(ev);
            out.join(',') + '/r:' + r;
        "#
        ),
        "window:true/true/2/1/r:true"
    );
}

#[test]
fn a_window_listener_is_removed_by_the_same_callback() {
    // The identity rule is the same on the window as on a node: the same
    // function removes, a different one does not.
    assert_eq!(
        eval(
            r#"
            let n = 0;
            const f = () => { n++; };
            const g = () => { n++; };
            window.addEventListener('x', f);
            window.removeEventListener('x', g);
            window.dispatchEvent(new Event('x'));
            window.removeEventListener('x', f);
            window.dispatchEvent(new Event('x'));
            String(n);
        "#
        ),
        "1"
    );
}

#[test]
fn a_window_listener_can_dispatch_a_node_event_and_be_seen() {
    // A window listener runs re-entrantly: dispatching a second event from
    // inside one must not disturb the outer dispatch's path. Both events
    // bubble, or the window would not be on their path at all.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            let out = [];
            window.addEventListener('outer', (e) => {
                out.push('outer-path:' + e.composedPath().length);
                el.dispatchEvent(new Event('inner', { bubbles: true }));
                out.push('outer-after:' + e.composedPath().length);
            });
            window.addEventListener('inner', () => { out.push('inner'); });
            el.dispatchEvent(new Event('outer', { bubbles: true }));
            out.join(',');
        "#
        ),
        "outer-path:5,inner,outer-after:5"
    );
}
