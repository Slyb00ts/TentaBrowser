//! Real QuickJS controls for the bounded native structured-clone entry.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::{NativeMemoryBudget, NativeMemoryComponent};
use tenta_js::engine::ScriptErrorKind;
use tenta_js::event_loop::{Clock, MonoTime};
use tenta_js::storage::PageStorage;

use crate::Engine;

fn engine_with_budget(
    document: Rc<RefCell<tenta_dom::Document>>,
    budget: NativeMemoryBudget,
) -> Engine {
    Engine::with_document_on_clock_storage_and_memory(
        document,
        Clock::virtual_at(MonoTime::ZERO),
        None,
        tenta_net::CookieJar::new(),
        PageStorage::isolated(),
        budget,
    )
    .expect("bounded QuickJS engine")
}

#[test]
fn clone_keeps_cycles_shared_identity_and_depth_first_getter_order() {
    let mut engine = Engine::new().unwrap();
    let result = engine
        .evaluate(
            r#"
            const log = [];
            const shared = { value: 7 };
            const original = {
              get a() { log.push('a'); return { get child() { log.push('child'); return shared; } }; },
              get b() { log.push('b'); return shared; }
            };
            original.self = original;
            const copy = structuredClone(original);
            [log.join(','), copy.self === copy, copy.a.child === copy.b,
             copy.b.value].join('|');
            "#,
        )
        .unwrap();
    assert_eq!(result, "a,child,b|true|true|7");
}

#[test]
fn clone_snapshots_enumerable_keys_but_checks_current_ownership() {
    let mut engine = Engine::new().unwrap();
    let result = engine
        .evaluate(
            r#"
            const source = {};
            Object.defineProperty(source, 'a', { enumerable:true, get() {
              delete source.b;
              Object.defineProperty(source, 'b', {value: 9, enumerable:false, configurable:true});
              return 1;
            }});
            source.b = 2;
            const copy = structuredClone(source);
            [Object.keys(copy).join(','), copy.b, Object.getOwnPropertyDescriptor(copy, 'b').enumerable].join('|');
            "#,
        )
        .unwrap();
    assert_eq!(result, "a,b|9|true");
}

#[test]
fn clone_uses_own_data_properties_and_preserves_lone_surrogates() {
    let mut engine = Engine::new().unwrap();
    let result = engine
        .evaluate(
            r#"
            const source = {};
            Object.defineProperty(source, '__proto__', {value: {marker: 5}, enumerable:true});
            source.text = String.fromCharCode(0xd800);
            const copy = structuredClone(source);
            [Object.getPrototypeOf(copy) === Object.prototype,
             Object.hasOwn(copy, '__proto__'), copy.__proto__.marker,
             copy.text.charCodeAt(0)].join('|');
            "#,
        )
        .unwrap();
    assert_eq!(result, "true|true|5|55296");
}

#[test]
fn transfer_detaches_in_order_and_keeps_an_earlier_detach_on_later_failure() {
    let mut engine = Engine::new().unwrap();
    let result = engine
        .evaluate(
            r#"
            const first = new ArrayBuffer(4), second = new ArrayBuffer(4);
            new Uint8Array(first)[0] = 77;
            const copy = structuredClone({ first }, { transfer: [first] });
            const copied = new Uint8Array(copy.first)[0];
            const third = new ArrayBuffer(2), fourth = new ArrayBuffer(2);
            structuredClone(fourth, { transfer: [fourth] });
            let name = 'none';
            try { structuredClone({}, { transfer: [third, fourth] }); }
            catch (error) { name = error.name; }
            [first.byteLength, copied, third.byteLength, fourth.byteLength, name].join('|');
            "#,
        )
        .unwrap();
    assert_eq!(result, "0|77|0|0|DataCloneError");
}

