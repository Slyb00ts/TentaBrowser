//! Tests for `Response` and the promise seam (P2 package (c) slice 4a-0b).
//!
//! `Response` is the first JS-owned host object whose members return *promises*,
//! which is what makes this file different from `dom_headers.rs`: it has to
//! observe a settlement as well as a value, and the settlement is a microtask
//! rather than a synchronous call. `Engine::evaluate` does not run a microtask
//! checkpoint — the drive paths do — so every test below that awaits a promise
//! calls `Engine::run_microtasks` and only then reads the answer.
//!
//! The host's own algorithms — the status range, the reason-phrase check, the
//! `Content-Type` append, the `"response"` guard, and `consume body`'s unusable
//! step — are tested without a wrapper world in
//! `crates/tenta-js/src/bindings/mod.rs`. What is left, and what needs the glue,
//! is the argument conversions too: whether a `Uint8Array` body really arrives
//! as bytes.
//!
//! They live here rather than under `crates/tenta-page/tests/` because they need
//! the V8 glue, and no V8 test may live there.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with `<html><body></body></html>`; these tests need a browsing
/// context but no markup.
fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::new();
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    d.mutate(Mutation::Append {
        parent: d.document_node(),
        child: html,
    })
    .unwrap();
    d.mutate(Mutation::Append {
        parent: html,
        child: body,
    })
    .unwrap();
    Rc::new(RefCell::new(d))
}

/// Runs `source` and returns its completion value. No microtask checkpoint is
/// run afterwards, so anything a promise would do has not happened yet.
fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(document());
    engine
        .evaluate(source)
        .expect("script should evaluate without throwing")
}

/// Runs `source`, drains the microtask queue it filled, and returns `after` read
/// from the same engine.
///
/// This is the shape every promise test needs: `evaluate` returns before a
/// settlement runs, and V8's queue is only drained by an explicit checkpoint.
fn settled(source: &str, after: &str) -> String {
    let mut engine = Engine::with_document(document());
    engine
        .evaluate(source)
        .expect("script should evaluate without throwing");
    engine.run_microtasks();
    engine
        .evaluate(after)
        .unwrap_or_else(|error| panic!("reading `{after}` failed: {error}"))
}

/// Runs `source`, whose `run` returns a value, and collects the constructor
/// name of whatever it throws.
fn caught(source: &str) -> String {
    eval(&format!(
        r#"
        const attempt = run => {{
            try {{ run(); return 'no throw'; }}
            catch (e) {{ return e.constructor.name + ':' + e.name; }}
        }};
        const results = [];
        {source}
        results.join('|');
        "#
    ))
}

// ---------------------------------------------------------------------------
// Installation and branding
// ---------------------------------------------------------------------------

#[test]
fn both_response_interfaces_are_installed_as_globals() {
    // The record names the interface rather than the shared host tag, so
    // `[object Response]` and `[object Headers]` differ even though both
    // wrappers carry the same tag. `Response` roots at `Object`, as the other
    // JS-owned hosts do.
    assert_eq!(
        eval(
            r#"
            const r = new Response();
            const h = r.headers;
            [
                globalThis.Response === Response,
                globalThis.Headers === Headers,
                typeof Response, typeof Headers,
                Response.name, Headers.name,
                // Both arguments are optional, so the constructor's `length` is
                // zero — which is what the IDL's `optional` on each says.
                Response.length,
                Response === Headers,
                r instanceof Response, r instanceof Headers,
                h instanceof Headers, h instanceof Response,
                r.constructor === Response, h.constructor === Headers,
                Object.getPrototypeOf(Response.prototype) === Object.prototype,
                Object.prototype.toString.call(r),
                Object.prototype.toString.call(h),
                typeof r.text, typeof r.json,
                // Both `Body` members are `[NewObject]` and both are on the
                // prototype, not on the instance.
                Object.prototype.hasOwnProperty.call(Response.prototype, 'text'),
                Object.prototype.hasOwnProperty.call(Response.prototype, 'json'),
                'text' in r,
            ].join('|');
        "#
        ),
        concat!(
            "true|true|function|function|Response|Headers|0|false",
            "|true|false|true|false|true|true|true",
            "|[object Response]|[object Headers]",
            "|function|function|true|true|true",
        )
    );
}

