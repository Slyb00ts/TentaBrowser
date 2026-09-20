# WebIDL Parser and Code Generator (`tenta-idl`)

`tenta-idl` turns WebIDL into Rust. It is a build-time library (plus a small
CLI for inspection) with no dependency on any other crate of the workspace, so
every feature crate can call it from its `build.rs` without creating a cycle
(`cargo xtask lint-deps` enforces the direction).

The pipeline has three stages:

```text
.webidl  ──parse──▶  weedle4 AST  ──lower──▶  Model  ──generate──▶  Rust files
```

Everything after the parser is pure: `parse_webidl(&str) -> Result<Model>` and
the generators are `&Model -> String`, so the output is reproducible and
golden-testable.

## Front end

The parser is [`weedle4`](https://crates.io/crates/weedle4) 0.4 (the Servo
fork), renamed to `weedle` in `Cargo.toml`. It was chosen over the
alternatives after running all three against `tests/fixtures/dom-excerpt.webidl`:

| Crate | Result on the fixture | Verdict |
|-------|-----------------------|---------|
| `weedle` 0.13 | 22 definitions | works, but is unmaintained and pulls in nom 5. |
| `weedle2` 5.0 | panics: "There is redundant raw data after parsing" | the uniffi fork rejects `[Exposed=*]`, which the DOM Standard puts on `EventTarget` and `Event`. |
| `weedle4` 0.4 | 22 definitions | chosen: Servo-maintained, nom 7, accepts `[Exposed=*]`, `async iterable`, `constructor()` and `undefined`. |

`weedle::parse` asserts when input is left over, so the front end calls
`Definitions::parse` directly and reports the remainder as
`Error::Syntax`; a malformed file never panics the build script.

`parse_webidl_units` takes several sources and treats them as one compilation
unit, so a `partial interface` or an `includes` statement may live in a
different file from the interface it extends. `parse_directory` reads every
`*.webidl` of a directory in sorted order and calls it.

Lowering runs in three passes:

1. **Bucket names.** Every declared name is classified (interface, mixin,
   dictionary, enum, callback, callback interface, namespace, typedef). A name
   declared twice, or with two different kinds, is `Error::Model`. Unknown
   names stay as opaque interface handles, which is what a trimmed per-crate
   IDL set looks like; they are listed in `Model::opaque_interfaces`.
2. **Resolve typedefs.** Typedefs are resolved transitively and memoized, with
   a cycle guard that reports `Error::Model` rather than recursing.
   `DOMHighResTimeStamp` becomes `double` at every use site, `EventHandler`
   becomes `EventHandlerNonNull?`.
3. **Lower definitions**, then merge `partial` blocks, then merge `includes`
   mixins, then finish each interface (overload grouping, Rust names, fast-API
   classification).

Unions are interned: `(Node or DOMString)` written in two places yields one
generated enum, named after its members (`NodeOrDomString`).

## Model

`tenta_idl::model` holds the resolved semantics, not the syntax.

- `Interface`: name, parent, `Exposure`, extended attributes, constructors,
  constants, members, `included_mixins`, `partial_blocks`, `iterable`,
  `stringifier`, indexed and named getter names, anonymous special operations.
- `Member` is either an `Attribute` or an `OperationGroup`; a group holds the
  overloads of one name. Member order is: the interface's own members in
  declaration order, then each `partial` block in file order, then each
  included mixin. Attributes and operations keep their relative declaration
  order; every member records its `MemberOrigin` so the generated
  documentation can say where it came from.
- `Dictionary` (with inheritance; `Model::dictionary_members` flattens parents
  first), `Enum`, `Callback`, `CallbackInterface`, `Namespace`, `Typedef`,
  `Union`.
- `Type` covers `undefined`, `any`, the eight integer widths, floats,
  `DOMString`/`ByteString`/`USVString`, `object`, `symbol`, interfaces,
  dictionaries, enums, callbacks, sequences, frozen and observable arrays,
  records, promises, unions and nullables.
- `Argument` carries `optional`, `variadic`, the default value and the
  extended attributes, so `[Clamp]` and `[EnforceRange]` reach the converters.

### Property enumeration order

