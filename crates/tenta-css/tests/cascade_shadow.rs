//! Styles across shadow boundaries: a shadow tree's sheets stay inside it,
//! the document's stay outside, `:host` and `::slotted()` reach across, and
//! inheritance follows the flat tree.
#![cfg(feature = "test-util")]

mod cascade_support;

use cascade_support::*;
use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_css::cascade::StyleEngine;
use tenta_dom::Document;

const PAGE: &str = r#"<!DOCTYPE html><html><head><style>
    p { color: rgb(1, 0, 0) }
    #host { color: rgb(9, 0, 0); font-size: 20px }
    .light { letter-spacing: 3px }
    b { text-indent: 7px }
    :host { margin-left: 77px }
</style></head><body>
<p id=outer>outer</p>
<div id=host>
  <template shadowrootmode=open>
    <style>
      :host { margin-left: 11px }
      p { color: rgb(0, 2, 0) }
      #outer, .light { text-indent: 99px }
      ::slotted(b) { margin-top: 5px }
      slot[name=named]::slotted(*) { margin-top: 6px }
    </style>
    <p id=inner>inner <slot></slot></p>
    <span id=plain>plain</span>
    <slot name=named><i id=replaced>replaced</i></slot>
    <slot name=empty><i id=fallback>fallback</i></slot>
  </template>
  <b id=slotted class=light>light</b>
  <u id=named slot=named>named</u>
  <em id=orphan slot=nowhere>orphan</em>
</div>
</body></html>"#;

/// The element with id `id`, in the document or in a shadow tree.
fn find(doc: &Document, id: &str) -> NodeId {
    doc.shadow_including_subtree(doc.document_node())
        .into_iter()
        .find(|&node| doc.is_element(node) && doc.attribute(node, atoms::ID) == Some(id))
        .unwrap_or_else(|| panic!("the fixture has no #{id}"))
}

fn at(doc: &Document, engine: &StyleEngine, id: &str, property: &str) -> String {
    let style = engine
        .style(find(doc, id))
        .unwrap_or_else(|| panic!("#{id} has no style"));
    computed(&style, property)
}

#[test]
fn a_shadow_trees_rules_stay_inside_it_and_the_documents_stay_outside() {
    let (doc, engine) = styled(PAGE);
    // The shadow tree's `p` rule, not the document's.
    assert_eq!(at(&doc, &engine, "inner", "color"), "rgb(0, 2, 0)");
    // The document's `p` rule, and nothing from the shadow sheet.
    assert_eq!(at(&doc, &engine, "outer", "color"), "rgb(1, 0, 0)");
    assert_eq!(at(&doc, &engine, "outer", "text-indent"), "0px");
    // A slotted node is still styled by the document it is written in, and a
    // shadow rule cannot select it by class.
    assert_eq!(at(&doc, &engine, "slotted", "text-indent"), "7px");
    assert_eq!(at(&doc, &engine, "slotted", "letter-spacing"), "3px");
}

#[test]
fn host_and_slotted_rules_reach_across_the_boundary() {
    let (doc, engine) = styled(PAGE);
    // `:host` in the shadow sheet matches the host; in a document sheet it
    // matches nothing.
    assert_eq!(at(&doc, &engine, "host", "margin-left"), "11px");
    assert_eq!(at(&doc, &engine, "slotted", "margin-top"), "5px");
    assert_eq!(at(&doc, &engine, "named", "margin-top"), "6px");
    assert_eq!(at(&doc, &engine, "plain", "margin-top"), "0px");
}

#[test]
fn inheritance_follows_the_flat_tree() {
    let (doc, engine) = styled(PAGE);
    // A child of the shadow root inherits from the host.
    assert_eq!(at(&doc, &engine, "plain", "color"), "rgb(9, 0, 0)");
    assert_eq!(at(&doc, &engine, "inner", "font-size"), "20px");
    // A slotted node inherits from where its slot is, not from its parent.
    assert_eq!(at(&doc, &engine, "slotted", "color"), "rgb(0, 2, 0)");
    // A slot nothing is assigned to shows its own children.
    assert_eq!(at(&doc, &engine, "fallback", "color"), "rgb(9, 0, 0)");
}

#[test]
fn what_is_not_in_the_flat_tree_is_not_styled() {
    let (doc, engine) = styled(PAGE);
    // The fallback of a slot that has assigned nodes, and a child of the host
    // that names a slot the shadow tree does not have.
    assert!(engine.style(find(&doc, "replaced")).is_none());
    assert!(engine.style(find(&doc, "orphan")).is_none());
    assert!(engine.style(find(&doc, "named")).is_some());
}
