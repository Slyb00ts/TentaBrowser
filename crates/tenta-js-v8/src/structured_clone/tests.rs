//! Real V8 controls for the bounded native structured-clone entry.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::{NativeMemoryBudget, NativeMemoryComponent};
use tenta_js::event_loop::{Clock, MonoTime};
use tenta_js::storage::PageStorage;

use crate::Engine;

#[test]
fn clone_preserves_cycles_identity_and_depth_first_getter_order() {
    let mut engine = Engine::new();
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
fn clone_snapshots_keys_and_checks_current_ownership() {
    let mut engine = Engine::new();
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
            [Object.keys(copy).join(','), copy.b,
             Object.getOwnPropertyDescriptor(copy, 'b').enumerable].join('|');
            "#,
        )
        .unwrap();
    assert_eq!(result, "a,b|9|true");
}

#[test]
fn throwing_getter_preserves_the_page_value_and_stops_later_reads() {
    let mut engine = Engine::new();
    let result = engine
        .evaluate(
            r#"
            const marker = {id: 71};
            let later = 0;
            const source = {
              get first() { throw marker; },
              get second() { later++; return 2; }
            };
            let identical = false;
            try { structuredClone(source); }
            catch (error) { identical = error === marker; }
            `${identical}|${later}`;
            "#,
        )
        .unwrap();
    assert_eq!(result, "true|0");
}

#[test]
fn clone_creates_own_data_and_keeps_lone_surrogates_and_sparse_length() {
    let mut engine = Engine::new();
    let result = engine
        .evaluate(
            r#"
            const source = {items: []};
            source.items.length = 8193;
            source.items[8192] = 'tail';
            Object.defineProperty(source, '__proto__', {value: {marker: 5}, enumerable:true});
            source.text = String.fromCharCode(0xd800);
            const copy = structuredClone(source);
            [Object.getPrototypeOf(copy) === Object.prototype,
             Object.hasOwn(copy, '__proto__'), copy.__proto__.marker,
             copy.text.charCodeAt(0), copy.items.length,
             Object.hasOwn(copy.items, 0), copy.items[8192]].join('|');
            "#,
        )
        .unwrap();
    assert_eq!(result, "true|true|5|55296|8193|false|tail");
}

#[test]
fn transfer_detaches_in_order_and_keeps_earlier_detach_on_later_failure() {
    let mut engine = Engine::new();
    let result = engine
        .evaluate(
            r#"
            const first = new ArrayBuffer(4);
            new Uint8Array(first)[0] = 77;
            const copy = structuredClone({first}, {transfer: [first]});
            const copied = new Uint8Array(copy.first)[0];
            const third = new ArrayBuffer(2), fourth = new ArrayBuffer(2);
            structuredClone(fourth, {transfer: [fourth]});
            let name = 'none';
            try { structuredClone({}, {transfer: [third, fourth]}); }
            catch (error) { name = error.name; }
            [first.byteLength, copied, third.byteLength, fourth.byteLength, name].join('|');
            "#,
        )
        .unwrap();
    assert_eq!(result, "0|77|0|0|DataCloneError");
}