`Interface::property_enumeration_order` returns the WebIDL order: constants
first, then the non-static attributes and operations in definition order, then
`Symbol.toStringTag`. Statics live on the interface object, not the prototype,
so they are not part of it. The same list is emitted into the generated
registry descriptor, which is how a runtime can assert the order without
re-parsing IDL.

### Fast-API classification

`docs/spec/05-javascript.md` §2 defines when a member may use a V8 fast-API
call. `FastApi::Eligible` requires all of:

- not `static` (a fast call needs a receiver);
- the return type is a numeric type, `boolean` or `undefined`;
- every argument is one of those primitives, or at most one argument is a
  one-byte string (`DOMString`/`ByteString`/`USVString`, non-nullable);
- the member has exactly one argument count: no optional or variadic
  arguments, and no overload set whose overloads differ in arity;
- it is not a special operation (`getter`/`setter`/`deleter`).

`FastApi::Ineligible(NotFastBecause)` records the reason, which the generator
writes into the trait's doc comment. One rule is ours rather than the spec's:
an `[EnforceRange]` argument narrower than 32 bits (`byte`, `octet`, `short`,
`unsigned short`) is ineligible, because V8 152 range-checks against the C
type of the fast call and offers no fallback flag, so the extra WebIDL
narrowing could not report a `TypeError`. `[Clamp]` has no such problem: the
prelude's `clamp_*` helpers cannot fail.

## Type mapping

| WebIDL | Rust (return, field) | Rust (argument) |
|--------|----------------------|-----------------|
| `undefined` | `()` | — |
| `boolean` | `bool` | `bool` |
| `byte`, `octet` | `i8`, `u8` | same |
| `short`, `unsigned short` | `i16`, `u16` | same |
| `long`, `unsigned long` | `i32`, `u32` | same |
| `long long`, `unsigned long long` | `i64`, `u64` | same |
| `float`, `unrestricted float` | `f32` | `f32` |
| `double`, `unrestricted double` | `f64` | `f64` |
| `DOMString`, `USVString` | `String` | `&str` |
| `ByteString` | `Vec<u8>` | `&[u8]` |
| `any` | `AnyValue` | `AnyValue` |
| `object`, `symbol` | `JsObject`, `JsSymbol` | same |
| interface `X` | `Handle<X>` | `Handle<X>` |
| `sequence<T>`, `FrozenArray<T>`, `ObservableArray<T>` | `Vec<T>` | `Vec<T>` |
| `record<K, V>` | `Vec<(K, V)>` | `Vec<(K, V)>` |
| `Promise<T>` | `Promise<T>` | `Promise<T>` |
| `T?` | `Option<T>` | `Option<T>` |
| dictionary | generated `struct` with `Default` | same |
| enum | generated `enum` with `VALUES`, `as_str`, `parse` | same |
| union | generated `enum`, one variant per member | same |
| callback | `Callback<(Args,), Ret>` | same |

`record` maps to `Vec<(K, V)>`, not a map: WebIDL records preserve insertion
order and allow duplicate keys on the way in, and the ordered vector is what
the V8 and QuickJS conversions can reproduce exactly. The generated V8
conversion ignores `K` and always produces `String` keys, so a record whose key
type is not a string type (only `ByteString` in the current corpus) does not
compile; no such record exists in `idl/` yet.

The conversions themselves follow the WebIDL algorithms rather than the shape
of the JavaScript value:

- `sequence<T>`, `FrozenArray<T>` and `ObservableArray<T>` convert through
  `GetMethod(V, @@iterator)` and `GetIterator`, not through an array check, so
  any iterable is a sequence (`new URLSearchParams(new Map([["a", "b"]]))` is
  the caller that needs it). A primitive is a `TypeError`: WebIDL rejects a
  non-object before it looks for the method.
- `record<K, V>` is `es-to-record`: own keys first, then each key's property
  descriptor decides whether it is enumerable, and only then is the value read.
  The descriptor read is observable through a proxy. Keys are read back as
  strings, so an integer-like own key is not dropped.
- `ByteString` is converted isomorphically: one code unit per byte, through the
  one-byte string APIs, and a code unit above `0xFF` is a `TypeError` rather
  than a truncation to its low byte.
