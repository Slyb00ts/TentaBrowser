// `Request`, its `RequestInit` dictionary and their enumerations
// (`docs/spec/05-javascript.md`). The member text is copied from the Fetch
// Standard's own IDL index (<https://fetch.spec.whatwg.org/#request-class>), so
// the names, types and order are the spec's. `ReferrerPolicy` is the Referrer
// Policy Standard's
// (<https://w3c.github.io/webappsec-referrer-policy/#referrer-policy>).
//
// **What `Request` is here.** The constructor's steps, `clone()`, the `Body`
// members and the accessors are implemented in
// `crates/tenta-js/src/bindings/host/request.rs`, and `fetch(input, init)` runs
// the same steps for both arms of `RequestInfo`. Deviations:
//
//  - `signal` is the signal the initializer named, or one the request owns
//    that nothing can abort. The standard gives every request a signal of its
//    own that *follows* the given one, so `request.signal === init.signal` is
//    `true` here and `false` in a browser.
//  - The body is a byte sequence held whole (there are no streams), so
//    `clone()` copies it where the standard tees a stream, and `body` — the
//    `ReadableStream` attribute — is not declared. `formData()` is not declared
//    either; it needs a multipart parser.
//  - `destination`, `referrer`, `referrerPolicy` and `integrity` always answer
//    with their defaults, because an initializer that names anything else is
//    refused (below). `isReloadNavigation` and `isHistoryNavigation` are not
//    declared.
//
// **`RequestInit` is declared in full and a member the engine cannot serve is
// refused by name** — in the pattern `responseType` uses, where the whole type
// is declared and the host refuses the values it cannot serve — so that a page
// passing one is *told* rather than silently sent a different request than it
// asked for. The refusal is by value: a member written out with the value the
// engine would use anyway is accepted. Which values are served is
// `refused_request_init_member`'s documentation
// (`crates/tenta-js/src/bindings/host.rs`). The refusal is the host's, not the
// conversion's: the dictionary converts every member it declares.
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

typedef (Request or USVString) RequestInfo;

[Exposed=(Window,Worker)]
interface Request {
  [Throws] constructor(RequestInfo input, optional RequestInit init = {});

  readonly attribute ByteString method;
  readonly attribute USVString url;
  [SameObject] readonly attribute Headers headers;

  readonly attribute RequestDestination destination;
  readonly attribute USVString referrer;
  readonly attribute ReferrerPolicy referrerPolicy;
  readonly attribute RequestMode mode;
  readonly attribute RequestCredentials credentials;
  readonly attribute RequestCache cache;
  readonly attribute RequestRedirect redirect;
  readonly attribute DOMString integrity;
  readonly attribute boolean keepalive;
  readonly attribute AbortSignal signal;
  readonly attribute RequestDuplex duplex;

  [NewObject, Throws] Request clone();
};

Request includes Body;

enum RequestDestination {
  "",
  "audio",
  "audioworklet",
  "document",
  "embed",
  "font",
  "frame",
  "iframe",
  "image",
  "json",
  "manifest",
  "object",
  "paintworklet",
  "report",
  "script",
  "sharedworker",
  "style",
  "track",
  "video",
  "worker",
  "xslt"
};

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
