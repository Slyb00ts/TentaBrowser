// Minimal TextEncoder WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`). The member text is copied from the Encoding
// Standard's own IDL index (<https://encoding.spec.whatwg.org/>), so the names,
// types and order are the spec's.
//
// Three notes are this file's own:
//
//  - `encodeInto` and the `TextEncoderEncodeIntoResult` dictionary it returns
//    are absent. The write half needs a mutable out-parameter that does not
//    exist anywhere in this generator: a host operation receives `&self`, an
//    `any` arrives as opaque `u64` bits, and `codegen/convert.rs` has no
//    concept of a mutable argument. `encode` covers the whole of the encoder
//    otherwise — the standard's own `encode` step is a UTF-8 encode with error
//    mode `"fatal"`, and UTF-8 encoding cannot fail.
//  - `TextEncoderStream` is absent: it needs Streams, which arrive with `d-5a`.
//  - `[Exposed=(Window,Worker)]` is the realm set `idl/MANIFEST.toml` records;
//    the standard's own set is `[Exposed=*]`. `[NewObject]` on `encode` is the
//    spec's, and the front end parses extended attributes generically.
//
// `TextEncoderCommon` is a mixin because that is how the standard declares it;
// the front end lowers it once per `includes` statement. `encoding` is the only
// member, and it is a constant: the encoder's encoding is always UTF-8.

[Exposed=(Window,Worker)]
interface TextEncoder {
  constructor();

  [NewObject] Uint8Array encode(optional USVString input = "");
};

interface mixin TextEncoderCommon {
  readonly attribute DOMString encoding;
};

TextEncoder includes TextEncoderCommon;
