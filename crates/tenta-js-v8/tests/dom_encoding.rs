//! Tests for the `TextDecoder` and `TextEncoder` bindings (P2 package (c)
//! slice 4b).
//!
//! The two encoding interfaces are JS-owned host objects on the shared host
//! tag: like `URL`, `URLSearchParams`, `Headers` and the abort pair they have
//! no document row and their wrapper is their only owner, so the record — not
//! the tag — is what names the interface. What they add over the earlier four
//! is that the host's *state* is the standard's: a `TextDecoder` owns a
//! decoder that survives a `{stream: true}` call, and the BOM rule of the
//! Encoding Standard's *serialize I/O queue*.
//!
//! These tests cover the JavaScript-visible half — installation and branding,
//! the argument conversions (including the recorded deviation below), the
//! typed array `encode` hands back, and the record's life across a GC. The
//! algorithms themselves are tested against the host without a wrapper world,
//! in `crates/tenta-js/src/bindings/mod.rs`.
//!
//! They live here rather than under `crates/tenta-page/tests/` because they need
//! the V8 glue, and no V8 test may live there.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with `<html><body></body></html>`; the encoding tests need a
/// browsing context but no markup.
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

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(document());
    engine
        .evaluate(source)
        .expect("script should evaluate without throwing")
}

/// Runs `source`, whose `run` returns a value, in a fresh engine and collects
/// the constructor name of whatever it throws.
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
fn both_encoding_interfaces_are_installed_as_globals() {
    // The record names the interface rather than the shared host tag, which is
    // why the two `[object ...]` results differ even though both wrappers carry
    // the same tag. `TextDecoder` is the one `TextEncoder` is not: they are two
    // constructors, not one.
    assert_eq!(
        eval(
            r#"
            const d = new TextDecoder();
            const e = new TextEncoder();
            [
                globalThis.TextDecoder === TextDecoder,
                globalThis.TextEncoder === TextEncoder,
                typeof TextDecoder, typeof TextEncoder,
                TextDecoder.name, TextEncoder.name,
                TextDecoder === TextEncoder,
                d instanceof TextDecoder, d instanceof TextEncoder,
                e instanceof TextEncoder, e instanceof TextDecoder,
                d.constructor === TextDecoder, e.constructor === TextEncoder,
                Object.getPrototypeOf(TextDecoder.prototype) === Object.prototype,
                Object.getPrototypeOf(TextEncoder.prototype) === Object.prototype,
                Object.prototype.toString.call(d),
                Object.prototype.toString.call(e),
                typeof d.decode, typeof e.encode,
                d.encoding, e.encoding,
            ].join('|');
        "#
        ),
        concat!(
            "true|true|function|function|TextDecoder|TextEncoder|false",
            "|true|false|true|false|true|true|true|true",
            "|[object TextDecoder]|[object TextEncoder]",
            "|function|function|utf-8|utf-8",
        )
    );
}

#[test]
fn a_member_called_on_the_wrong_receiver_throws_a_type_error() {
    // The generated signature is the brand check, so neither interface's
    // members can be driven off a plain object or off the other's instance.
    assert_eq!(
        caught(
            r#"
            const d = new TextDecoder();
            const e = new TextEncoder();
            results.push(attempt(() => TextDecoder.prototype.decode.call({}, new Uint8Array())));
            results.push(attempt(() => TextDecoder.prototype.decode.call(e, new Uint8Array())));
            results.push(attempt(() => TextEncoder.prototype.encode.call({}, 'x')));
            results.push(attempt(() => Object.getOwnPropertyDescriptor(
                TextDecoder.prototype, 'encoding').get.call({})));
            results.push(attempt(() => TextDecoder.call({})));
            "#,
        ),
        concat!(
            "TypeError:TypeError|TypeError:TypeError|TypeError:TypeError",
            "|TypeError:TypeError|TypeError:TypeError",
        )
    );
}

// ---------------------------------------------------------------------------
// The constructor
// ---------------------------------------------------------------------------

