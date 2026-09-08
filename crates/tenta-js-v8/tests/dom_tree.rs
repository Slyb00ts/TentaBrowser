//! Binding phase 1a: read-only DOM tree navigation through the generated V8
//! glue (`docs/spec/05-javascript.md` §3).
//!
//! This extends the Phase 0c slice from a single `documentElement` read to the
//! full read-only navigation surface: `nodeName`/`tagName`, the parent, child
//! and sibling links, the reflected `id`, and the live `childNodes`
//! `NodeList` with its `length` and indexed access. It builds a small element
//! tree with no stray text nodes and asserts the JavaScript view of it.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_js_v8::Engine;

/// A document whose tree is `document -> html -> body -> div#app`, built purely
/// through the DOM mutation API so no whitespace text nodes appear between the
/// elements.
fn tree_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let div = document.create_html_element(atoms::DIV);

    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append <html> under the document node");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append <body> under <html>");
    document
        .mutate(Mutation::Append {
            parent: body,
            child: div,
        })
        .expect("append <div> under <body>");
    document
        .mutate(Mutation::SetAttribute {
            element: div,
            name: QualName::new(atoms::EMPTY, atoms::ID),
            value: "app".to_owned(),
        })
        .expect("set the id attribute of <div>");

    Rc::new(RefCell::new(document))
}

/// Evaluates `source` against a fresh engine over the shared tree and returns
/// the stringified result.
fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(tree_document());
    engine
        .evaluate(source)
        .expect("the navigation slice evaluates without throwing")
}

#[test]
fn document_element_tag_name_is_uppercased() {
    // `tagName` ASCII-uppercases the qualified name of an HTML element.
    assert_eq!(eval("document.documentElement.tagName === 'HTML'"), "true");
}

#[test]
fn first_child_navigation_reaches_the_div() {
    // documentElement(html) -> firstChild(body) -> firstChild(div).
    assert_eq!(
        eval("document.documentElement.firstChild.firstChild.tagName === 'DIV'"),
        "true",
    );
}

#[test]
fn node_name_matches_tag_name_for_elements() {
    assert_eq!(eval("document.documentElement.nodeName === 'HTML'"), "true",);
    // The document node reports the fixed `#document` name.
    assert_eq!(eval("document.nodeName === '#document'"), "true");
}

#[test]
fn reflected_id_attribute_reads_back() {
    assert_eq!(
        eval("document.documentElement.firstChild.firstChild.id === 'app'"),
        "true",
    );
    // An element with no id reflects the empty string.
    assert_eq!(eval("document.documentElement.id === ''"), "true");
}

#[test]
fn sibling_and_parent_links_round_trip() {
    // html has exactly one child, so body has no siblings, and its parent is
    // the document element again.
    assert_eq!(
        eval("document.documentElement.firstChild.nextSibling === null"),
        "true",
    );
    assert_eq!(
        eval("document.documentElement.firstChild.parentNode === document.documentElement"),
        "true",
    );
    assert_eq!(
        eval("document.documentElement.firstChild.parentElement === document.documentElement"),
        "true",
    );
    // The document element's parent is the document, which is not an element.
    assert_eq!(
        eval("document.documentElement.parentElement === null"),
        "true",
    );
}

#[test]
fn first_element_child_skips_to_the_element() {
    assert_eq!(
        eval("document.documentElement.firstElementChild.tagName === 'BODY'"),
        "true",
    );
}

#[test]
fn child_nodes_length_counts_children() {
    // html has one child (body); body has one child (div).
    assert_eq!(
        eval("document.documentElement.childNodes.length === 1"),
        "true"
    );
    assert_eq!(
        eval("document.documentElement.firstChild.childNodes.length === 1"),
        "true",
    );
}

#[test]
fn child_nodes_indexed_access_matches_first_child() {
    // The indexed getter maps to `item`, and identity is preserved: the same
    // node yields the same wrapper whether reached through the list or the
    // sibling links.
    assert_eq!(
        eval("document.documentElement.childNodes[0] === document.documentElement.firstChild"),
        "true",
    );
    // The indexed getter and the `item` method are the same access path.
    assert_eq!(
        eval(
            "document.documentElement.childNodes.item(0) === document.documentElement.childNodes[0]"
        ),
        "true",
    );
    // `item` returns null past the end (https://dom.spec.whatwg.org/#dom-nodelist-item).
    assert_eq!(
        eval("document.documentElement.childNodes.item(5) === null"),
        "true",
    );
}

#[test]
fn child_nodes_is_the_same_object_each_read() {
    // `[SameObject]`: the wrapper world keys the collection on the owner's
    // tagged handle, so two reads return the identical object.
    assert_eq!(
        eval("document.documentElement.childNodes === document.documentElement.childNodes"),
        "true",
    );
}

#[test]
fn wrappers_expose_the_prototype_chain() {
    // The installed constructors are globals, so `instanceof` walks the
    // prototype chain the templates set up.
    assert_eq!(eval("document.documentElement instanceof Element"), "true");
    assert_eq!(eval("document.documentElement instanceof Node"), "true");
    assert_eq!(
        eval("document.documentElement.childNodes instanceof NodeList"),
        "true"
    );
    assert_eq!(eval("document instanceof Document"), "true");
}

#[test]
fn teardown_clears_has_wrapper_for_wrappers_still_live() {
    // A wrapper still reachable from JS at engine teardown is dropped without
    // its finalizer ever running, so `Engine::Drop` must clear its
    // `HAS_WRAPPER` bit explicitly. Otherwise a document that outlives the
    // engine (its `Rc` is shared) keeps those nodes pinned and unreclaimable
    // by a later `sweep_detached`.
    let document = tree_document();
    let (doc_node, root) = {
        let doc = document.borrow();
        (
            doc.document_node(),
            doc.document_element()
                .expect("the tree has a document element"),
        )
    };

    let mut engine = Engine::with_document(document.clone());
    // Materialise several wrappers and root them in a global for the whole
    // isolate lifetime, so none is collected before teardown. The `childNodes`
    // wrapper anchors the same node id as `documentElement`, exercising the
    // shared-bit case.
    engine
        .evaluate(
            "globalThis.keep = [document, document.documentElement, \
             document.documentElement.childNodes];",
        )
        .expect("the wrapper-rooting script evaluates without throwing");

    // While the engine is alive, the live wrappers pin their nodes.
    assert!(
        document.borrow().has_wrapper(doc_node),
        "the live document wrapper pins the document node"
    );
    assert!(
        document.borrow().has_wrapper(root),
        "the live element wrapper pins the document element"
    );

    drop(engine);

    // After teardown every bit must be cleared, even for wrappers that were
    // still live and thus never finalized.
    assert!(
        !document.borrow().has_wrapper(doc_node),
        "teardown must clear HAS_WRAPPER for the document node"
    );
    assert!(
        !document.borrow().has_wrapper(root),
        "teardown must clear HAS_WRAPPER for the document element"
    );
}
