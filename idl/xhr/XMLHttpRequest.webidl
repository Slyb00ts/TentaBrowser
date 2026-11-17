// Minimal XMLHttpRequest WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`), slices b-5a, b-5a2, b-5a3, b-7 and b-9. The
// member text is copied from the XHR Standard's own IDL index
// (<https://xhr.spec.whatwg.org/>), so the names, types and order are the
// spec's; the members it does not list are the ones this product does not
// implement.
//
// This file covers the object's two halves: the five states of its state
// machine, the request `open()` sets up and `send()` hands to the engine, the
// author request headers `setRequestHeader()` combines into, and the response
// the machine stores, with the accessors and events that go with it. **What a
// request *is* is the engine's**: `send()` builds one from the record and hands
// it to the callback boundary (`ScriptHost::start_xhr`,
// `crates/tenta-js/src/xhr.rs`), so a page's request reaches a driver only
// through the seam the host layer owns.
//
// `abort()` and the `timeout` attribute are the two ways a request *ends*
// before its answer arrives, and both are the same machinery seen twice: a
// request that is in flight is one the driver holds, so ending it is a call
// through the same seam (`ScriptHost::abort_xhr`), and a request the driver is
// still holding must not be delivered afterwards however it ended. The timer
// `timeout` arms is the page's own (`ScriptHost::schedule_background_task`),
// which is what keeps the deadline on the clock a page's `setTimeout` runs on.
//
// Deferred, and not declared here. Each is recorded rather than stubbed, because
// a declared-but-unimplemented member is the bug class 4a removed:
//
//  - `[SameObject] readonly attribute XMLHttpRequestUpload upload`, and the
//    `XMLHttpRequestUpload` interface with it. The object would be inert: the
//    engine hands a request body to the transport in one call, so there is no
//    byte count to report progress against, and an inert target invites handlers
//    that can never fire. This is a deviation from the standard, recorded rather
//    than implemented. `abort()` inherits it: the *request error steps* fire the
//    failing event and its `loadend` at the upload object first when the upload
//    is not complete, so the member that lands the object owes `abort()` that
//    half of the sequence — and the flag that gates it is now `false` for any
//    request that carries a body, because `send()` accepts one. A page that
//    sends a body can therefore observe the absence: the standard fires
//    `loadstart`, `progress`, `load` and `loadend` at the upload object while
//    such a request is transmitted, and this engine fires none of them.
//  - `readonly attribute Document? responseXML`, which needs a document the
//    parsed body would build. The `"document"` response type is refused for the
//    same reason.
//
// `[Throws]` is this repository's spelling for "this member can fail, so it
// takes the fallible, non-fast-API path" (`idl/dom/AbortSignal.webidl`): a fast
// callback cannot raise an exception at all, so a refusal written into one would
// be lost. Every member that can raise is marked: the `open`, `setRequestHeader`
// and `send` operations, the `overrideMimeType` operation, and the
// `withCredentials` and `responseType` setters. The `[Throws]` on an attribute
// leaves its *getter* fast-API eligible, which is why `withCredentials` still
// reads back as a primitive.
//
// `abort()` and the `timeout` attribute are declared *without* it, because
// neither can raise here. Nothing in `abort()`'s steps throws at all, and the
// standard's own IDL index declares it plain; the `timeout` setter's one throw
// — an `InvalidAccessError` when `synchronous` is true and the global is a
// `Window` — is unreachable in this engine, because `open()` refuses
// `async = false` for every case, so the assertion can never be true. A marker
// on either would put a member on the fallible path to report a failure it
// cannot produce.
//
// Five deliberate deviations from the standard, all of them narrowings this
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
//  - `responseType` accepts every value but `"document"`, which is refused
//    with a `NotSupportedError`: there is no `responseXML` to read it through.
//    `response` is the text for `""` and `"text"`, and for the other three a
//    value built once per finished response — an `ArrayBuffer`, a `Blob` typed
//    by the response's `Content-Type`, or the parsed JSON (`null` when it does
//    not parse). `overrideMimeType()` does not change the `Blob`'s type.
//  - `send()`'s parameter is `XMLHttpRequestBodyInit?` where the standard's is
//    `(Document or XMLHttpRequestBodyInit)?`. The Document arm needs
//    whole-document serialization with the standard's inferred content type.
//    The serializer (`crates/tenta-dom/src/serialize.rs`) exposes `inner_html`,
//    `outer_html` and `serialize_xml` over a `NodeId`, and its walk does carry
//    a document case, but nothing is named for a whole document and nothing
//    applies the standard's two type branches (`text/html;charset=UTF-8` for an
//    HTML document, `application/xml;charset=UTF-8` otherwise). A document
//    passed to `send()` is therefore stringified.
//
// One more deviation is a limit of the transport rather than a narrowing:
// `statusText` reads back as the status code's canonical reason text, and an
// empty string for a status that has none, because the response the driver
// hands over carries no reason phrase for the server's own to come from.
//
// One further deviation is a limit of the MIME parser rather than a narrowing,
// and it is the second half of the body step. When the author *has* set a
// `Content-Type` and the body is a string, the standard does not simply leave it
// alone: it parses the value, and if the `charset` parameter exists and is not
// an ASCII case-insensitive match for `UTF-8`, it rewrites that parameter and
// re-serializes the whole type back into the author request headers
// (<https://xhr.spec.whatwg.org/#the-send()-method>, step 4's sub-step 5). This
// engine sets the inferred type only when the author set none and never rewrites
// one: `parse_mime_type` (`crates/tenta-js/src/bindings/host.rs`) reduces a MIME
// type to its *essence* and its `charset` and drops every other parameter, so it
// cannot re-serialize the value the standard's step asks for. An author
// `Content-Type: text/plain;charset=iso-8859-1` therefore survives as written,
// where a browser rewrites it to `text/plain;charset=UTF-8`. A *buffer-source*
// body infers no type at all, so for it the two agree and an author
// `Content-Type` is kept verbatim either way.
//
// One further deviation is a limit of the generated string conversion rather than
// a narrowing, and it is the third thing the body step can do. WebIDL converts an
// operation's arguments *before* its steps run, so a body whose conversion raises
// leaves `send()` with nothing to do; here the conversion is the generated
// `string_from_v8` (`crates/tenta-idl/src/codegen/prelude.rs`), which is
// infallible — it reads V8's `to_rust_string_lossy`, so a failing `to_string`
// yields the empty string and leaves the exception pending. `send(Symbol())`
// therefore does **two things**: it starts a real request with an empty body and
// `text/plain;charset=UTF-8`, and it throws the `TypeError` V8 was holding when
// the binding returns. The fix is `string_from_v8` becoming fallible, which
// rewrites every generated call site and so needs a generator slice with its own
// goldens; the defect is not `send()`'s, because it is the shared
// `DOMString`/`USVString` conversion — `document.title = Symbol()` throws its
// `TypeError` and stores the empty string for the same reason.
// `a_number_body_is_stringified_and_a_symbol_starts_a_request_before_it_throws`
// (`crates/tenta-js-v8/tests/dom_xhr.rs`) pins the pair.
//
// Two further deviations belong to `timeout`, and neither is a narrowing:
//
//  - The deadline is armed **once**, by `send()`, from the value the record
//    holds at that moment. The standard's wait reads "this's *timeout*" as a
//    live condition — "Wait until either *req*'s done flag is set or this's
//    *timeout* is not 0 and this's *timeout* milliseconds have passed since
//    *now*" — so a value set *after* `send()` governs the request already in
//    flight in a browser, and only the next request here. The setter's own
//    steps are the standard's, so the difference is visible only to a page that
//    changes the value mid-flight.
//  - The standard's *timed out* and *aborted* flags are not held. They are what
//    selects the failing event's name inside the one *request error steps*
//    algorithm; this engine has a distinct caller per cause, so the name is
//    passed from the one place the cause is known instead
//    (`xhr_request_error_steps`, `crates/tenta-js/src/bindings/host.rs`). No
//    page-visible behaviour rests on holding them, because nothing else reads
//    either flag.
//
// `overrideMimeType` is *not* on either list. It stores the MIME type it is
// given — the `charset` parameter of it is what a text response decodes with —
// and raises only where the standard raises: an `InvalidStateError` when the
// state is `LOADING` or `DONE`.
//
// `XMLHttpRequestEventTarget` and the `EventHandler` declarations live in the
// sibling file; the lexer lowers all of `idl/` as one model, so the parent and
// the attribute type are visible here.

