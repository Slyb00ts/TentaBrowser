//! Tests for `ProgressEvent` through V8: the constructor, its
//! `ProgressEventInit` dictionary and the three readonly members.
//!
//! The XHR standard's getter steps return the values the record was constructed
//! with, so `lengthComputable` is *not* derived from `total`. The tests below
//! pin that independence, which is what a `total: Option<u64>` model gets wrong:
//! it would answer `true` where the spec answers `false`.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// `<html><body></body></html>`.
fn test_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);

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

    Rc::new(RefCell::new(document))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn progress_event_constructor_defaults() {
    // `new ProgressEvent(type)` yields the init defaults, and the instance is a
    // `ProgressEvent` and an `Event` with the type it was given.
    assert_eq!(
        eval(
            r#"
            const e = new ProgressEvent('x');
            [e.type, e.lengthComputable, e.loaded, e.total,
             e instanceof ProgressEvent, e instanceof Event].join(',');
        "#
        ),
        "x,false,0,0,true,true"
    );
}

#[test]
fn progress_event_reads_the_init_dictionary() {
    // Both `double` members flow into the record.
    assert_eq!(
        eval(
            r#"
            const e = new ProgressEvent('x', { loaded: 1, total: 2 });
            [e.loaded, e.total].join(',');
        "#
        ),
        "1,2"
    );
}

/// The discriminating pair: `lengthComputable` is the dictionary member's own
/// value, so a non-zero `total` under the init default still answers `false`,
/// and an explicit `true` is kept.
#[test]
fn length_computable_is_not_derived_from_total() {
    assert_eq!(
        eval(
            r#"
            const derived = new ProgressEvent('x', { loaded: 1, total: 2 });
            const explicit = new ProgressEvent('x', { lengthComputable: true, loaded: 1, total: 2 });
            [derived.lengthComputable, explicit.lengthComputable].join(',');
        "#
        ),
        "false,true"
    );
}

/// The three members are accessor-only: a sloppy-mode assignment is a silent
/// no-op (in strict mode it is a `TypeError`, which the second half asserts),
/// so no value is stored.
#[test]
fn the_progress_members_are_getters_only() {
    assert_eq!(
        eval(
            r#"
            const e = new ProgressEvent('x', { lengthComputable: true, loaded: 1, total: 2 });
            e.lengthComputable = false; e.loaded = 9; e.total = 9;
            [e.lengthComputable, e.loaded, e.total].join(',');
        "#
        ),
        "true,1,2"
    );

    assert_eq!(
        eval(
            r#"
            const e = new ProgressEvent('x', { loaded: 1 });
            try {
                (() => { 'use strict'; e.loaded = 9; })();
                'no throw';
            } catch (error) {
                (error instanceof TypeError) + '';
            }
        "#
        ),
        "true"
    );
}
