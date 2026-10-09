// Shared real-engine behavior for Window.crypto.getRandomValues.

#[test]
fn crypto_window_identity_and_constructor_brand() {
    assert_eq!(
        eval(
            "const first = crypto; first.expando = 17;
             const getter = Object.getOwnPropertyDescriptor(globalThis, 'crypto').get;
             let illegal = false; try { new Crypto(); } catch (e) { illegal = e instanceof TypeError; }
             [first === window.crypto, getter.call(window) === first,
              first.expando, typeof first.getRandomValues, illegal].join('|')"
        ),
        "true|true|17|function|true"
    );
}

#[test]
fn every_integer_view_returns_its_exact_receiver() {
    assert_eq!(
        eval(
            "const names = ['Int8Array','Uint8Array','Uint8ClampedArray',
              'Int16Array','Uint16Array','Int32Array','Uint32Array',
              'BigInt64Array','BigUint64Array'];
             names.map(name => {
               const ctor = globalThis[name];
               const backing = new ArrayBuffer(272);
               const bytes = new Uint8Array(backing); bytes.fill(91);
               const view = new ctor(backing, 8, 256 / ctor.BYTES_PER_ELEMENT);
               const returned = crypto.getRandomValues(view);
               return returned === view && view.byteLength === 256 &&
                 bytes[0] === 91 && bytes[7] === 91 &&
                 bytes[264] === 91 && bytes[271] === 91 &&
                 bytes.subarray(8, 264).some(byte => byte !== 91);
             }).every(Boolean)"
        ),
        "true"
    );
}

#[test]
fn quota_counts_view_bytes_and_preserves_offset_sentinels() {
    assert_eq!(
        eval(
            "const backing = new Uint8Array(65540); backing.fill(91);
             const view = new Uint8Array(backing.buffer, 2, 65536);
             const same = crypto.getRandomValues(view) === view;
             let quota = ''; try { crypto.getRandomValues(new Uint8Array(65537)); }
             catch (e) { quota = e.name + ':' + (e instanceof DOMException); }
             [same, backing[0], backing[1], backing[65538], backing[65539], quota].join('|')"
        ),
        "true|91|91|91|91|QuotaExceededError:true"
    );
}

#[test]
fn wrong_native_kinds_precede_byte_quota() {
    assert_eq!(
        eval(
            "const names = [new DataView(new ArrayBuffer(65537)),
               new Float32Array(16385), new Float64Array(8193)];
             const failures = names.map(value => {
               try { crypto.getRandomValues(value); return 'accepted'; }
               catch (e) { return e.name + ':' + (e instanceof DOMException); }
             });
             failures.join('|')"
        ),
        "TypeMismatchError:true|TypeMismatchError:true|TypeMismatchError:true"
    );
}

#[test]
fn conversion_rejects_forgery_without_page_traps() {
    assert_eq!(
        eval(
            "let traps = 0;
             const proxy = new Proxy(new Uint8Array(4), {
               get() { traps++; throw Error('get trap'); },
               getPrototypeOf() { traps++; throw Error('prototype trap'); }
             });
             let results = []; for (const value of [new ArrayBuffer(4), [], proxy]) {
               try { crypto.getRandomValues(value); results.push('accepted'); }
               catch (e) { results.push(e instanceof TypeError); }
             }
             [results.join(','), traps].join('|')"
        ),
        "true,true,true|0"
    );
}

#[test]
fn zero_length_still_returns_same_view() {
    assert_eq!(
        eval(
            "const view = new Uint32Array(0);
             [crypto.getRandomValues(view) === view, view.byteLength].join('|')"
        ),
        "true|0"
    );
}

#[test]
fn hard_web_crypto_memory_failure_is_typed_and_uncatchable() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use tenta_core::{NativeMemoryBudget, NativeMemoryComponent};

    let document = Rc::new(RefCell::new(tenta_dom::Document::new()));
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 4);
    let mut engine = new_engine_with_budget(document.clone(), budget.clone());
    let error = tenta_js::engine::PageEngine::evaluate(
        &mut engine,
        "const root = document.createElement('div'); document.appendChild(root);\n         try { crypto.getRandomValues(new Uint8Array(8)); root.setAttribute('after', 'yes'); }\n         catch (_) { root.setAttribute('caught', 'yes'); }\n         finally { root.setAttribute('finalized', 'yes'); }",
        std::time::Duration::from_secs(5),
    ).expect_err("the native Web Crypto sublimit must stop this task");
    assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
    assert_eq!(
        budget.first_failure().unwrap().component,
        NativeMemoryComponent::WebCrypto
    );
    assert_eq!(budget.used(NativeMemoryComponent::WebCrypto), 0);
    assert_eq!(budget.total_used(), 0);
    let doc = document.borrow();
    let root = doc
        .document_element()
        .expect("the element was inserted first");
    for name in ["after", "caught", "finalized"] {
        assert_eq!(doc.attribute_str(root, name), None, "{name} must not run");
    }
    drop(doc);
    assert!(
        engine.evaluate("1 + 1").is_err(),
        "the page remains stopped"
    );
}

#[test]
fn illegal_receiver_and_missing_argument_throw_type_error() {
    assert_eq!(
        eval(
            "const a = new Uint8Array(1); const method = Crypto.prototype.getRandomValues;\n             let values = [];\n             for (const call of [() => method.call({ }, a), () => crypto.getRandomValues()]) {\n               try { call(); values.push('accepted'); }\n               catch (e) { values.push(e instanceof TypeError); }\n             } values.join('|')"
        ),
        "true|true"
    );
}

#[test]
fn native_dom_exception_ignores_replaced_global_constructor() {
    assert_eq!(
        eval(
            "const original = DOMException; globalThis.DOMException = function() { throw Error('hijacked'); };\n             let answer = ''; try { crypto.getRandomValues(new Float32Array(2)); }\n             catch (e) { answer = [e instanceof original, e.name, e.code].join('|'); }\n             answer"
        ),
        "true|TypeMismatchError|17"
    );
}

#[test]
fn forged_view_fields_are_never_read() {
    assert_eq!(
        eval(
            "const view = new Uint8Array(8); let traps = 0;\n             Object.defineProperty(view, 'byteLength', {get() { traps++; throw Error('byteLength'); }});\n             Object.defineProperty(view, 'constructor', {get() { traps++; throw Error('constructor'); }});\n             const same = crypto.getRandomValues(view) === view;\n             [same, traps].join('|')"
        ),
        "true|0"
    );
}
