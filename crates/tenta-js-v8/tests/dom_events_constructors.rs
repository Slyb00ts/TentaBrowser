//! Tests for JS-constructed events wired through V8 (Phase 3c): the `Event` and
//! `CustomEvent` constructors, `class E extends Event` subclassing, `any`
//! `detail` round-tripping and identity, `initCustomEvent`, and
//! `createEvent('CustomEvent')`.
//!
//! These exercise the "construct with realm, then adopt" path: the generated
//! constructor callback builds a host via `T::construct(realm, ...)` and adopts
//! it onto the `this` object V8 allocated for the `new` call, so a constructed
//! (or subclassed) event carries a live DOM host and a stable identity through
//! dispatch. A separate test drives the world's reference-counted value store
//! to show a reclaimed `CustomEvent` releases its interned `detail`.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// `<html><body><div id="target"></div></body></html>`
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
            name: tenta_html::QualName::new(atoms::EMPTY, atoms::ID),
            value: "target".to_owned(),
        })
        .expect("set id attribute");

    Rc::new(RefCell::new(document))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn event_constructor_defaults() {
    // `new Event(type)` yields an initialized, untrusted, non-bubbling,
    // non-cancelable event whose type is set and default not prevented.
    assert_eq!(
        eval(
            r#"
            const e = new Event('x');
            [e.type, e.bubbles, e.cancelable, e.composed, e.isTrusted, e.defaultPrevented]
                .join(',');
        "#
        ),
        "x,false,false,false,false,false"
    );
}

#[test]
fn event_constructor_reads_init_dictionary() {
    // The EventInit members flow into the record.
    assert_eq!(
        eval(
            r#"
            const e = new Event('x', { bubbles: true, cancelable: true, composed: true });
            [e.bubbles, e.cancelable, e.composed].join(',');
        "#
        ),
        "true,true,true"
    );
}

#[test]
fn calling_event_without_new_throws_type_error() {
    assert_eq!(
        eval(
            r#"
            try {
                Event('x');
                'no throw';
            } catch (e) {
                (e instanceof TypeError) + '';
            }
        "#
        ),
        "true"
    );
}

#[test]
fn constructed_event_dispatches_to_a_listener() {
    // A constructed event needs no initEvent: it is already initialized, so it
    // dispatches and the listener sees the same wrapper object.
    assert_eq!(
        eval(
            r#"
            const el = document.querySelector('#target');
            const e = new Event('boom', { bubbles: true });
            let out = '';
            el.addEventListener('boom', (ev) => {
                out = (ev === e) + '/' + ev.type + '/' + (ev.target === el);
            });
            const returned = el.dispatchEvent(e);
            out + '/' + returned;
        "#
        ),
        "true/boom/true/true"
    );
}

#[test]
fn event_subclass_constructs_dispatches_and_keeps_identity() {
    // `class E extends Event` must construct through the base constructor onto
    // the subclass instance V8 allocated, so the host is adopted onto `this`
    // (not a fresh object). The instance is an E and an Event, keeps its JS
    // expando, and is delivered by identity to a listener.
    assert_eq!(
        eval(
            r#"
            class E extends Event {
                constructor(type) { super(type, { bubbles: true }); this.extra = 9; }
            }
            const el = document.querySelector('#target');
            const e = new E('boom');
            let out = '';
            el.addEventListener('boom', (ev) => {
                out = (ev === e) + '/' + (ev instanceof E) + '/' + ev.type + '/' + ev.extra;
            });
            el.dispatchEvent(e);
            (e instanceof E) + '/' + (e instanceof Event) + '/' + out;
        "#
        ),
        "true/true/true/true/boom/9"
    );
}

#[test]
fn custom_event_detail_preserves_object_identity() {
    // `detail` is an `any`: the exact object passed comes back by identity.
    assert_eq!(
        eval(
            r#"
            const obj = { marker: 123 };
            const e = new CustomEvent('x', { detail: obj });
            (e.detail === obj) + '/' + e.detail.marker + '/' + (e instanceof CustomEvent)
                + '/' + (e instanceof Event);
        "#
        ),
        "true/123/true/true"
    );
}

#[test]
fn custom_event_detail_defaults_to_null_and_round_trips_primitives() {
    // Absent and explicit-null detail both read back as null; primitive details
    // round-trip by value.
    assert_eq!(
        eval(
            r#"
            const absent = new CustomEvent('x').detail;
            const explicit = new CustomEvent('x', { detail: null }).detail;
            const number = new CustomEvent('x', { detail: 42 }).detail;
            const text = new CustomEvent('x', { detail: 'hi' }).detail;
            [absent === null, explicit === null, number, text].join(',');
        "#
        ),
        "true,true,42,hi"
    );
}

#[test]
fn init_custom_event_sets_type_flags_and_detail() {
    // initCustomEvent initializes an event from createEvent and swaps in the
    // new detail.
    assert_eq!(
        eval(
            r#"
            const obj = { k: 7 };
            const e = document.createEvent('CustomEvent');
            e.initCustomEvent('go', true, true, obj);
            [e.type, e.bubbles, e.cancelable, e.detail === obj, e.detail.k].join(',');
        "#
        ),
        "go,true,true,true,7"
    );
}

#[test]
fn create_event_custom_event_starts_with_null_detail() {
    // createEvent('CustomEvent') yields a CustomEvent whose detail is null until
    // initialized.
    assert_eq!(
        eval(
            r#"
            const e = document.createEvent('CustomEvent');
            (e instanceof CustomEvent) + '/' + (e.detail === null);
        "#
        ),
        "true/true"
    );
}

#[test]
fn init_custom_event_replacing_detail_releases_the_old_value() {
    // Re-initializing with a new detail must release the previously stored one,
    // so the value store does not grow across repeated initialization. Driving
    // it many times keeps the live-value count bounded (a leak would climb with
    // the loop count).
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(
            r#"
            const e = document.createEvent('CustomEvent');
            for (let i = 0; i < 50; i++) {
                e.initCustomEvent('x', false, false, { i });
            }
            e.initCustomEvent('x', false, false, null);
        "#,
        )
        .expect("evaluate the init loop");
    // One CustomEvent wrapper survives (referenced by `e`); its detail is now
    // null, so it holds no stored value. The 50 object details were released as
    // each initCustomEvent replaced the last, so the live-value count is small
    // and independent of the loop count — not ~50.
    assert!(
        engine.stored_value_count() <= 2,
        "value store should not accumulate replaced details, got {}",
        engine.stored_value_count()
    );
}

#[test]
fn reclaimed_custom_event_releases_its_detail() {
    // A CustomEvent whose wrapper becomes unreachable is collected; the world's
    // dead-event sweep must release its interned object detail so the value
    // store returns to its baseline. Uses the low-memory GC and an explicit
    // reclaim, both test-only hooks.
    let mut engine = Engine::with_document(test_document());
    let baseline = engine.stored_value_count();
    engine
        .evaluate(
            r#"
            // Create many CustomEvents with distinct object details, keeping no
            // references, so every wrapper is immediately unreachable.
            for (let i = 0; i < 100; i++) {
                new CustomEvent('x', { detail: { i } });
            }
        "#,
        )
        .expect("evaluate the allocation loop");

    // Force collection and drain the finalizer-recorded dead events a few times;
    // finalizers may run across more than one GC.
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_events();
    }

    assert!(
        engine.stored_value_count() <= baseline + 2,
        "reclaimed CustomEvent details should be released: baseline {}, now {}",
        baseline,
        engine.stored_value_count()
    );
}