- `USVString` needs no separate conversion here. `String` on the Rust side
  cannot hold a lone surrogate, and V8's `to_rust_string_lossy` maps an unpaired
  surrogate to U+FFFD, which is the substitution WebIDL's scalar-value
  conversion prescribes.
- Each arm of a union carries its own WebIDL test — `is_object()` for the
  object-like arms, `@@iterator` for a sequence, `is_object() && !is_array()`
  for a record, `is_number()` for the numeric arms — and the arms are tried in
  declaration order. The first arm with no test of its own is the fallback;
  declaring one earlier cannot shadow a later arm of the same kind.

Fast-API-eligible members return their value directly; every other member
returns `Result<_, JsError>` so the glue can turn a failure into a JavaScript
exception.

## Generated files

`tenta_idl::generate(&model)` returns, in this order:

| File | Contents |
|------|----------|
| `prelude.rs` | Runtime support: `Handle`, `Promise`, `Callback`, `AnyValue`, `JsError`, the WebIDL integer conversions (`to_*`, `clamp_*`, `enforce_*`), the registry types, and the engine helpers behind `cfg(feature = "v8")` / `cfg(feature = "quickjs")`. |
| `types.rs` | Interface markers, typedef aliases, enums, dictionaries, unions, callback aliases, with their engine conversions. |
| `<snake>_ops.rs` | The `*Ops` trait the host implements, one per interface and namespace. |
| `<snake>_v8.rs` | The V8 glue. |
| `<snake>_qjs.rs` | The QuickJS glue. |
| `<snake>_register.rs` | `register_<snake>()` and the descriptor tables. |

All files of one binding module are `include!`d into the same module, prelude
first; generated code refers to prelude items by bare name and carries no
imports of its own. Every generated file starts with a `//` header rather than
`//!`, so the include order is unconstrained.

### `*Ops` traits

One trait per interface. Inheritance becomes a supertrait bound
(`pub trait NodeOps: EventTargetOps`), so a host type implements the whole
chain. Constants become associated consts with their IDL value. Attributes
become `fn name(&self)` plus `fn set_name(&self, value)` when writable.
Overloads become one method per overload, named `{base}_{arity}`. Constructors
live in a separate `{Name}Constructor` trait so that a subclass is not forced
to inherit its parent's constructor. An `iterable<K, V>` adds one hook,
`fn iterable_entries(&self) -> Result<Vec<(K, V)>, JsError>`, which is the
snapshot the three generated iterator views and the generated `forEach` all
read. Doc comments reproduce the IDL declaration, the fast-API verdict and, for
mixin members, the mixin name.

### V8 glue (rusty_v8 152.x)

Each file emits `install_<snake><T: …Ops>(scope, global_template, registry)`,
which builds the constructor `FunctionTemplate`, wires the parent through
`inherit`, installs constants on both the interface object and the prototype,
accessors, methods, statics, `@@unscopables`, `@@iterator`, `@@toStringTag`,
caches the template for subclasses and calls `register_<snake>`. An interface
with no `constructor` gets a callback that throws
`TypeError: Illegal constructor`.

Every API item is verified against <https://docs.rs/v8/152.2.0/v8/> and listed
in the doc comment of the generated `install_*` function:

- `FunctionTemplate::{builder, new}`, `FunctionBuilder::{signature, length, build, build_fast}`
- `FunctionTemplate::{set_class_name, inherit, prototype_template, instance_template, set_accessor_property}`
- `ObjectTemplate::{new, set_internal_field_count, set_accessor_property, set_indexed_property_handler, set_named_property_handler}`
- `Template::{set, set_with_attr, set_intrinsic_data_property}` with `v8::String::new`
- `v8::Signature::new` — the brand check that makes `unwrap_this` sound
- `v8::FunctionCallbackArguments::{this, get, length, is_construct_call}`, `v8::ReturnValue::{set, set_bool, set_int32, set_uint32, set_double, set_null, set_undefined}`
- `v8::fast_api::{CFunction, CFunctionInfo, CTypeInfo, Flags, Int64Representation, Type, FastApiOneByteString}`
- `Object::{get_aligned_pointer_from_internal_field, set_aligned_pointer_in_internal_field, get, set, create_data_property, internal_field_count, get_own_property_descriptor, get_own_property_names}`
- `v8::GetPropertyNamesArgs`, `v8::PropertyFilter`, `v8::KeyConversionMode`, `v8::WriteFlags`
- `v8::String::{new_from_one_byte, write_v2}`
- `v8::Global`, `v8::Local::new`, `v8::PropertyAttribute::{NONE, READ_ONLY, DONT_ENUM, DONT_DELETE}`
- `v8::Symbol::{get_to_string_tag, get_iterator, get_unscopables}`,
  `v8::Intrinsic::{ArrayProtoEntries, ArrayProtoForEach, ArrayProtoKeys, ArrayProtoValues}`
