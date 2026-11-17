//! Tests for `Blob` and `File` (P2 slice `d-4b`'s `Blob`/`File` prerequisite)
//! and the first `idl/fileapi` declarations.
//!
//! Two things make this file different from `dom_headers.rs` and
//! `dom_response.rs`:
//!
//! 1. `File` is the corpus's first interface with a **parent**, so the
//!    prototype chain (`File.prototype -> Blob.prototype -> Object.prototype`)
//!    is a property of the generated glue rather than of the host, and it is
//!    asserted here. The constructor function inherits from `Blob` too.
//!    `File` is installed *after* `Blob`, because `constructor.inherit` reads
//!    the parent's template out of the isolate's cache
//!    (`crates/tenta-js-v8/src/lib.rs`).
//! 2. `BlobPart` is the first union with an interface arm. Generated V8 glue
//!    selects that arm only for an actual `Blob` wrapper (including a `File`,
//!    through interface inheritance). Other objects fall through to
//!    `USVString`, so their string conversion supplies the bytes. The host
//!    retains its record check as a defensive invariant.
//!
//! The algorithms themselves — the slice arithmetic and the type normalization —
//! are unit-tested without a wrapper world in
//! `crates/tenta-js/src/bindings/host.rs`'s `blob_tests`.
//!
//! These tests live here rather than under `crates/tenta-page/tests/` because
//! they need the V8 glue, and no V8 test may live there.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with `<html><body><p></p></body></html>`.
///
/// The `<p>` and the body provide node wrappers for the interface-union tests.
fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::new();
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    let p = d.create_html_element(atoms::P);
    for (parent, child) in [(d.document_node(), html), (html, body), (body, p)] {
        d.mutate(Mutation::Append { parent, child })
            .expect("append");
    }
    Rc::new(RefCell::new(d))
}

/// Runs `source` and returns its completion value. No microtask checkpoint is
/// run afterwards, so a promise's settlement has not happened yet.
fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(document());
    engine
        .evaluate(source)
        .expect("script should evaluate without throwing")
}

/// Runs `source`, drains the microtask queue it filled, then reads `after` from
/// the same engine.
///
/// `Blob.text()` settles through the promise seam (`PromiseOutcome::
/// Resolve(HostValue::String(..))`), which is a microtask, so `evaluate` alone
/// returns before the answer exists. No fixed sleep: `Engine::run_microtasks` is
/// the checkpoint the turn loop would run, and it drains until the queue is
/// empty, so a reaction queued by a reaction has run too.
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

/// Runs each of the `run`s in `source` and collects what it returns or throws.
///
/// A comment on the shape: the helper defines `attempt` and `results` and joins
/// the collected entries with `|`, so an expectation is one entry per
/// `results.push`.
fn caught(source: &str) -> String {
    eval(&format!(
        r#"
        const attempt = run => {{
            try {{ return 'ok:' + run(); }}
            catch (e) {{ return 'threw:' + e.constructor.name; }}
        }};
        const results = [];
        {source}
        results.join('|');
        "#
    ))
}

// ---------------------------------------------------------------------------
// Installation and the File -> Blob chain
// ---------------------------------------------------------------------------

#[test]
fn blob_and_file_are_installed_with_file_below_blob() {
    // `File.prototype`'s chain is the generated installer's, not a hand-written
    // link: WebIDL says a derived interface's prototype inherits the parent's,
    // and `constructor.inherit` is what sets it on the instance template.
    assert_eq!(
        eval(
            r#"
            [
                globalThis.Blob === Blob,
                globalThis.File === File,
                typeof Blob, typeof File,
                Object.getPrototypeOf(File.prototype) === Blob.prototype,
                Object.getPrototypeOf(Blob.prototype) === Object.prototype,
                // A member declared on `Blob` is reachable through `File`, and
                // it is the *same* function object rather than a copy.
                'slice' in File.prototype, 'text' in File.prototype,
                Blob.prototype.slice === File.prototype.slice,
                // Both prototypes enumerate in definition order, and a derived
                // prototype carries only its own two members.
                Object.keys(Blob.prototype).join(','),
                Object.keys(File.prototype).join(','),
                // `Blob`'s `length` is 0 because both of its arguments are
                // `optional`. `File`'s is 2 — see the next test.
                Blob.length,
            ].join('|');
            "#
        ),
        "true|true|function|function|true|true|true|true|true|size,type,slice,text,arrayBuffer,bytes|name,lastModified|0"
    );
}

#[test]
fn constructor_lengths_and_file_inheritance() {
    // File has two required arguments, while Blob has only optional ones.
    assert_eq!(
        eval(
            r#"
            [
                Object.getPrototypeOf(File) === Blob,
                typeof File.length + ':' + File.length,
                typeof Blob.length + ':' + Blob.length,
            ].join('|');
            "#
        ),
        "true|number:2|number:0"
    );
}

