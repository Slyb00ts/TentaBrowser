// Minimal Response WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`). The member text of the `Response` interface,
// its `ResponseInit` dictionary, its `ResponseType` enumeration and the `Body`
// mixin's `bodyUsed`, `json()` and `text()` operations are copied from the
// Fetch Standard's own IDL index (<https://fetch.spec.whatwg.org/>), so the
// names, types and order are the spec's.
//
// This slice implements the part of the interface a constructor can build with
// no network and no streams. Four decisions are this file's own, and all four
// are reductions:
//
//  - `BodyInit` is declared as the arms this slice can convert. The standard's
//    `BodyInit` is `(ReadableStream or XMLHttpRequestBodyInit)` and
//    `XMLHttpRequestBodyInit` is `(Blob or BufferSource or FormData or
//    URLSearchParams or USVString)`. `ReadableStream`, `Blob` and `FormData`
//    exist nowhere in this repository, so the union is the two arm types that
//    do: a buffer source and a scalar value string. The arm order is
//    load-bearing and cannot be rearranged — the first arm with *no* test of
//    its own becomes the union's fallback (`crates/tenta-idl/src/codegen/
//    types.rs`'s `union_code`), `USVString` has no test, and a buffer source
//    has one only because slice 4a-0a gave `Type::Buffer` a `buffer_union_test`
//    (`union_test` in the same file). The generated `from_v8` is therefore
//    `if value.is_array_buffer() || value.is_shared_array_buffer() ||
//    value.is_array_buffer_view() { <bytes> } else { <USVString> }`, which is
//    what makes `new Response(new Uint8Array([104, 105])).text()` the string
//    `"hi"` rather than `"104,105"`. Dropping `URLSearchParams` from the union
//    does **not** cost nothing observable, and an earlier revision of this
//    comment claimed it did. It is an object with no buffer-source shape, so it
//    does fall to the `USVString` arm, whose conversion calls the object's
//    stringifier, and `URLSearchParams`'s stringifier *is* its form-encoded
//    serialization — so the **bytes** are right. What is wrong is the body
//    *type*: the standard's *extract a body with type* gives `URLSearchParams`
//    the type `application/x-www-form-urlencoded;charset=UTF-8`
//    (<https://fetch.spec.whatwg.org/#bodyinit-safely-extract>) and everything
//    else the scalar value string branch's `text/plain;charset=UTF-8`, and the
//    `Response` constructor infers `Content-Type` from that type
//    (`crates/tenta-js/src/bindings/host.rs`). So
//    `new Response(new URLSearchParams("a=b")).headers.get("Content-Type")`
//    reads `text/plain;charset=UTF-8` here against the standard's
//    `application/x-www-form-urlencoded;charset=UTF-8` — a page-visible
//    deviation, and the same one `fetch()`'s `body` has.
//    Declaring the arm is not the fix: `union_test`
//    (`crates/tenta-idl/src/codegen/types.rs`) gives `Type::Interface(_)` the
//    test `value.is_object()` — every object, not a brand check — so an arm
//    declared before `USVString` would capture `{}`, an `Array` and a `Date`,
//    and `new Response({})` would fail the `URLSearchParams` conversion instead
//    of stringifying to `"[object Object]"`. Serving it properly needs a
//    platform-object brand-check helper in the generator's prelude beside
//    `has_iterator`, which is a separate slice.
//  - `Body` declares only the three members that need neither a
//    `ReadableStream` nor a `Blob`: `bodyUsed`, `json()` and `text()`. `body`,
//    `arrayBuffer()`, `blob()`, `bytes()`, `formData()` and `textStream()` are
//    left out, because every one of them returns or reads a `ReadableStream`
//    (or a `Blob`/`FormData` that does not exist here). `Response.body` in
//    particular arrives with Streams (`d-5b`). A declared-but-unimplemented
//    member is the bug class slice 4a removed, so none is declared.
//  - The static `error()`, `redirect()` and `json()` methods, the static
//    `from()` method of later revisions, and `clone()` are absent. `clone()`
//    needs a body tee and the statics need the request/redirect machinery.
//    All are recorded in `docs/dev/idl.md`.
//  - `ResponseType` keeps the standard's six values even though a
//    constructor-built response can only ever be `"default"`: the standard
//    declares the enumeration and `type` is a plain read of it, so narrowing it
//    would delete a value a future network response will carry while buying
//    nothing.
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

typedef (BufferSource or USVString) BodyInit;

interface mixin Body {
  readonly attribute boolean bodyUsed;
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
};

Response includes Body;

dictionary ResponseInit {
  unsigned short status = 200;
  ByteString statusText = "";
  HeadersInit headers;
};

enum ResponseType { "basic", "cors", "default", "error", "opaque", "opaqueredirect" };
