// Minimal Blob + File WebIDL for the P2 package (c) bindings
// (`docs/spec/05-javascript.md`): `d-4b`'s `Blob`/`File` prerequisite, which
// `b-8`'s `FormData` needs. It carries no slice number of its own — `d-4b`'s
// `b-1`…`b-10` are all spent, and `b-10` names `fetch`'s `RequestInit` (which
// landed early, as `b-6.5`).
//
// The member text of `Blob`, `BlobPropertyBag`, `EndingType`, the `BlobPart`
// typedef, `File` and `FilePropertyBag` is copied from the File API
// specification's own IDL index (<https://w3c.github.io/FileAPI/>), so the
// names, types and order are the spec's. The algorithms behind the members are
// the spec's too, each cited where `crates/tenta-js/src/bindings/host.rs`
// implements it.
//
// This slice implements the *constructible, byte-carrying* half of `Blob`:
// both constructors, `size`, `type`, `slice()`, `text()`, and `File`'s `name`
// and `lastModified`. Everything it leaves out is named below rather than
// silently dropped, because a declared-but-unimplemented member is the bug
// class slice 4a removed.
//
// ## What is absent, and why
//
//  - `[NewObject] ReadableStream stream()`
//    (<https://w3c.github.io/FileAPI/#stream-method-algo>),
//    `[NewObject] ReadableStream textStream()`
//    (<https://w3c.github.io/FileAPI/#text-stream-method-algo>) and
//    `[NewObject] Promise<ArrayBuffer> arrayBuffer()`
//    (<https://w3c.github.io/FileAPI/#arraybuffer-method-algo>) all need the
//    Streams machinery this engine does not have (`d-5a`, the Streams package;
//    `d-5b` is `Response.body`, which consumes one). `textStream()` is
//    the fourth member of the standard's block and is easy to miss: it is a
//    `ReadableStream` and not a promise, so it does not travel with
//    `arrayBuffer()`/`bytes()`.
//  - `[NewObject] Promise<Uint8Array> bytes()`
//    (<https://w3c.github.io/FileAPI/#bytes-method-algo>) cannot settle a
//    promise in this engine at all, and that is a property of the *promise
//    seam*, not of `Blob`. `PromiseOutcome::Resolve` carries a `HostValue`
//    (`crates/tenta-js/src/dom/events/mod.rs`), `HostValue` has exactly four
//    variants — `Undefined`, `DomException`, `String`, `TypeError` — and the V8
//    implementation of `build_value_in` (`crates/tenta-js-v8/src/script_host.rs`)
//    matches them exhaustively. A byte-sequence variant is a slice of its own,
//    and it is the same slice that unblocks `Body.arrayBuffer()`, `Body.bytes()`
//    and `Body.blob()` in `idl/fetch/Response.webidl` and `idl/xhr/
//    XMLHttpRequest.webidl`. `text()` is served because
//    `PromiseOutcome::Resolve(HostValue::String(..))` is one of the four.
//  - `FileList` (<https://w3c.github.io/FileAPI/#filelist-section>), and with it
//    the `files` attribute every `HTMLInputElement` would carry: nothing in
//    this engine can produce one. `HTMLInputElement` in `idl/dom/dom.webidl`
//    has no `type` and no `files`, so there is no `<input type=file>` and no
//    `DataTransfer` to select a file *from*. `FileList` is also, per its own
//    note, at risk of being replaced by an `Array`.
//  - `FileReader` (<https://w3c.github.io/FileAPI/#dfn-filereader>),
//    `FileReaderSync` (<https://w3c.github.io/FileAPI/#dfn-FileReaderSync>) and
//    the `file reading task source`
//    (<https://w3c.github.io/FileAPI/#fileReadingTaskSource>): both read a
//    `Blob` — `text()` is the one read this slice serves, and `FileReader`'s
//    `readAsText` differs from it exactly in honouring a page-supplied encoding
//    — and both need `Blob.arrayBuffer()`, which is above.
//  - `URL.createObjectURL()`/`revokeObjectURL()`
//    (<https://w3c.github.io/FileAPI/#dfn-createObjectURL>) need a blob-URL
//    registry and a `blob:` fetch path. Their only evidence in this repository
//    is one `static: true` member in `golden/inventory/youtube.json`.
//  - The `Serializable` extended attribute
//    (<https://w3c.github.io/FileAPI/#serialization>) — the `structuredClone`/
//    `postMessage` serialization steps — and the `Worker` half of
//    `[Exposed=(Window,Worker)]`: this engine has no Worker and no structured
//    clone. `[Exposed]` keeps the standard's two-value list, as
//    `Response.webidl` does, so the declared surface is the spec's.
//
// ## Deviations these declarations create
//
// 1. **`BlobPropertyBag.endings` is refused by name, by presence.** The CRLF
//    branch of *convert line endings to native*
//    (<https://w3c.github.io/FileAPI/#convert-line-endings-to-native>) is
//    unreachable on a Unix-like host, so an implementation of it would be
//    unverifiable here, and `processing blob parts` applies it only when
//    `endings` is `"native"`
//    (<https://w3c.github.io/FileAPI/#process-blob-parts>, substep 1.2). A
//    refusal says so; a wrong acceptance would silently change a page's bytes
//    on Windows only. The refusal is the *host's* (`crates/tenta-js/src/
//    bindings/host.rs`), the same error direction slice b-6.5 chose for
//    `RequestInit`'s twelve unserved members, and it carries the same cost: it
//    is by **presence** and not by value, so `{endings: "transparent"}` — the
//    standard's own default written out — is refused here where a browser
//    proceeds.
// 2. **`endings` is declared without its default, and that is a deviation from
//    the standard's IDL text.** The standard declares
//    `EndingType endings = "transparent";`. A dictionary member with a default
//    lowers to a plain Rust field and a member without one lowers to an
//    `Option`, so a default makes the member's *presence* unobservable to the
//    host and deviation 1 unimplementable. The host applies the standard's
//    default itself when the member is absent, so the only observable
//    difference is the one deviation 1 records. `type` keeps its `= ""`.
// 3. **A non-`Blob` object in `blobParts` throws here and stringifies in a
//    browser.** `BlobPart` is `(BufferSource or Blob or USVString)` and its
//    `Blob` arm is a brand check the host performs on the record: resolving a
//    `Handle<Blob>` argument requires the record to be a `Blob` or a `File`,
//    and anything else is a `TypeError` naming the argument. So
//    `new Blob([{}])` and `new Blob([new Date()])` throw here where a browser
//    takes the `USVString` arm and produces `"[object Object]"` and the date
//    string. The arm order is the only reason the alternative is not worse:
//    the generator's union test for an interface arm is `value.is_object()`
//    (`crates/tenta-idl/src/codegen/types.rs`'s `union_test`) — *every* object,
//    not a brand check — and the V8 `WrapperWorld::handle` ignores the
//    interface name it is passed, so a `Blob` arm declared without a host-side
//    brand check would accept `{}`, an `Array`, a `Date` and even the window
//    and hand back a `Handle<Blob>` whose bits point at a different record.
//    This is the deliberately chosen error direction: a wrong acceptance
//    produces a silently different object, a wrong refusal says so.
//    One consequence of the order: the `BufferSource` arm comes **first**,
//    because its test is the narrow one
//    (`is_array_buffer() || is_shared_array_buffer() || is_array_buffer_view()`)
//    and the `Blob` arm would swallow every typed array if it came before.
// 4. **`new Blob(null)` throws here and produces an empty `Blob` in a browser.**
//    This one is the *generator's* rather than these declarations': the
//    optional-argument test is `args.length() > 0 && !args.get(0).is_undefined()`,
//    so an explicit `null` reaches `sequence_from_v8`, whose `iteration_method`
//    refuses a non-object. WebIDL's sequence conversion returns the empty
//    sequence for `null` as well as for `undefined`
//    (<https://webidl.spec.whatwg.org/#es-sequence>, step 2). `blobParts` and
//    `fileBits` are the corpus's first optional `sequence<>` parameters, which
//    is what makes it reachable; the fix belongs in the codegen and is recorded
//    in `docs/dev/idl.md`.
//
// ## One editorial defect in the standard, read around rather than reproduced
//
// Both constructors guard the `type` normalization on `type` being non-empty
// (`Blob`'s step 3, `File`'s sub-step 3.1) and then read the guarded local `t`
// from *outside* that guard (`Blob`'s step 4, "its `type` set to the value of
// `t` from the substeps above"; `File`'s sub-step 3.2, "Convert every character
// in `t` to ASCII lowercase"). An absent or empty `type` therefore leaves `t`
// unbound. `BlobPropertyBag` declares `type = ""`, so the empty string is the
// only value the two readings can disagree about, and
// `normalize_blob_type` (`crates/tenta-js/src/bindings/host.rs`) answers the
// empty string for it — the one value for which reading around the defect is
// indistinguishable from the guarded reading.

typedef (BufferSource or Blob or USVString) BlobPart;

[Exposed=(Window,Worker)]
interface Blob {
  constructor(optional sequence<BlobPart> blobParts,
              optional BlobPropertyBag options = {});

  readonly attribute unsigned long long size;
  readonly attribute DOMString type;

  Blob slice(optional [Clamp] long long start,
             optional [Clamp] long long end,
             optional DOMString contentType);

  [NewObject] Promise<USVString> text();
};

dictionary BlobPropertyBag {
  DOMString type = "";
  EndingType endings;
};

enum EndingType { "transparent", "native" };

[Exposed=(Window,Worker)]
interface File : Blob {
  constructor(sequence<BlobPart> fileBits,
              USVString fileName,
              optional FilePropertyBag options = {});
  readonly attribute DOMString name;
  readonly attribute long long lastModified;
};

dictionary FilePropertyBag : BlobPropertyBag {
  long long lastModified;
};