#[test]
fn a_label_that_is_not_an_encoding_throws_a_range_error() {
    assert_eq!(
        caught(
            r#"
            const labels = ['no-such-encoding', '', 'utf-9', 'utf-8x', '  ', 'uTf-8  '];
            const names = [];
            for (const label of labels) {
                const thrown = attempt(() => new TextDecoder(label));
                // The last two are labels, not unknown ones: `  ` is stripped to
                // the empty label and `uTf-8  ` to `utf-8`.
                names.push(thrown === 'RangeError:RangeError' ? 'RangeError' : thrown);
            }
            results.push(names.join(','));
            results.push(attempt(() => new TextDecoder(undefined).encoding));
            results.push(attempt(() => new TextDecoder().encoding));
            results.push(attempt(() => new TextDecoder(String.fromCharCode(0xd800)).encoding));
            "#,
        ),
        concat!(
            "RangeError,RangeError,RangeError,RangeError,RangeError,no throw",
            "|no throw|no throw|RangeError:RangeError",
        )
    );
}

#[test]
fn the_replacement_encodings_are_refused_like_unknown_labels() {
    // `api-replacement-encodings.any.js`: the six labels that resolve to the
    // *replacement* encoding are the ones the host drops by calling
    // `for_label_no_replacement`, and a constructor that used `for_label` would
    // accept every one of them and then decode to a single U+FFFD.
    assert_eq!(
        caught(
            r#"
            const names = [];
            for (const label of ['replacement', 'csiso2022kr', 'hz-gb-2312',
                                 'iso-2022-cn', 'iso-2022-cn-ext', 'iso-2022-kr']) {
                names.push(attempt(() => new TextDecoder(label) && 'accepted'));
            }
            results.push(names.join(','));
            // The same six spellings with ASCII whitespace around them, which
            // the lookup strips before it fails, and a valid label that only
            // looks like them.
            results.push(attempt(() => new TextDecoder('  replacement  ') && 'accepted'));
            results.push(attempt(() => new TextDecoder('replacement-x') && 'accepted'));
            results.push(new TextDecoder('utf-8').encoding);
            "#,
        ),
        concat!(
            "RangeError:RangeError,RangeError:RangeError,RangeError:RangeError",
            ",RangeError:RangeError,RangeError:RangeError,RangeError:RangeError",
            "|RangeError:RangeError|RangeError:RangeError|utf-8",
        )
    );
}

#[test]
fn the_options_are_read_off_the_dictionary_and_reported_back() {
    assert_eq!(
        eval(
            r#"
            const a = new TextDecoder('utf-8');
            const b = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
            const c = new TextDecoder('utf-8', {});
            const d = new TextDecoder('utf-8', { fatal: 1, ignoreBOM: 'yes' });
            const e = new TextDecoder('utf-16le', { fatal: false, ignoreBOM: false });
            [
                [a.fatal, a.ignoreBOM], [b.fatal, b.ignoreBOM], [c.fatal, c.ignoreBOM],
                [d.fatal, d.ignoreBOM], [e.fatal, e.ignoreBOM], [e.encoding, e instanceof TextDecoder],
                // Both are own accessors on the prototype, so neither is on the
                // instance and both are enumerable-less configurable getters.
                Object.getOwnPropertyNames(TextDecoder.prototype).sort().join(','),
                Object.getOwnPropertyDescriptor(TextDecoder.prototype, 'encoding').set === undefined,
                'fatal' in a,
            ].map(x => Array.isArray(x) ? x.join('/') : x).join('|');
        "#
        ),
        concat!(
            "false/false|true/true|false/false|true/true|false/false|utf-16le/true",
            "|constructor,decode,encoding,fatal,ignoreBOM|true|true",
        )
    );
}

#[test]
fn a_constructor_called_without_new_throws_a_type_error() {
    assert_eq!(
        caught(
            r#"
            results.push(attempt(() => TextDecoder()));
            results.push(attempt(() => TextEncoder()));
            "#,
        ),
        "TypeError:TypeError|TypeError:TypeError"
    );
}

// ---------------------------------------------------------------------------
// `decode`
// ---------------------------------------------------------------------------

