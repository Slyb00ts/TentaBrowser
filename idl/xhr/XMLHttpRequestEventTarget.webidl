// Minimal XMLHttpRequestEventTarget WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`). The member text is copied from the XHR
// Standard's own IDL index (<https://xhr.spec.whatwg.org/>), so the names,
// types and order are the spec's.
//
// This is the first interface in the product to declare an event handler IDL
// attribute, so it is also where the two declarations that attribute's type is
// built from live.
//
// HTML Standard §8.1.8.1, verbatim. They belong to HTML, not to XHR, and they
// move to an HTML IDL file when the global event handlers (`onclick` on the
// window, the `on*` content attributes) land. They are declared here for now
// because this is the interface whose type needs them.
//
// `[LegacyTreatNonObjectAsNull]` is load-bearing and not decoration: it is what
// makes `xhr.onload = 5` read back as `null` while `xhr.onload = {}` stores the
// object (WebIDL §3.4.8). Without it the assignment would throw, because a
// `callback` converts as a callable (§3.2.19).

[LegacyTreatNonObjectAsNull]
callback EventHandlerNonNull = any (Event event);
typedef EventHandlerNonNull? EventHandler;

// The seven attributes are the standard's set, in its order. Each is an
// ordinary event listener under the hood (HTML §8.1.8.1): the host routes it to
// `HostObject::set_event_handler`, which names the listener by stripping the
// `on` prefix (`onload` -> `load`), so no attribute here needs a mapping of its
// own.
//
// `[Exposed=Window]` is spelled as the realm the tracer actually saw
// (`idl/MANIFEST.toml` records `realms = ["window"]` for this interface). The
// standard's set is `(Window,Worker)`, and the derived `XMLHttpRequest` *was*
// seen in both, so the two differ here by the manifest's record and not by the
// standard's. Worker realms are not implemented in this product at all.

[Exposed=Window]
interface XMLHttpRequestEventTarget : EventTarget {
  attribute EventHandler onloadstart;
  attribute EventHandler onprogress;
  attribute EventHandler onabort;
  attribute EventHandler onerror;
  attribute EventHandler onload;
  attribute EventHandler ontimeout;
  attribute EventHandler onloadend;
};