- `v8::Exception::{type_error, range_error, syntax_error}`, `PinScope::throw_exception`

Notes on the shape of the generated code:

- The host pointer lives in aligned internal field 0 with embedder tag `0`
  (the only tag guaranteed in range in 152); the `v8::Signature` on each
  template is what guarantees the field was written by `attach_host::<T>`.
  Both accessors are wrapped in `unsafe` blocks with that invariant spelled
  out.
- Fast callbacks are `extern "C" fn(receiver: v8::Local<v8::Object>, …)` with
  a matching `CTypeInfo` list. V8's fast C types are 32-bit or wider, so a
  `[Clamp] octet` argument is narrowed inside the fast callback with the
  prelude's `clamp_octet`.
- A value iterator (`iterable<V>`) is installed as the five members the WebIDL
  value-iterator definition names: `entries`, `forEach`, `keys` and `values`
  as `set_intrinsic_data_property` with the matching
  `v8::Intrinsic::ArrayProto*`, and `@@iterator` as
  `v8::Intrinsic::ArrayProtoValues` again. Using the intrinsics is what makes
  each member *the same function object* as `%Array.prototype%`'s, which is
  what the spec asks for; `@@iterator` is therefore also identical to `values`.
  All five are `DONT_ENUM`, and a member the interface declares itself wins.
  The members read the receiver through `length` and the indexed getter on
  every call, so they are live rather than snapshots.
- A pair iterator (`iterable<K, V>`) has no `length` and no indexed getter to
  drive an iterator from, so it cannot use the intrinsics. Instead the `*Ops`
  trait gains one hook, `fn iterable_entries(&self) -> Result<Vec<(K, V)>,
  JsError>`, and the generated `entries`, `keys` and `values` members each
  build a plain array from one call to it and hand that array to the prelude's
  `array_iterator`, which calls the captured `%Array.prototype.values%` on it.
  One hook call feeds all three, so a collection mutated between two of them
  cannot be observed half-way. The returned object is an Array Iterator, not an
  interface-branded one: `Object.prototype.toString.call(box.entries())` is
  `"[object Array Iterator]"`. `@@iterator` is installed from the *same*
  `FunctionTemplate` as `entries`, which is what makes the two the same function
  object as WebIDL requires: V8 instantiates a template once per context, so two
  properties set from it hold one function (asserted by `tenta-js`'s
  `one_template_is_one_function_object_per_context`).
- A pair iterator also declares `forEach`. It is not a view, so it is a fourth
  generated callback rather than an intrinsic: it takes the callback as a
  required argument (its `FunctionTemplate::length` is 1, where the three views
  report 0) and calls it once per entry of one hook call with `(value, key,
  container)`, where `container` is the receiver the member was called on and
  the optional `thisArg` becomes the callback's `this`. The prelude's
  `call_js_function` carries that receiver and rethrows a callback's own
  exception, so the loop stops without replacing the page's error. WebIDL
  defines `forEach` for the pair form, and `wpt/checkout/fetch/api/headers/
  headers-basic.any.js` exercises it on `Headers`.
- Overloaded operations get a dispatcher that switches on `args.length()`.
- `[Replaceable]` setters use `create_data_property`; `[PutForwards=x]`
  setters forward to the target attribute; `[LegacyUnforgeable]` members get
  `READ_ONLY | DONT_DELETE`.

