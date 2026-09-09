//! Tests for Phase 4b-3: document fragments and namespace-aware creation.
//!
//! `document.createDocumentFragment()` + the `DocumentFragment` wrapper (React
//! range insertion, Vue fragment blocks), `document.createElementNS()` (SVG /
//! MathML subtrees), and `<template>.content` (the inert contents fragment
//! Vue's compiler reads). Also folds in the `documentElement.remove()`
//! robustness case the 4b-2 `ChildNode.remove()` critic flagged as uncovered.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_js_v8::Engine;

const SVG_NS: &str = "http://www.w3.org/2000/svg";

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

fn eval_err(source: &str) -> String {
    let mut engine = Engine::with_document(test_document());
    match engine.evaluate(source) {
        Ok(value) => panic!("expected the script to throw, got Ok({value:?})"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn create_document_fragment_is_an_empty_fragment() {
    assert_eq!(
        eval(
            r#"
            const frag = document.createDocumentFragment();
            [
                frag.nodeType,
                frag.nodeName,
                frag.childNodes.length,
                frag instanceof DocumentFragment,
                frag instanceof Node,
                frag.ownerDocument === document,
            ].join(',');
        "#
        ),
        "11,#document-fragment,0,true,true,true"
    );
}

#[test]
fn appending_a_fragment_moves_its_children_into_the_target() {
    assert_eq!(
        eval(
            r#"
            const frag = document.createDocumentFragment();
            const a = document.createElement('span');
            const b = document.createElement('span');
            frag.appendChild(a);
            frag.appendChild(b);
            const beforeFrag = frag.childNodes.length;
            const body = document.body;
            body.appendChild(frag);
            [
                beforeFrag,
                frag.childNodes.length,
                body.childElementCount,
                a.parentNode === body,
                b.parentNode === body,
            ].join(',');
        "#
        ),
        // body already held <div id=target>, so it now has three element children.
        "2,0,3,true,true"
    );
}

#[test]
fn create_element_ns_puts_the_element_in_the_svg_namespace() {
    assert_eq!(
        eval(&format!(
            r#"
            const rect = document.createElementNS('{SVG_NS}', 'rect');
            [
                rect.namespaceURI,
                rect.localName,
                rect instanceof SVGElement,
                rect instanceof Element,
            ].join(',');
        "#
        )),
        format!("{SVG_NS},rect,true,true")
    );
}

#[test]
fn create_element_ns_preserves_case_and_a_null_namespace() {
    assert_eq!(
        eval(
            r#"
            const el = document.createElementNS(null, 'myElement');
            [
                el.namespaceURI === null,
                el.localName,
            ].join(',');
        "#
        ),
        "true,myElement"
    );
}

#[test]
fn create_element_ns_rejects_a_prefix_without_a_namespace() {
    assert!(
        eval_err(
            r#"
            document.createElementNS(null, 'svg:rect');
        "#
        )
        .contains("NamespaceError"),
        "a prefixed name in the null namespace must raise NamespaceError"
    );
}

#[test]
fn template_content_is_an_owned_document_fragment() {
    assert_eq!(
        eval(
            r#"
            const tpl = document.createElement('template');
            const content = tpl.content;
            [
                tpl instanceof HTMLTemplateElement,
                content instanceof DocumentFragment,
                content.nodeType,
                content === tpl.content,
                content.parentNode === null,
            ].join(',');
        "#
        ),
        // SameObject: content === tpl.content on every read.
        "true,true,11,true,true"
    );
}

#[test]
fn template_children_land_in_content_not_in_the_template() {
    assert_eq!(
        eval(
            r#"
            const tpl = document.createElement('template');
            const child = document.createElement('div');
            tpl.content.appendChild(child);
            [
                tpl.childElementCount,
                tpl.content.childNodes.length,
                child.parentNode === tpl.content,
            ].join(',');
        "#
        ),
        "0,1,true"
    );
}

#[test]
fn removing_the_document_element_is_safe() {
    // The 4b-2 critic flagged `documentElement.remove()` as an uncovered edge:
    // detaching the root must leave the document consistent, not crash.
    assert_eq!(
        eval(
            r#"
            const root = document.documentElement;
            root.remove();
            [
                document.documentElement === null,
                document.body === null,
                root.isConnected,
                root.parentNode === null,
            ].join(',');
        "#
        ),
        "true,true,false,true"
    );
}