#[test]
fn a_member_called_on_the_wrong_receiver_throws_a_type_error() {
    // The generated signature is the brand check, so no member can be driven
    // off a plain object or off a `Headers` instance.
    assert_eq!(
        caught(
            r#"
            const r = new Response();
            const h = new Headers();
            results.push(attempt(() => Response.prototype.text.call({})));
            results.push(attempt(() => Response.prototype.text.call(h)));
            results.push(attempt(() => Response.prototype.json.call(h)));
            results.push(attempt(() => Object.getOwnPropertyDescriptor(
                Response.prototype, 'url').get.call({})));
            results.push(attempt(() => Response.call({})));
            "#,
        ),
        concat!(
            "TypeError:TypeError|TypeError:TypeError|TypeError:TypeError",
            "|TypeError:TypeError|TypeError:TypeError",
        )
    );
}

// ---------------------------------------------------------------------------
// The promise seam
// ---------------------------------------------------------------------------

#[test]
fn text_resolves_a_real_promise_with_the_body() {
    assert_eq!(
        settled(
            r#"
            const p = new Response('hi').text();
            globalThis.kind = typeof p.then + ':' + (p instanceof Promise);
            p.then(value => { globalThis.result = 'text:' + value; });
            "#,
            "[globalThis.kind, globalThis.result].join('|')",
        ),
        "function:true|text:hi"
    );
}

#[test]
fn a_settlement_is_queued_rather_than_applied_in_place() {
    // The whole reason the settlement is a microtask. `r.text()` must return a
    // promise that resolves in a *later* job, so a `queueMicrotask` queued
    // synchronously after the call runs first — settling in place would queue
    // `then`'s reaction at the moment of the call, ahead of it.
    //
    // The second half is the property the ordering exists for: a settlement
    // requested from inside a microtask settles in the *same* checkpoint pass,
    // so `inner` is set by the time this one call to `run_microtasks` returns.
    assert_eq!(
        settled(
            r#"
            const outer = new Response('one');
            const inner = new Response('two');
            globalThis.order = [];
            outer.text().then(value => order.push('outer:' + value));
            queueMicrotask(() => order.push('other'));
            queueMicrotask(() => {
                inner.text().then(value => order.push('inner:' + value));
            });
            "#,
            "globalThis.order.join(',')",
        ),
        "other,outer:one,inner:two"
    );
}

#[test]
fn text_decodes_utf8_with_replacement_and_strips_a_leading_bom() {
    // Backed by the shared `utf8_decode`, so this is also the check that
    // `json()` and `text()` read the same decoder: the malformed byte becomes
    // U+FFFD rather than failing the read, which is what makes the decode a
    // *decode* and not a parse.
    assert_eq!(
        settled(
            r#"
            const bytes = new Uint8Array([0x41, 0xFF, 0x42]);
            new Response(bytes).text().then(value => {
                globalThis.result = JSON.stringify(value) + ':' + value.length;
            });
            "#,
            "globalThis.result",
        ),
        "\"A\u{FFFD}B\":3"
    );

    assert_eq!(
        settled(
            r#"
            // A BOM is decoration, not content: "UTF-8 decode" strips one.
            new Response(new Uint8Array([0xEF, 0xBB, 0xBF, 0x41])).text()
                .then(value => { globalThis.result = JSON.stringify(value); });
            "#,
            "globalThis.result",
        ),
        "\"A\""
    );
}

#[test]
fn json_resolves_with_a_real_object() {
    assert_eq!(
        settled(
            r#"
            new Response('{"a":1,"b":[true,null]}').json().then(value => {
                globalThis.result = [value.a, value.b[0], String(value.b[1]),
                                 Array.isArray(value.b)].join('|');
            });
            "#,
            "globalThis.result",
        ),
        "1|true|null|true"
    );
}

