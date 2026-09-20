// Minimal URL WebIDL for the P2 package (c) bindings (`docs/spec/05-javascript.md`).
// The member text is copied from the URL Standard's own IDL index
// (<https://url.spec.whatwg.org/>) so the names, types and order are the
// spec's; the left-aligned attribute block is the spec's own alignment.
//
// `url.searchParams` is a *live view*: `url.searchParams.set('a', '1')` writes
// through to `url.href`, and `url.search = '?b=2'` is visible through an
// already-obtained `URLSearchParams`. The host implements that with a shared
// record pair rather than a copy (see `crates/tenta-js/src/bindings/host.rs`).
//
// Deferred, and not declared here: `static URL? parse()` and `static boolean
// canParse()` (the generator has no static-operation installation path yet),
// `USVString toJSON()` (identical to `href`), and the `[LegacyWindowAlias=
// webkitURL]` window alias, which no installer emits. `[Exposed=*]` is spelled
// as the realms the tracer actually saw (`idl/MANIFEST.toml`).

// `[Throws]` on `href` is this repository's spelling for "this accessor can
// fail, so it takes the fallible, non-fast-API path" (`idl/dom`'s `outerHTML`).
// The URL Standard states the throw in prose: assigning a string the parser
// rejects throws a `TypeError` and leaves the URL untouched. Every other
// attribute's setter is a silent no-op on failure, so those stay fast.

[Exposed=(Window,Worker,ServiceWorker)]
interface URL {
  constructor(USVString url, optional USVString base);

  [Throws] stringifier attribute USVString href;
  readonly attribute USVString origin;
           attribute USVString protocol;
           attribute USVString username;
           attribute USVString password;
           attribute USVString host;
           attribute USVString hostname;
           attribute USVString port;
           attribute USVString pathname;
           attribute USVString search;
  [SameObject] readonly attribute URLSearchParams searchParams;
           attribute USVString hash;
};