#[test]
fn listed_buffer_can_be_detached_by_an_earlier_getter() {
    let mut engine = Engine::new();
    let result = engine
        .evaluate(
            r#"
            const buffer = new ArrayBuffer(4);
            const source = {get data() {
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
fn primitive_transfer_item_does_not_call_iterator_return() {
    let mut engine = Engine::new();
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
    let mut engine = Engine::with_document(Rc::new(RefCell::new(tenta_dom::Document::new())));
    let result = engine
        .evaluate(
            r#"
            let traps = 0;
            const proxy = new Proxy({}, {get(){traps++;throw 1},
                                         ownKeys(){traps++;throw 2}});
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
fn data_clone_error_uses_the_realm_intrinsic_after_global_replacement() {
    let mut engine = Engine::new();
    let result = engine
        .evaluate(
            r#"
            const Original = DOMException;
            let calls = 0;
            globalThis.DOMException = function() { calls++; throw Error('replaced'); };
            Object.create = function() { calls++; throw Error('create'); };
            Object.defineProperty = function() { calls++; throw Error('define'); };
            Object.prototype.__defineSetter__('__proto__', function() { calls++; });
            let brand = 'none';
            try { structuredClone(function unsupported() {}); }
            catch (error) {
              const name = Object.getOwnPropertyDescriptor(error, 'name');
              const message = Object.getOwnPropertyDescriptor(error, 'message');
              brand = `${error instanceof Original}|${error.name}|${typeof error.stack}|${name.writable}|${message.writable}`;
            }
            `${brand}|${calls}`;
            "#,
        )
        .unwrap();
    assert_eq!(result, "true|DataCloneError|string|false|false|0");
}

#[test]
fn ordinary_null_prototype_clones_but_special_brands_do_not() {
    let mut engine = Engine::new();
    let result = engine
        .evaluate(
            r#"
            const plain = Object.create(null);
            plain.answer = 42;
            const copy = structuredClone(plain);
            const special = [new Date(), new Map(),
                             new ArrayBuffer(4, {maxByteLength: 8})];
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
fn budget_failure_is_not_catchable_and_releases_native_workspace() {
    for (input, nested) in [
        ("'x'.repeat(2000)", false),
        (
            "{ get value() { cloneProgress.getterStarted = true; structuredClone('x'.repeat(2000)); cloneProgress.innerAfter = true; return 1; } }",
            true,
        ),
    ] {
        let document = Rc::new(RefCell::new(tenta_dom::Document::new()));
        let budget = NativeMemoryBudget::with_limits(32 * 1024, 4096, 4096, 4096);
        let mut engine = Engine::with_document_on_clock_storage_and_memory(
            document.clone(),
            Clock::virtual_at(MonoTime::ZERO),
            None,
            tenta_net::CookieJar::new(),
            PageStorage::isolated(),
            budget.clone(),
        );
        let source = r#"
            const root = document.createElement('div');
            document.appendChild(root);
            globalThis.cloneProgress = { after: false, caught: false, finalized: false, innerAfter: false, getterStarted: false };
            try {
                structuredClone(CLONE_INPUT);
                cloneProgress.after = true;
                root.setAttribute('after', 'yes');
            } catch (error) {
                cloneProgress.caught = true;
                root.setAttribute('caught', 'yes');
            } finally {
                cloneProgress.finalized = true;
            }
            "#.replace("CLONE_INPUT", input);
        let result = engine.evaluate(&source);
        assert!(result.is_err());
        assert_eq!(
            budget.first_failure().unwrap().component,
            NativeMemoryComponent::StructuredClone
        );
        assert_eq!(budget.used(NativeMemoryComponent::StructuredClone), 0);
        // Inspect the saved context without entering another script task: the
        // sticky budget correctly refuses subsequent public evaluate calls.
        {
            let _entry = engine.entered();
            v8::scope!(let handle_scope, &mut engine.isolate);
            let context = v8::Local::new(handle_scope, &engine.context);
            let mut context_scope = v8::ContextScope::new(handle_scope, context);
            v8::scope!(let scope, &mut context_scope);
            let key = v8::String::new(scope, "cloneProgress").unwrap();
            let progress = context.global(scope).get(scope, key.into()).unwrap();
            let progress = v8::Local::<v8::Object>::try_from(progress).unwrap();
            let key = v8::String::new(scope, "getterStarted").unwrap();
            assert_eq!(progress.get(scope, key.into()).unwrap().is_true(), nested);

            for name in ["after", "caught", "finalized", "innerAfter"] {
                let key = v8::String::new(scope, name).unwrap();
                assert!(
                    progress.get(scope, key.into()).unwrap().is_false(),
                    "{name}"
                );
            }
        }
        let document = document.borrow();
        let root = document
            .document_element()
            .expect("the root was appended first");
        assert_eq!(document.attribute_str(root, "after"), None);
        assert_eq!(document.attribute_str(root, "caught"), None);
        drop(document);
        assert!(engine.evaluate("1").is_err());
    }
}
