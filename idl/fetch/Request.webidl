// The `RequestInit` dictionary for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`), slice b-6.5.
//
// **There is no `Request` interface here, and this slice does not add one.** A
// declared-but-unimplemented interface is the bug class slice 4a removed, and
// `Request` is a whole object — `clone()`, the `Body` mixin, `signal`, ten more
// accessors — that `fetch()` has no use for. What `fetch()` needs is the
// *initializer*, so the dictionary lands in the file `idl/MANIFEST.toml`
// reserves for the `Request` interface and the interface itself stays deferred.
// `RequestDestination`, which is `Request`'s own enumeration and not a
// `RequestInit` member, is deferred with it.
//
// The dictionary's member text is copied from the Fetch Standard's own IDL
// index (<https://fetch.spec.whatwg.org/#requestinit>), so the names, types and
// order are the spec's. Six of the seven enumerations are the standard's
// (`fetch.spec.whatwg.org/#requestmode` and the five beside it);
// `ReferrerPolicy` is the Referrer Policy Standard's
// (<https://w3c.github.io/webappsec-referrer-policy/#referrer-policy>).
//
// **Three of the fifteen members are served and the other twelve are refused by
// name.** `method`, `headers` and `body` are the three an engine request has,
// and the rest are declared in full — in the pattern `responseType` uses, where
// the whole type is declared and the host refuses the values it cannot serve —
// so that a page passing one is *told* rather than silently sent a different
// request than it asked for. Each refusal is the host's
// (`crates/tenta-js/src/bindings/host.rs`), not the conversion's: the dictionary
// converts every member it declares, and the twelve the engine cannot honour
// are then refused by name.
//
// **The refusal is by presence, not by value, so it over-refuses.** A page that
// writes out the standard's default explicitly — `{keepalive: false}`,
// `{signal: null}`, `{window: null}`, `{integrity: ""}`, `{cache: "default"}` —
// asks for exactly the request this engine would have sent anyway, and is
// refused where a browser proceeds. That is the chosen error direction: a wrong
// acceptance would send a different request than the page asked for, silently.
//
// Two consequences of declaring a member whose type the conversion acts on:
//
//  - A member value the *type* rejects throws the type's own `TypeError` before
//    the refusal runs — `credentials: "bogus"` is not a `RequestCredentials` and
//    `signal: 1` is not an `AbortSignal`. That is WebIDL's own order, and it is
//    what a browser does with the same call.
//  - Because the dictionary reads its members with `Get` and not
//    `[[GetOwnProperty]]`, a member inherited from the prototype chain or
//    defined non-enumerably *is* a member
//    (<https://webidl.spec.whatwg.org/#js-dictionary>: "Properties on the object
//    (or its prototype chain) correspond to dictionary members", and the
//    conversion's *jsDict* step is `? Get(jsDict, key)`). A key the dictionary
//    does not name is ignored, which is what a dictionary conversion does.
//
// `body` reuses `BodyInit`, declared in the sibling `Response.webidl`: the
// lexer lowers all of `idl/` as one model, so the name is visible here, and a
// second declaration of it would be a fatal `Error::Model` rather than a
// deduplication (`crates/tenta-idl/src/front.rs`). It is the standard's
// `BodyInit` without the `ReadableStream` arm.

dictionary RequestInit {
  ByteString method;
  HeadersInit headers;
  BodyInit? body;
  USVString referrer;
  ReferrerPolicy referrerPolicy;
  RequestMode mode;
  RequestCredentials credentials;
  RequestCache cache;
  RequestRedirect redirect;
  DOMString integrity;
  boolean keepalive;
  AbortSignal? signal;
  RequestDuplex duplex;
  RequestPriority priority;
  any window; // can only be set to null
};

enum ReferrerPolicy {
  "",
  "no-referrer",
  "no-referrer-when-downgrade",
  "same-origin",
  "origin",
  "strict-origin",
  "origin-when-cross-origin",
  "strict-origin-when-cross-origin",
  "unsafe-url"
};

enum RequestMode { "navigate", "same-origin", "no-cors", "cors" };
enum RequestCredentials { "omit", "same-origin", "include" };
enum RequestCache { "default", "no-store", "reload", "no-cache", "force-cache", "only-if-cached" };
enum RequestRedirect { "follow", "error", "manual" };
enum RequestDuplex { "half" };
enum RequestPriority { "high", "low", "auto" };
