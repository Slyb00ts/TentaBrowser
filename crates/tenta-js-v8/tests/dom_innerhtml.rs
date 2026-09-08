//! Tests for Element.innerHTML getter and setter.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_js_v8::Engine;

fn test_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();

    // Create basic structure: <html><body><div id="target"></div></body></html>
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
fn inner_html_getter_returns_empty_for_empty_element() {
    assert_eq!(
        eval(
            r#"
        const target = document.getElementById('target');
        target.innerHTML
    "#
        ),
        ""
    );
}

#[test]
fn inner_html_setter_adds_single_element() {
    assert_eq!(
        eval(
            r#"
        const target = document.getElementById('target');
        target.innerHTML = '<p>hello</p>';

        // Verify the child was added
        (target.firstChild !== null && target.firstChild.nodeType === 1) ? 'true' : 'false'
    "#
        ),
        "true"
    );
}

#[test]
fn inner_html_getter_returns_set_content() {
    assert_eq!(
        eval(
            r#"
        const target = document.getElementById('target');
        target.innerHTML = '<p>hello</p>';

        // innerHTML getter should return the content
        target.innerHTML.includes('p') ? 'true' : 'false'
    "#
        ),
        "true"
    );
}

#[test]
fn inner_html_setter_replaces_existing_content() {
    assert_eq!(
        eval(
            r#"
        const target = document.getElementById('target');
        target.innerHTML = '<p>first</p>';

        // Set new content
        target.innerHTML = '<span>second</span>';

        // Old content should be gone, new content present
        const hasSpan = target.firstChild !== null && target.firstChild.nodeName === 'SPAN';
        const content = target.textContent;

        (hasSpan && content === 'second') ? 'true' : 'false'
    "#
        ),
        "true"
    );
}

#[test]
fn inner_html_setter_parses_nested_markup() {
    assert_eq!(
        eval(
            r#"
        const target = document.getElementById('target');
        target.innerHTML = '<div><span>nested</span></div>';

        // Check structure
        const div = target.firstChild;
        const span = div.firstChild;

        (div.nodeName === 'DIV' && span.nodeName === 'SPAN' && span.textContent === 'nested') ? 'true' : 'false'
    "#
        ),
        "true"
    );
}

#[test]
fn inner_html_setter_clears_all_children_with_empty_string() {
    assert_eq!(
        eval(
            r#"
        const target = document.getElementById('target');
        target.innerHTML = '<p>content</p><span>more</span>';

        // Clear all children
        target.innerHTML = '';

        target.firstChild === null ? 'true' : 'false'
    "#
        ),
        "true"
    );
}

#[test]
fn inner_html_setter_creates_text_node_for_plain_text() {
    assert_eq!(
        eval(
            r#"
        const target = document.getElementById('target');
        target.innerHTML = 'plain text';

        // Should have a single text node
        const node = target.firstChild;
        (node !== null && node.nodeType === 3 && node.textContent === 'plain text') ? 'true' : 'false'
    "#
        ),
        "true"
    );
}

#[test]
fn inner_html_old_children_become_detached() {
    assert_eq!(
        eval(
            r#"
        const target = document.getElementById('target');
        target.innerHTML = '<p>original</p>';

        // Keep a reference to the old child
        const oldP = target.firstChild;

        // Replace innerHTML
        target.innerHTML = '<span>new</span>';

        // Old reference should still be readable but no longer a child of target
        const isDetached = (
            oldP.nodeName === 'P' &&
            oldP.textContent === 'original' &&
            oldP.parentNode === null
        );

        isDetached ? 'true' : 'false'
    "#
        ),
        "true"
    );
}

#[test]
fn inner_html_multiple_siblings() {
    assert_eq!(
        eval(
            r#"
        const target = document.getElementById('target');
        target.innerHTML = '<p>one</p><p>two</p><p>three</p>';

        // Count children
        const count = target.children.length;

        // All should be p elements
        let allP = true;
        for (let i = 0; i < target.children.length; i++) {
            if (target.children[i].nodeName !== 'P') {
                allP = false;
                break;
            }
        }

        (count === 3 && allP) ? 'true' : 'false'
    "#
        ),
        "true"
    );
}

#[test]
fn inner_html_with_text_and_elements() {
    assert_eq!(
        eval(
            r#"
        const target = document.getElementById('target');
        target.innerHTML = 'hello <b>world</b> and <i>more</i>';

        // Should have text nodes and element nodes mixed
        const textCount = Array.from(target.childNodes).filter(n => n.nodeType === 3).length;
        const elemCount = target.children.length;

        (textCount >= 2 && elemCount === 2) ? 'true' : 'false'
    "#
        ),
        "true"
    );
}

#[test]
fn inner_html_html_entities_decoded() {
    assert_eq!(
        eval(
            r#"
        const target = document.getElementById('target');
        target.innerHTML = '<p>Hello &amp; goodbye</p>';

        const text = target.firstChild.textContent;
        text === 'Hello & goodbye' ? 'true' : 'false'
    "#
        ),
        "true"
    );
}
