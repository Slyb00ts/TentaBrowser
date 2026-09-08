//! Phase 0c vertical slice: `document.documentElement.nodeType === 1` end to
//! end through the generated V8 DOM glue (`docs/spec/05-javascript.md` §3).
//!
//! The test drives the whole path the binding phases build on: a real
//! `tenta_dom::Document`, the generated `install_*`/`WrapperWorld` runtime, the
//! `V8World` wrapper lifecycle and the generated getters. It proves the
//! `Document` and `Element` wrappers materialise on demand and answer
//! `nodeType`, and that a live node maps to a single stable wrapper.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document whose document element is a single `<html>` element, so
/// `document.documentElement` resolves to an `Element` of `nodeType` 1.
fn html_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append <html> under the document node");
    Rc::new(RefCell::new(document))
}

#[test]
fn document_element_node_type_is_element() {
    let mut engine = Engine::with_document(html_document());
    assert_eq!(
        engine
            .evaluate("document.documentElement.nodeType === 1")
            .expect("the slice evaluates without throwing"),
        "true",
    );
}

#[test]
fn document_element_wrapper_has_stable_identity() {
    let mut engine = Engine::with_document(html_document());
    // The wrapper world revives one wrapper per live node, so two reads of the
    // same element are the same JavaScript object.
    assert_eq!(
        engine
            .evaluate("document.documentElement === document.documentElement")
            .expect("the slice evaluates without throwing"),
        "true",
    );
}

#[test]
fn document_itself_reports_the_document_node_type() {
    let mut engine = Engine::with_document(html_document());
    assert_eq!(
        engine
            .evaluate("document.nodeType === 9")
            .expect("the slice evaluates without throwing"),
        "true",
    );
}
