//! Tests for Phase 4c-1: `Element.classList` (`DOMTokenList`).
//!
//! classList reflects the `class` attribute as a DOM ordered set. Frameworks
//! toggle classes through add/remove/toggle/contains constantly. Includes the
//! R3 GC case: a collected `DOMTokenList` wrapper must not clear the owning
//! element's `HAS_WRAPPER` bit while the element wrapper is still alive.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::Atom;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_js_v8::Engine;

/// `<html><body><div id="target" class="a b c"></div></body></html>`
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
            name: QualName::new(atoms::EMPTY, Atom::new("class")),
            value: "a b c".to_owned(),
        })
        .expect("set class attribute");

    Rc::new(RefCell::new(document))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

fn eval_err(source: &str) -> String {
    let mut engine = Engine::with_document(test_document());
    match engine.evaluate(source) {
        Ok(value) => panic!("expected the script to throw, got Ok({value:?})"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn class_list_reflects_the_class_attribute() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const cl = el.classList;
            [
                cl.length,
                cl.item(0),
                cl.item(2),
                cl.item(3) === null,
                cl.contains('b'),
                cl.contains('x'),
                cl.value,
            ].join(',');
        "#
        ),
        "3,a,c,true,true,false,a b c"
    );
}

#[test]
fn add_appends_unique_tokens_and_updates_class_name() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            el.classList.add('d');
            el.classList.add('a');          // already present: no-op
            el.classList.add('e', 'f');     // variadic
            [el.classList.length, el.className].join('|');
        "#
        ),
        "6|a b c d e f"
    );
}

#[test]
fn remove_drops_tokens_and_ignores_absent_ones() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            el.classList.remove('a');
            el.classList.remove('x');       // absent: no-op
            el.classList.remove('b', 'c');  // variadic
            [el.classList.length, el.className].join('|');
        "#
        ),
        "0|"
    );
}

#[test]
fn toggle_without_force_flips_membership() {
    assert_eq!(
        eval(
            r#"
            const cl = document.querySelector('#target').classList;
            const removed = cl.toggle('a');   // present -> removed, returns false
            const added = cl.toggle('z');     // absent  -> added,   returns true
            [removed, added, cl.contains('a'), cl.contains('z')].join(',');
        "#
        ),
        "false,true,false,true"
    );
}

#[test]
fn toggle_with_force_pins_the_outcome() {
    assert_eq!(
        eval(
            r#"
            const cl = document.querySelector('#target').classList;
            const a = cl.toggle('q', true);   // add -> true
            const b = cl.toggle('q', true);   // stays -> true
            const c = cl.toggle('q', false);  // remove -> false
            const d = cl.toggle('a', true);   // already present, forced true
            [a, b, c, d, cl.contains('q'), cl.contains('a')].join(',');
        "#
        ),
        "true,true,false,true,false,true"
    );
}

#[test]
fn replace_swaps_in_place_and_reports_presence() {
    assert_eq!(
        eval(
            r#"
            const cl = document.querySelector('#target').classList;
            const ok = cl.replace('b', 'z');       // present -> true, order kept
            const miss = cl.replace('nope', 'x');  // absent  -> false
            [ok, miss, cl.value].join('|');
        "#
        ),
        "true|false|a z c"
    );
}

#[test]
fn replace_dedups_when_the_new_token_already_exists() {
    assert_eq!(
        eval(
            r#"
            const cl = document.querySelector('#target').classList;
            const ok = cl.replace('a', 'c');   // 'c' already present -> collapse
            [ok, cl.value].join('|');
        "#
        ),
        "true|c b"
    );
}

#[test]
fn add_rejects_empty_and_whitespace_tokens() {
    assert!(
        eval_err("document.querySelector('#target').classList.add('');").contains("SyntaxError"),
        "an empty token must raise SyntaxError"
    );
    assert!(
        eval_err("document.querySelector('#target').classList.add('a b');")
            .contains("InvalidCharacterError"),
        "a token with whitespace must raise InvalidCharacterError"
    );
}

#[test]
fn a_failing_add_does_not_partially_mutate() {
    // The empty second token throws; the first must not have been applied.
    assert_eq!(
        eval(
            r#"
            const cl = document.querySelector('#target').classList;
            let threw = false;
            try { cl.add('ok', ''); } catch (e) { threw = true; }
            [threw, cl.contains('ok'), cl.value].join(',');
        "#
        ),
        "true,false,a b c"
    );
}

#[test]
fn value_setter_rewrites_the_whole_class() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            el.classList.value = '  x   y  ';   // raw string stored verbatim
            [el.className, el.classList.length, el.classList.item(0)].join('|');
        "#
        ),
        "  x   y  |2|x"
    );
}

#[test]
fn class_list_is_same_object_and_stays_in_sync_with_class_name() {
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const same = el.classList === el.classList;
            el.className = 'p q';
            const afterClassName = el.classList.contains('p') && el.classList.length === 2;
            el.classList.add('r');
            [same, afterClassName, el.className].join('|');
        "#
        ),
        "true|true|p q r"
    );
}

#[test]
fn a_collected_token_list_wrapper_keeps_the_element_wrapper_alive() {
    // R3: dropping and GC-ing a DOMTokenList wrapper must not clear the owning
    // element's HAS_WRAPPER bit, or the element's wrapper identity would break.
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(
            r#"
            globalThis.__el = document.querySelector('#target');
            void __el.classList;   // create a token-list wrapper, keep no reference
            'ok';
        "#,
        )
        .expect("set up element and token-list wrappers");

    for _ in 0..5 {
        engine.low_memory_gc();
    }

    assert_eq!(
        engine
            .evaluate(
                r#"
                const still = document.querySelector('#target') === globalThis.__el;
                [still, __el.classList.length, __el.classList.contains('a')].join(',');
            "#
            )
            .expect("re-check identity after GC"),
        "true,3,true"
    );
}
