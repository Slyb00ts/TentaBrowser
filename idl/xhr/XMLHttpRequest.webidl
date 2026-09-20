// Minimal XMLHttpRequest WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`), slices b-5a and b-5a2. The member text is
// copied from the XHR Standard's own IDL index (<https://xhr.spec.whatwg.org/>),
// so the names, types and order are the spec's; the members it does not list are
// the ones this product does not implement.
//
// This file covers the object's first half: the five states of its state
// machine, the request `open()` sets up, and the response the machine stores,
// with the accessors and events that go with it. **Nothing is sent**: a
// completion is delivered to the object through the seam slice b-4 landed, and
// `send()` and `setRequestHeader()` with the driver seam are the slice after
// this one (b-5a3).
//
// Deferred, and not declared here. Each is recorded rather than stubbed, because
// a declared-but-unimplemented member is the bug class 4a removed:
//
//  - `undefined send(...)` and `undefined setRequestHeader(ByteString,
//    ByteString)`. They are the slice after the response half (b-5a3), which is
//    where the network driver is reached; a `send()` that returned without a
//    request would leave the object in a state the page cannot distinguish from
//    a slow response.
//  - `undefined abort()`. The state machine exists now, but nothing is ever in
//    flight, so there is no request to terminate and no reason to enter the
//    `abort` path that would fire an `abort` event. It arrives with `send()`.
//  - `[SameObject] readonly attribute XMLHttpRequestUpload upload`, and the
//    `XMLHttpRequestUpload` interface with it. The object would be inert — no
//    upload progress can be reported before `send()` — and an inert target
//    invites handlers that can never fire. This is a deviation from the
//    standard, recorded rather than implemented.
//  - `readonly attribute USVString responseURL`, `readonly attribute any
//    response` and `readonly attribute Document? responseXML`. The machine and
//    the text response are here, but `response` needs the response type's
//    conversions (`arraybuffer`, `blob`, `json`), each of which needs an
//    interface this engine has no value for yet, `responseXML` needs a document
//    the parsed body would build, and `responseURL` needs the URL field the
//    transport does not carry at this boundary. Every one of them is recorded
//    rather than declared with a stand-in value.
//
// `[Throws]` is this repository's spelling for "this member can fail, so it
// takes the fallible, non-fast-API path" (`idl/dom/AbortSignal.webidl`): a fast
// callback cannot raise an exception at all, so a refusal written into one would
// be lost. Every member that can raise is marked: the `open` operation, the
// `overrideMimeType` operation, and the `timeout`, `withCredentials` and
// `responseType` setters. The `[Throws]` on an attribute leaves its *getter*
// fast-API eligible, which is why `timeout` and `withCredentials` still read
// back as primitives.
//
// Four deliberate deviations from the standard, all of them narrowings this
// engine is responsible for:
//
//  - `open()` refuses `async = false` with a `NotSupportedError`. A synchronous
//    request cannot be expressed by the network driver and would block the
//    page's own turn loop, which deadlocks the settle machinery rather than
//    merely answering slowly.
//  - `open()` refuses a non-null `username` or `password` with a
//    `NotSupportedError`. The standard writes them onto the parsed URL; this
//    engine sends no credentialed request at all (see `withCredentials` below),
//    and what the network layer does with URL userinfo is not verified.
//  - `withCredentials = true` is refused with a `NotSupportedError`: the shared
//    cookie jar attaches credentials on its own policy, and honouring a
//    per-request opt-out would silently mean the opposite of what it says.
//  - `responseType` accepts only `""` and `"text"`, the two values a decoded
//    text response can serve; every other value of the (fully declared)
//    enumeration is refused with a `NotSupportedError`.
//
// One more deviation is a limit of the transport rather than a narrowing:
// `statusText` reads back as the status code's canonical reason text, and an
// empty string for a status that has none, because the response the driver
// hands over carries no reason phrase for the server's own to come from.
//
// `overrideMimeType` is *not* on either list. It stores the MIME type it is
// given — the `charset` parameter of it is what a text response decodes with —
// and raises only where the standard raises: an `InvalidStateError` when the
// state is `LOADING` or `DONE`.
//
// `XMLHttpRequestEventTarget` and the `EventHandler` declarations live in the
// sibling file; the lexer lowers all of `idl/` as one model, so the parent and
// the attribute type are visible here.

[Exposed=(Window,Worker)]
interface XMLHttpRequest : XMLHttpRequestEventTarget {
  constructor();

  // event handler
  attribute EventHandler onreadystatechange;

  // states
  const unsigned short UNSENT = 0;
  const unsigned short OPENED = 1;
  const unsigned short HEADERS_RECEIVED = 2;
  const unsigned short LOADING = 3;
  const unsigned short DONE = 4;

  readonly attribute unsigned short readyState;

  // request
  [Throws] undefined open(ByteString method, USVString url, optional boolean async = true, optional USVString? username = null, optional USVString? password = null);
  [Throws] attribute unsigned long timeout;
  [Throws] attribute boolean withCredentials;

  // response
  readonly attribute unsigned short status;
  readonly attribute ByteString statusText;
  DOMString? getResponseHeader(ByteString name);
  ByteString getAllResponseHeaders();
  [Throws] undefined overrideMimeType(DOMString mime);
  [Throws] attribute XMLHttpRequestResponseType responseType;
  readonly attribute USVString responseText;
};

// The standard's enumeration, complete. Every value is declared even though the
// setter refuses all but two of them: the enumeration is the type of a plain
// read, and narrowing it would delete a value a later slice's response
// conversion has to name while buying nothing here.
enum XMLHttpRequestResponseType { "", "arraybuffer", "blob", "document", "json", "text" };
