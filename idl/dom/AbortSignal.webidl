// Minimal AbortSignal WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`). The member text is copied from the DOM
// Standard's own IDL index (<https://dom.spec.whatwg.org/>), so the names,
// types and order are the spec's.
//
// This is the first `EventTarget` that is neither a node nor the window, and the
// reason `TargetKey` gains a third variant (`crates/tenta-js/src/dom/events`):
// a listener registered on one signal must not fire for another, so the
// listener table cannot be keyed by the shared `TAG_HOST` handle tag or by the
// interface name.
//
// `[Throws]` is this repository's spelling for "this member can fail, so it
// takes the fallible, non-fast-API path" (`idl/dom`'s `outerHTML`,
// `idl/fetch`'s `Headers`). The DOM Standard states the throw in prose: the
// method throws the signal's abort reason. Without the marker the generator
// makes it fast-API eligible — a `void` return with no arguments — and a fast
// callback has no way to raise an exception at all, so the throw would be lost.
//
// Deferred, and not declared here:
//
//  - `attribute EventHandler onabort`. No event-handler IDL attribute is
//    implemented anywhere in this codebase, so the generator would emit a
//    member with no host behind it. `addEventListener('abort', …)` is the path
//    that matters and it works.
//  - `static AbortSignal timeout(milliseconds)` and `any(signals)` are not
//    declared here. They are defined by a script over `AbortController`
//    (`crates/tenta-js/src/conveniences.rs`), on the V8 host only: `timeout`
//    is a timer the page owns and `any` a set of `abort` listeners, and both
//    are what a page would write itself. `abort(reason)` is native: a static
//    operation is handed the realm, as a constructor is.
//  - `AddEventListenerOptions.signal`, which would let one signal abort a
//    listener; it belongs with the fetch work that first needs it.
//
// `[Exposed=Window]` is spelled as the realm the tracer actually saw
// (`idl/MANIFEST.toml`); the standard's set is `*`.

[Exposed=Window]
interface AbortSignal : EventTarget {
  [NewObject] static AbortSignal abort(optional any reason);

  readonly attribute boolean aborted;
  readonly attribute any reason;
  [Throws] undefined throwIfAborted();
};