#[test]
fn json_rejects_with_a_syntax_error_and_does_not_throw() {
    // The parse happens *inside* the promise, so a malformed body is a
    // rejection: the call itself returns a promise, and nothing is thrown out
    // of `json()`.
    assert_eq!(
        settled(
            r#"
            const r = new Response('{');
            let returned = 'no';
            let p;
            try { p = r.json(); returned = typeof p.then; }
            catch (e) { returned = 'threw:' + e.name; }
            p.then(
                () => { globalThis.result = 'resolved'; },
                error => {
                    globalThis.result = [returned, error.constructor.name,
                                         error.name, error instanceof SyntaxError].join('|');
                }
            );
            "#,
            "globalThis.result",
        ),
        "function|SyntaxError|SyntaxError|true"
    );

    // The same rejection for a body that is not UTF-8 at all, which is the
    // other half of "parse JSON from bytes": the decode replaces the bad byte,
    // and the replacement fails the parse.
    assert_eq!(
        settled(
            r#"
            new Response(new Uint8Array([0xFF])).json().then(
                () => { globalThis.result = 'resolved'; },
                error => { globalThis.result = error.name; }
            );
            "#,
            "globalThis.result",
        ),
        "SyntaxError"
    );
}

// ---------------------------------------------------------------------------
// `consume body`
// ---------------------------------------------------------------------------

#[test]
fn a_body_read_disturbs_the_body_and_a_second_read_rejects() {
    assert_eq!(
        settled(
            r#"
            const r = new Response('hi');
            const trace = [];
            trace.push('before:' + r.bodyUsed);
            const first = r.text();
            trace.push('after:' + r.bodyUsed);
            first.then(value => trace.push('first:' + value));
            // A second read is the same promise contract: it returns one, and
            // that one rejects with a `TypeError` rather than throwing.
            const second = r.text();
            const third = r.json();
            second.then(
                () => trace.push('second:resolved'),
                error => trace.push('second:' + error.constructor.name + ':' + error.message)
            );
            third.then(
                () => trace.push('third:resolved'),
                error => trace.push('third:' + error.constructor.name)
            );
            trace.push('tail:' + r.bodyUsed);
            globalThis.trace = trace;
            "#,
            "globalThis.trace.join(',')",
        ),
        concat!(
            "before:false,after:true,tail:true",
            ",first:hi,second:TypeError:The body has already been read,third:TypeError",
        )
    );
}

#[test]
fn a_null_body_reads_as_the_empty_string() {
    assert_eq!(
        settled(
            r#"
            const r = new Response();
            globalThis.trace = [];
            trace.push('used:' + r.bodyUsed);
            r.text().then(value => trace.push('text:' + JSON.stringify(value)));
            // Reading a null body never disturbs it, so a second read is the
            // empty string again rather than a rejection.
            r.text().then(value => trace.push('again:' + JSON.stringify(value)));
            // The empty sequence is not JSON, so this one rejects — the same
            // rejection a malformed body gets, for the same reason.
            r.json().then(
                () => trace.push('json:resolved'),
                error => trace.push('json:' + error.constructor.name)
            );
            "#,
            "globalThis.trace.join(',')",
        ),
        "used:false,text:\"\",again:\"\",json:SyntaxError"
    );
}

