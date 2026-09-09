//! Tests for Phase 3d wired through V8: the `HTMLElement` interface with
//! `click()`, and the `UIEvent`/`MouseEvent` interfaces + constructors.
//!
//! `element.click()` dispatches a synthetic, untrusted `click` MouseEvent that
//! bubbles and is cancelable — the path a React/Vue app takes when code triggers
//! a click. `click()` is `[Throws]` (not fast-API) so it may reenter JS; the hot
//! loop test would crash if it were mistakenly fast-API.

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
fn html_element_prototype_chains_through_element() {
    // An HTML-namespace element gets the HTMLElement prototype, whose prototype
    // is Element.prototype.
    assert_eq!(
        eval(
            r#"
            const div = document.querySelector('#target');
            [
                div instanceof HTMLElement,
                div instanceof Element,
                div instanceof Node,
                Object.getPrototypeOf(HTMLElement.prototype) === Element.prototype,
            ].join(',');
        "#
        ),
        "true,true,true,true"
    );
}

#[test]
fn click_dispatches_an_untrusted_mouse_event_to_the_target() {
    assert_eq!(
        eval(
            r#"
            const div = document.querySelector('#target');
            let out = '';
            div.addEventListener('click', (e) => {
                out = [e.type, e.target === div, e.eventPhase, e.isTrusted,
                       e instanceof MouseEvent, e.detail, e.button].join(',');
            });
            div.click();
            out;
        "#
        ),
        // type, target, AT_TARGET(2), untrusted, MouseEvent, detail 1, button 0
        "click,true,2,false,true,1,0"
    );
}

#[test]
fn click_bubbles_and_is_seen_by_a_delegated_document_listener() {
    // React-style event delegation: a single listener on `document` sees the
    // click bubble up from a nested button, as a cancelable MouseEvent.
    assert_eq!(
        eval(
            r#"
            const btn = document.createElement('button');
            document.querySelector('#target').appendChild(btn);
            let out = '';
            document.addEventListener('click', (e) => {
                out = [e.target === btn, e instanceof MouseEvent, e.bubbles,
                       e.cancelable, e.button, e.isTrusted, e.eventPhase].join(',');
            });
            btn.click();
            out;
        "#
        ),
        // target===btn, MouseEvent, bubbles, cancelable, button 0, untrusted, BUBBLING_PHASE(3)
        "true,true,true,true,0,false,3"
    );
}

#[test]
fn click_default_can_be_prevented() {
    // Vue-style per-element handler: preventDefault on the click is observable
    // by a later listener via defaultPrevented (the event is cancelable).
    assert_eq!(
        eval(
            r#"
            const btn = document.createElement('button');
            document.querySelector('#target').appendChild(btn);
            let seen;
            btn.addEventListener('click', (e) => { e.preventDefault(); });
            btn.addEventListener('click', (e) => { seen = e.defaultPrevented; });
            btn.click();
            String(seen);
        "#
        ),
        "true"
    );
}

#[test]
fn reentrant_click_is_suppressed_by_the_in_progress_guard() {
    // A click handler that calls click() again on the same element must not
    // recurse: the nested click is a no-op, so the handler runs once.
    assert_eq!(
        eval(
            r#"
            const btn = document.createElement('button');
            document.querySelector('#target').appendChild(btn);
            let count = 0;
            btn.addEventListener('click', () => { count++; if (count < 5) btn.click(); });
            btn.click();
            String(count);
        "#
        ),
        "1"
    );
}

#[test]
fn mouse_event_constructor_reads_its_init_dictionary() {
    assert_eq!(
        eval(
            r#"
            const e = new MouseEvent('click', {
                bubbles: true, cancelable: true, detail: 2,
                screenX: 3, screenY: 4, clientX: 42, clientY: 7,
                button: 1, buttons: 5, ctrlKey: true, shiftKey: false,
                altKey: true, metaKey: false,
            });
            [
                e.type, e.bubbles, e.cancelable, e.detail,
                e.screenX, e.screenY, e.clientX, e.clientY,
                e.button, e.buttons, e.ctrlKey, e.shiftKey, e.altKey, e.metaKey,
                e instanceof MouseEvent, e instanceof UIEvent, e instanceof Event,
            ].join(',');
        "#
        ),
        "click,true,true,2,3,4,42,7,1,5,true,false,true,false,true,true,true"
    );
}

#[test]
fn mouse_event_defaults_and_related_target_null() {
    // Absent init members take their defaults; relatedTarget defaults to null.
    assert_eq!(
        eval(
            r#"
            const e = new MouseEvent('click');
            [e.detail, e.screenX, e.clientX, e.button, e.buttons,
             e.ctrlKey, e.relatedTarget === null, e.bubbles, e.cancelable].join(',');
        "#
        ),
        "0,0,0,0,0,false,true,false,false"
    );
}

#[test]
fn ui_event_constructor_and_detail() {
    assert_eq!(
        eval(
            r#"
            const e = new UIEvent('resize', { detail: 5, bubbles: true });
            [e.detail, e.bubbles, e instanceof UIEvent, e instanceof Event,
             e instanceof MouseEvent].join(',');
        "#
        ),
        "5,true,true,true,false"
    );
}

#[test]
fn a_mouse_event_relates_to_another_target() {
    // relatedTarget round-trips an EventTarget by identity.
    assert_eq!(
        eval(
            r#"
            const div = document.querySelector('#target');
            const e = new MouseEvent('mouseout', { relatedTarget: div });
            String(e.relatedTarget === div);
        "#
        ),
        "true"
    );
}

#[test]
fn a_hot_loop_of_clicks_does_not_crash() {
    // click() is [Throws] (not fast-API), so it can reenter JS to dispatch; a
    // fast-API click would crash the isolate on the first reentry. 100k clicks
    // exercises the reentrant path hard.
    assert_eq!(
        eval(
            r#"
            const btn = document.createElement('button');
            document.querySelector('#target').appendChild(btn);
            let n = 0;
            btn.addEventListener('click', () => { n++; });
            for (let i = 0; i < 100000; i++) btn.click();
            String(n);
        "#
        ),
        "100000"
    );
}
