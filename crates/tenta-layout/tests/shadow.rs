//! Layout of the flat tree: a shadow host's boxes are those of its shadow
//! tree, slotted nodes are laid out where their slot is, and what the flat
//! tree leaves out has no box.

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

mod support;

use support::lay_out;

const COMPONENT: &str = r#"<!doctype html><body style="margin:0">
<div id=host style="width:300px">
  <template shadowrootmode=open>
    <style>
      :host { display: block; padding: 10px }
      #row { display: flex; gap: 10px }
      #label { width: 100px; height: 30px }
      ::slotted(span) { display: block; width: 50px; height: 20px }
    </style>
    <div id=row><div id=label></div><slot></slot></div>
    <slot name=footer><div id=fallback style="height:7px"></div></slot>
  </template>
  <span id=a></span><span id=b></span>
  <p id=orphan slot=missing style="height:500px"></p>
</div>
<div id=after style="height:5px"></div>"#;

#[test]
fn a_shadow_tree_is_laid_out_in_place_of_its_hosts_children() {
    let laid = lay_out(COMPONENT, (800.0, 600.0));
    let rect = |id: &str| {
        let r = laid.rect(id);
        [r.x, r.y, r.w, r.h]
    };
    // 300px of content and the padding `:host` adds; the row, the fallback of
    // the empty slot and the padding again.
    assert_eq!(rect("host"), [0.0, 0.0, 320.0, 57.0]);
    assert_eq!(rect("row"), [10.0, 10.0, 300.0, 30.0]);
    assert_eq!(rect("label"), [10.0, 10.0, 100.0, 30.0]);
    assert_eq!(rect("fallback"), [10.0, 40.0, 300.0, 7.0]);
    assert_eq!(rect("after"), [0.0, 57.0, 800.0, 5.0]);
}

#[test]
fn slotted_nodes_are_items_of_the_container_their_slot_is_in() {
    let laid = lay_out(COMPONENT, (800.0, 600.0));
    let rect = |id: &str| {
        let r = laid.rect(id);
        [r.x, r.y, r.w, r.h]
    };
    // The slot has no box of its own, so the two spans are flex items of the
    // row, after the label and sized by `::slotted()`.
    assert_eq!(rect("a"), [120.0, 10.0, 50.0, 20.0]);
    assert_eq!(rect("b"), [180.0, 10.0, 50.0, 20.0]);
}

#[test]
fn a_host_child_that_no_slot_takes_has_no_box() {
    let laid = lay_out(COMPONENT, (800.0, 600.0));
    let orphan = laid.node("orphan");
    assert!(laid.frags.fragments_of(orphan).is_empty());
}
