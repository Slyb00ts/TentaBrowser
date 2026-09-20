// Minimal AbortController WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`). The member text is copied from the DOM
// Standard's own IDL index (<https://dom.spec.whatwg.org/>), so the names,
// types and order are the spec's.
//
// The controller is the only way to reach a signal in this slice: it holds the
// one signal the constructor makes, and `abort()` is the only trigger that
// exists anywhere in the workspace. `AbortSignal.timeout` and `AbortSignal.any`
// are the statics that would add a second trigger, and both are deferred (see
// `idl/dom/AbortSignal.webidl`).
//
// `[SameObject]` is load-bearing rather than decorative: the record the
// controller points at is created once by the constructor and returned by every
// read of `signal`, so two reads are the *same* JavaScript object, which is what
// `wpt/checkout/dom/abort/event.any.js` asserts.
//
// `[Exposed=Window]` is spelled as the realm the tracer actually saw
// (`idl/MANIFEST.toml`); the standard's set is `*`.

[Exposed=Window]
interface AbortController {
  constructor();

  [SameObject] readonly attribute AbortSignal signal;

  undefined abort(optional any reason);
};
