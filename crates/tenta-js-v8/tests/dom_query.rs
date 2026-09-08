//! Binding phase 1b: node-derived query/create methods, text nodes, and live
//! HTMLCollection through the generated V8 glue (`docs/spec/05-javascript.md` §3).
//!
//! This extends the Phase 1a slice with query methods (querySelector, matches,
//! closest), element creation (createElement, createTextNode, createComment),
//! text nodes (Text and Comment interfaces), CharacterData members (data, length),
//! the live HTMLCollection with child-element iteration, and textContent across
//! all node types. It builds a document tree with text nodes and elements, then
//! exercises the full query and creation surface.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_js_v8::Engine;

/// A document whose tree is `document -> html -> body -> div#app -> div -> text`,
/// with text nodes interspersed and a structure suitable for testing query and
/// element creation.
fn tree_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let app_div = document.create_html_element(atoms::DIV);
    let child_div = document.create_html_element(atoms::DIV);

    // Build: document -> html -> body -> div#app -> div
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
            child: app_div,
        })
        .expect("append <div#app>");
    document
        .mutate(Mutation::Append {
            parent: app_div,
            child: child_div,
        })
        .expect("append <div> under <div#app>");
    document
        .mutate(Mutation::SetAttribute {
            element: app_div,
            name: QualName::new(atoms::EMPTY, atoms::ID),
            value: "app".to_owned(),
        })
        .expect("set id on #app");

    Rc::new(RefCell::new(document))
}

/// Evaluates `source` against a fresh engine over the tree and returns the stringified result.
fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(tree_document());
    engine
        .evaluate(source)
        .expect("the script evaluates without throwing")
}

/// Evaluates `source` and returns whether it threw an exception.
fn throws(source: &str) -> bool {
    let mut engine = Engine::with_document(tree_document());
    engine.evaluate(source).is_err()
}

// ===== EXIT GATE TESTS =====

#[test]
fn exit_gate_query_selector_finds_first_element_child() {
    // The exit gate: querySelector('#app').firstElementChild.tagName === 'DIV'
    assert_eq!(
        eval("document.querySelector('#app').firstElementChild.tagName === 'DIV'"),
        "true"
    );
}

// ===== QUERY METHODS =====

#[test]
fn query_selector_finds_element_by_id() {
    assert_eq!(eval("document.querySelector('#app').id === 'app'"), "true");
}

#[test]
fn query_selector_returns_null_for_non_matching_selector() {
    assert_eq!(
        eval("document.querySelector('.nonexistent') === null"),
        "true"
    );
}

#[test]
fn query_selector_with_type_selector() {
    assert_eq!(
        eval("document.querySelector('body').tagName === 'BODY'"),
        "true"
    );
}

#[test]
fn query_selector_with_descendant_combinator() {
    assert_eq!(
        eval("document.querySelector('body div').id === 'app'"),
        "true"
    );
}

#[test]
fn query_selector_with_invalid_selector_throws() {
    // A syntactically invalid selector should throw (SyntaxError).
    assert!(throws("document.querySelector('[')"));
}

#[test]
fn element_query_selector_is_scoped_to_element() {
    // element.querySelector is scoped to element and its descendants
    assert_eq!(
        eval("document.querySelector('#app').querySelector('div').tagName === 'DIV'"),
        "true"
    );
}

#[test]
fn element_query_selector_returns_first_match_only() {
    assert_eq!(
        eval("document.querySelector('#app').querySelector('div').tagName === 'DIV'"),
        "true"
    );
}

#[test]
fn matches_returns_true_for_matching_selector() {
    assert_eq!(
        eval("document.querySelector('#app').matches('#app')"),
        "true"
    );
}

#[test]
fn matches_returns_false_for_non_matching_selector() {
    assert_eq!(
        eval("document.querySelector('#app').matches('.nonexistent')"),
        "false"
    );
}

