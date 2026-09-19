//! Tests for Phase 4a-2a: the window/global self-aliases and the
//! `HTMLIFrameElement`/`SVGElement` interface shells. Frameworks gate
//! `canUseDOM`/`inBrowser` on `typeof window` and read
//! `element instanceof win.HTMLIFrameElement` / `container instanceof
//! SVGElement`, both of which throw if the constructor is missing.

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
fn window_is_the_global_object_and_self_aliases_agree() {
    assert_eq!(
        eval(
            r#"
            [
                typeof window,
                window === globalThis,
                self === window,
                window.window === window,
                window.self === window,
                frames === window,
                top === window,
                parent === window,
            ].join(',');
        "#
        ),
        "object,true,true,true,true,true,true,true"
    );
}

#[test]
fn document_is_reachable_through_window() {
    assert_eq!(eval("window.document === document"), "true");
}

#[test]
fn html_iframe_element_constructor_exists_and_brands_iframes() {
    assert_eq!(
        eval(
            r#"
            const frame = document.createElement('iframe');
            const div = document.createElement('div');
            [
                typeof HTMLIFrameElement,
                typeof window.HTMLIFrameElement,
                frame instanceof HTMLIFrameElement,
                frame instanceof HTMLElement,
                frame instanceof Element,
                frame instanceof Node,
                div instanceof HTMLIFrameElement, // a div is not an iframe
                Object.getPrototypeOf(HTMLIFrameElement.prototype) === HTMLElement.prototype,
            ].join(',');
        "#
        ),
        "function,function,true,true,true,true,false,true"
    );
}

#[test]
fn a_missing_constructor_check_does_not_throw() {
    // React's `getActiveElementDeep` does `el instanceof win.HTMLIFrameElement`;
    // with the constructor present this evaluates instead of throwing.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            String(el instanceof window.HTMLIFrameElement);
        "#
        ),
        "false"
    );
}

#[test]
fn svg_element_constructor_exists_and_chains_through_element() {
    // SVG-namespace element instances arrive with createElementNS (Phase 4b);
    // here the constructor and its prototype chain are what matter.
    assert_eq!(
        eval(
            r#"
            [
                typeof SVGElement,
                typeof window.SVGElement,
                Object.getPrototypeOf(SVGElement.prototype) === Element.prototype,
                document.createElement('div') instanceof SVGElement, // HTML div is not SVG
            ].join(',');
        "#
        ),
        "function,function,true,false"
    );
}

#[test]
fn the_window_is_an_event_target() {
    // `instanceof` walks the prototype chain, so the window's own
    // `addEventListener` is not enough for code that feature-detects event
    // support with `window instanceof EventTarget`.
    //
    // `constructor` reports `EventTarget` because the chain joins there: a
    // browser has `Window.prototype` and `WindowProperties.prototype` in
    // between, and neither interface exists in this engine yet.
    assert_eq!(
        eval(
            r#"
            [
                window instanceof EventTarget,
                self instanceof EventTarget,
                top instanceof EventTarget,
                document instanceof EventTarget,
                Object.getPrototypeOf(window) === EventTarget.prototype,
                window.constructor === EventTarget,
            ].join(',');
        "#
        ),
        "true,true,true,true,true,true"
    );
}

#[test]
fn chaining_the_window_to_event_target_keeps_its_own_properties() {
    // The window carries a prototype at all only because V8 resolves global
    // names through the global proxy's named-property handler rather than
    // through the prototype chain. This test is what says so: if that ever
    // stopped being true, every global would vanish the moment the window got
    // a prototype.
    //
    // The first entry is the precondition — the prototype really was replaced,
    // so the rest is not vacuously true.
    assert_eq!(
        eval(
            r#"
            [
                Object.getPrototypeOf(window) === EventTarget.prototype,
                typeof document,
                typeof setTimeout,
                typeof EventTarget,
                typeof navigator,
                typeof performance,
                window.document === document,
                window.window === window,
                typeof window.addEventListener,
            ].join(',');
        "#
        ),
        "true,object,function,function,object,object,true,true,function"
    );
}
