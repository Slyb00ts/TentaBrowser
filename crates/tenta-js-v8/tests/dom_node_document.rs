//! Tests for Phase 4a-1: the Node/Document conveniences the frameworks read on
//! load — `ownerDocument`, `isConnected`, `contains`, `hasChildNodes`, the full
//! set of `nodeType` constants, and `document.body`/`head`/`readyState`/
//! `activeElement`. Pure binding surface: none of these reenter JS.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// `<html><head></head><body><div id="target"></div></body></html>`
fn test_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let head = document.create_html_element(atoms::HEAD);
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
            child: head,
        })
        .expect("append <head>");
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
fn owner_document_points_at_the_document_and_is_null_for_it() {
    assert_eq!(
        eval(
            r#"
            const div = document.querySelector('#target');
            [div.ownerDocument === document, document.ownerDocument === null].join(',');
        "#
        ),
        "true,true"
    );
}

#[test]
fn is_connected_tracks_attachment() {
    // An in-tree node is connected; a freshly created, unattached one is not;
    // attaching it connects it and removing it disconnects it again.
    assert_eq!(
        eval(
            r#"
            const div = document.querySelector('#target');
            const fresh = document.createElement('span');
            const before = fresh.isConnected;
            div.appendChild(fresh);
            const attached = fresh.isConnected;
            fresh.remove ? fresh.remove() : div.removeChild(fresh);
            [div.isConnected, before, attached, fresh.isConnected].join(',');
        "#
        ),
        // div connected, fresh not yet, connected after append, not after remove
        "true,false,true,false"
    );
}

#[test]
fn contains_is_inclusive_and_rejects_null() {
    assert_eq!(
        eval(
            r#"
            const body = document.body;
            const div = document.querySelector('#target');
            const outside = document.createElement('p');
            [
                body.contains(div),   // descendant
                body.contains(body),  // inclusive: itself
                div.contains(body),   // ancestor is not contained
                body.contains(outside), // detached node
                body.contains(null),  // null is never contained
            ].join(',');
        "#
        ),
        "true,true,false,false,false"
    );
}

#[test]
fn has_child_nodes_reflects_children() {
    assert_eq!(
        eval(
            r#"
            const div = document.querySelector('#target');
            const empty = document.createElement('span');
            [document.body.hasChildNodes(), empty.hasChildNodes()].join(',');
        "#
        ),
        "true,false"
    );
}

#[test]
fn document_body_and_head_resolve_to_the_html_elements() {
    assert_eq!(
        eval(
            r#"
            [
                document.body.tagName,
                document.head.tagName,
                document.body instanceof HTMLElement,
                document.body.ownerDocument === document,
            ].join(',');
        "#
        ),
        "BODY,HEAD,true,true"
    );
}

#[test]
fn ready_state_is_complete() {
    assert_eq!(eval("document.readyState"), "complete");
}

#[test]
fn active_element_falls_back_to_the_body() {
    assert_eq!(
        eval("document.activeElement === document.body ? 'body' : 'other'"),
        "body"
    );
}

#[test]
fn node_type_constants_are_exposed() {
    // The standard Node.nodeType constants (a subset of which real nodes report)
    // are present on both the interface and its instances.
    assert_eq!(
        eval(
            r#"
            [
                Node.ELEMENT_NODE, Node.ATTRIBUTE_NODE, Node.TEXT_NODE,
                Node.CDATA_SECTION_NODE, Node.PROCESSING_INSTRUCTION_NODE,
                Node.COMMENT_NODE, Node.DOCUMENT_NODE, Node.DOCUMENT_TYPE_NODE,
                Node.DOCUMENT_FRAGMENT_NODE, Node.NOTATION_NODE,
                document.querySelector('#target').ELEMENT_NODE,
            ].join(',');
        "#
        ),
        "1,2,3,4,7,8,9,10,11,12,1"
    );
}

#[test]
fn node_type_reports_the_right_codes() {
    assert_eq!(
        eval(
            r#"
            const div = document.querySelector('#target');
            [
                div.nodeType,                          // ELEMENT_NODE
                document.nodeType,                     // DOCUMENT_NODE
                document.createTextNode('x').nodeType, // TEXT_NODE
                document.createComment('c').nodeType,  // COMMENT_NODE
            ].join(',');
        "#
        ),
        "1,9,3,8"
    );
}