// The standard's typedef (<https://fetch.spec.whatwg.org/>, where it is defined
// even though only `send()` consumes it). The arm order is load-bearing: the
// three interfaces are selected by brand and `BufferSource` by a test of its own
// (`buffer_union_test`), and `USVString` has none, so it is the union's fallback
// and any other value is stringified rather than refused.
typedef (Blob or BufferSource or FormData or URLSearchParams or USVString) XMLHttpRequestBodyInit;

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
  [Throws] undefined setRequestHeader(ByteString name, ByteString value);
  attribute unsigned long timeout;
  [Throws] attribute boolean withCredentials;
  [Throws] undefined send(optional XMLHttpRequestBodyInit? body = null);
  undefined abort();

  // response
  readonly attribute unsigned short status;
  readonly attribute ByteString statusText;
  DOMString? getResponseHeader(ByteString name);
  ByteString getAllResponseHeaders();
  [Throws] undefined overrideMimeType(DOMString mime);
  readonly attribute USVString responseURL;
  [Throws] attribute XMLHttpRequestResponseType responseType;
  [Throws] readonly attribute any response;
  [Throws] readonly attribute USVString responseText;
};

// The standard's enumeration, complete. Every value is declared even though the
// setter refuses all but two of them: the enumeration is the type of a plain
// read, and narrowing it would delete a value a later slice's response
// conversion has to name while buying nothing here.
enum XMLHttpRequestResponseType { "", "arraybuffer", "blob", "document", "json", "text" };
