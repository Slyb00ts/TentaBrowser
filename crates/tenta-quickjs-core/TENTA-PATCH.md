# Native QuickJS property hooks

These local forks retain upstream class allocation and GC integration while
adding the native hooks needed by WebIDL legacy platform objects. Both
manifests have `publish = false`; no `[patch]` is used.

## Upstream and scope

The Rust sources are the exact crates.io `rquickjs-core` and `rquickjs-sys`
0.12.2 packages from rquickjs revision
`635f9322ccc875e3f9851e7f75ec538cf3274fd5`. Each fork records the original
package checksum in `UPSTREAM.json`. The sys package contains QuickJS-ng
0.15.1 (the version constants in `quickjs/quickjs.h`). Original README files,
Cargo manifests, and licenses are retained. The Rust binding MIT license,
omitted from the published core/sys archives, is copied from the facade's
0.12.2 archive at the same revision; QuickJS-ng's own MIT license is retained
at `tenta-quickjs-sys/quickjs/LICENSE`.

The existing rquickjs class allocation, class identity, tracing,
and wrapper lifetime machinery stays in use. The changes do not reproduce
private Rust ABI structures in a separate crate.

The immediate consumer is generated WebIDL legacy platform objects. A
JavaScript Proxy cannot implement their full descriptor behavior: WebIDL
permits a named setter to accept a data descriptor with an explicitly false
`configurable` field while subsequently reporting the virtual property as
configurable. A Proxy must instead enforce target descriptor invariants.
The native class hook avoids that mismatch. The generic browser binding
implementation and its conformance tests are separate from these engine hooks.

