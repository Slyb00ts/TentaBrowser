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
// both constructors, `size`, `type`, `slice()`, `text()`, `arrayBuffer()`,
// `bytes()`, and `File`'s `name` and `lastModified`. Everything it leaves out is named below rather than
// silently dropped, because a declared-but-unimplemented member is the bug
// class slice 4a removed.
//
// ## What is absent, and why
//
//  - `[NewObject] ReadableStream stream()`
//    (<https://w3c.github.io/FileAPI/#stream-method-algo>) and
//    `[NewObject] ReadableStream textStream()`
//    (<https://w3c.github.io/FileAPI/#text-stream-method-algo>) need the
//    Streams machinery this engine does not have (`d-5a`, the Streams package;
//    `d-5b` is `Response.body`, which consumes one). `arrayBuffer()` and
//    `bytes()` are read through the stream in the standard and from the byte
//    sequence the record holds here; each resolves with a copy.
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
//    — and neither has been written.
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
// 3. **`new Blob(null)` throws here and produces an empty `Blob` in a browser.**
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
  [NewObject] Promise<ArrayBuffer> arrayBuffer();
  [NewObject] Promise<Uint8Array> bytes();
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
