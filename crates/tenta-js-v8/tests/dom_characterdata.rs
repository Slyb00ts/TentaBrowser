//! Tests for the CharacterData mutation operations: substringData, appendData,
//! insertData, deleteData and replaceData (Phase 2b-2).

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

fn throws(source: &str) -> bool {
    let mut engine = Engine::with_document(test_document());
    engine.evaluate(source).is_err()
}

#[test]
fn substring_data_returns_range() {
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("Hello, world");
        t.substringData(7, 5)
    "#
        ),
        "world"
    );
}

#[test]
fn substring_data_clamps_count_to_end() {
    // A count that runs past the end returns everything from the offset on.
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("Hello");
        t.substringData(2, 100)
    "#
        ),
        "llo"
    );
}

#[test]
fn substring_data_at_length_is_empty() {
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("abc");
        t.substringData(3, 1)
    "#
        ),
        ""
    );
}

#[test]
fn substring_data_offset_past_length_throws() {
    assert!(throws(
        r#"
        const t = document.createTextNode("abc");
        t.substringData(4, 1)
    "#
    ));
}

#[test]
fn append_data_concatenates() {
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("Hello");
        t.appendData(", world");
        t.data
    "#
        ),
        "Hello, world"
    );
}

#[test]
fn insert_data_inserts_at_offset() {
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("Hello world");
        t.insertData(5, ",");
        t.data
    "#
        ),
        "Hello, world"
    );
}

#[test]
fn insert_data_at_start_and_end() {
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("b");
        t.insertData(0, "a");
        t.insertData(2, "c");
        t.data
    "#
        ),
        "abc"
    );
}

#[test]
fn insert_data_offset_past_length_throws() {
    assert!(throws(
        r#"
        const t = document.createTextNode("abc");
        t.insertData(4, "x");
    "#
    ));
}

#[test]
fn delete_data_removes_range() {
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("Hello, world");
        t.deleteData(5, 2);
        t.data
    "#
        ),
        "Helloworld"
    );
}

#[test]
fn delete_data_clamps_count_to_end() {
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("Hello");
        t.deleteData(2, 100);
        t.data
    "#
        ),
        "He"
    );
}

#[test]
fn delete_data_offset_past_length_throws() {
    assert!(throws(
        r#"
        const t = document.createTextNode("abc");
        t.deleteData(4, 1);
    "#
    ));
}

#[test]
fn replace_data_substitutes_range() {
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("Hello, world");
        t.replaceData(7, 5, "there");
        t.data
    "#
        ),
        "Hello, there"
    );
}

#[test]
fn replace_data_with_shorter_and_longer_text() {
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("aXXXb");
        t.replaceData(1, 3, "-");
        t.data
    "#
        ),
        "a-b"
    );
}

#[test]
fn replace_data_clamps_count_to_end() {
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("Hello");
        t.replaceData(2, 100, "y");
        t.data
    "#
        ),
        "Hey"
    );
}

#[test]
fn replace_data_offset_past_length_throws() {
    assert!(throws(
        r#"
        const t = document.createTextNode("abc");
        t.replaceData(4, 0, "x");
    "#
    ));
}

#[test]
fn operations_update_length() {
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("abc");
        t.appendData("de");
        t.deleteData(0, 1);
        String(t.length)
    "#
        ),
        "4"
    );
}

#[test]
fn operations_work_on_comment_nodes() {
    assert_eq!(
        eval(
            r#"
        const c = document.createComment("todo");
        c.appendData(": done");
        c.deleteData(0, 4);
        c.data
    "#
        ),
        ": done"
    );
}

#[test]
fn offsets_are_utf16_code_units() {
    // An astral character (grinning face) is two UTF-16 code units, so a
    // substring after it starts at offset 2, and the node's length counts it
    // as two.
    assert_eq!(
        eval(
            r#"
        const t = document.createTextNode("\u{1F600}ab");
        String(t.length) + "|" + t.substringData(2, 2)
    "#
        ),
        "4|ab"
    );
}
