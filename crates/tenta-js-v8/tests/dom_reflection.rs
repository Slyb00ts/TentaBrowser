//! Tests for Phase 4b-1: Element reflection — `localName`, `namespaceURI`, a
//! writable `id`/`className`, `hasAttributes()` and `toggleAttribute()`.
//! Frameworks read `el.className`/`el.localName` constantly and set
//! `el.id`/`el.className` directly rather than through `setAttribute`.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
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
            name: QualName::new(atoms::EMPTY, atoms::ID),
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
fn local_name_and_namespace_uri_reflect_the_element() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            [el.localName, el.namespaceURI].join(',');
        "#
        ),
        "div,http://www.w3.org/1999/xhtml"
    );
}

#[test]
fn id_is_writable_and_reflects_the_attribute() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const before = el.id;
            el.id = 'renamed';
            [before, el.id, el.getAttribute('id')].join(',');
        "#
        ),
        "target,renamed,renamed"
    );
}

#[test]
fn class_name_is_writable_and_reflects_the_class_attribute() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const empty = el.className;
            el.className = 'alpha beta';
            [empty, el.className, el.getAttribute('class')].join('|');
        "#
        ),
        "|alpha beta|alpha beta"
    );
}

#[test]
fn class_name_reflects_the_attribute_verbatim() {
    // `className` returns the `class` attribute value literally — irregular
    // whitespace and duplicates are preserved, not normalized — when the value
    // was written through `setAttribute` rather than the `className` setter.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            el.setAttribute('class', '  a   b  a ');
            el.className;
        "#
        ),
        "  a   b  a "
    );
}

// `namespaceURI` returning null (a namespace-less element) needs
// `createElementNS`, which lands in Phase 4b-3; this file only covers the
// HTML-namespace case for now.

#[test]
fn has_attributes_reflects_presence() {
    assert_eq!(
        eval(
            r#"
            const withId = document.querySelector('#target');
            const bare = document.createElement('span');
            [withId.hasAttributes(), bare.hasAttributes()].join(',');
        "#
        ),
        "true,false"
    );
}

#[test]
fn toggle_attribute_adds_removes_and_forces() {
    assert_eq!(
        eval(
            r#"
            const el = document.createElement('div');
            const out = [];
            out.push(el.toggleAttribute('hidden'));      // added -> true
            out.push(el.hasAttribute('hidden'));         // true
            out.push(el.getAttribute('hidden'));         // "" (empty value)
            out.push(el.toggleAttribute('hidden'));      // removed -> false
            out.push(el.hasAttribute('hidden'));         // false
            out.push(el.toggleAttribute('data-x', true));  // force on -> true
            out.push(el.toggleAttribute('data-x', true));  // still on -> true
            out.push(el.toggleAttribute('data-x', false)); // force off -> false
            out.push(el.hasAttribute('data-x'));         // false
            out.join(',');
        "#
        ),
        "true,true,,false,false,true,true,false,false"
    );
}

#[test]
fn toggle_attribute_rejects_an_empty_name() {
    assert_eq!(
        eval(
            r#"
            const el = document.createElement('div');
            let name = 'no throw';
            try { el.toggleAttribute(''); } catch (e) { name = e.name; }
            name;
        "#
        ),
        "InvalidCharacterError"
    );
}