#[test]
fn a_uint8_array_body_is_taken_as_bytes_and_not_stringified() {
    // The union arm the shared buffer-source conversion exists for: a
    // `BodyInit` is a `BufferSourceOrUsvString`, and its conversion tries the
    // buffer-source arm on the value's *class*. A conversion that fell through
    // to the `USVString` arm would read `"104,105"`.
    assert_eq!(
        settled(
            r#"
            globalThis.trace = [];
            new Response(new Uint8Array([104, 105])).text()
                .then(value => trace.push('view:' + value));
            new Response(new Uint8Array([104, 105]).buffer).text()
                .then(value => trace.push('buffer:' + value));
            new Response(new Uint8Array([0x68, 0x69, 0x21]).subarray(1)).text()
                .then(value => trace.push('slice:' + value));
            new Response(new DataView(new Uint8Array([104, 105]).buffer)).text()
                .then(value => trace.push('dataview:' + value));
            "#,
            "globalThis.trace.join(',')",
        ),
        "view:hi,buffer:hi,slice:i!,dataview:hi"
    );

    // And the body's *type* is what tells the two arms apart from the outside:
    // a buffer source's body has none, so the list gains no `Content-Type`.
    assert_eq!(
        settled(
            r#"
            new Response(new Uint8Array([104, 105])).text().then(() => {
                globalThis.result = [
                    String(new Response(new Uint8Array([104, 105]))
                        .headers.get('content-type')),
                    String(new Response(new Uint8Array([104, 105]).buffer)
                        .headers.get('content-type')),
                    new Response('hi').headers.get('content-type'),
                ].join('|');
            });
            "#,
            "globalThis.result",
        ),
        "null|null|text/plain;charset=UTF-8"
    );
}

#[test]
fn headers_is_the_same_object_and_the_guard_applies_to_the_constructor() {
    assert_eq!(
        eval(
            r#"
            const r = new Response('hi');
            const h = new Headers({ 'Set-Cookie': 'a=b' });
            const guarded = new Response('hi', { headers: {
                'Set-Cookie': 'a=b', 'Set-Cookie2': 'c=d', 'X-Kept': '1',
            } });
            [
                r.headers === r.headers,
                r.headers instanceof Headers,
                Object.getOwnPropertyDescriptor(Response.prototype, 'headers')
                    .get.call(r) === r.headers,
                r.headers.get('content-type'),
                // Under the `"response"` guard the two forbidden names are
                // dropped *silently*, which is the standard's guarded append.
                String(guarded.headers.get('set-cookie')),
                String(guarded.headers.get('set-cookie2')),
                guarded.headers.get('x-kept'),
                [...guarded.headers].map(pair => pair.join('=')).join(','),
                // And the same names under the `Headers` constructor's guard are
                // kept, so the drop above is the guard and not the name.
                h.get('set-cookie'),
                // An append to the guarded object is dropped the same way.
                (guarded.headers.append('Set-Cookie', 'z=z'),
                 String(guarded.headers.get('set-cookie'))),
            ].join('|');
        "#
        ),
        concat!(
            "true|true|true|text/plain;charset=UTF-8|null|null|1",
            "|content-type=text/plain;charset=UTF-8,x-kept=1|a=b|null",
        )
    );
}

// ---------------------------------------------------------------------------
// The constructor
// ---------------------------------------------------------------------------

#[test]
fn initialize_a_response_rejects_a_bad_status_or_body() {
    assert_eq!(
        caught(
            r#"
            results.push(attempt(() => new Response('hi', { status: 199 })));
            results.push(attempt(() => new Response('hi', { status: 600 })));
            results.push(attempt(() => new Response('hi', { status: 200 })));
            results.push(attempt(() => new Response('hi', { status: 599 })));
            results.push(attempt(() => new Response('hi', { statusText: 'a\nb' })));
            results.push(attempt(() => new Response('hi', { statusText: 'OK' })));
            results.push(attempt(() => new Response('hi', { statusText: '' })));
            // A null body status, with a body and without one.
            results.push(attempt(() => new Response('hi', { status: 204 })));
            results.push(attempt(() => new Response(null, { status: 204 })));
            results.push(attempt(() => new Response(undefined, { status: 205 })));
            results.push(attempt(() => new Response('', { status: 304 })));
            results.push(attempt(() => new Response(new Uint8Array(0), { status: 304 })));
            // Below the range, so the range check wins over the null body rule.
            results.push(attempt(() => new Response('hi', { status: 103 })));
            // A throwing dictionary getter propagates its original exception
            // even when later members would otherwise be read.
            results.push(attempt(() => new Response('hi', { get status() {
                throw new RangeError('boom');
            } })));
            // A throwing getter for another member follows the same rule.
            results.push(attempt(() => new Response('hi', { get headers() {
                throw new RangeError('boom');
            } })));
            "#,
        ),
        concat!(
            "RangeError:RangeError|RangeError:RangeError|no throw|no throw",
            "|TypeError:TypeError|no throw|no throw",
            "|TypeError:TypeError|no throw|no throw",
            "|TypeError:TypeError|TypeError:TypeError",
            "|RangeError:RangeError|RangeError:RangeError|RangeError:RangeError",
        )
    );
}