Primary algorithms: [WebIDL legacy platform objects](https://webidl.spec.whatwg.org/#legacy-platform-objects),
[HTML Storage](https://html.spec.whatwg.org/multipage/webstorage.html#the-storage-interface).

## Native delta

`quickjs.c` adds one callback pointer to its private `JSClass` and initializes
it to null when a class is registered. `JS_SetClassCanPreventExtensions`
installs the pointer for a registered class. `JS_PreventExtensions` invokes
it before the ordinary extensibility transition: -1 propagates an exception,
0 refuses the operation, and 1 permits the normal transition. Ordinary
classes have a null pointer and retain their existing behavior. No public
`JSClassDef` or `JSClassExoticMethods` layout changes.

`JS_RejectPropertyChange` exposes the engine's existing
`JS_ThrowTypeErrorOrFalse` decision, preserving `JS_PROP_THROW` and
`JS_PROP_THROW_STRICT` with the engine's actual current strict frame.
It uses a fixed diagnostic and allocates only the ordinary TypeError when
the existing caller flags require one.

The sys crate shares scalar ABI declarations between bundled and generated
bindings, blocklisting only these extension names in bindgen. The upstream
host-target bindgen shortcut also uses this route. No ABI structures are
copied. Both routes are tested on aarch64 macOS; other targets still need
validation before cross-platform compatibility is claimed.

The core crate connects its existing exotic class vtable to
`define_own_property`, preserving absent descriptor fields. A trait result
of `None` delegates to `JS_DefineProperty` with `JS_PROP_NO_EXOTIC`, avoiding
recursion. A false result preserves caller failure semantics. A new default
`exotic_can_prevent_extensions` allows the ordinary transition, so existing
exotic types remain extensible in the usual way unless they opt out.
All exotic property-operation extern-C callbacks guard class lookup and trait execution with
the existing rquickjs `handle_panic_exotic` catch-unwind boundary. The panic
resumes only after the QuickJS call returns to Rust. TentaBrowser's own
binding boundary adds its established sticky page-failure behavior.

Upstream GC trace/finalizer dispatch is not yet guarded. Browser host
destructors contain their own panics, and the current legacy wrapper traces
its backing object and key contexts without calling JavaScript or holding a
mutable key borrow across collection. Those bounded properties are not a full
native-boundary guarantee. A remaining security gate is a fail-closed GC panic
contract that contains unwinding while preserving collector accounting and
page isolation; catching a trace panic and skipping unmarked edges is not an
acceptable recovery strategy.

The six original exotic hooks now receive the original object separately
from the Receiver; this preserves inherited and Reflect receiver semantics
without looking up a wrapper through a WeakRef. `Atom::to_property_key`
preserves symbol identity using `JS_AtomToValue`; upstream's string-oriented
`to_value` remains unchanged. Two upstream expressions receive explicit
same-type conversions so bindgen's dependencies do not create ambiguous
`PartialEq` inference in the context helper and SystemTime unit test.

The class type predicate uses nonthrowing `JS_GetOpaque` while retaining the
same class-id and native vtable checks. Upstream used `JS_GetOpaque2`, leaving
a pending TypeError when a boolean predicate rejected a different class kind;
real mixed plain/exotic DOM tests reproduced this. Actual typed dereference
continues to use the throwing validation. A focused regression checks that a
false predicate leaves no pending exception.

Empty exotic property enumeration now returns a null table and zero length
without calling `js_malloc(ctx, 0)`. The upstream helper returned success
after that allocation had installed an OOM exception. An isolated empty-class
regression reproduced the hidden exception; the browser's boundary correctly
surfaced it when Storage's only supported name was hidden by its prototype.
Nonempty results check both the ABI's 32-bit count and allocation-size
multiplication before allocating or publishing output pointers.

## Setter ownership finding

The existing `VTable::set_property_impl` wraps the assignment argument with
`Value::from_js_value`, taking ownership. Its C caller,
`JS_SetPropertyInternal2`, calls `em->set_property(..., val, ...)` and then
`JS_FreeValue(ctx, val)` immediately after callback return. The hook therefore
receives a borrowed value. The patch uses `from_js_value_const`, which calls
`JS_DupValue`, matching the receiver and atom handling in the same callback.

The real-class regression reproduced SIGSEGV with only the original owning
conversion restored. The borrowed-value fix passed the same assignment,
collection and teardown test, including AddressSanitizer. Nextest isolated
the crashing original test in its own process.

## Dependency routing

The renamed packages are `crates/tenta-quickjs-core` and
`crates/tenta-quickjs-sys`. The workspace alias is:

```toml
rquickjs = { package = "tenta-quickjs-core", path = "crates/tenta-quickjs-core", version = "=0.12.2" }
```

The renamed core depends on `tenta-quickjs-sys = =0.12.2` by exact local path,
using the existing `rquickjs-sys` dependency alias internally. There is no
facade fork: the current feature set does not use facade derive macros,
and all consumed runtime/class APIs are exported by core. The active graph
must contain exactly one QuickJS sys package and no original `rquickjs`,
`rquickjs-core`, or `rquickjs-sys` route. Retain the sys package's original
link name and verify `cargo tree` and the lockfile after switching.

Repository integration records:

- The two packages before `tenta-idl` in the foundation crate map, with sys
  preceding core; alias-aware dependency linting must verify the real package
  edge instead of skipping it under `rquickjs` names.
- A focused `deny.toml` ban on original QuickJS package routes, if needed to
  prevent accidental duplicate native linkage; no new license exception.
- `THIRD_PARTY_NOTICES.md`, native dependency audit notes, architecture crate
  map, and the decision record updated with the exact local fork and rationale.
- An upstream-diff artifact plus recorded checksums; `publish = false` until
  a separately authorized publication workflow is prepared.

This patch does not implement the per-context timezone hook required by
specification 20 or change D-023's current process timezone behavior. It also
does not provide Intl, workers, or full legacy platform object semantics by
itself. The upstream WASI SDK download build path remains unchanged in this
review payload; WASI is outside the repository's five-platform target set.

## String conversion additions

The local Rust string wrapper exposes a borrowed RAII UTF-16 view through the
existing native `JS_ToCStringLenUTF16`/`JS_FreeCStringUTF16` API, plus exact
`JS_NewStringUTF16` construction. The view preserves unpaired code units;
the separate lossy Rust text adapter replaces them with U+FFFD. These helpers
do not change the native string representation or make Rust `String` capable
of representing every JavaScript string.

`CString::from_string` now validates native bytes before exposing any Rust
`str` view. QuickJS preserves unmatched surrogates as WTF-8, so the prior
unchecked UTF-8 assumption was unsound. Invalid bytes return the existing
`Error::Utf8`; the RAII owner releases the native export on that error path.
Valid Unicode and embedded NUL retain their existing byte-length semantics.
The initial regression reproduced rejection failure without constructing an
invalid Rust `str`; final validation is recorded in the implementation status.

## Structured-clone classification additions (staged)

The QuickJS-ng fork adds three scalar class/metadata queries for the bounded
`structuredClone` slice: exact ordinary-object class identity, resizable
ArrayBuffer state, and detached ArrayBuffer state. Each reads native class or
buffer metadata without running page getters or Proxy traps. The sys binding
blocklists only these declarations during bindgen and supplies matching manual
externs, preserving the existing native layouts. The core wrapper adds an
own-property query using `JS_GetOwnProperty` with a null descriptor after the
ordinary-object brand gate; it does not read inherited values or call a getter.

The matching C controls and real engine clone tests are part of the staged
integration package. They have **not yet executed** in the workspace. This
addition does not claim support for resizable buffers, typed views, Proxy,
platform objects, or the rest of HTML structured serialization.

## Native property-hook validation

Eleven real-class tests cover nonconfigurable virtual definitions, partial
descriptors, refusal to prevent extensions with ordinary object controls,
strict versus Reflect refusal, borrowed object/string assignment through GC,
panic containment for each new callback, original-object/Receiver identity,
symbol property-key identity, nonthrowing class predicates and empty
enumeration without a pending OOM. The tests use no JavaScript Proxy or
host-global monkeypatch.

On 2026-09-23, aarch64 macOS isolated validation passed:

- Full default core library suite with the bindgen host fallback: 158/158.
- Actual workspace default core library suite after the additional class
  predicate and empty-enumeration fixes: 161/161, including all eleven native
  probes. Each fix has an independently reviewed source delta.
- Actual bindgen regeneration (`bindgen,rquickjs-sys/update-bindings`): 9/9
  focused native probes, including the later symbol-key regression.
- AddressSanitizer, C and Rust instrumented: 8/8 probes before the additional
  symbol-key, class-predicate and empty-enumeration tests. Used pinned Rust
  1.98.0 with `RUSTC_BOOTSTRAP=1` for the
  sanitizer flag, `RUSTFLAGS=-Zsanitizer=address`, and upstream LLVM clang.
  Apple's clang sanitizer ABI did not match Rust's bundled sanitizer runtime.
- Original setter conversion reproduction: the assignment probe failed with
  SIGSEGV; patched runs passed, as recorded above.

The host-fallback and actual generated bindings are distinct checks: upstream
reuses bundled host bindings unless `update-bindings` is enabled. The generated
bundled file was restored afterward to avoid unrelated generated churn.
Cross-platform checks and complete generated DOM/Storage integration are
separate validation gates. These tests do not establish WebIDL conformance.