#[test]
fn decode_reads_a_buffer_source_in_the_decoders_encoding() {
    assert_eq!(
        eval(
            r#"
            const bytes = new Uint8Array([0x41, 0xC3, 0xA9]);
            const d = new TextDecoder();
            const fromView = d.decode(bytes);
            const fromBuffer = d.decode(bytes.buffer);
            const fromViewAgain = d.decode(new DataView(bytes.buffer));
            // A view with a byte offset: only its own range is read.
            const offset = new Uint8Array([0x00, 0x41, 0x42]);
            const sub = d.decode(new Uint8Array(offset.buffer, 1));
            const empty = d.decode(new Uint8Array(0));
            const slice = d.decode(new Uint8Array([0x41, 0x42, 0x43, 0x44]).subarray(1, 3));
            [
                fromView, fromView === fromBuffer, fromView === fromViewAgain, fromBuffer === fromViewAgain,
                sub, empty === '', slice, typeof fromView,
                new TextDecoder('windows-1252').decode(new Uint8Array([0x80, 0x93])),
                new TextDecoder('utf-16le').decode(new Uint8Array([0x41, 0x00, 0x42, 0x00])),
                new TextDecoder('utf-16be').decode(new Uint8Array([0x00, 0x41, 0x00, 0x42])),
            ].join('|');
        "#
        ),
        concat!(
            "Aé|true|true|true|AB|true|BC|string",
            "|\u{20AC}\u{201C}|AB|AB",
        )
    );
}

#[test]
fn decode_rejects_a_value_that_is_not_a_buffer_source() {
    // `BufferSource` is neither nullable nor a sequence: a string, a number, a
    // plain object, `null`, an array and a `SharedArrayBuffer` all fail the
    // conversion. The array is the interesting one — a sequence of bytes is a
    // plausible implementation and is not WebIDL's `BufferSource`.
    //
    // A `SharedArrayBuffer`-backed *view* converts and a bare
    // `SharedArrayBuffer` does not. That split is this slice's IDL decision
    // rather than a bug in the conversion: the Encoding Standard spells the
    // argument `[AllowShared] BufferSource`, `AllowSharedBufferSource` cannot be
    // declared (weedle has no `SharedArrayBuffer` keyword), and the bare
    // `BufferSource` this file declares has no `[AllowShared]` to admit a bare
    // shared buffer. The view is accepted because the brand check is on the
    // view's class, which is what `encoding/sharedarraybuffer.https.html`
    // asserts. Both cases are pinned at the end of this test so that changing
    // either one fails here; the wrapper is not named `AllowSharedBufferSource`
    // until the generator can spell it.
    assert_eq!(
        caught(
            r#"
            const d = new TextDecoder();
            for (const value of ['abc', 42, {}, null, [0x41], Symbol('x'), true,
                                 new SharedArrayBuffer(4)]) {
                results.push(attempt(() => d.decode(value)));
            }
            results.push(attempt(() => d.decode(new ArrayBuffer(2))));
            results.push(attempt(() => d.decode(new Uint8Array([0x41]))));
            results.push(attempt(() => d.decode(new Uint8Array(new SharedArrayBuffer(4)))));
            "#,
        ),
        concat!(
            "TypeError:TypeError|TypeError:TypeError|TypeError:TypeError|TypeError:TypeError",
            "|TypeError:TypeError|TypeError:TypeError|TypeError:TypeError|TypeError:TypeError",
            "|no throw|no throw|no throw",
        )
    );
}

#[test]
fn decode_drops_one_leading_bom_unless_asked_not_to() {
    assert_eq!(
        eval(
            r#"
            const bytes = new Uint8Array([0xEF, 0xBB, 0xBF, 0x41]);
            const d = new TextDecoder();
            const kept = new TextDecoder('utf-8', { ignoreBOM: true });
            const json = JSON.stringify.bind(JSON);
            [
                d.decode(bytes), json(d.decode(bytes)),
                kept.decode(bytes), json(kept.decode(bytes)),
                json(d.decode(new Uint8Array([0x41, 0xEF, 0xBB, 0xBF]))),
                json(d.decode(new Uint8Array([0xEF, 0xBB, 0xBF, 0xEF, 0xBB, 0xBF, 0x41]))),
                new TextDecoder('utf-16le').decode(new Uint8Array([0xFF, 0xFE, 0x41, 0x00])),
                new TextDecoder('utf-16le', { ignoreBOM: true })
                    .decode(new Uint8Array([0xFF, 0xFE, 0x41, 0x00])).length,
                new TextDecoder('windows-1252').decode(new Uint8Array([0xEF, 0xBB, 0xBF])),
            ].join('|');
        "#
        ),
        concat!(
            "A|\"A\"",
            "|\u{FEFF}A|\"\u{FEFF}A\"",
            "|\"A\u{FEFF}\"",
            "|\"\u{FEFF}A\"",
            "|A|2|\u{EF}\u{BB}\u{BF}",
        )
    );
}

