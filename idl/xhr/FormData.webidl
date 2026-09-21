// Minimal FormData WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`), slice b-8. The member text is copied from the
// XHR Standard's own IDL index (<https://xhr.spec.whatwg.org/#interface-formdata>),
// except in exactly the one place the header below records, so the names, types
// and order are the spec's.
//
// A `FormData` is an **entry list**: a sequence of entries, each a *name* and a
// *value*. Both halves of that definition live outside this file. The *value*
// is `FormDataEntryValue` — `(File or USVString)`, never a `Blob` — and the
// rules that produce an entry from a page's arguments are the HTML Standard's
// *create an entry* (<https://html.spec.whatwg.org/multipage/
// form-control-infrastructure.html#create-an-entry>), not XHR's:
//
//   To create an entry given a string name, a string or Blob object value, and
//   optionally a scalar value string filename: set name to the result of
//   converting name into a scalar value string. If value is a string, then set
//   value to the result of converting value into a scalar value string.
//   Otherwise: if value is not a File object, then set value to a new File
//   object, representing the same bytes, whose name attribute value is "blob";
//   if filename is given, then set value to a new File object, representing the
//   same bytes, whose name attribute is filename.
//
// with the standard's own note that "these operations will create a new File
// object if either filename is given or the passed Blob is not a File object.
// In those cases, the identity of the passed Blob object is not kept." That
// note is the whole of this slice's difficulty, and the reason the entry cannot
// be a copy: the value's *identity* is observable (`formData.get(name)`,
// `getAll(name)[0]` and the pair iterator must all answer with the same `File`
// object the page passed in, or with one object the first read minted), which
// `wpt/checkout/xhr/formdata/set-blob.any.js` pins. A host record's lifetime is
// therefore two-way here in the same way a `Response`'s and its `Headers`' are:
// the entry holds the record's key, so the record must outlive both the
// wrapper's death and the page's own reference. `crates/tenta-js/src/bindings/
// host.rs` implements that protocol and records it.
//
// ## The one deliberate departure from the standard's IDL text
//
// `append` and `set` are declared here with the *value* parameter of the
// two-argument form as a union, `(Blob or USVString)`, where the standard
// writes two distinct overloads:
//
//   undefined append(USVString name, USVString value);
//   undefined append(USVString name, Blob blobValue, optional USVString filename);
//
// The standard's pair is **not expressible through this generator's overload
// resolution**, and the difference is observable. The generated dispatcher
// selects an overload by `args.length()` alone
// (`crates/tenta-idl/src/codegen/v8.rs`'s `overload_dispatch`, whose tie-break
// is "the first declared overload wins"), while WebIDL's own algorithm
// (<https://webidl.spec.whatwg.org/#es-overloads>) truncates each overload's
// argument list at the call's arity and then selects on the *distinguishing
// argument value*. With the standard's declaration the effective overload set
// for `append(name, file)` holds both `(USVString, USVString)` and
// `(USVString, Blob)`, and a platform object at the distinguishing index picks
// the `Blob` arm — which is why a browser's `formData.set("file-1", file)`
// stores the file itself, as `set-blob.any.js` requires. Under arity dispatch
// the same call lands on the `USVString` overload and stores the string
// `"[object File]"`, so four of that file's six subtests fail. Collapsing the
// two overloads into one whose value parameter is the union the algorithm
// actually consumes restores the standard's *semantics* for every input a
// browser distinguishes, and is what this file declares.
//
// The cost is deviation 2 below: the union's interface arm has no brand test in
// the generator, so a value that is an object the host does not recognise is
// refused where a browser would stringify it. The *optional* `filename` stays
// where the standard puts it — on the three-argument form only — so a
// three-argument call is still the Blob form and `append(name, value, filename)`
// on a string argument is still refused, as WebIDL refuses it.
//
// ## Deviations these declarations create
//
// 1. **`new FormData(form, submitter)` is refused synchronously.** The
//    constructor's parameter types are the standard's, but the form arm needs
//    HTML's *constructing the entry list*, which walks a form's submittable
//    elements in tree order. `HTMLFormElement` exists in this repository's IDL
//    (`idl/dom/dom.webidl`) as a *name* only: neither it nor `HTMLInputElement`
//    has a `type`, a `files` or a `value`, so there is nothing to walk and no
//    way to read a control's current value. A non-null `form` is therefore
//    refused with a `NotSupportedError` DOMException, the same error direction
//    slice b-6.5 chose for `RequestInit`'s twelve unserved members and `open()`
//    chose for `async = false`. `new FormData()` is the standard's and is
//    served. One further consequence is a limit of the generator rather than of
//    this file: the argument conversion reaches the host for *any* node wrapper
//    — `crates/tenta-js-v8/src/world.rs`'s `WrapperWorld::handle` ignores the
//    interface name it is handed — so `new FormData(document.body)` is refused
//    with the `NotSupportedError` above where a browser raises a `TypeError`
//    for the wrong type. Both throw; the name differs.
// 2. **A non-`Blob`, non-`File` object value throws where a browser takes the
//    string arm and stringifies.** The lowered test for the union's `Blob` arm
//    is `value.is_object()`
//    (`crates/tenta-idl/src/codegen/types.rs`'s `union_test`) — *every* object,
//    not a brand check — and nothing downstream re-tests it: the arm's
//    conversion succeeds for any object carrying a host pointer, and the *host*
//    is what refuses the rest (`blob_source_of`,
//    `crates/tenta-js/src/bindings/host.rs`). So `formData.append("k", {})`,
//    `...("k", new Date())` and `...("k", new URL("https://example.com/"))`
//    each raise a `TypeError` here where a browser stores `"[object Object]"`,
//    the date string and the serialized URL. This is `BlobPart`'s deviation
//    restated (`idl/fileapi/Blob.webidl`, deviation 3) and its rationale is the
//    same: a wrong acceptance produces a silently different object, a wrong
//    refusal says so. Refusing is also the *only* option this generator leaves
//    open, because the union's `Blob` arm must precede the test-less `USVString`
//    fallback for `formData.append("k", blob)` to be a `Blob` at all.
// 3. **A mutation during iteration is not observed, because a pair iterator is
//    a snapshot.** This one is the generator's and is shared with every landed
//    pair `iterable` (`Headers`, `URLSearchParams`):
//    `crates/tenta-idl/src/codegen/v8.rs`'s `pair_iterator_callback` calls the
//    host's `iterable_entries()` once and builds a plain array from it, which
//    `@@iterator`, `entries`, `keys` and `values` all view.
//    `wpt/checkout/xhr/formdata/iteration.any.js` pins the opposite — its three
//    subtests require `delete` inside a `for...of` to skip the entry and
//    `append` inside one to be reached — and all three fail under any snapshot.
//    The entry list itself is not the problem: the snapshot is taken when the
//    iteration begins, so `for...of fd` over a list nothing mutates is exact.
//    Making it live means a live iterator object per pair `iterable` in the
//    generator, which is a slice of its own. `docs/dev/idl.md` records the same
//    consequence for the pair form generally, and
//    `crates/tenta-js-v8/tests/dom_formdata.rs` pins it so that slice has to
//    come back and flip the test.
// 4. **An entry's value can be collected while the `FormData` still holds it,
//    so a later read can mint a *new* wrapper object for the same record.**
//    A browser's entry list holds the `File` object itself, which keeps it
//    uncollectable; here the entry holds the record's key and the record
//    outlives its wrapper (that is what makes it readable at all), but the
//    *wrapper* is collectable as soon as the page drops every reference to it,
//    and the next read builds another one. The observable difference is
//    narrow — comparing the two objects requires holding the first, which keeps
//    it alive — so this is a latent deviation, recorded rather than worked
//    around: reaching the same `[[Get]]` result twice is the property
//    `set-blob.any.js` pins, and it holds.

typedef (File or USVString) FormDataEntryValue;

[Exposed=(Window,Worker)]
interface FormData {
  constructor(optional HTMLFormElement form, optional HTMLElement? submitter = null);

  undefined append(USVString name, (Blob or USVString) value);
  undefined append(USVString name, Blob blobValue, optional USVString filename);
  undefined delete(USVString name);
  FormDataEntryValue? get(USVString name);
  sequence<FormDataEntryValue> getAll(USVString name);
  boolean has(USVString name);
  undefined set(USVString name, (Blob or USVString) value);
  undefined set(USVString name, Blob blobValue, optional USVString filename);
  iterable<USVString, FormDataEntryValue>;
};