#[test]
fn closest_returns_self_if_matching() {
    assert_eq!(
        eval("document.querySelector('#app').closest('#app').id === 'app'"),
        "true"
    );
}

#[test]
fn closest_returns_ancestor_if_matching() {
    assert_eq!(
        eval("document.querySelector('#app').closest('html').tagName === 'HTML'"),
        "true"
    );
}

#[test]
fn closest_returns_null_if_no_ancestor_matches() {
    assert_eq!(
        eval("document.querySelector('#app').closest('.nonexistent') === null"),
        "true"
    );
}

// ===== ELEMENT CREATION =====

#[test]
fn create_element_returns_uppercase_tag_name() {
    // HTML elements are lowercased at creation then uppercased on retrieval
    assert_eq!(
        eval("document.createElement('DIV').tagName === 'DIV'"),
        "true"
    );
}

#[test]
fn create_element_returns_detached_node() {
    // A created element has no parent
    assert_eq!(
        eval("document.createElement('div').parentNode === null"),
        "true"
    );
}

#[test]
fn create_element_node_type_is_element() {
    assert_eq!(eval("document.createElement('div').nodeType === 1"), "true");
}

#[test]
fn create_element_rejects_empty_name() {
    assert!(throws("document.createElement('')"));
}

#[test]
fn create_element_rejects_non_letter_start() {
    assert!(throws("document.createElement('1div')"));
}

#[test]
fn create_element_accepts_lowercase_then_uppercases() {
    assert_eq!(
        eval("document.createElement('div').tagName === 'DIV'"),
        "true"
    );
}

// ===== TEXT NODE CREATION =====

#[test]
fn create_text_node_has_node_type_three() {
    assert_eq!(
        eval("document.createTextNode('hello').nodeType === 3"),
        "true"
    );
}

#[test]
fn create_text_node_data_attribute() {
    assert_eq!(
        eval("document.createTextNode('hello').data === 'hello'"),
        "true"
    );
}

#[test]
fn create_text_node_length_attribute() {
    assert_eq!(eval("document.createTextNode('hi').length === 2"), "true");
}

#[test]
fn create_text_node_is_detached() {
    assert_eq!(
        eval("document.createTextNode('text').parentNode === null"),
        "true"
    );
}

#[test]
fn text_node_is_instance_of_text() {
    // Check prototype chain
    assert_eq!(eval("document.createTextNode('x') instanceof Text"), "true");
}

#[test]
fn text_node_length_counts_utf16_code_units() {
    // A surrogate pair (emoji) counts as 2
    assert_eq!(eval("document.createTextNode('😀').length === 2"), "true");
}

// ===== COMMENT NODE CREATION =====

#[test]
fn create_comment_has_node_type_eight() {
    assert_eq!(
        eval("document.createComment('note').nodeType === 8"),
        "true"
    );
}

#[test]
fn create_comment_data_attribute() {
    assert_eq!(
        eval("document.createComment('note').data === 'note'"),
        "true"
    );
}

#[test]
fn create_comment_is_instance_of_comment() {
    assert_eq!(
        eval("document.createComment('x') instanceof Comment"),
        "true"
    );
}

// ===== TEXT CONTENT =====

#[test]
fn text_content_on_text_node_returns_data() {
    assert_eq!(
        eval("document.createTextNode('hi').textContent === 'hi'"),
        "true"
    );
}

#[test]
fn text_content_on_comment_returns_data() {
    assert_eq!(
        eval("document.createComment('hi').textContent === 'hi'"),
        "true"
    );
}

#[test]
fn text_content_on_document_returns_null() {
    assert_eq!(eval("document.textContent === null"), "true");
}

#[test]
fn text_content_on_element_concatenates_descendant_text() {
    // Create an element with a text node child, then read textContent
    assert_eq!(
        eval("const el = document.createElement('p'); el.textContent === ''"),
        "true"
    );
}

// ===== ELEMENT CHILDREN COLLECTION =====

