// Minimal TextDecoder WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`). The member text and the two dictionaries are
// copied from the Encoding Standard's own IDL index
// (<https://encoding.spec.whatwg.org/>), so the names, types and order are the
// spec's.
//
// Four decisions are this file's own:
//
//  - `decode` takes `BufferSource`, not the spec's `AllowSharedBufferSource`.
//    That typedef is `(ArrayBuffer or SharedArrayBuffer or ArrayBufferView)`,
//    and `SharedArrayBuffer` has no weedle keyword arm, so the spelling would
//    fall through to the typedef/identifier resolver and become an uninhabited
//    `Type::Interface("SharedArrayBuffer")` — a declaration that compiles and
//    then fails at runtime, which is the bug class 4a removed. A union
//    *containing* a buffer source also has no `is_…` test in this generator, so
//    a hand-written union typedef would silently take the `DOMString`
//    fallback. Spelling the spec's own `AllowSharedBufferSource` therefore costs
//    two things. A view *backed by* a `SharedArrayBuffer` still converts, because
//    the brand check is on the view's class and not on its backing store — which
//    is exactly what `encoding/sharedarraybuffer.https.html` asserts, since it
//    hands `decode` a `new Uint8Array(buffer)`. A *bare* `SharedArrayBuffer`
//    argument is refused with a `TypeError` where the typedef admits it, and
//    `encoding/textdecoder-copy.any.js` is the only file in the corpus that does
//    that: it passes `createBuffer(...)` itself to `decode` rather than a view
//    over it. `encoding/textdecoder-streaming.any.js` wraps every buffer in a
//    `Uint8Array` and so observes nothing of this decision. Neither file can run
//    in this engine today in any case — both take their `SharedArrayBuffer`
//    constructor from `/common/sab.js`'s `createBuffer`, which builds it from
//    `new WebAssembly.Memory({shared: true, …})`, and no `WebAssembly` interface
//    is lowered here.
//  - `[Throws]` marks `decode` as taking the fallible, non-fast-API path. The
//    standard states the throw in prose — a decode in `"fatal"` error mode
//    raises a `TypeError` when it meets a malformed sequence — and in this
//    repository `[Throws]` is the spelling for "this member can fail"
//    (`idl/fetch/Headers.webidl`). Without it the member would be fast-API
//    eligible (one optional argument, a string return) and a fast callback
//    cannot throw, so the fatal-mode exception would be lost.
//  - `[Exposed=(Window,Worker)]` is the realm set `idl/MANIFEST.toml` records
//    for this interface; the standard's own set is `[Exposed=*]`.
//  - `TextDecoderStream` is absent: it needs Streams, which arrive with `d-5a`.
//
// `TextDecoderCommon` is a mixin rather than inline members because that is how
// the standard declares it; the front end lowers a mixin once per `includes`
// statement (`crates/tenta-idl/src/front.rs`), and the generator then treats the
// three members as if the interface had declared them.
//
// Known deviation, recorded here so that it is not "fixed" by reordering the
// arguments: `wpt/checkout/encoding/textdecoder-arguments.any.js` ends with a
// call whose `options` is `{get stream() { arr.buffer.transfer(0); return
// false; }}` over a 10000-byte input, and expects `''` — "array buffer
// detached during options conversion". WebIDL converts arguments in
// declaration order, but the buffer-source *conversion* only references the
// value; the operation that reads the bytes is the standard's *serialize I/O
// queue*, which runs after the whole argument list has been converted, by
// which point `[[stream]]` has been read and the buffer is gone. This engine
// copies `input` eagerly *during* argument conversion (4a's decision: `Vec<u8>`
// in both positions, "reading always allocates"), so `input`'s copy is made
// before `options`' read and this call returns the 10000 asterisks. Reordering
// the IDL would change the JavaScript-visible argument order, and adding a lazy
// buffer type would contradict 4a, so the deviation stands; it is pinned by
// `tenta-js-v8`'s
// `a_getter_that_detaches_the_input_runs_after_the_bytes_are_copied`. That
// file's other cases — a buffer detached *before* the call — return `''`, which
// is what 4a already claims.

dictionary TextDecoderOptions {
  boolean fatal = false;
  boolean ignoreBOM = false;
};

dictionary TextDecodeOptions {
  boolean stream = false;
};

[Exposed=(Window,Worker)]
interface TextDecoder {
  constructor(optional DOMString label = "utf-8", optional TextDecoderOptions options = {});

  [Throws] USVString decode(optional BufferSource input, optional TextDecodeOptions options = {});
};

interface mixin TextDecoderCommon {
  readonly attribute DOMString encoding;
  readonly attribute boolean fatal;
  readonly attribute boolean ignoreBOM;
};

TextDecoder includes TextDecoderCommon;
