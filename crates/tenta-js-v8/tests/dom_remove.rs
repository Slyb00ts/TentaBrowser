//! Tests for Phase 4b-2: `ChildNode.remove()` — the first `includes` mixin in
//! `idl/dom`, shared by Element and CharacterData (Text/Comment). Frameworks call
//! `node.remove()` to detach a node without reaching for its parent.

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
fn element_remove_detaches_from_the_parent() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const body = document.body;
            const had = body.childElementCount;
            el.remove();
            [
                had,
                body.childElementCount,
                el.parentNode === null,
                el.isConnected,
                document.querySelector('#target') === null,
            ].join(',');
        "#
        ),
        "1,0,true,false,true"
    );
}

#[test]
fn character_data_remove_detaches_a_text_node() {
    assert_eq!(
        eval(
            r#"
            const body = document.body;
            const t = document.createTextNode('hello');
            body.appendChild(t);
            const before = body.contains(t);
            t.remove();
            [before, body.contains(t), t.parentNode === null].join(',');
        "#
        ),
        "true,false,true"
    );
}

#[test]
fn remove_on_a_parentless_node_is_a_no_op() {
    assert_eq!(
        eval(
            r#"
            const orphan = document.createElement('div');
            orphan.remove();
            const t = document.createTextNode('x');
            t.remove();
            'ok';
        "#
        ),
        "ok"
    );
}

#[test]
fn a_removed_element_stays_alive_and_can_be_reattached() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const body = document.body;
            el.remove();
            body.appendChild(el);
            [
                el.parentNode === body,
                el.isConnected,
                document.querySelector('#target') === el,
            ].join(',');
        "#
        ),
        "true,true,true"
    );
}