#[test]
fn children_returns_html_collection() {
    // children is [SameObject], so multiple reads return the same wrapper
    assert_eq!(
        eval(
            "document.documentElement.firstElementChild.children === document.documentElement.firstElementChild.children"
        ),
        "true"
    );
}

#[test]
fn children_length_counts_only_elements() {
    assert_eq!(
        eval("document.documentElement.firstElementChild.children.length > 0"),
        "true"
    );
}

#[test]
fn children_item_access_by_index() {
    assert_eq!(
        eval("document.documentElement.firstElementChild.children[0].tagName === 'DIV'"),
        "true"
    );
}

#[test]
fn children_item_method() {
    assert_eq!(
        eval("document.documentElement.firstElementChild.children.item(0).tagName === 'DIV'"),
        "true"
    );
}

#[test]
fn children_named_item_finds_by_id() {
    assert_eq!(
        eval("document.documentElement.firstElementChild.children.namedItem('app').id === 'app'"),
        "true"
    );
}

#[test]
fn children_named_item_returns_null_if_not_found() {
    assert_eq!(
        eval(
            "document.documentElement.firstElementChild.children.namedItem('nonexistent') === null"
        ),
        "true"
    );
}

// ===== ELEMENT CHILD PROPERTIES =====

#[test]
fn first_element_child_is_div() {
    assert_eq!(
        eval("document.documentElement.firstElementChild.firstElementChild.id === 'app'"),
        "true"
    );
}

#[test]
fn last_element_child_is_div() {
    assert_eq!(
        eval("document.documentElement.firstElementChild.lastElementChild.id === 'app'"),
        "true"
    );
}

#[test]
fn child_element_count() {
    assert_eq!(
        eval("document.documentElement.firstElementChild.childElementCount === 1"),
        "true"
    );
}

#[test]
fn first_element_child_of_app_div() {
    assert_eq!(
        eval("document.querySelector('#app').firstElementChild.tagName === 'DIV'"),
        "true"
    );
}

#[test]
fn last_element_child_of_app_div() {
    assert_eq!(
        eval("document.querySelector('#app').lastElementChild.tagName === 'DIV'"),
        "true"
    );
}

// ===== DOCUMENT CHILD QUERIES =====

#[test]
fn document_first_element_child() {
    // The document's first element child is the root element
    assert_eq!(
        eval("document.firstElementChild.tagName === 'HTML'"),
        "true"
    );
}

#[test]
fn document_last_element_child() {
    assert_eq!(eval("document.lastElementChild.tagName === 'HTML'"), "true");
}

#[test]
fn document_child_element_count() {
    // A document normally has exactly one element child (the root)
    assert_eq!(eval("document.childElementCount === 1"), "true");
}

#[test]
fn document_children_collection() {
    // document.children is [SameObject]
    assert_eq!(eval("document.children === document.children"), "true");
    assert_eq!(eval("document.children.length === 1"), "true");
    assert_eq!(eval("document.children[0].tagName === 'HTML'"), "true");
}

#[test]
fn document_get_element_by_id() {
    assert_eq!(eval("document.getElementById('app').id === 'app'"), "true");
}

#[test]
fn document_get_element_by_id_returns_null_if_not_found() {
    assert_eq!(
        eval("document.getElementById('nonexistent') === null"),
        "true"
    );
}

#[test]
fn document_query_selector() {
    assert_eq!(
        eval("document.querySelector('div#app').id === 'app'"),
        "true"
    );
}

// ===== IDENTITY AND SAME_OBJECT =====

#[test]
fn created_element_wrapped_twice_keeps_identity() {
    // If we wrap a created (detached) element twice, it should keep the same wrapper.
    assert_eq!(
        eval("const el = document.createElement('div'); el === el"),
        "true"
    );
}