#[test]
fn generated_interface_constructors_follow_the_webidl_inheritance_chain() {
    assert_eq!(
        eval(
            r#"
            const links = [
                Object.getPrototypeOf(File) === Blob,
                Object.getPrototypeOf(Blob) === Function.prototype,
                Object.getPrototypeOf(HTMLDivElement) === HTMLElement,
                Object.getPrototypeOf(HTMLElement) === Element,
                Object.getPrototypeOf(Element) === Node,
                Object.getPrototypeOf(Node) === EventTarget,
                Object.getPrototypeOf(EventTarget) === Function.prototype,
                Object.getPrototypeOf(UIEvent) === Event,
                Object.getPrototypeOf(MouseEvent) === UIEvent,
                Object.getPrototypeOf(FocusEvent) === UIEvent,
                Object.getPrototypeOf(WheelEvent) === MouseEvent,
                Object.getPrototypeOf(Event) === Function.prototype,
            ];
            const savedFile = File, savedBlob = Blob;
            const savedWheel = WheelEvent, savedMouse = MouseEvent;
            globalThis.Blob = function ReplacementBlob() {};
            globalThis.MouseEvent = function ReplacementMouseEvent() {};
            links.every(Boolean) &&
                Object.getPrototypeOf(savedFile) === savedBlob &&
                Object.getPrototypeOf(savedWheel) === savedMouse;
            "#
        ),
        "true"
    );
}

// ---------------------------------------------------------------------------
// Constructing a Blob
// ---------------------------------------------------------------------------

#[test]
fn the_constructor_collects_bytes_from_every_arm_of_blob_part() {
    // The three arms, in the order the union's conversions run: a scalar value
    // string, a buffer source (both a whole typed array and a view with an
    // offset), and a `Blob` whose own type must be ignored. `size` is the byte
    // count, so each number below is over the concatenated bytes.
    assert_eq!(
        eval(
            r#"
            const inner = new Blob(['ignored'], {type: 'text/plain'});
            const mixed = new Blob(['ab', new Uint8Array([0x63, 0x64]), inner, 'e']);
            const fromView = new Blob([new Uint8Array([1,2,3,4,5]).subarray(1, 3)]);
            [
                new Blob().size,
                new Blob([]).size,
                new Blob(['abc']).size,
                // "aé😀" is 1 + 2 + 4 bytes: the string arm is UTF-8.
                new Blob(['aé\u{1F600}']).size,
                // 2 + 2 + 7 + 1: the inner `Blob` contributes its bytes only.
                mixed.size,
                mixed.type,
                new Blob([inner]).size,
                fromView.size,
            ].join(',');
            "#
        ),
        "0,0,3,7,12,,7,2"
    );
}

#[test]
fn a_part_list_that_is_not_an_object_is_refused() {
    // `sequence<BlobPart>` needs an object with a callable `@@iterator`; a
    // primitive has none, so a bare string is not a way into the string arm.
    assert_eq!(
        caught(
            r#"
            results.push(attempt(() => new Blob('ab')));
            results.push(attempt(() => new Blob({})));
            results.push(attempt(() => new Blob(3)));
            results.push(attempt(() => new Blob(['a', 'b']).size));
            // An absent or explicitly `undefined` part list is the empty
            // sequence. `null` is not: WebIDL returns the empty sequence for
            // `null` too, and the generated optional-argument test only skips
            // `undefined`. Recorded as a deviation in `docs/dev/idl.md`.
            results.push(attempt(() => new Blob().size));
            results.push(attempt(() => new Blob(undefined).size));
            results.push(attempt(() => new Blob(null).size));
            "#
        ),
        "threw:TypeError|threw:TypeError|threw:TypeError|ok:2|ok:0|ok:0|threw:TypeError"
    );
}

#[test]
fn size_and_type_read_what_the_constructor_stored() {
    assert_eq!(
        eval(
            r#"
            [
                new Blob(['abc']).type,
                new Blob(['abc'], {}).type,
                new Blob(['abc'], {type: 'Text/Plain'}).type,
                new Blob(['abc'], {type: '  '}).type,
                new Blob(['abc'], {type: 'text/plain;charset=UTF-8'}).type,
                typeof new Blob(['abc']).size,
            ].join('|');
            "#
        ),
        // An absent `type` is the empty string, the two spaces survive the range
        // rule, and the lowercase rule leaves a MIME parameter alone.
        "||text/plain|  |text/plain;charset=utf-8|number"
    );
}