#[test]
fn a_streaming_call_keeps_the_decoder_and_a_call_without_stream_replaces_it() {
    assert_eq!(
        eval(
            r#"
            const d = new TextDecoder();
            const first = d.decode(new Uint8Array([0xC9]), { stream: true });
            const split = d.decode(new Uint8Array([0xA9]), { stream: true });
            const done = d.decode();

            const flushed = new TextDecoder();
            flushed.decode(new Uint8Array([0xC9]), { stream: true });
            const flush = flushed.decode();

            // The decoder was replaced, so the continuation byte stands alone.
            const after = flushed.decode(new Uint8Array([0xA9]));

            // A four-byte character over three calls is one character.
            const long = new TextDecoder();
            const parts = [
                long.decode(new Uint8Array([0xF0]), { stream: true }),
                long.decode(new Uint8Array([0x9F, 0x98]), { stream: true }),
                long.decode(new Uint8Array([0x80]), { stream: true }),
                long.decode(),
            ];
            [
                JSON.stringify(first), split, JSON.stringify(done),
                JSON.stringify(flush), JSON.stringify(after),
                // A four-byte character is two UTF-16 units long in JavaScript,
                // even though it is one code point to the decoder.
                parts.map(p => p.length).join(','), JSON.stringify(parts[2]),
                // `stream` is read from the dictionary, so any truthy value
                // streams and an absent dictionary does not.
                new TextDecoder().decode(new Uint8Array([0xC9]), { stream: 1 }).length,
                new TextDecoder().decode(new Uint8Array([0xC9])).length,
                new TextDecoder().decode(new Uint8Array([0xC9]), {}).length,
            ].join('|');
        "#
        ),
        concat!(
            r#""""#,
            r#"|ɩ|"""#,
            r#"|"�"|"�""#,
            "|0,0,2,0|\"😀\"",
            "|0|1|1",
        )
    );
}

#[test]
fn an_absent_input_still_flushes_a_streaming_decoder() {
    // `decode()`, `decode(undefined)`, `decode(undefined, undefined)` and
    // `decode(undefined, {})` all lower to the same call: an empty input with
    // the options' default. Each of them flushes, so each report the one
    // replacement character for the byte the previous call held.
    assert_eq!(
        eval(
            r#"
            const flush = call => {
                const d = new TextDecoder();
                d.decode(new Uint8Array([0xC9]), { stream: true });
                return call(d);
            };
            const n = s => s.length + ':' + JSON.stringify(s.charCodeAt(0));
            [
                n(flush(d => d.decode())),
                n(flush(d => d.decode(undefined))),
                n(flush(d => d.decode(undefined, undefined))),
                n(flush(d => d.decode(undefined, {}))),
                n(flush(d => d.decode(undefined, { stream: false }))),
                // With no held byte every one of them is the empty string.
                n(new TextDecoder().decode()),
                n(new TextDecoder().decode(undefined, {})),
            ].join('|');
        "#
        ),
        "1:65533|1:65533|1:65533|1:65533|1:65533|0:null|0:null"
    );
}

