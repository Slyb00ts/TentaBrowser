//! Tests for Element.outerHTML (getter + setter) and insertAdjacentHTML
//! (Phase 2b-5).

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
fn outer_html_getter_includes_own_tag() {
    assert_eq!(
        eval("document.querySelector('#target').outerHTML"),
        r#"<div id="target"></div>"#
    );
}

#[test]
fn outer_html_getter_includes_children() {
    assert_eq!(
        eval(
            r#"
        const t = document.querySelector('#target');
        t.innerHTML = "<span>hi</span>";
        t.outerHTML
    "#
        ),
        r#"<div id="target"><span>hi</span></div>"#
    );
}

#[test]
fn set_outer_html_replaces_element() {
    // After replacing #target, the body holds the new node and #target is gone.
    assert_eq!(
        eval(
            r#"
        const t = document.querySelector('#target');
        const body = document.querySelector('body');
        t.outerHTML = "<p class='x'>new</p>";
        body.innerHTML
    "#
        ),
        r#"<p class="x">new</p>"#
    );
}

#[test]
fn set_outer_html_inserts_multiple_nodes() {
    assert_eq!(
        eval(
            r#"
        const t = document.querySelector('#target');
        const body = document.querySelector('body');
        t.outerHTML = "<a></a><b></b>";
        body.childElementCount + "/" + body.firstElementChild.tagName + "/" + body.lastElementChild.tagName
    "#
        ),
        "2/A/B"
    );
}

#[test]
fn set_outer_html_on_document_child_throws() {
    // documentElement's parent is the document node -> NoModificationAllowedError.
    assert_eq!(
        eval(
            r#"
        try {
            document.documentElement.outerHTML = "<html></html>";
            "no throw"
        } catch (e) {
            (e instanceof DOMException) + "/" + e.name
        }
    "#
        ),
        "true/NoModificationAllowedError"
    );
}

#[test]
fn set_outer_html_on_detached_element_is_noop() {
    // A detached element has no parent: setting outerHTML does nothing and
    // does not throw. The element itself is unchanged.
    assert_eq!(
        eval(
            r#"
        const e = document.createElement("div");
        e.outerHTML = "<span></span>";
        e.tagName + "/" + e.outerHTML
    "#
        ),
        "DIV/<div></div>"
    );
}

#[test]
fn insert_adjacent_html_beforebegin() {
    assert_eq!(
        eval(
            r#"
        const t = document.querySelector('#target');
        const body = document.querySelector('body');
        t.insertAdjacentHTML("beforebegin", "<i></i>");
        body.firstElementChild.tagName + "/" + body.childElementCount
    "#
        ),
        "I/2"
    );
}

#[test]
fn insert_adjacent_html_afterbegin() {
    assert_eq!(
        eval(
            r#"
        const t = document.querySelector('#target');
        t.innerHTML = "<b>old</b>";
        t.insertAdjacentHTML("afterbegin", "<i>new</i>");
        t.firstElementChild.tagName + "/" + t.innerHTML
    "#
        ),
        "I/<i>new</i><b>old</b>"
    );
}

#[test]
fn insert_adjacent_html_beforeend() {
    assert_eq!(
        eval(
            r#"
        const t = document.querySelector('#target');
        t.innerHTML = "<b>old</b>";
        t.insertAdjacentHTML("beforeend", "<i>new</i>");
        t.innerHTML
    "#
        ),
        "<b>old</b><i>new</i>"
    );
}

#[test]
fn insert_adjacent_html_afterend() {
    assert_eq!(
        eval(
            r#"
        const t = document.querySelector('#target');
        const body = document.querySelector('body');
        t.insertAdjacentHTML("afterend", "<i></i>");
        body.lastElementChild.tagName + "/" + body.childElementCount
    "#
        ),
        "I/2"
    );
}

#[test]
fn insert_adjacent_html_position_is_case_insensitive() {
    assert_eq!(
        eval(
            r#"
        const t = document.querySelector('#target');
        t.insertAdjacentHTML("BeforeEnd", "<i></i>");
        t.innerHTML
    "#
        ),
        "<i></i>"
    );
}

#[test]
fn insert_adjacent_html_invalid_position_throws_syntax_error() {
    assert_eq!(
        eval(
            r#"
        const t = document.querySelector('#target');
        try {
            t.insertAdjacentHTML("nope", "<i></i>");
            "no throw"
        } catch (e) {
            (e instanceof DOMException) + "/" + e.name
        }
    "#
        ),
        "true/SyntaxError"
    );
}

#[test]
fn insert_adjacent_html_beforebegin_on_document_child_throws() {
    assert_eq!(
        eval(
            r#"
        try {
            document.documentElement.insertAdjacentHTML("beforebegin", "<x></x>");
            "no throw"
        } catch (e) {
            (e instanceof DOMException) + "/" + e.name
        }
    "#
        ),
        "true/NoModificationAllowedError"
    );
}