#[test]
fn a_getter_can_detach_a_listed_buffer_before_the_transfer_phase() {
    let mut engine = Engine::new().unwrap();
    let result = engine
        .evaluate(
            r#"
            const buffer = new ArrayBuffer(4);
            const source = { get data() {
              structuredClone(buffer, {transfer: [buffer]});
              return 1;
            }};
            let name = 'none';
            try { structuredClone(source, {transfer: [buffer]}); }
            catch (error) { name = error.name; }
            `${name}|${buffer.byteLength}`;
            "#,
        )
        .unwrap();
    assert_eq!(result, "DataCloneError|0");
}

#[test]
fn a_primitive_transfer_item_does_not_close_the_web_idl_iterator() {
    let mut engine = Engine::new().unwrap();
    let result = engine
        .evaluate(
            r#"
            let closed = 0;
            const transfer = {
              [Symbol.iterator]() {
                return {
                  next() { return {done: false, value: 7}; },
                  return() { closed++; return {done: true}; }
                };
              }
            };
            let name = 'none';
            try { structuredClone({}, {transfer}); }
            catch (error) { name = error.name; }
            `${name}|${closed}`;
            "#,
        )
        .unwrap();
    assert_eq!(result, "TypeError|0");
}

#[test]
fn proxy_platform_and_global_values_fail_without_classification_traps() {
    let mut engine = Engine::new().unwrap();
    let result = engine
        .evaluate(
            r#"
            let traps = 0;
            const proxy = new Proxy({}, { get() { traps++; throw Error('get'); },
                                           ownKeys() { traps++; throw Error('keys'); } });
            const names = [];
            for (const value of [proxy, new Event('x'), globalThis]) {
              try { structuredClone(value); names.push('accepted'); }
              catch (error) { names.push(error.name); }
            }
            `${names.join(',')}|${traps}`;
            "#,
        )
        .unwrap();
    assert_eq!(result, "DataCloneError,DataCloneError,DataCloneError|0");
}

#[test]
fn ordinary_null_prototype_clones_but_special_native_objects_do_not() {
    let mut engine = Engine::new().unwrap();
    let result = engine
        .evaluate(
            r#"
            const plain = Object.create(null);
            plain.answer = 42;
            const copy = structuredClone(plain);
            const special = [new Date(), new Map(), new ArrayBuffer(4, {maxByteLength: 8})];
            const rejected = special.map(value => {
              Object.setPrototypeOf(value, Object.prototype);
              try { structuredClone(value); return 'accepted'; }
              catch (error) { return error.name; }
            });
            [copy.answer, Object.getPrototypeOf(copy) === Object.prototype,
             rejected.join(',')].join('|');
            "#,
        )
        .unwrap();
    assert_eq!(
        result,
        "42|true|DataCloneError,DataCloneError,DataCloneError"
    );
}

#[test]
fn native_budget_failure_cannot_enter_catch_or_run_later_page_code() {
    let document = Rc::new(RefCell::new(tenta_dom::Document::new()));
    let budget = NativeMemoryBudget::with_limits(32 * 1024, 4096, 4096, 4096);
    let mut engine = engine_with_budget(document.clone(), budget.clone());
    let error = engine
        .evaluate(
            r#"
            const root = document.createElement('div');
            document.appendChild(root);
            try { structuredClone('x'.repeat(2000)); root.setAttribute('after', 'yes'); }
            catch (error) { root.setAttribute('caught', 'yes'); }
            "#,
        )
        .expect_err("the clone crosses the component cap");
    assert_eq!(error.kind, ScriptErrorKind::Terminated);
    assert_eq!(
        budget.first_failure().unwrap().component,
        NativeMemoryComponent::StructuredClone
    );
    assert_eq!(
        budget.used(NativeMemoryComponent::StructuredClone),
        0,
        "the failed workspace has returned every native reservation"
    );
    let document = document.borrow();
    let root = document
        .document_element()
        .expect("root was appended first");
    assert_eq!(document.attribute_str(root, "caught"), None);
    assert_eq!(document.attribute_str(root, "after"), None);
    drop(document);
    assert!(
        engine.evaluate("1").is_err(),
        "the failed page stays stopped"
    );
}
