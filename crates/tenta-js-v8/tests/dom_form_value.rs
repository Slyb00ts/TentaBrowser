//! Tests for Phase 4d-2: the form control dirty value model.
//!
//! `docs/spec/05-javascript.md` §9a: `input.value` reads the `value` content
//! attribute until script (or the user) assigns to it, at which point the
//! control holds a *dirty value* that the attribute no longer feeds.
//! `input.checked` mirrors this over the `checked` attribute, and
//! `textarea.value` falls back to the raw value - its child text - instead of
//! to an attribute. Frameworks depend on exactly this split: React writes
//! `value` on every render and reads back what it wrote, not the markup.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with `<html><body></body></html>`; tests build their own markup.
fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::new();
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    d.mutate(Mutation::Append {
        parent: d.document_node(),
        child: html,
    })
    .unwrap();
    d.mutate(Mutation::Append {
        parent: html,
        child: body,
    })
    .unwrap();
    Rc::new(RefCell::new(d))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(document());
    engine
        .evaluate(source)
        .expect("script should evaluate without throwing")
}

#[test]
fn input_value_reads_the_content_attribute_until_it_is_dirtied() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<input value="from markup"><input>';
            const inputs = document.querySelectorAll('input');
            const withAttr = inputs[0], without = inputs[1];
            [withAttr.value, without.value].join('|');
        "#
        ),
        "from markup|"
    );
}

#[test]
fn assigning_input_value_leaves_the_content_attribute_alone() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<input value="default">';
            const input = document.querySelector('input');
            input.value = 'typed';
            [input.value, input.getAttribute('value')].join('|');
        "#
        ),
        "typed|default"
    );
}

#[test]
fn the_content_attribute_stops_feeding_value_once_it_is_dirty() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<input value="one">';
            const input = document.querySelector('input');
            const before = input.value;
            input.value = 'dirty';
            input.setAttribute('value', 'two');
            [before, input.value].join('|');
        "#
        ),
        "one|dirty"
    );
}

#[test]
fn assigning_the_empty_string_still_raises_the_dirty_flag() {
    // The flag is about *having been assigned*, not about the value being
    // non-empty: clearing a prefilled field must not fall back to the markup.
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<input value="prefilled">';
            const input = document.querySelector('input');
            input.value = '';
            [input.value === '', input.getAttribute('value')].join('|');
        "#
        ),
        "true|prefilled"
    );
}

#[test]
fn input_checked_reads_the_attribute_until_it_is_dirtied() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML =
                '<input type="checkbox" checked><input type="checkbox">';
            const boxes = document.querySelectorAll('input');
            [boxes[0].checked, boxes[1].checked].join('|');
        "#
        ),
        "true|false"
    );
}

#[test]
fn assigning_checked_wins_over_the_attribute_in_both_directions() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML =
                '<input type="checkbox" checked><input type="checkbox">';
            const boxes = document.querySelectorAll('input');
            const on = boxes[0], off = boxes[1];
            on.checked = false;
            off.checked = true;
            [
                on.checked,
                on.hasAttribute('checked'),
                off.checked,
                off.hasAttribute('checked'),
            ].join('|');
        "#
        ),
        "false|true|true|false"
    );
}

#[test]
fn textarea_value_falls_back_to_its_child_text() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<textarea>raw value</textarea><textarea></textarea>';
            const areas = document.querySelectorAll('textarea');
            const filled = areas[0], empty = areas[1];
            [filled.value, empty.value === ''].join('|');
        "#
        ),
        "raw value|true"
    );
}

#[test]
fn a_dirty_textarea_ignores_later_child_text_changes() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<textarea>original</textarea>';
            const area = document.querySelector('textarea');
            area.value = 'edited';
            area.textContent = 'replaced';
            [area.value, area.textContent].join('|');
        "#
        ),
        "edited|replaced"
    );
}

#[test]
fn cloning_a_dirtied_control_copies_the_attribute_not_the_dirty_value() {
    // Cloning copies attributes; the dirty value flag is per-node state that
    // the clone does not inherit, so the copy reads the markup default again.
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<input value="default">';
            const input = document.querySelector('input');
            input.value = 'typed';
            const copy = input.cloneNode(false);
            [input.value, copy.value, copy.getAttribute('value')].join('|');
        "#
        ),
        "typed|default|default"
    );
}

#[test]
fn value_and_checked_live_on_the_input_prototype_only() {
    // The dirty value members belong to HTMLInputElement/HTMLTextAreaElement,
    // not to the shared HTMLElement prototype every tag inherits.
    assert_eq!(
        eval(
            r#"
            const onInput = 'value' in document.createElement('input')
                && 'checked' in document.createElement('input');
            const onTextarea = 'value' in document.createElement('textarea');
            const onDiv = 'value' in document.createElement('div')
                || 'checked' in document.createElement('div');
            const checkedOnTextarea = 'checked' in document.createElement('textarea');
            [onInput, onTextarea, onDiv, checkedOnTextarea].join('|');
        "#
        ),
        "true|true|false|false"
    );
}