#[test]
fn element_children_is_same_object_across_reads() {
    // children is [SameObject], meaning the same wrapper is returned each time
    assert_eq!(
        eval(
            "const el = document.documentElement.firstElementChild; const c1 = el.children; const c2 = el.children; c1 === c2"
        ),
        "true"
    );
}

// ===== QUERY_SELECTOR_ALL (STATIC SNAPSHOTS) =====

#[test]
fn query_selector_all_finds_all_matching_divs() {
    assert_eq!(
        eval("document.querySelectorAll('div').length === 2"),
        "true"
    );
}

#[test]
fn query_selector_all_returns_results_in_tree_order() {
    // The first div is #app, the second is its child
    assert_eq!(
        eval(
            "document.querySelectorAll('div')[0].id === 'app' && document.querySelectorAll('div')[1].tagName === 'DIV'"
        ),
        "true"
    );
}

#[test]
fn query_selector_all_item_method_works() {
    assert_eq!(
        eval("document.querySelectorAll('div').item(0).id === 'app'"),
        "true"
    );
}

#[test]
fn query_selector_all_matches_first_element_with_query_selector() {
    // The first result from querySelectorAll should match querySelector
    assert_eq!(
        eval("document.querySelectorAll('div')[0] === document.querySelector('div')"),
        "true"
    );
}

#[test]
fn query_selector_all_returns_new_object_on_each_call() {
    // [NewObject] means each call returns a new wrapper
    assert_eq!(
        eval("document.querySelectorAll('div') !== document.querySelectorAll('div')"),
        "true"
    );
}

#[test]
fn query_selector_all_with_invalid_selector_throws() {
    assert!(throws("document.querySelectorAll('[')"));
}

#[test]
fn element_query_selector_all_is_scoped() {
    // Element.querySelectorAll is scoped to that element's subtree
    assert_eq!(
        eval("document.querySelector('#app').querySelectorAll('div').length === 1"),
        "true"
    );
}

#[test]
fn query_selector_all_empty_result() {
    assert_eq!(
        eval("document.querySelectorAll('.nonexistent').length === 0"),
        "true"
    );
}

// ===== GET_ELEMENTS_BY_TAG_NAME =====

#[test]
fn get_elements_by_tag_name_finds_all_divs() {
    assert_eq!(
        eval("document.getElementsByTagName('div').length === 2"),
        "true"
    );
}

#[test]
fn get_elements_by_tag_name_case_insensitive() {
    // HTML tag names are case-insensitive
    assert_eq!(
        eval("document.getElementsByTagName('DIV').length === 2"),
        "true"
    );
}

#[test]
fn get_elements_by_tag_name_returns_html_collection() {
    assert_eq!(
        eval("document.getElementsByTagName('div') instanceof HTMLCollection"),
        "true"
    );
}

#[test]
fn get_elements_by_tag_name_wildcard_returns_all_elements() {
    // "*" returns all elements
    assert_eq!(
        eval("document.getElementsByTagName('*').length > 2"),
        "true"
    );
}

#[test]
fn get_elements_by_tag_name_nonexistent_returns_empty() {
    assert_eq!(
        eval("document.getElementsByTagName('nonexistent').length === 0"),
        "true"
    );
}

#[test]
fn element_get_elements_by_tag_name_is_scoped() {
    assert_eq!(
        eval("document.querySelector('#app').getElementsByTagName('div').length === 1"),
        "true"
    );
}

#[test]
fn get_elements_by_tag_name_returns_new_object() {
    assert_eq!(
        eval("document.getElementsByTagName('div') !== document.getElementsByTagName('div')"),
        "true"
    );
}

// ===== GET_ELEMENTS_BY_CLASS_NAME =====