### QuickJS glue (`rquickjs` 0.12)

Verified against <https://docs.rs/rquickjs/0.12.2/rquickjs/> and listed in the
doc comment of the generated class:

- `rquickjs::class::{JsClass, Trace, Tracer, Readable}`, `rquickjs::JsLifetime`
- `rquickjs::Class::{define, prototype, instance}`
- `rquickjs::function::Constructor::new_class`
- `rquickjs::Object::{new, prop, set, set_prototype, into_value}`,
  `rquickjs::object::{Accessor, Property}`
- `rquickjs::function::{Func, This, Opt, Rest}`
- `rquickjs::atom::PredefinedAtom::{SymbolToStringTag, SymbolIterator}`
- `rquickjs::Exception::{throw_type, throw_range, throw_syntax}`
- `rquickjs::{FromJs, IntoJs}`

Each interface gets a `{Name}Class<T>` with a `Trace` impl, a `JsLifetime`
impl (documented `unsafe`: the wrapper borrows no `'js` value), a `JsClass`
impl whose `prototype` installs the members and links the parent prototype
with `set_prototype`, and an `install_{snake}` that defines the constructor on
the global. QuickJS has no fast path, so every member goes through the regular
conversion.

### Registry

`register_<snake>(registry: &mut dyn InterfaceRegistry)` hands the runtime an
`InterfaceDescriptor` with the interface name, parent, exposure, constant,
attribute and operation tables and the property enumeration order. The trait
is one method, `fn register(&mut self, descriptor: InterfaceDescriptor)`, so a
runtime can collect the descriptors without linking against the generator.

## Using the generator from a feature crate (P2)

```rust
// build.rs
fn main() {
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let idl_dir = std::path::Path::new("idl");
    println!("cargo::rerun-if-changed=idl");
    tenta_idl::generate_directory(idl_dir, &out_dir).expect("generate WebIDL bindings");
}
```

```rust
// src/bindings.rs
include!(concat!(env!("OUT_DIR"), "/prelude.rs"));
include!(concat!(env!("OUT_DIR"), "/types.rs"));
include!(concat!(env!("OUT_DIR"), "/node_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/node_register.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/node_v8.rs"));
#[cfg(feature = "quickjs")]
include!(concat!(env!("OUT_DIR"), "/node_qjs.rs"));
```

The consuming crate declares `v8` and `quickjs` features next to its `v8` and
`rquickjs` dependencies; with both off the module compiles with no engine
dependency at all. Enable `tenta-idl`'s `format` feature in the build
dependency to run the output through `prettyplease`.

The CLI does the same thing for inspection:

```bash
cargo run -p tenta-idl -- gen --idl idl/toy --out /tmp/generated
```

## Extended attributes

| Attribute | Effect |
|-----------|--------|
| `[Exposed=…]`, `[Exposed=*]` | recorded per definition, emitted into the descriptor |
| `[LegacyUnforgeable]` | installed on the instance with `READ_ONLY \| DONT_DELETE` |
| `[Replaceable]` | the setter defines an own data property on the receiver |
| `[PutForwards=x]` | the setter forwards to `x` of the returned object |
| `[SameObject]`, `[NewObject]`, `[CEReactions]` | recorded and documented on the trait method |
| `[Unscopable]` | added to the prototype's `@@unscopables` object |
| `[Clamp]`, `[EnforceRange]` | select `clamp_*` / `enforce_*` in the argument conversion |
| `[Global]` | recorded on the interface |

Other extended attributes are preserved in `ExtendedAttributes` and reachable
from the model even when no generator acts on them.

## Tests

`cargo nextest run -p tenta-idl`:

- `tests/model.rs` — parsing and lowering against both fixtures: member and
  definition counts, merge order for partials and mixins, prototype chains,
  property enumeration order, overload grouping and naming, typedef
  resolution, union interning, argument modifiers, extended attributes,
  exposure, dictionary flattening, iterables and special operations, and the
  two error paths.
- `tests/codegen.rs` — golden files under `tests/golden/{toy,dom}/`, every
  generated file parsed with `syn` (`full`), a placeholder scan, determinism,
  the emitted file set, and the engine API items each glue file uses. Refresh
  the golden files with `UPDATE_GOLDEN=1 cargo test -p tenta-idl`.
