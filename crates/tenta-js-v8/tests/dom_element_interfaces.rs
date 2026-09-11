//! Tests for Phase 4d: per-tag HTML element interfaces.
//!
//! Each interactive/common tag gets its own interface (and global constructor)
//! so `instanceof HTMLInputElement` and friends work, with the prototype chain
//! HTMLXxxElement -> HTMLElement -> Element -> Node -> EventTarget. Only the
//! safe reflected attributes are wired: `name`, `placeholder` (DOMString) and
//! `disabled` (boolean); `HTMLLabelElement.htmlFor` reflects the `for`
//! attribute. The dirty value model (`value`/`checked`/`selected`) and the
//! enumerated/URL attributes are intentionally absent.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with `<html><body></body></html>`; tests create their own nodes.
fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::new();
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    d.mutate(Mutation::Append { parent: d.document_node(), child: html }).unwrap();
    d.mutate(Mutation::Append { parent: html, child: body }).unwrap();
    Rc::new(RefCell::new(d))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(document());
    engine.evaluate(source).expect("script should evaluate without throwing")
}

#[test]
fn per_tag_elements_get_their_own_interface_prototype() {
    assert_eq!(
        eval(
            r#"
            const mk = (t) => document.createElement(t);
            [
                mk('div')      instanceof HTMLDivElement,
                mk('span')     instanceof HTMLSpanElement,
                mk('p')        instanceof HTMLParagraphElement,
                mk('a')        instanceof HTMLAnchorElement,
                mk('img')      instanceof HTMLImageElement,
                mk('input')    instanceof HTMLInputElement,
                mk('button')   instanceof HTMLButtonElement,
                mk('select')   instanceof HTMLSelectElement,
                mk('textarea') instanceof HTMLTextAreaElement,
                mk('option')   instanceof HTMLOptionElement,
                mk('form')     instanceof HTMLFormElement,
                mk('label')    instanceof HTMLLabelElement,
            ].join(',');
        "#
        ),
        "true,true,true,true,true,true,true,true,true,true,true,true"
    );
}

#[test]
fn the_prototype_chain_reaches_htmlelement_and_element() {
    assert_eq!(
        eval(
            r#"
            const input = document.createElement('input');
            [
                input instanceof HTMLElement,
                input instanceof Element,
                input instanceof Node,
                Object.getPrototypeOf(HTMLInputElement.prototype) === HTMLElement.prototype,
                typeof input.click,   // inherited from HTMLElement
            ].join(',');
        "#
        ),
        "true,true,true,true,function"
    );
}

#[test]
fn a_wrong_tag_is_not_an_instance_of_a_sibling_interface() {
    assert_eq!(
        eval(
            r#"
            const div = document.createElement('div');
            const input = document.createElement('input');
            [
                div instanceof HTMLInputElement,
                input instanceof HTMLDivElement,
                div instanceof HTMLElement,
            ].join(',');
        "#
        ),
        "false,false,true"
    );
}

#[test]
fn name_reflects_as_a_string_attribute() {
    assert_eq!(
        eval(
            r#"
            const input = document.createElement('input');
            const before = input.name;              // absent -> ""
            input.name = 'email';
            [before, input.name, input.getAttribute('name')].join('|');
        "#
        ),
        "|email|email"
    );
}

#[test]
fn placeholder_reflects_on_input_and_textarea() {
    assert_eq!(
        eval(
            r#"
            const input = document.createElement('input');
            input.setAttribute('placeholder', 'Type here');
            const area = document.createElement('textarea');
            area.placeholder = 'Body';
            [input.placeholder, area.placeholder, area.getAttribute('placeholder')].join('|');
        "#
        ),
        "Type here|Body|Body"
    );
}

#[test]
fn disabled_reflects_as_a_boolean_attribute() {
    assert_eq!(
        eval(
            r#"
            const b = document.createElement('button');
            const initial = b.disabled;             // absent -> false
            b.disabled = true;
            const present = b.getAttribute('disabled');   // "" when set
            const on = b.disabled;
            b.disabled = false;
            const off = b.disabled;
            const gone = b.hasAttribute('disabled');
            [initial, present, on, off, gone].join('|');
        "#
        ),
        "false||true|false|false"
    );
}

#[test]
fn disabled_is_true_for_any_present_value() {
    assert_eq!(
        eval(
            r#"
            const s = document.createElement('select');
            s.setAttribute('disabled', 'disabled');
            String(s.disabled);
        "#
        ),
        "true"
    );
}

#[test]
fn label_html_for_reflects_the_for_attribute() {
    assert_eq!(
        eval(
            r#"
            const label = document.createElement('label');
            const before = label.htmlFor;
            label.htmlFor = 'email';
            [before, label.htmlFor, label.getAttribute('for')].join('|');
        "#
        ),
        "|email|email"
    );
}

#[test]
fn form_name_reflects() {
    assert_eq!(
        eval(
            r#"
            const form = document.createElement('form');
            form.name = 'login';
            [form.name, form.getAttribute('name'), form instanceof HTMLFormElement].join('|');
        "#
        ),
        "login|login|true"
    );
}

#[test]
fn constructors_are_present_on_the_global() {
    assert_eq!(
        eval(
            r#"
            [
                typeof HTMLInputElement,
                typeof HTMLDivElement,
                typeof HTMLFormElement,
                typeof HTMLLabelElement,
            ].join(',');
        "#
        ),
        "function,function,function,function"
    );
}
