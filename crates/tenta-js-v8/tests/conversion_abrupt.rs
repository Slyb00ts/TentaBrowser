//! Abrupt WebIDL conversions must retain the page's thrown value and abort host work.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn eval(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::new()));
    Engine::with_document(document)
        .evaluate(source)
        .expect("the script handles its own conversion failure")
}

#[test]
fn dom_string_value_throw_preserves_identity_and_does_not_mutate_an_attribute() {
    assert_eq!(
        eval(
            r#"
            const element = document.createElement('div');
            element.setAttribute('data-value', 'before');
            const marker = { error: 'value conversion' };
            let same = false;
            try {
                element.setAttribute('data-value', {
                    [Symbol.toPrimitive]() { throw marker; }
                });
            } catch (error) { same = error === marker; }
            [same, element.getAttribute('data-value'), element.attributes.length].join('|');
            "#,
        ),
        "true|before|1"
    );
}

#[test]
fn dom_string_name_throw_stops_later_conversion_and_host_mutation() {
    assert_eq!(
        eval(
            r#"
            const element = document.createElement('div');
            const marker = { error: 'name conversion' };
            let laterConversions = 0;
            let same = false;
            try {
                element.setAttribute(
                    { toString() { throw marker; } },
                    { toString() { laterConversions++; return 'value'; } },
                );
            } catch (error) { same = error === marker; }
            [same, laterConversions, element.attributes.length].join('|');
            "#,
        ),
        "true|0|0"
    );
}

#[test]
fn symbol_string_conversion_throws_type_error_without_mutating() {
    assert_eq!(
        eval(
            r#"
            const element = document.createElement('div');
            let name = '';
            try { element.setAttribute('data-value', Symbol('value')); }
            catch (error) { name = error.name; }
            [name, element.hasAttribute('data-value')].join('|');
            "#,
        ),
        "TypeError|false"
    );
}

#[test]
fn numeric_to_primitive_throw_preserves_identity_and_range_boundary() {
    assert_eq!(
        eval(
            r#"
            const range = new Range();
            const marker = { error: 'offset conversion' };
            let same = false;
            try {
                range.setStart(document, {
                    [Symbol.toPrimitive]() { throw marker; }
                });
            } catch (error) { same = error === marker; }
            [same, range.startOffset, range.startContainer === document].join('|');
            "#,
        ),
        "true|0|true"
    );
}

#[test]
fn byte_string_throw_preserves_identity_and_headers_value() {
    assert_eq!(
        eval(
            r#"
            const headers = new Headers();
            headers.append('X-Probe', 'before');
            const marker = { error: 'header conversion' };
            let same = false;
            try {
                headers.append('X-Probe', { toString() { throw marker; } });
            } catch (error) { same = error === marker; }
            [same, headers.get('X-Probe')].join('|');
            "#,
        ),
        "true|before"
    );
}

#[test]
fn handwritten_event_listener_string_throw_does_not_register_a_listener() {
    assert_eq!(
        eval(
            r#"
            const marker = { error: 'listener type conversion' };
            let calls = 0;
            let same = false;
            try {
                window.addEventListener(
                    { [Symbol.toPrimitive]() { throw marker; } },
                    () => { calls++; },
                );
            } catch (error) { same = error === marker; }
            window.dispatchEvent(new Event(''));
            [same, calls].join('|');
            "#,
        ),
        "true|0"
    );
}

#[test]
fn handwritten_fetch_input_throw_rejects_a_promise_with_the_page_value() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    assert_eq!(
        engine
            .evaluate(
                r#"
                const marker = { error: 'request input conversion' };
                globalThis.sameReason = false;
                let synchronousThrow = false;
                let result;
                try {
                    result = fetch({ [Symbol.toPrimitive]() { throw marker; } });
                } catch (_) { synchronousThrow = true; }
                if (result) {
                    result.catch(error => { globalThis.sameReason = error === marker; });
                }
                [synchronousThrow, result instanceof Promise].join('|');
                "#,
            )
            .expect("fetch returns a Promise"),
        "false|true"
    );
    engine.run_microtasks();
    assert_eq!(
        engine.evaluate("String(globalThis.sameReason)").unwrap(),
        "true"
    );
}

#[test]
fn dictionary_throw_is_distinct_from_missing_member_and_default() {
    assert_eq!(
        eval(
            r#"
            const node = document.createElement('div');
            const marker = { error: 'dictionary getter' };
            let same = false;
            try {
                new StaticRange({
                    startContainer: node,
                    get startOffset() { throw marker; },
                    endContainer: node,
                    endOffset: 0,
                });
            } catch (error) { same = error === marker; }
            let missing = '';
            try { new StaticRange({}); }
            catch (error) { missing = error.name; }
            let undefinedMember = '';
            try {
                new StaticRange({
                    startContainer: node,
                    startOffset: undefined,
                    endContainer: node,
                    endOffset: 0,
                });
            } catch (error) { undefinedMember = error.name; }
            const absentDefault = new Blob([], {}).type;
            const undefinedDefault = new Blob([], { type: undefined }).type;
            [same, missing, undefinedMember, absentDefault, undefinedDefault].join('|');
            "#,
        ),
        "true|TypeError|TypeError||"
    );
}

#[test]
fn record_and_iterator_traps_preserve_their_thrown_values() {
    assert_eq!(
        eval(
            r#"
            const recordMarker = { error: 'own keys' };
            const descriptorMarker = { error: 'descriptor' };
            const iteratorMarker = { error: 'next getter' };
            let recordSame = false;
            let descriptorSame = false;
            let iteratorSame = false;
            try {
                new Headers(new Proxy({ 'X-Probe': 'value' }, {
                    ownKeys() { throw recordMarker; }
                }));
            } catch (error) { recordSame = error === recordMarker; }
            try {
                new Headers(new Proxy({ 'X-Probe': 'value' }, {
                    getOwnPropertyDescriptor() { throw descriptorMarker; }
                }));
            } catch (error) { descriptorSame = error === descriptorMarker; }
            try {
                new Headers({
                    [Symbol.iterator]() {
                        return { get next() { throw iteratorMarker; } };
                    }
                });
            } catch (error) { iteratorSame = error === iteratorMarker; }
            [recordSame, descriptorSame, iteratorSame].join('|');
            "#,
        ),
        "true|true|true"
    );
}

#[test]
fn ordinary_conversion_mismatches_still_report_type_error() {
    assert_eq!(
        eval(
            r#"
            const headers = new Headers([['X-Probe', 'before']]);
            let invalidByteString = '';
            try { headers.append('X-Probe', '\u0100'); }
            catch (error) { invalidByteString = error.name; }
            let nonCallableIterator = '';
            try { new Headers({ [Symbol.iterator]: 1 }); }
            catch (error) { nonCallableIterator = error.name; }
            [invalidByteString, nonCallableIterator, headers.get('X-Probe')].join('|');
            "#,
        ),
        "TypeError|TypeError|before"
    );
}