#[test]
fn fatal_turns_a_malformed_sequence_into_a_type_error() {
    assert_eq!(
        caught(
            r#"
            const d = new TextDecoder('utf-8', { fatal: true });
            const lenient = new TextDecoder('utf-8');
            results.push(attempt(() => d.decode(new Uint8Array([0xFF]))));
            results.push(attempt(() => d.decode(new Uint8Array([0x41, 0xC3]))));
            // UTF-8 spelled over three bytes may not encode a surrogate.
            results.push(attempt(() => d.decode(new Uint8Array([0xED, 0xA0, 0x80]))));

            // A fresh decoder: an incomplete sequence is held rather than
            // reported while it may still continue, so nothing throws yet.
            const held = new TextDecoder('utf-8', { fatal: true });
            results.push(held.decode(new Uint8Array([0xC9]), { stream: true }) === ''
                ? 'held' : 'wrong');
            results.push(attempt(() => held.decode()));
            // A call that is not streaming had already ended the stream before
            // it failed, so the decoder is replaced and the byte that arrives
            // with the next call is a plain `A` rather than the completion of
            // the sequence above.
            results.push(attempt(() => held.decode(new Uint8Array([0x41]))));
            results.push(held.decode(new Uint8Array([0x41])));
            results.push(held.decode(new Uint8Array([0xEF, 0xBB, 0xBF, 0x41])));

            // The same input, leniently.
            results.push(JSON.stringify(lenient.decode(new Uint8Array([0xFF]))));
            // A UTF-16 decoder's malformed case is a lone trailing byte and an
            // unpaired surrogate.
            results.push(attempt(() => new TextDecoder('utf-16le', { fatal: true })
                .decode(new Uint8Array([0x41]))));
            results.push(attempt(() => new TextDecoder('utf-16le', { fatal: true })
                .decode(new Uint8Array([0x00, 0xD8]))));
            results.push(new TextDecoder('utf-16le', { fatal: true })
                .decode(new Uint8Array([0x41, 0x00])));
            "#,
        ),
        concat!(
            "TypeError:TypeError|TypeError:TypeError|TypeError:TypeError",
            "|held|TypeError:TypeError|no throw|A|A|\"�\"",
            "|TypeError:TypeError|TypeError:TypeError|A",
        )
    );
}

#[test]
fn decode_uses_the_named_encoding() {
    assert_eq!(
        eval(
            r#"
            const decode = (label, bytes, options) =>
                new TextDecoder(label, options).decode(new Uint8Array(bytes));
            const json = JSON.stringify.bind(JSON);
            [
                decode('windows-1252', [0x80]),
                decode('windows-1252', [0x81]),
                decode('windows-1252', [0x93, 0x94]),
                json(decode('iso-8859-2', [0xEF, 0xBB, 0xBF])),
                decode('utf-16le', [0x41, 0x00, 0xE9, 0x00]),
                decode('utf-16be', [0x00, 0x41, 0x00, 0xE9]),
                json(decode('utf-16le', [0x3D, 0xD8, 0x00, 0xDE])),
                json(decode('utf-8', [0xF0, 0x9F, 0x98, 0x80])),
                // `latin1`, `ascii`, `iso-8859-1` and `cp1252` are all names of
                // windows-1252, so they report its name and decode 0x80 as €.
                ['latin1', 'ascii', 'iso-8859-1', 'cp1252', 'US-ASCII']
                    .map(l => new TextDecoder(l).encoding + ':' + decode(l, [0x80])).join(','),
            ].join('|');
        "#
        ),
        concat!(
            "\u{20AC}|\u{81}|“”",
            r#"|"ďťż""#,
            "|Aé|Aé",
            r#"|"😀""#,
            r#"|"😀""#,
            "|windows-1252:€,windows-1252:€,windows-1252:€,windows-1252:€,windows-1252:€",
        )
    );
}

#[test]
fn a_detached_input_decodes_as_no_bytes_at_all() {
    assert_eq!(
        eval(
            r#"
            const buffer = new ArrayBuffer(4);
            const view = new Uint8Array(buffer);
            view.set([0x41, 0x42, 0x43, 0x44]);
            const before = new TextDecoder().decode(view);
            buffer.transfer();
            const after = new TextDecoder().decode(view);
            const empty = JSON.stringify(after);
            [before, view.byteLength, after.length, empty].join('|');
        "#
        ),
        "ABCD|0|0|\"\""
    );
}

// ---------------------------------------------------------------------------
// The recorded deviation
// ---------------------------------------------------------------------------

