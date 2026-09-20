// Minimal Headers WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`). The member text and the `HeadersInit` typedef
// are copied from the Fetch Standard's own IDL index
// (<https://fetch.spec.whatwg.org/>), so the names, types and order are the
// spec's.
//
// Three of the spec's own rules are load-bearing here and are implemented in
// `crates/tenta-js/src/bindings/host.rs` rather than by the generator:
//
//  - Names are byte-lowercased and matched byte-case-insensitively, and a
//    value has its leading and trailing HTTP whitespace stripped. Both are
//    Fetch's *header name* / *header value* definitions, not WebIDL's.
//  - `iterable<ByteString, ByteString>` iterates the *sorted and combined*
//    list, which `get` and `getSetCookie` do not.
//  - Every constructor and mutator step validates before it inserts, so a
//    rejected name or value leaves the header list untouched.
//
// `[Throws]` is this repository's spelling for "this member can fail, so it
// takes the fallible, non-fast-API path" (`idl/dom`'s `outerHTML`). Fetch
// states the throw in prose: "If name is not a header name, then throw a
// TypeError". Without the marker `delete` and `has` are fast-API eligible — one
// one-byte-string argument, a primitive or void return — and a fast callback
// cannot throw, so the invalid-name case would be lost.
//
// `getSetCookie()` is the accessor the spec's index has today; earlier
// revisions spelled it `getAll()`. The rest of the spec's 2023 additions and
// the `Request`/`Response` guards (`"immutable"`, `"request"`,
// `"request-no-cors"`, `"response"`) arrive with the interfaces that carry
// them (`d-4a`); a script-built `Headers` has guard `"none"`, which is the
// only one this IDL's constructor can produce.
//
// `[Exposed=(Window,Worker)]` is the standard's own set. The tracer saw this
// interface on `window` and in a service worker (`idl/MANIFEST.toml`); the
// difference is recorded in the descriptor only, since no installer gates on
// the exposure list.

typedef (sequence<sequence<ByteString>> or record<ByteString, ByteString>) HeadersInit;

[Exposed=(Window,Worker)]
interface Headers {
  constructor(optional HeadersInit init);

  [Throws] undefined append(ByteString name, ByteString value);
  [Throws] undefined delete(ByteString name);
  [Throws] ByteString? get(ByteString name);
  sequence<ByteString> getSetCookie();
  [Throws] boolean has(ByteString name);
  [Throws] undefined set(ByteString name, ByteString value);
  iterable<ByteString, ByteString>;
};