- `tests/compile.rs` — includes the golden prelude, types, `*Ops` traits and
  registry files of both fixtures into two test modules, implements
  `CounterOps` for a real host object and exercises the generated enums,
  dictionaries, unions and registry descriptors.
- `tests/cli.rs` — runs the `tenta-idl` binary and compares its output with
  the library's, including the failure paths.

The golden files assert the *shape* of the emitted code; the behaviour behind
the prelude helpers is covered at runtime by `tenta-js`'s `prelude_helpers`
module (`cargo test -p tenta-js --features v8`), which runs
`sequence_from_v8`, `record_from_v8`, `byte_string_from_v8`/`_to_v8`,
`array_iterator` and `call_js_function` against a real isolate. Nothing in
`idl/` declares a sequence, a record or a `ByteString` yet, so without those
tests the three conversions would only ever be compiled.

Fixtures:

- `idl/toy/Counter.webidl` — the small end-to-end fixture (enum, dictionary,
  mixin, inheritance, constants, an optional argument).
- `idl/toy/PairBox.webidl` — the pair-iterator fixture: `iterable<DOMString,
  DOMString>` on an interface with neither `length` nor an indexed getter,
  which is the shape `Headers` and `URLSearchParams` have and the only one that
  exercises the host hook instead of the `%Array.prototype%` intrinsics.
- `crates/tenta-idl/tests/fixtures/dom-excerpt.webidl` — verbatim excerpts of
  the DOM, HTML, CSSOM, CSSOM View and High Resolution Time specifications
  (`EventTarget`, `Event`, `AbortSignal`, `Node`, the `ParentNode` mixin,
  `NodeList`, the `CSS` namespace), with the two deviations from the specs
  marked `ADAPTED` in the file.

## Not implemented

Deliberately deferred, because neither fixture nor any P0/P1 interface needs
it; each returns `Error::Unsupported` or is recorded in the model without
generated code:

- `maplike` and `setlike` declarations (`Error::Unsupported`).
- `async iterable` — parsed and stored in `Iterable::is_async`, but no async
  iterator prototype is generated.
- The QuickJS glue generates no iterator members for either form of
  `iterable<>`: `{name}_qjs.rs` installs the class and prototype plumbing and
  nothing else. Only the V8 glue implements them.
- An interface that declares the member `@@iterator` has to be identical to
  itself is not honoured. WebIDL defines `@@iterator` as the same function
  object as `values` (value form) or `entries` (pair form); when the interface
  declares that member instead of taking the generated one, the value form still
  installs `%Array.prototype.values%` as `@@iterator` and the pair form installs
  no `@@iterator` at all, because the declared member's template is scoped to its
  own block and a generated second function would merely behave alike. No
  interface in the corpus declares `entries`, `keys` or `values`, so this is
  latent.
- `FrozenArray` and `ObservableArray` are lowered like `sequence`; neither the
  freezing nor the change-notification semantics are generated.
- The QuickJS arm of a union selects `sequence<T>` with `is_array()` rather
  than with `@@iterator`, because `rquickjs`'s `Vec<T>: FromJs` reads an array
  and has no iterator step of its own. The V8 arm probes `@@iterator` as the
  spec requires.
- Named property getters, setters and deleters are recorded
  (`Interface::named_getter`, `special_operations`) but only the indexed
  getter is installed as a V8 property handler.
- `[LegacyNullToEmptyString]`, `[LegacyLenientThis]` and the other legacy
  extended attributes are kept in the model but change no conversion.

## References

- WebIDL: <https://webidl.spec.whatwg.org/>
- `weedle4`: <https://docs.rs/weedle4/0.4.0/weedle/>
- rusty_v8 152.2.0: <https://docs.rs/v8/152.2.0/v8/>
- `rquickjs` 0.12.2: <https://docs.rs/rquickjs/0.12.2/rquickjs/>
- TentaBrowser spec: `docs/spec/05-javascript.md` §2–§3,
  `docs/spec/02-architecture.md` §2.1