#[test]
fn a_body_that_is_not_a_buffer_source_goes_through_the_string_arm() {
    // WPT-visible, and worth pinning because it is the opposite of what "the
    // body must be a buffer source" suggests: WebIDL's union conversion selects
    // the buffer-source arm by the value's *class*, so a value that is not an
    // `ArrayBuffer`, a `SharedArrayBuffer` or a view falls to `USVString`, i.e.
    // a `ToString`. A number and a plain object are therefore strings, not
    // `TypeError`s.
    assert_eq!(
        settled(
            r#"
            globalThis.trace = [];
            new Response(42).text().then(value => trace.push(String(value)));
            new Response(true).text().then(value => trace.push(String(value)));
            new Response({ toString() { return 'obj'; } }).text()
                .then(value => trace.push(value));
            "#,
            "globalThis.trace.join(',')",
        ),
        "42,true,obj"
    );

    // What does fail is a `ToString` that throws, and a `Symbol` — whose
    // `ToString` is a `TypeError` by definition — is the one such value that
    // needs no help.
    assert_eq!(
        caught(
            r#"
            results.push(attempt(() => new Response(Symbol('x'))));
            results.push(attempt(() => new Response({ toString() {
                throw new RangeError('boom');
            } })));
            "#,
        ),
        "TypeError:TypeError|RangeError:RangeError"
    );
}

#[test]
fn the_body_init_union_reads_a_string_as_a_ustring() {
    // The other arm: a string body is UTF-8 encoded, so a lone surrogate is
    // replaced before it becomes bytes, and the read-back is the same string.
    assert_eq!(
        settled(
            r#"
            new Response('a\uD800b').text().then(value => {
                globalThis.result = JSON.stringify(value) + ':' + value.length;
            });
            "#,
            "globalThis.result",
        ),
        "\"a\u{FFFD}b\":3"
    );
}

#[test]
fn the_readonly_getters_reject_an_assignment() {
    // No attribute has a setter, so an assignment is a `TypeError` in strict
    // mode. The `'use strict'` is inside each arrow because a directive
    // prologue only means anything at the start of a function body, and this
    // one is not at the start of the script.
    assert_eq!(
        caught(
            r#"
            const r = new Response('hi', { status: 201, statusText: 'Created' });
            results.push(attempt(() => { 'use strict'; r.status = 500; }));
            results.push(attempt(() => { 'use strict'; r.url = 'https://example.com/'; }));
            results.push(attempt(() => { 'use strict'; r.headers = new Headers(); }));
            results.push(attempt(() => { 'use strict'; r.bodyUsed = true; }));
            "#,
        ),
        concat!(
            "TypeError:TypeError|TypeError:TypeError|TypeError:TypeError",
            "|TypeError:TypeError",
        )
    );

    // And in sloppy mode the same assignments are silent no-ops: the values are
    // the constructor's before and after, and the `[SameObject]` headers object
    // is untouched.
    assert_eq!(
        eval(
            r#"
            const r = new Response('hi', { status: 201, statusText: 'Created' });
            const before = r.headers;
            const was = [r.status, r.statusText, r.url, r.ok, r.type, r.redirected];
            r.status = 500; r.url = 'https://example.com/';
            r.headers = new Headers(); r.bodyUsed = true;
            [
                was.join(','),
                [r.status, r.statusText, r.url, r.ok, r.type, r.redirected].join(','),
                r.headers === before, r.headers === r.headers, r.bodyUsed,
            ].join('|');
        "#
        ),
        concat!(
            "201,Created,,true,default,false",
            "|201,Created,,true,default,false|true|true|false",
        )
    );
}