#[test]
fn a_type_outside_printable_ascii_empties_the_whole_type() {
    // One character outside U+0020..=U+007E empties the type rather than being
    // dropped, which is what makes `type` a filter and a range test both.
    assert_eq!(
        eval(
            r#"
            [
                new Blob([], {type: 'a\nb'}).type,
                new Blob([], {type: 'a\tb'}).type,
                new Blob([], {type: 'a\u007fb'}).type,
                new Blob([], {type: 'aé'}).type,
                new Blob([], {type: 'a~'}).type,
                new Blob([], {type: 'a b'}).type,
            ].join('|');
            "#
        ),
        "||||a~|a b"
    );
}

#[test]
fn a_blob_argument_is_copied_into_the_new_blob() {
    // "If element is a `Blob`, append the bytes it represents": the copy is of
    // the bytes, and the source's own `type` does not travel with it.
    assert_eq!(
        settled(
            r#"
            const source = new Blob(['hello'], {type: 'text/plain'});
            globalThis.source = source;
            globalThis.copy = new Blob([source]);
            "#,
            "[copy.size, copy.type, source.size, copy === source].join('|');"
        ),
        "5||5|false"
    );
}

#[test]
fn a_file_carries_its_name_and_modification_time() {
    assert_eq!(
        eval(
            r#"
            const f = new File(['abc'], 'note.txt', {type: 'Text/Plain', lastModified: 7});
            const bare = new File([], 'a');
            [
                f.name, f.size, f.type, f.lastModified,
                f instanceof File, f instanceof Blob, f instanceof Object,
                bare.name, bare.size, bare.type,
                // The default is a wall-clock read, so it is *some* number in
                // the range a date a page could build covers.
                bare.lastModified > 0 && bare.lastModified < 8.64e15,
                // A member declared on `Blob` works on a `File` receiver, and
                // its result is a `Blob` and not a `File`.
                new File(['abc'], 'a').slice(1) instanceof Blob,
                new File(['abc'], 'a').slice(1) instanceof File,
                // `lastModified` is not clamped, so a negative value is stored
                // as written.
                new File([], 'a', {lastModified: -1}).lastModified,
            ].join('|');
            "#
        ),
        "note.txt|3|text/plain|7|true|true|true|a|0||true|true|false|-1"
    );
}

#[test]
fn slice_takes_three_independent_arguments() {
    assert_eq!(
        eval(
            r#"
            const b = new Blob(['0123456789'], {type: 'text/plain'});
            const sizes = [
                b.slice().size,
                b.slice(undefined).size,
                b.slice(0).size,
                b.slice(4).size,
                b.slice(-4).size,
                b.slice(0, 4).size,
                b.slice(-4, -1).size,
                b.slice(7, 2).size,
                b.slice(0, 99).size,
                b.slice(-99).size,
            ];
            // A missing third argument loses the type; an empty one empties it.
            const types = [
                b.slice(0, 1).type,
                b.slice(0, 1, '').type,
                b.slice(0, 1, 'TEXT/Plain').type,
                b.slice(0, 1, 'aé').type,
            ];
            // The slice is a copy, so the source is untouched, and only the
            // third `types` entry is non-empty: a missing or empty third
            // argument both give the empty string.
            [sizes.join(','), types.join('|'), b.size, b.type].join(';');
            "#
        ),
        "10,10,10,6,4,4,3,0,10,10;||text/plain|;10;text/plain"
    );
}

#[test]
fn text_resolves_with_the_utf8_decoded_bytes() {
    // The promise seam: `text()` resolves through a microtask, so the value is
    // only observable after a checkpoint. The decode is UTF-8 and strips a
    // leading BOM, which is what `Response.text()` does with the same helper.
    assert_eq!(
        settled(
            r#"
            globalThis.seen = [];
            const record = p => p.then(v => { seen.push(v); });
            record(new Blob(['héllo']).text());
            record(new Blob([]).text());
            record(new Blob([new Uint8Array([0xEF, 0xBB, 0xBF, 0x61])]).text());
            record(new Blob(['abcdef']).slice(1, 3).text());
            record(new File(['x'], 'a').text());
            "#,
            "seen.length + ':' + seen.join('|');"
        ),
        "5:héllo||a|bc|x"
    );
}

// ---------------------------------------------------------------------------
// Interface selection in the BlobPart union
// ---------------------------------------------------------------------------

#[test]
fn objects_without_the_blob_brand_use_the_string_arm() {
    // WebIDL tests the interface brand before selecting the `Blob` arm. Every
    // object below therefore reaches `USVString`, including wrappers for other
    // platform interfaces. Reading each constructed Blob back proves that its
    // bytes came from exactly that object's string conversion.
    assert_eq!(
        settled(
            r#"
            globalThis.values = [
                new URL('https://example.com/'),
                new Headers(),
                new Response(),
                new TextDecoder(),
                new AbortController().signal,
                document.body,
                document,
                window,
                {},
                new Date(0),
                ['ab'],
            ];
            globalThis.expected = values.map(value => String(value));
            globalThis.texts = Array(values.length).fill(null);
            values.forEach((value, index) => {
                new Blob([value]).text().then(text => { texts[index] = text; });
            });
            "#,
            r#"
            [
                texts.length,
                texts.every((text, index) => text === expected[index]),
                texts[0],
                texts[1],
                texts[8],
                texts[10],
            ].join('|');
            "#,
        ),
        "11|true|https://example.com/|[object Headers]|[object Object]|ab"
    );
}

