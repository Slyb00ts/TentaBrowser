//! Tests that DOM exceptions surface as real DOMException objects (name, code,
//! message, and `instanceof`), not as plain TypeErrors (Phase 2b-4 / generator G2).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

fn test_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
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
    Rc::new(RefCell::new(document))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn dom_exception_constructor_exists() {
    assert_eq!(eval("typeof DOMException"), "function");
}

#[test]
fn index_size_error_is_a_dom_exception() {
    assert_eq!(
        eval(
            r#"
        try {
            const t = document.createTextNode("ab");
            t.substringData(5, 1);
            "no throw"
        } catch (e) {
            (e instanceof DOMException) ? "true" : "false"
        }
    "#
        ),
        "true"
    );
}

#[test]
fn index_size_error_has_name_and_code() {
    assert_eq!(
        eval(
            r#"
        try {
            const t = document.createTextNode("ab");
            t.substringData(5, 1);
            "no throw"
        } catch (e) {
            e.name + "/" + e.code
        }
    "#
        ),
        "IndexSizeError/1"
    );
}

#[test]
fn dom_exception_message_has_no_name_prefix() {
    // The message must be the human-readable text only; the name lives in .name.
    assert_eq!(
        eval(
            r#"
        try {
            const t = document.createTextNode("ab");
            t.insertData(5, "x");
            "no throw"
        } catch (e) {
            e.message.startsWith("IndexSizeError") ? "prefixed" : "clean"
        }
    "#
        ),
        "clean"
    );
}

#[test]
fn hierarchy_request_error_is_a_dom_exception() {
    assert_eq!(
        eval(
            r#"
        try {
            const div = document.createElement("div");
            div.appendChild(div); // a node cannot contain itself
            "no throw"
        } catch (e) {
            (e instanceof DOMException) + "/" + e.name
        }
    "#
        ),
        "true/HierarchyRequestError"
    );
}

#[test]
fn dom_exception_extends_error() {
    assert_eq!(
        eval(
            r#"
        try {
            const t = document.createTextNode("ab");
            t.deleteData(9, 1);
            "no throw"
        } catch (e) {
            (e instanceof Error) ? "true" : "false"
        }
    "#
        ),
        "true"
    );
}

#[test]
fn invalid_character_error_from_set_attribute() {
    assert_eq!(
        eval(
            r#"
        try {
            const div = document.createElement("div");
            div.setAttribute("", "v");
            "no throw"
        } catch (e) {
            e.name + "/" + e.code
        }
    "#
        ),
        "InvalidCharacterError/5"
    );
}

#[test]
fn constructing_dom_exception_directly() {
    assert_eq!(
        eval(
            r#"
        const e = new DOMException("boom", "NotFoundError");
        e.name + "/" + e.message + "/" + e.code + "/" + (e instanceof DOMException)
    "#
        ),
        "NotFoundError/boom/8/true"
    );
}

#[test]
fn dom_exception_defaults_and_unknown_name_code() {
    assert_eq!(
        eval(
            r#"
        const a = new DOMException();
        const b = new DOMException("msg", "TotallyMadeUpError");
        a.name + "/" + a.message + "/" + a.code + "|" + b.code
    "#
        ),
        "Error//0|0"
    );
}

#[test]
fn dom_exception_legacy_constants() {
    assert_eq!(
        eval(
            "DOMException.INDEX_SIZE_ERR + \",\" + DOMException.HIERARCHY_REQUEST_ERR + \",\" + DOMException.NOT_FOUND_ERR"
        ),
        "1,3,8"
    );
}