#[test]
fn a_status_text_survives_the_bytestring_conversion() {
    // `statusText` is a `ByteString`, so a byte above 0x7F is carried through as
    // a byte rather than refused — the getter hands it back as the corresponding
    // code unit, which is what `ByteString` means in both directions.
    assert_eq!(
        eval(
            r#"
            const r = new Response('hi', { statusText: 'ÿ\u0080' });
            [r.statusText, r.statusText.length, r.statusText.charCodeAt(0)].join('|');
        "#
        ),
        "\u{FF}\u{80}|2|255"
    );
}

// ---------------------------------------------------------------------------
// Reclamation
// ---------------------------------------------------------------------------

#[test]
fn a_collected_response_wrapper_frees_its_record_and_its_headers() {
    // A response owns a headers record, so a leak in either direction shows up
    // as a slab that does not return to its baseline: the response's finalizer
    // has to release the headers too, and the headers record on its own is not
    // an owner.
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate(
            r#"
            for (let i = 0; i < 50; i += 1) {
                const r = new Response(String(i), { headers: { 'X-I': String(i) } });
                r.headers.get('x-i');
                r.text();
                new Headers({ 'X-J': String(i) });
            }
        "#,
        )
        .expect("evaluate the allocation loop");
    // Every promise the loop made has to settle, or its two value slots are
    // still held when the world is torn down.
    engine.run_microtasks();
    assert_eq!(
        engine.stored_value_count(),
        0,
        "no promise outlives the loop"
    );

    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }

    assert_eq!(
        engine.host_record_count(),
        baseline,
        "a collected response wrapper should free its record and its headers"
    );
}

#[test]
fn a_live_response_keeps_its_record_across_a_sweep() {
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate(
            r#"
            globalThis.keep = new Response('hi', { status: 201 });
            globalThis.kept = null;
            keep.text().then(value => { globalThis.kept = value; });
        "#,
        )
        .expect("evaluate the response");
    engine.run_microtasks();
    assert_eq!(
        engine
            .evaluate("globalThis.kept")
            .expect("the read settled"),
        "hi"
    );

    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(
        engine.host_record_count(),
        baseline + 2,
        "the live response and its headers both survive"
    );
    assert_eq!(
        engine
            .evaluate(
                r#"
                [keep.status, keep.ok, keep.bodyUsed, keep.headers === keep.headers,
                 keep.headers.get('content-type')].join('|');
            "#
            )
            .expect("the record is still readable"),
        "201|true|true|true|text/plain;charset=UTF-8",
        "and its own body is still disturbed, because the read really happened"
    );

    engine
        .evaluate("globalThis.keep = null;")
        .expect("evaluate the release");
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), baseline);
}

// ---------------------------------------------------------------------------
// The body reads that are not text: arrayBuffer(), bytes(), blob(), and clone()
// ---------------------------------------------------------------------------

#[test]
fn array_buffer_and_bytes_resolve_with_the_bodys_bytes() {
    assert_eq!(
        settled(
            r#"
            globalThis.out = [];
            new Response(new Uint8Array([1, 2, 255])).arrayBuffer().then(buffer => {
                out.push(Object.prototype.toString.call(buffer), buffer.byteLength,
                         Array.from(new Uint8Array(buffer)).join(','));
            });
            new Response('hi').bytes().then(view => {
                out.push(view.constructor.name, view.length, Array.from(view).join(','));
            });
            new Response(null).arrayBuffer().then(buffer => out.push(buffer.byteLength));
            "#,
            "out.join('|')"
        ),
        "[object ArrayBuffer]|3|1,2,255|Uint8Array|2|104,105|0"
    );
}