#[test]
fn a_typed_array_still_reaches_the_buffer_arm() {
    // Buffer-source class checks select typed arrays before the Blob/string
    // alternatives. An offset view is pinned because that is where a wrong
    // reading of the underlying bytes shows up.
    assert_eq!(
        eval(
            r#"
            const viewed = new Uint8Array([9, 1, 2, 3, 9]).subarray(1, 4);
            [
                new Blob([viewed]).size,
                new Blob([new Uint8Array([1,2,3])]).size,
                new Blob([new ArrayBuffer(4)]).size,
                new Blob([new DataView(new ArrayBuffer(6), 1, 3)]).size,
            ].join(',');
            "#
        ),
        "3,3,4,3"
    );
}

#[test]
fn a_blob_is_not_accepted_where_a_buffer_is_required() {
    // The other direction. A `Blob` has no buffer-source shape, so `Response`'s
    // `BodyInit` — `(BufferSource or USVString)`, which this slice deliberately
    // does *not* widen — stringifies it instead of reading its bytes. Pinned so
    // that widening `BodyInit` later is a deliberate change with a test to
    // update, not a silent one.
    assert_eq!(
        settled(
            r#"
            globalThis.seen = 'pending';
            new Response(new Blob(['hi'])).text().then(v => { globalThis.seen = v; });
            "#,
            "seen;"
        ),
        "[object Blob]"
    );
}

// ---------------------------------------------------------------------------
// The `endings` refusal
// ---------------------------------------------------------------------------

#[test]
fn the_endings_member_is_refused_by_name_and_by_presence() {
    // `endings` is refused by *presence*, so the standard's own default written
    // out is refused too. `FilePropertyBag` inherits the member, so `File`
    // refuses it the same way, and the dictionary is read with `Get`, so an
    // inherited member is the same refusal.
    assert_eq!(
        caught(
            r#"
            results.push(attempt(() => new Blob(['a'], {endings: 'native'}).size));
            results.push(attempt(() => new Blob(['a'], {endings: 'transparent'}).size));
            results.push(attempt(() => new Blob(['a'], {type: 'x/y'}).size));
            results.push(attempt(() => new File(['a'], 'f', {endings: 'native'}).size));
            results.push(attempt(() => new File(['a'], 'f', {lastModified: 1}).size));
            results.push(attempt(() => new Blob(['a'], Object.create({endings: 'native'})).size));
            // A value outside the enumeration throws in the conversion, before
            // the host's own refusal is reached.
            results.push(attempt(() => new Blob(['a'], {endings: 'crlf'}).size));
            "#
        ),
        "threw:TypeError|threw:TypeError|ok:1|threw:TypeError|ok:1|threw:TypeError|threw:TypeError"
    );
}

#[test]
fn the_refusal_names_the_member_and_the_constructor() {
    assert_eq!(
        eval(
            r#"
            const message = run => { try { run(); return 'no throw'; } catch (e) { return e.message; } };
            [
                message(() => new Blob(['a'], {endings: 'native'})),
                message(() => new File(['a'], 'f', {endings: 'native'})),
            ].join('|');
            "#
        ),
        "Failed to construct 'Blob': the BlobPropertyBag member 'endings' is not implemented|\
         Failed to construct 'File': the FilePropertyBag member 'endings' is not implemented"
    );
}

// ---------------------------------------------------------------------------
// arrayBuffer() and bytes()
// ---------------------------------------------------------------------------

#[test]
fn array_buffer_and_bytes_resolve_with_a_copy_of_the_blobs_bytes() {
    assert_eq!(
        settled(
            r#"
            globalThis.out = [];
            const blob = new Blob([new Uint8Array([1, 2]), 'a']);
            blob.arrayBuffer().then(buffer => {
                out.push(Object.prototype.toString.call(buffer),
                         Array.from(new Uint8Array(buffer)).join(','));
                new Uint8Array(buffer)[0] = 9;
                return blob.bytes();
            }).then(view => out.push(view.constructor.name, Array.from(view).join(',')));
            new File(['xy'], 'f.txt').arrayBuffer().then(buffer => out.push(buffer.byteLength));
            "#,
            "out.join('|')"
        ),
        "[object ArrayBuffer]|1,2,97|2|Uint8Array|1,2,97"
    );
}
