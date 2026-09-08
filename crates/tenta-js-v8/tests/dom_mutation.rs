//! Tests for DOM mutation operations: appendChild, insertBefore, removeChild,
//! replaceChild, cloneNode, setAttribute, getAttribute, removeAttribute,
//! hasAttribute, textContent setter, nodeValue setter, and data setter.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_js_v8::Engine;

fn test_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();

    // Create basic structure: <html><body><div id="app"></div></body></html>
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
            value: "app".to_owned(),
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
fn append_child_returns_node() {
    assert_eq!(
        eval(
            r#"
        const body = document.documentElement.firstChild;
        const p = document.createElement("p");
        const result = body.appendChild(p);

        // appendChild should return the appended node
        (result === p && body.lastChild === p) ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn append_child_identity_preserved() {
    assert_eq!(
        eval(
            r#"
        const body = document.documentElement.firstChild;
        const p = document.createElement("p");
        p.textContent = "Hello";

        // Keep a reference before appending
        const pBefore = p;
        body.appendChild(p);

        // The reference should still be the same object
        (p === pBefore && p.textContent === "Hello") ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn append_child_hierarchy_error() {
    assert!(
        eval(
            r#"
        const div = document.getElementById("app");

        // Try to append a node to itself
        try {
            div.appendChild(div);
            "false"
        } catch (e) {
            // Expected
            "true"
        }
    "#
        ) == "true"
    );
}

#[test]
fn insert_before_with_null_appends() {
    assert_eq!(
        eval(
            r#"
        const body = document.documentElement.firstChild;
        const p1 = document.createElement("p");
        const p2 = document.createElement("p");
        p1.textContent = "First";
        p2.textContent = "Second";

        body.appendChild(p1);
        // insertBefore with null should append
        body.insertBefore(p2, null);

        (body.lastChild === p2) ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn insert_before_with_ref() {
    assert_eq!(
        eval(
            r#"
        const body = document.documentElement.firstChild;
        const p1 = document.createElement("p");
        const p2 = document.createElement("p");
        const p3 = document.createElement("p");
        p1.textContent = "First";
        p2.textContent = "Second";
        p3.textContent = "Third";

        body.appendChild(p1);
        body.appendChild(p3);
        const origP3Pos = body.childNodes.length - 1;
        // Insert p2 before p3
        body.insertBefore(p2, p3);

        // Verify ordering
        (body.childNodes[body.childNodes.length - 3] === p1 &&
         body.childNodes[body.childNodes.length - 2] === p2 &&
         body.childNodes[body.childNodes.length - 1] === p3) ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn remove_child_returns_child() {
    assert_eq!(
        eval(
            r#"
        const body = document.documentElement.firstChild;
        const p = document.createElement("p");
        p.textContent = "Hello";
        body.appendChild(p);

        const removed = body.removeChild(p);

        (removed === p && body.lastChild !== p) ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn remove_child_preserves_wrapper() {
    assert_eq!(
        eval(
            r#"
        const body = document.documentElement.firstChild;
        const p = document.createElement("p");
        p.textContent = "Hello";
        body.appendChild(p);

        // Keep a reference
        const pRef = p;

        // Remove it
        body.removeChild(p);

        // Should still be able to access it
        const txt = pRef.textContent;

        // Can re-append it
        body.appendChild(pRef);

        (txt === "Hello" && body.lastChild === pRef) ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn replace_child_returns_old_child() {
    assert_eq!(
        eval(
            r#"
        const body = document.documentElement.firstChild;
        const p1 = document.createElement("p");
        const p2 = document.createElement("p");
        p1.textContent = "Old";
        p2.textContent = "New";

        body.appendChild(p1);
        const old = body.replaceChild(p2, p1);

        // replaceChild should return the OLD child
        (old === p1 && body.lastChild === p2) ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn set_attribute_get_attribute() {
    assert_eq!(
        eval(
            r#"
        const div = document.getElementById("app");

        div.setAttribute("data-test", "value123");
        const val = div.getAttribute("data-test");

        val === "value123" ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn get_attribute_absent() {
    assert_eq!(
        eval(
            r#"
        const div = document.getElementById("app");

        const val = div.getAttribute("nonexistent");

        val === null ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn has_attribute() {
    assert_eq!(
        eval(
            r#"
        const div = document.getElementById("app");

        const before = div.hasAttribute("id");
        const beforeNone = !div.hasAttribute("data-test");

        div.setAttribute("data-test", "value");

        const after = div.hasAttribute("data-test");

        (before && beforeNone && after) ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn remove_attribute() {
    assert_eq!(
        eval(
            r#"
        const div = document.getElementById("app");

        const before = div.hasAttribute("id");
        div.removeAttribute("id");
        const after = div.hasAttribute("id");

        (before && !after) ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn set_attribute_empty_name() {
    assert!(
        eval(
            r#"
        const div = document.getElementById("app");

        try {
            div.setAttribute("", "value");
            "false"
        } catch (e) {
            "true"
        }
    "#
        ) == "true"
    );
}

#[test]
fn text_content_setter_element() {
    assert_eq!(
        eval(
            r#"
        const body = document.documentElement.firstChild;
        const div = document.createElement("div");
        const p = document.createElement("p");
        p.textContent = "Paragraph";

        body.appendChild(div);
        div.appendChild(p);

        // Set textContent on element replaces all children
        div.textContent = "New content";

        // Should have only one text node child
        (div.childNodes.length === 1 && div.textContent === "New content") ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn text_content_setter_clears() {
    assert_eq!(
        eval(
            r#"
        const body = document.documentElement.firstChild;
        const div = document.createElement("div");
        const p = document.createElement("p");

        body.appendChild(div);
        div.appendChild(p);

        // Setting empty string should remove all children
        div.textContent = "";

        div.childNodes.length === 0 ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn text_content_setter_text_node() {
    assert_eq!(
        eval(
            r#"
        const text = document.createTextNode("Hello");

        text.textContent = "World";
        text.textContent === "World" ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn clone_node_shallow() {
    assert_eq!(
        eval(
            r#"
        const div = document.createElement("div");
        const p = document.createElement("p");
        div.appendChild(p);

        const clone = div.cloneNode(false);

        (clone !== div && clone.childNodes.length === 0) ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn clone_node_deep() {
    assert_eq!(
        eval(
            r#"
        const div = document.createElement("div");
        div.textContent = "Hello";
        const p = document.createElement("p");
        p.textContent = "World";
        div.appendChild(p);

        const clone = div.cloneNode(true);

        (clone !== div && clone.childNodes.length > 0) ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn clone_node_default_shallow() {
    assert_eq!(
        eval(
            r#"
        const div = document.createElement("div");
        const p = document.createElement("p");
        div.appendChild(p);

        // cloneNode() without argument should default to false (shallow)
        const clone = div.cloneNode();

        clone.childNodes.length === 0 ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn data_setter_text_node() {
    assert_eq!(
        eval(
            r#"
        const text = document.createTextNode("Hello");

        text.data = "World";
        text.data === "World" ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn set_id_via_attribute_find_by_id() {
    assert_eq!(
        eval(
            r#"
        const body = document.documentElement.firstChild;
        const p = document.createElement("p");
        body.appendChild(p);

        p.setAttribute("id", "myid");
        const found = document.getElementById("myid");

        found === p ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn node_value_text_node() {
    assert_eq!(
        eval(
            r#"
        const text = document.createTextNode("Hello");

        const before = text.nodeValue === "Hello";

        text.nodeValue = "World";

        (before && text.nodeValue === "World") ? "true" : "false"
    "#
        ),
        "true"
    );
}

#[test]
fn node_value_element() {
    assert_eq!(
        eval(
            r#"
        const div = document.createElement("div");
        div.textContent = "Hello";

        // Element nodeValue should be null
        const isNull = div.nodeValue === null;

        // Setting nodeValue on element should be a no-op
        div.nodeValue = "ignored";

        (isNull && div.textContent === "Hello") ? "true" : "false"
    "#
        ),
        "true"
    );
}
