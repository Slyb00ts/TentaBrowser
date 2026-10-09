//! V8 WebIDL interface-brand selection for `(Node or DOMString)` and direct
//! `Node` dictionary members.
//!
//! These tests keep union selection distinct from JavaScript prototype shape:
//! a real platform wrapper carries a native `FunctionTemplate` brand, while a
//! plain object, iterable, unrelated platform wrapper, or object inheriting
//! from a Node must reach the string fallback.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with the HTML/body structure the scripts mutate.
fn document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let root = document.document_node();
    document
        .mutate(Mutation::Append {
            parent: root,
            child: html,
        })
        .expect("append html");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append body");
    Rc::new(RefCell::new(document))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(document());
    engine
        .evaluate(source)
        .expect("the script evaluates without throwing")
}

#[test]
fn ordinary_objects_arrays_and_iterables_use_the_string_arm_once() {
    assert_eq!(
        eval(
            r#"
            const host = document.createElement('div');
            let calls = 0;
            const plain = { toString() { calls++; return 'plain'; } };
            const array = ['ignored'];
            array.toString = () => { calls++; return 'array'; };
            const iterable = {
                *[Symbol.iterator]() { yield document.body; },
                toString() { calls++; return 'iterable'; },
            };
            host.append(plain, array, iterable);
            [calls, host.childNodes.length, host.textContent].join('|');
            "#,
        ),
        "3|3|plainarrayiterable"
    );
}

#[test]
fn unrelated_platform_wrappers_use_the_string_arm() {
    assert_eq!(
        eval(
            r#"
            const host = document.createElement('div');
            host.append(
                new URL('https://example.com/path'),
                new Headers(),
                new Blob(['bytes']),
            );
            [host.childNodes.length, host.textContent].join('|');
            "#,
        ),
        "3|https://example.com/path[object Headers][object Blob]"
    );
}

#[test]
fn prototype_spoofs_use_the_string_arm() {
    assert_eq!(
        eval(
            r#"
            const host = document.createElement('div');
            const real = document.createElement('span');
            let calls = 0;
            const fromPrototype = Object.create(Node.prototype);
            fromPrototype.toString = () => { calls++; return 'prototype'; };
            const fromWrapper = Object.create(real);
            fromWrapper.toString = () => { calls++; return 'wrapper'; };
            host.append(fromPrototype, fromWrapper);
            [calls, host.childNodes.length, host.textContent].join('|');
            "#,
        ),
        "2|2|prototypewrapper"
    );
}

#[test]
fn genuine_nodes_and_derived_interface_wrappers_keep_the_interface_arm() {
    assert_eq!(
        eval(
            r#"
            const host = document.createElement('div');
            const span = document.createElement('span');
            const text = document.createTextNode('tail');
            host.append(span, text);
            const file = new File(['x'], 'x.txt');
            [
                host.childNodes[0] === span,
                host.childNodes[1] === text,
                span instanceof Node,
                span instanceof Element,
                new Blob([file]).size,
            ].join('|');
            "#,
        ),
        "true|true|true|true|1"
    );
}

#[test]
fn a_throwing_string_conversion_is_observed_once_and_preserved() {
    assert_eq!(
        eval(
            r#"
            const host = document.createElement('div');
            const original = { marker: 7 };
            let calls = 0;
            let caught;
            try {
                host.append({
                    [Symbol.toPrimitive]() {
                        calls++;
                        throw original;
                    },
                    toString() { calls += 100; return 'wrong'; },
                });
            } catch (error) {
                caught = error;
            }
            [calls, caught === original, host.childNodes.length].join('|');
            "#,
        ),
        "1|true|0"
    );
}

#[test]
fn a_throwing_iterator_getter_does_not_reach_later_union_arms() {
    assert_eq!(
        eval(
            r#"
            const original = { marker: 9 };
            let stringCalls = 0;
            const input = {
                get [Symbol.iterator]() { throw original; },
                toString() { stringCalls++; return 'a=1'; },
            };
            let caught;
            try {
                new URLSearchParams(input);
            } catch (error) {
                caught = error;
            }
            [caught === original, stringCalls].join('|');
            "#,
        ),
        "true|0"
    );
}

#[test]
fn direct_node_members_reject_unrelated_and_spoofed_objects() {
    assert_eq!(
        eval(
            r#"
            const node = document.body;
            const init = value => ({
                startContainer: value,
                startOffset: 0,
                endContainer: node,
                endOffset: 0,
            });
            const errorName = value => {
                try {
                    new StaticRange(init(value));
                    return 'no throw';
                } catch (error) {
                    return error.name;
                }
            };
            const attr = document.createElement('div');
            attr.setAttribute('data-x', 'x');
            [
                errorName(new URL('https://example.com/')),
                errorName(new Headers()),
                errorName({}),
                errorName(Object.create(Node.prototype)),
                errorName(Object.create(node)),
                errorName(attr.getAttributeNode('data-x')),
                new StaticRange(init(node)) instanceof StaticRange,
            ].join('|');
            "#,
        ),
        "TypeError|TypeError|TypeError|TypeError|TypeError|InvalidNodeTypeError|true"
    );
}
