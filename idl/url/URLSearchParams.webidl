// Minimal URLSearchParams WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`). The member text is copied from the URL
// Standard's own IDL index (<https://url.spec.whatwg.org/>).
//
// The constructor's `init` union is the reason this interface is in this
// slice: it is the first runtime exercise of the generated `record` arm and of
// the sequence arm that selects on `@@iterator`, so an object literal, an array
// of pairs, a `Map` and a string all take a different path.
//
// `iterable<USVString, USVString>` is the pair form, so `entries`, `keys`,
// `values`, `forEach` and `@@iterator` come from the generated pair-iterator
// glue, not from `%Array.prototype%`.
//
// The spec's `= ""` default on `init` is not spelled here: a default on a
// union-typed argument has no Rust expression in the generator, and an absent
// argument already means the empty list, which is what `""` parses to. The
// `readonly attribute unsigned long size` is deferred with the rest of the
// spec's 2023 additions. `[Exposed=*]` is spelled as the realms the tracer
// actually saw (`idl/MANIFEST.toml`).

[Exposed=(Window,ServiceWorker)]
interface URLSearchParams {
  constructor(optional (sequence<sequence<USVString>> or record<USVString, USVString> or USVString) init);

  undefined append(USVString name, USVString value);
  undefined delete(USVString name, optional USVString value);
  USVString? get(USVString name);
  sequence<USVString> getAll(USVString name);
  boolean has(USVString name, optional USVString value);
  undefined set(USVString name, USVString value);

  undefined sort();

  iterable<USVString, USVString>;
  stringifier;
};