#[test]
fn get_elements_by_class_name_single_class() {
    // Build a test tree with class attributes in a separate test
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let el1 = document.create_html_element(atoms::DIV);
    let el2 = document.create_html_element(atoms::DIV);

    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append html");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append body");
    document
        .mutate(Mutation::Append {
            parent: body,
            child: el1,
        })
        .expect("append el1");
    document
        .mutate(Mutation::Append {
            parent: body,
            child: el2,
        })
        .expect("append el2");

    // Set class attribute on el1
    document
        .mutate(Mutation::SetAttribute {
            element: el1,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "foo".to_owned(),
        })
        .expect("set class on el1");

    // Set class attribute on el2
    document
        .mutate(Mutation::SetAttribute {
            element: el2,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "foo bar".to_owned(),
        })
        .expect("set class on el2");

    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    assert_eq!(
        engine
            .evaluate("document.getElementsByClassName('foo').length === 2")
            .unwrap(),
        "true"
    );
}

#[test]
fn get_elements_by_class_name_multiple_classes_matching_all() {
    // Elements must have ALL classes to match
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let el1 = document.create_html_element(atoms::DIV);
    let el2 = document.create_html_element(atoms::DIV);

    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append html");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append body");
    document
        .mutate(Mutation::Append {
            parent: body,
            child: el1,
        })
        .expect("append el1");
    document
        .mutate(Mutation::Append {
            parent: body,
            child: el2,
        })
        .expect("append el2");

    // el1 has only "foo"
    document
        .mutate(Mutation::SetAttribute {
            element: el1,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "foo".to_owned(),
        })
        .expect("set class on el1");

    // el2 has "foo bar"
    document
        .mutate(Mutation::SetAttribute {
            element: el2,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "foo bar".to_owned(),
        })
        .expect("set class on el2");

    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    // Search for "foo bar" should match only el2
    assert_eq!(
        engine
            .evaluate("document.getElementsByClassName('foo bar').length === 1")
            .unwrap(),
        "true"
    );
}

#[test]
fn get_elements_by_class_name_returns_new_object() {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);

    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append html");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append body");

    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    assert_eq!(
        engine
            .evaluate(
                "document.getElementsByClassName('foo') !== document.getElementsByClassName('foo')"
            )
            .unwrap(),
        "true"
    );
}

#[test]
fn get_elements_by_class_name_empty_and_whitespace_match_nothing() {
    // Per DOM, `getElementsByClassName("")` (and a whitespace-only argument)
    // yields an empty collection: an empty token set must match no element,
    // not every element (the `iter().all()` vacuous-truth trap).
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let el1 = document.create_html_element(atoms::DIV);
    let el2 = document.create_html_element(atoms::DIV);
    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append html");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append body");
    document
        .mutate(Mutation::Append {
            parent: body,
            child: el1,
        })
        .expect("append el1");
    document
        .mutate(Mutation::Append {
            parent: body,
            child: el2,
        })
        .expect("append el2");
    document
        .mutate(Mutation::SetAttribute {
            element: el1,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "foo".to_owned(),
        })
        .expect("set class on el1");
    document
        .mutate(Mutation::SetAttribute {
            element: el2,
            name: QualName::new(atoms::EMPTY, atoms::ID),
            value: "second".to_owned(),
        })
        .expect("set id on el2");
    document
        .mutate(Mutation::SetAttribute {
            element: el2,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "foo".to_owned(),
        })
        .expect("set class on el2");

    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    // Empty and whitespace-only arguments select nothing.
    assert_eq!(
        engine
            .evaluate("document.getElementsByClassName('').length === 0")
            .unwrap(),
        "true"
    );
    assert_eq!(
        engine
            .evaluate("document.getElementsByClassName('   ').length === 0")
            .unwrap(),
        "true"
    );
    // Sanity: a real class still matches, and namedItem on the returned static
    // HTMLCollection finds a member by its id.
    assert_eq!(
        engine
            .evaluate("document.getElementsByClassName('foo').length === 2")
            .unwrap(),
        "true"
    );
    assert_eq!(
        engine
            .evaluate("document.getElementsByClassName('foo').namedItem('second').id === 'second'")
            .unwrap(),
        "true"
    );
}