#[test]
fn a_getter_that_detaches_the_input_runs_after_the_bytes_are_copied() {
    // RECORDED DEVIATION from `wpt/checkout/encoding/textdecoder-arguments.any.js`,
    // whose last case builds a 10000-byte input and an options object whose
    // `stream` getter detaches the input's buffer, then expects `''`.
    //
    // It expects `''` because WebIDL converts an argument's members in
    // *declaration order* and `input` is declared before `options` — but
    // `BufferSource` is a *buffer source*, and WebIDL's buffer-source
    // conversion's own first step is to keep a reference to the bytes, not to
    // copy them. Reading the bytes is a later step of the algorithm that
    // consumes the argument, and by then the buffer is detached, so the
    // implementation reads no bytes.
    //
    // This one copies eagerly in `buffer_source_from_v8` (4a), so the bytes are
    // already in hand when the `stream` getter detaches, and the decode
    // proceeds over what was copied. Returning `''` would need the conversion
    // to hand the algorithm a *view* rather than a `Vec<u8>`, which is a
    // `BufferSource` representation change with no second caller asking for it.
    //
    // The deviation is recorded in the provenance header of
    // `idl/encoding/TextDecoder.webidl` and in `docs/dev/idl.md`. The assertion
    // below is the deviation written down: it fails if the copying ever becomes
    // borrowing, which is when the deviation closes.
    assert_eq!(
        eval(
            r#"
            // The detach mechanism the WPT case relies on, checked on its own
            // so a missing `ArrayBuffer.prototype.transfer` cannot be mistaken
            // for the deviation holding.
            const probe = new ArrayBuffer(4);
            probe.transfer();
            const detaches = probe.byteLength === 0;

            const input = new Uint8Array(10000).fill(0x2A);
            const options = { get stream() { input.buffer.transfer(0); return false; } };
            const got = new TextDecoder().decode(input, options);

            // And the getter is reached after the copy, which is what makes the
            // result above a copy rather than the empty input.
            let reached = false;
            const trace = { get stream() { reached = true; return false; } };
            const other = new TextDecoder().decode(new Uint8Array([0x41]), trace);

            [detaches, got.length, got[0], got.charCodeAt(9999), reached, other].join('|');
        "#
        ),
        "true|10000|*|42|true|A"
    );
}

#[test]
fn decode_propagates_what_a_dictionary_member_throws() {
    // `Symbol('x')` is not a conversion failure: WebIDL's `boolean` is
    // `ToBoolean`, which no value fails.
    assert_eq!(
        caught(
            r#"
            const d = new TextDecoder();
            const bytes = new Uint8Array([0x41]);
            results.push(attempt(() => d.decode(bytes, {
                get stream() { throw new RangeError('boom'); }
            })));
            results.push(attempt(() => d.decode(bytes, { stream: Symbol('x') })));
            results.push(attempt(() => d.decode(bytes, { get stream() { return 1; } })));
            // The input is converted first, so its own failure wins.
            results.push(attempt(() => d.decode(42, {
                get stream() { throw new RangeError('boom'); }
            })));
            "#,
        ),
        "RangeError:RangeError|no throw|no throw|TypeError:TypeError"
    );
}

// ---------------------------------------------------------------------------
// `TextEncoder`
// ---------------------------------------------------------------------------

#[test]
fn encode_returns_a_new_uint8_array_of_the_utf8_bytes() {
    assert_eq!(
        eval(
            r#"
            const e = new TextEncoder();
            const bytes = e.encode('Aé😀');
            const empty = e.encode();
            const again = e.encode('Aé😀');
            [
                bytes instanceof Uint8Array,
                bytes.constructor === Uint8Array,
                Object.prototype.toString.call(bytes),
                bytes.length, bytes.byteLength,
                bytes.buffer.byteLength, bytes.byteOffset,
                Array.from(bytes).join(','),
                // The bytes are UTF-8, so the round trip is the decoder's.
                new TextDecoder().decode(bytes),
                // `[NewObject]`: a second call is a second array, and the bytes
                // are not a view on anything the encoder keeps.
                bytes === again,
                bytes.buffer === again.buffer,
                empty.length, empty.buffer.byteLength,
                e.encode('').length === 0,
                // A lone surrogate is not a scalar value; the USV conversion
                // replaces it before the bytes exist.
                Array.from(e.encode('\uD800')).join(','),
                Array.from(e.encode('a\uD83Db')).join(','),
                Array.from(e.encode('\uDC00\uD800')).join(','),
            ].join('|');
        "#
        ),
        concat!(
            "true|true|[object Uint8Array]|7|7|7|0",
            "|65,195,169,240,159,152,128|Aé😀",
            "|false|false|0|0|true",
            "|239,191,189|97,239,191,189,98|239,191,189,239,191,189",
        )
    );
}

