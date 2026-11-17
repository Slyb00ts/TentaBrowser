// Minimal Response WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`). The member text of the `Response` interface,
// its `ResponseInit` dictionary, its `ResponseType` enumeration and the `Body`
// mixin's attribute and operations are copied from the
// Fetch Standard's own IDL index (<https://fetch.spec.whatwg.org/>), so the
// names, types and order are the spec's.
//
// This slice implements the part of the interface a constructor can build with
// no network and no streams. Four decisions are this file's own, and all four
// are reductions:
//
//  - `BodyInit` is `XMLHttpRequestBodyInit`, the typedef `idl/xhr/
//    XMLHttpRequest.webidl` declares: `(Blob or BufferSource or FormData or
//    URLSearchParams or USVString)`. The standard's is `(ReadableStream or
//    XMLHttpRequestBodyInit)`; Streams is unimplemented, so that arm is absent.
//    Each arm's bytes and inferred `Content-Type` are *extract a body with
//    type* (`extract_body` in `crates/tenta-js/src/bindings/host.rs`). The arm
//    order is load-bearing — the first arm with *no* test of its own becomes
//    the union's fallback (`crates/tenta-idl/src/codegen/types.rs`'s
//    `union_code`), and `USVString` is the one with none: the three interface
//    arms are selected by brand and the buffer source by its class, which is
//    what makes `new Response(new Uint8Array([104, 105])).text()` the string
//    `"hi"` rather than `"104,105"`, and anything else is stringified.
//  - `Body` declares the members that read the whole body: `bodyUsed`,
//    `arrayBuffer()`, `blob()`, `bytes()`, `json()` and `text()`. `body` and
//    `textStream()` are left out because each is a `ReadableStream`
//    (`Response.body` arrives with Streams, `d-5b`), and `formData()` because
//    it needs a multipart parser. A declared-but-unimplemented member is the
//    bug class slice 4a removed, so none is declared. `blob()`'s type is the
//    first `Content-Type` value that parses as a MIME type, lower-cased and
//    otherwise as the header spelled it; the standard serializes the parsed
//    type, which also rewrites the spacing of its parameters.
//  - `clone()` copies the response: the body is a byte sequence held whole, so
//    the standard's tee of a stream is a copy of it. The static `error()`,
//    `redirect()` and `json()` methods and the static `from()` method of later
//    revisions are absent. All are recorded in `docs/dev/idl.md`.
//  - `ResponseType` keeps the standard's six values even though a
//    constructor-built response can only ever be `"default"`: the standard
//    declares the enumeration and `type` is a plain read of it, so narrowing it
//    would delete a value a network response carries while buying nothing.
//
// The `Headers` object the constructor installs is under guard `"response"`
// (constructor step 2), which is the one guard this repository implements
// beyond `"none"`. Under it a mutator drops a *forbidden response-header name*
// — `Set-Cookie` and `Set-Cookie2` — silently rather than throwing
// (<https://fetch.spec.whatwg.org/#forbidden-response-header-name>, consumed by
// *validating* a header: "If headers's guard is `"response"` and name is a
// forbidden response-header name, then return false"). The constructor's *fill*
// step runs the same rule, because *fill* appends through *append a header*.
// This is observable without any network, which is why it lands here rather
// than with `d-4a-1`:
//
//     new Response(null, {headers: {"Set-Cookie": "a=b"}}).get("Set-Cookie")
//     // null
//     new Headers({"Set-Cookie": "a=b"}).get("Set-Cookie")
//     // "a=b"
//
// `HeadersInit` is the typedef `idl/fetch/Headers.webidl` declares, not a
// second one: `crates/tenta-js/build.rs` lowers `idl/dom`, `idl/url`,
// `idl/fetch` and `idl/encoding` as one unit, so the name is visible here, and
// a second declaration of it would be a fatal `Error::Model` rather than a
// deduplication (`crates/tenta-idl/src/front.rs`).

typedef XMLHttpRequestBodyInit BodyInit;

interface mixin Body {
  readonly attribute boolean bodyUsed;
  [NewObject] Promise<ArrayBuffer> arrayBuffer();
  [NewObject] Promise<Blob> blob();
  [NewObject] Promise<Uint8Array> bytes();
  [NewObject] Promise<any> json();
  [NewObject] Promise<USVString> text();
};

[Exposed=(Window,Worker)]
interface Response {
  constructor(optional BodyInit? body = null, optional ResponseInit init = {});

  readonly attribute ResponseType type;
  readonly attribute USVString url;
  readonly attribute boolean redirected;
  readonly attribute unsigned short status;
  readonly attribute boolean ok;
  readonly attribute ByteString statusText;
  [SameObject] readonly attribute Headers headers;

  [NewObject, Throws] Response clone();
};

Response includes Body;

dictionary ResponseInit {
  unsigned short status = 200;
  ByteString statusText = "";
  HeadersInit headers;
};

enum ResponseType { "basic", "cors", "default", "error", "opaque", "opaqueredirect" };
