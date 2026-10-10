//! The document's focused element and the `:focus` family
//! (HTML Standard §6.6), through the document API and `querySelectorAll`.

use tenta_core::NodeId;
use tenta_dom::{Document, Mutation};
use tenta_html::{Parser, ParserOptions};

const PAGE: &str = "<!DOCTYPE html><div id=outer><section id=mid><input id=field></section>\
    <button id=other>b</button></div>";

fn page() -> Document {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    parser.feed_last(PAGE.as_bytes());
    parser.finish()
}

fn by_id(doc: &Document, id: &str) -> NodeId {
    doc.query_selector(doc.document_node(), &format!("#{id}"))
        .unwrap()
        .expect("the fixture has the element")
}

fn ids(doc: &Document, selector: &str) -> Vec<String> {
    doc.query_selector_all(doc.document_node(), selector)
        .unwrap()
        .into_iter()
        .filter_map(|node| doc.element_id(node).map(str::to_owned))
        .collect()
}

#[test]
fn nothing_is_focused_in_a_new_document() {
    let doc = page();
    assert_eq!(doc.focused_element(), None);
    assert!(!doc.focus_visible());
    assert!(ids(&doc, ":focus, :focus-visible, :focus-within").is_empty());
}

#[test]
fn the_focus_pseudo_classes_follow_the_focused_element() {
    let mut doc = page();
    let field = by_id(&doc, "field");
    assert!(doc.set_focused_element(Some(field), true));
    assert_eq!(doc.focused_element(), Some(field));
    assert_eq!(ids(&doc, ":focus"), ["field"]);
    assert_eq!(ids(&doc, ":focus-visible"), ["field"]);
    assert_eq!(ids(&doc, "[id]:focus-within"), ["outer", "mid", "field"]);

    assert!(
        doc.set_focused_element(Some(field), false),
        "visibility changed"
    );
    assert!(ids(&doc, ":focus-visible").is_empty());
    assert_eq!(ids(&doc, ":focus"), ["field"]);

    assert!(doc.set_focused_element(None, false));
    assert!(ids(&doc, ":focus, :focus-within").is_empty());
}

#[test]
fn an_unchanged_focus_does_not_invalidate_styles() {
    let mut doc = page();
    let field = by_id(&doc, "field");
    let start = doc.mutation_generation();
    assert!(doc.set_focused_element(Some(field), true));
    assert_ne!(doc.mutation_generation(), start, "a change must restyle");
    let focused = doc.mutation_generation();
    assert!(!doc.set_focused_element(Some(field), true));
    assert_eq!(doc.mutation_generation(), focused);
}

#[test]
fn focus_ends_when_the_element_leaves_the_document() {
    let mut doc = page();
    let field = by_id(&doc, "field");
    doc.set_focused_element(Some(field), true);
    doc.mutate(Mutation::Remove { child: field }).unwrap();
    assert_eq!(doc.focused_element(), None);
    assert!(!doc.focus_visible());

    let mid = by_id(&doc, "mid");
    doc.mutate(Mutation::Append {
        parent: mid,
        child: field,
    })
    .unwrap();
    assert_eq!(
        doc.focused_element(),
        None,
        "inserting the element again does not give focus back"
    );
}