#[test]
fn encode_reads_the_input_as_a_usv_string() {
    // `encode` is declared with a default, so an explicit `undefined`, an
    // explicit empty string and no argument at all are one call; every other
    // input goes through the string conversion first.
    assert_eq!(
        eval(
            r#"
            const e = new TextEncoder();
            const decode = bytes => new TextDecoder().decode(bytes);
            [
                e.encode(undefined).length, e.encode('').length, e.encode().length,
                e.encode(42).length, decode(e.encode(42)),
                decode(e.encode(null)),
                decode(e.encode(true)),
                decode(e.encode({ toString() { return 'ab'; } })),
                decode(e.encode(new Uint8Array(0))),
                // A `USVString`: each unpaired surrogate is one replacement.
                e.encode('\uD800').length,
                decode(e.encode('a\uD83Db')),
                e.encode('\uDC00\uD800').length,
            ].join('|');
        "#
        ),
        "0|0|0|2|42|null|true|ab||3|a\u{FFFD}b|6"
    );
}

// ---------------------------------------------------------------------------
// Reclamation
// ---------------------------------------------------------------------------

#[test]
fn a_collected_encoding_wrapper_frees_its_record() {
    // An encoding record has no second referent, so the wrapper's finalizer is
    // the record's only owner and the sweep must return the slab to its
    // baseline.
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate(
            r#"
            for (let i = 0; i < 50; i += 1) {
                const d = new TextDecoder('windows-1252');
                d.decode(new Uint8Array([0x80]));
                new TextEncoder().encode(String(i));
            }
        "#,
        )
        .expect("evaluate the allocation loop");

    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }

    assert_eq!(
        engine.host_record_count(),
        baseline,
        "a collected encoding wrapper should free its record"
    );
}

#[test]
fn a_live_decoder_keeps_its_record_and_its_held_bytes_across_a_sweep() {
    // The decoder is part of the record, so a `{stream: true}` call's held byte
    // has to survive a collection that runs while the wrapper is reachable.
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate(
            r#"
            globalThis.keep = new TextDecoder('utf-8');
            globalThis.kept = keep.decode(new Uint8Array([0x41, 0xC3]), { stream: true });
        "#,
        )
        .expect("evaluate the streaming call");
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), baseline + 1);
    assert_eq!(
        engine
            .evaluate("[globalThis.kept, globalThis.keep.decode()].join('|')")
            .expect("read the kept record"),
        "A|\u{FFFD}",
        "the record still holds the byte the streaming call left behind"
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

#[test]
fn encoding_hosts_survive_many_round_trips() {
    // Interleaves construction, streaming and reads many times over, so a
    // record freed while a live wrapper still needs it — or a slot reused
    // behind one — shows up as a wrong value rather than as a crash. Each
    // round trip splits the text *inside* its last character, so the middle
    // call has to carry a partial sequence into the next one: `1000` is added
    // for every length that came back wrong. The sum is `Σ(i)` for `i` in
    // `0..50`.
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    assert_eq!(
        engine
            .evaluate(
                r#"
                (() => {
                    const encoder = new TextEncoder();
                    let sum = 0;
                    for (let i = 0; i < 50; i += 1) {
                        const text = String(i) + 'é';
                        const bytes = encoder.encode(text);
                        if (bytes.length !== text.length + 1) { sum += 1000; }
                        const d = new TextDecoder();
                        const head = d.decode(bytes.subarray(0, bytes.length - 2));
                        const middle = d.decode(bytes.subarray(bytes.length - 2, bytes.length - 1),
                                                { stream: true });
                        const tail = d.decode(bytes.subarray(bytes.length - 1), { stream: true });
                        if (head + middle + tail !== text) { sum += 1000; }
                        if (d.decode() !== '') { sum += 1000; }
                        sum += i;
                    }
                    return String(sum === 1225);
                })()
            "#
            )
            .expect("evaluate the round-trip loop"),
        "true"
    );
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), baseline);
}