#[test]
fn every_read_resolves_with_a_buffer_of_its_own() {
    // The bytes are copied into the value, so writing to one read's buffer is
    // not visible through a clone's.
    assert_eq!(
        settled(
            r#"
            globalThis.out = [];
            const first = new Response('ab');
            const second = first.clone();
            first.arrayBuffer().then(buffer => {
                new Uint8Array(buffer)[0] = 0;
                return second.text();
            }).then(text => out.push(text));
            "#,
            "out.join('|')"
        ),
        "ab"
    );
}

#[test]
fn blob_resolves_with_a_blob_typed_by_the_content_type() {
    assert_eq!(
        settled(
            r#"
            globalThis.out = [];
            new Response('hello', {headers: {'Content-Type': 'Text/HTML; charset=utf-8'}})
                .blob()
                .then(blob => {
                    out.push(blob instanceof Blob, blob.size, blob.type);
                    return blob.text();
                })
                .then(text => out.push(text));
            new Response(new Uint8Array([1])).blob().then(blob => out.push('[' + blob.type + ']'));
            new Response('x').blob().then(blob => out.push(blob.type));
            "#,
            "out.join('|')"
        ),
        "true|5|text/html; charset=utf-8|[]|text/plain;charset=utf-8|hello"
    );
}

#[test]
fn the_new_reads_disturb_the_body_like_text_does() {
    assert_eq!(
        settled(
            r#"
            globalThis.out = [];
            for (const member of ['arrayBuffer', 'bytes', 'blob']) {
                const response = new Response('x');
                response[member]();
                out.push(response.bodyUsed);
                response[member]().then(
                    () => out.push(member + ' resolved'),
                    error => out.push(error.constructor.name),
                );
            }
            "#,
            "out.join('|')"
        ),
        "true|true|true|TypeError|TypeError|TypeError"
    );
}

#[test]
fn clone_copies_the_response_and_reads_separately() {
    assert_eq!(
        settled(
            r#"
            globalThis.out = [];
            const original = new Response('body', {
                status: 201, statusText: 'Made', headers: {'X-One': '1'},
            });
            const copy = original.clone();
            out.push(copy instanceof Response, copy !== original, copy.status, copy.statusText,
                     copy.headers.get('x-one'), copy.headers !== original.headers,
                     copy.type, copy.bodyUsed);
            copy.headers.set('x-one', '2');
            out.push(original.headers.get('x-one'));
            original.text().then(text => {
                out.push(text, original.bodyUsed, copy.bodyUsed);
                return copy.text();
            }).then(text => out.push(text));
            "#,
            "out.join('|')"
        ),
        "true|true|201|Made|1|true|default|false|1|body|true|false|body"
    );
}

#[test]
fn clone_of_a_read_response_throws() {
    assert_eq!(
        caught(
            r#"
            const read = new Response('x');
            read.text();
            results.push(attempt(() => read.clone()));
            // A null body is never disturbed, so a response without one can
            // always be cloned.
            const empty = new Response(null);
            empty.text();
            results.push(attempt(() => empty.clone()));
            "#
        ),
        "TypeError:TypeError|no throw"
    );
}

#[test]
fn the_blob_of_a_body_and_a_clone_are_freed_with_their_wrappers() {
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate(
            r#"
            globalThis.kept = [];
            {
                const response = new Response('hi');
                kept.push(response.clone());
                response.blob().then(blob => kept.push(blob));
            }
            "#,
        )
        .expect("evaluate");
    engine.run_microtasks();
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(
        engine.host_record_count(),
        baseline + 3,
        "the clone, its headers and the blob are held by the page"
    );
    engine
        .evaluate("globalThis.kept = null;")
        .expect("evaluate the release");
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), baseline);
}
