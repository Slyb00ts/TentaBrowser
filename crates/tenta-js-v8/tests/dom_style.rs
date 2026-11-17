//! Tests for Phase 4c-2: `HTMLElement.style` (`CSSStyleDeclaration`).
//!
//! The inline `style` attribute is exposed as a CSSOM object model:
//! cssText/length/item/getPropertyValue/getPropertyPriority/setProperty/
//! removeProperty. The per-property attributes (`style.color`) are covered by
//! `css_style_properties.rs`. Includes the R3 wrapper-GC identity case.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::Atom;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_js_v8::Engine;

/// `<html><body><div id="target" style="color: red; width: 10px"></div></body></html>`
fn test_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
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
    document
        .mutate(Mutation::SetAttribute {
            element: div,
            name: QualName::new(atoms::EMPTY, Atom::new("style")),
            value: "color: red; width: 10px".to_owned(),
        })
        .expect("set style attribute");

    Rc::new(RefCell::new(document))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn style_reflects_the_inline_declaration_block() {
    assert_eq!(
        eval(
            r#"
            const s = document.querySelector('#target').style;
            [
                s.length,
                s.item(0),
                s.item(1),
                s.item(2),
                s.getPropertyValue('color'),
                s.getPropertyValue('width'),
                s.getPropertyValue('missing'),
                s.cssText,
            ].join('|');
        "#
        ),
        "2|color|width||red|10px||color: red; width: 10px;"
    );
}

#[test]
fn set_property_updates_existing_and_appends_new() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            el.style.setProperty('color', 'blue');   // update in place
            el.style.setProperty('height', '5px');   // append
            [el.style.getPropertyValue('color'), el.style.length, el.getAttribute('style')].join('|');
        "#
        ),
        "blue|3|color: blue; width: 10px; height: 5px;"
    );
}

#[test]
fn set_property_with_important_priority() {
    assert_eq!(
        eval(
            r#"
            const s = document.querySelector('#target').style;
            s.setProperty('color', 'green', 'important');
            [s.getPropertyPriority('color'), s.getPropertyPriority('width'), s.cssText].join('|');
        "#
        ),
        "important||color: green !important; width: 10px;"
    );
}

#[test]
fn set_property_with_an_empty_value_removes_it() {
    assert_eq!(
        eval(
            r#"
            const s = document.querySelector('#target').style;
            s.setProperty('color', '');
            [s.getPropertyValue('color'), s.length, s.cssText].join('|');
        "#
        ),
        "|1|width: 10px;"
    );
}

#[test]
fn set_property_with_an_invalid_priority_is_a_no_op() {
    assert_eq!(
        eval(
            r#"
            const s = document.querySelector('#target').style;
            s.setProperty('margin', '0', 'urgent');   // invalid priority: ignored
            [s.getPropertyValue('margin'), s.length].join('|');
        "#
        ),
        "|2"
    );
}

#[test]
fn remove_property_returns_the_old_value() {
    assert_eq!(
        eval(
            r#"
            const s = document.querySelector('#target').style;
            const old = s.removeProperty('color');
            const absent = s.removeProperty('nope');
            [old, absent, s.getPropertyValue('color'), s.cssText].join('|');
        "#
        ),
        "red|||width: 10px;"
    );
}

#[test]
fn css_text_setter_replaces_the_whole_block() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            el.style.cssText = 'margin: 0; padding: 2px';
            [
                el.style.length,
                el.style.getPropertyValue('margin'),
                el.style.getPropertyValue('color'),
                el.style.cssText,
            ].join('|');
        "#
        ),
        // CSSOM exposes four longhands per shorthand and reserializes zero
        // lengths as 0px, matching Chromium and Stylo's declaration block.
        "8|0px||margin: 0px; padding: 2px;"
    );
}

#[test]
fn important_is_parsed_from_the_attribute() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            el.setAttribute('style', 'color: red !important; width: 5px');
            const s = el.style;
            [s.getPropertyPriority('color'), s.getPropertyValue('color'), s.getPropertyPriority('width')].join('|');
        "#
        ),
        "important|red|"
    );
}

#[test]
fn a_duplicated_property_keeps_the_last_declaration() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            el.setAttribute('style', 'color: red; color: green');
            [el.style.getPropertyValue('color'), el.style.length].join('|');
        "#
        ),
        "green|1"
    );
}

#[test]
fn stylo_parses_css_values_that_contain_declaration_punctuation() {
    assert_eq!(
        eval(
            r#"
            const s = document.querySelector('#target').style;
            s.cssText = 'content: "a;b:c"; color: blue !important';
            [s.getPropertyValue('content'), s.getPropertyValue('color'),
             s.getPropertyPriority('color')].join('|');
        "#
        ),
        "\"a;b:c\"|blue|important"
    );
}

#[test]
fn stylo_rejects_invalid_mutations_and_handles_custom_and_shorthand_properties() {
    assert_eq!(
        eval(
            r#"
            const s = document.querySelector('#target').style;
            s.setProperty('color', 'not-a-color');
            const unchanged = s.getPropertyValue('color');
            s.setProperty('--Tone', 'teal');
            s.setProperty('margin', '1px 2px');
            const shorthand = s.getPropertyValue('margin');
            const removed = s.removeProperty('margin');
            [unchanged, s.getPropertyValue('--Tone'), s.getPropertyValue('--tone'),
             shorthand, removed, s.getPropertyValue('margin-top')].join('|');
        "#
        ),
        "red|teal||1px 2px|1px 2px|"
    );
}

#[test]
fn css_supports_uses_stylo_for_both_overloads() {
    assert_eq!(
        eval(
            r#"
            [CSS.supports('display', 'grid'),
             CSS.supports('color', 'not-a-color'),
             CSS.supports('(display: grid) and (color: red)'),
             CSS.supports('(display: not-a-display)')].join('|');
        "#
        ),
        "true|false|true|false"
    );
}

/// `CSS.escape()` serializes an identifier as CSSOM does: a leading digit and
/// control characters as code points, other special characters with a
/// backslash, and the result selects the element whose id it was.
#[test]
fn css_escape_serializes_an_identifier() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            el.id = '1 a.b:c';
            const found = document.querySelector('#' + CSS.escape(el.id)) === el;
            el.id = 'target';
            let missing;
            try { CSS.escape(); missing = 'accepted'; } catch (error) { missing = error.name; }
            [CSS.escape('abc'), CSS.escape('1 a.b:c'), CSS.escape('-'), CSS.escape('--x'),
             CSS.escape('ab'), CSS.escape('é_-9'), CSS.escape(''), found, missing,
             CSS.escape.length].join('|');
        "#
        ),
        "abc|\\31 \\ a\\.b\\:c|\\-|--x|a\\1 b|\u{e9}_-9||true|TypeError|1"
    );
}

#[test]
fn style_is_the_same_object_on_every_read() {
    assert_eq!(
        eval("const el = document.querySelector('#target'); (el.style === el.style).toString();"),
        "true"
    );
}

#[test]
fn a_collected_style_wrapper_keeps_the_element_wrapper_alive() {
    // R3: dropping and GC-ing a CSSStyleDeclaration wrapper must not clear the
    // owning element's HAS_WRAPPER bit or its wrapper identity would break.
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(
            r#"
            globalThis.__el = document.querySelector('#target');
            void __el.style;   // create a style wrapper, keep no reference
            'ok';
        "#,
        )
        .expect("set up element and style wrappers");

    for _ in 0..5 {
        engine.low_memory_gc();
    }

    assert_eq!(
        engine
            .evaluate(
                r#"
                const still = document.querySelector('#target') === globalThis.__el;
                [still, __el.style.getPropertyValue('color')].join(',');
            "#
            )
            .expect("re-check identity after GC"),
        "true,red"
    );
}
