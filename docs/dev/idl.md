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
- `Type::Buffer(BufferType)` is the buffer-source family: one variant per
  spelling (`ArrayBuffer`, `DataView`, the nine typed arrays, and the
  `ArrayBufferView` and `BufferSource` typedefs), with `BufferType::ALL` as the
  single list of them. One variant per spelling rather than one for the family
  keeps the brand check and the Rust element type per type, which is what the
  conversion needs; `idl_name` and `conversion_suffix` both read it, so the
  emitted call and the prelude's definition of it come from one source.
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
| `ArrayBuffer`, `DataView`, the typed arrays, `ArrayBufferView`, `BufferSource` | `Vec<u8>` | `Vec<u8>` |
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
the V8 and QuickJS conversions can reproduce exactly. Both directions convert
each key through the *declared* key type rather than through a lossy `String`,
so the key type the conversion produces is the one the table's `rust_type`
spelling names. A key type outside `ByteString`, `DOMString` and `USVString` is
not valid WebIDL and is emitted as a `TypeError`, not converted as some other
type; no such declaration exists in `idl/`.

The conversions themselves follow the WebIDL algorithms rather than the shape
of the JavaScript value:

- `sequence<T>`, `FrozenArray<T>` and `ObservableArray<T>` convert through
  `GetMethod(V, @@iterator)` and `GetIterator`, not through an array check, so
  any iterable is a sequence (`new URLSearchParams(new Map([["a", "b"]]))` is
  the caller that needs it). A primitive is a `TypeError`: WebIDL rejects a
  non-object before it looks for the method.
- `record<K, V>` is `es-to-record`: own keys first, then each key's property
  descriptor decides whether it is enumerable, and only then is the value read.
  The descriptor read is observable through a proxy. Each key is then converted
  through `K` — `ConvertByType(key, K)` runs before `ConvertByType(value, V)`,
  and a `ByteString` key is one code unit per byte, so a key above `0xFF` is the
  same `TypeError` there as anywhere else. An integer-like own key is read back
  as its string name, so it is not dropped.
- `ByteString` is converted isomorphically: one code unit per byte, through the
  one-byte string APIs, and a code unit above `0xFF` is a `TypeError` rather
  than a truncation to its low byte.
- `USVString` needs no separate conversion here. `String` on the Rust side
  cannot hold a lone surrogate, and V8's `to_rust_string_lossy` maps an unpaired
  surrogate to U+FFFD, which is the substitution WebIDL's scalar-value
  conversion prescribes.
- Each arm of a union carries its own WebIDL test — `is_object()` for the
  object-like arms, `@@iterator` for a sequence, `is_object() && !is_array()`
  for a record, `is_number()` for the numeric arms, and the buffer-source
  predicates below — and the arms are tried in declaration order. The first arm
  with no test of its own is the fallback; declaring one earlier cannot shadow a
  later arm of the same kind.
- A buffer source is converted by its bytes, and it maps to `Vec<u8>` in *both*
  positions. Reading one always allocates: the bytes have to be copied out of
  the engine's own storage before the host may look at them, so a borrowed
  argument form would save the move and nothing else. `sequence`,
  `FrozenArray`, `ObservableArray` and `record` already map `Vec<T>` in both
  positions, and buffers following them keeps one rule instead of adding a
  third borrow case (`ByteString` is the only container with a borrowed
  argument form). The cost at the call site is therefore one allocation and one
  copy per inbound boundary crossing, and one allocation and one copy per
  outbound one.
- A view (`DataView` and the nine typed arrays) is read as the
  `byteOffset..byteOffset + byteLength` range it covers, not as its whole
  backing `ArrayBuffer`, so `new Uint8Array(buf, 2, 3)` reads three bytes —
  the range WebIDL's *get a copy of the bytes held by the buffer source* names.
  The read is `ArrayBufferView::copy_contents` over the view's own range, and its
  return value is compared with `debug_assert_eq!` rather than used as the
  length, so a view whose byte length is reported wrongly cannot quietly
  truncate a conversion.
- A detached buffer, a detached view and a zero-length one all read as the
  empty sequence and are **not** an error: V8 reports their byte length as 0,
  and the prelude short-circuits on that before it touches a backing store.
  The standard's algorithm says so outright — *if `IsDetachedBuffer` is true,
  return the empty byte sequence* — and
  `encoding/textdecoder-arguments.any.js` observes it from a page by handing
  `TextDecoder.decode` a detached buffer.
- The copy is eager, and that is observable in exactly one place in the corpus.
  `encoding/textdecoder-arguments.any.js` ends with a 10000-byte input and an
  options dictionary whose `stream` getter detaches the input's buffer, and
  expects `''`: WebIDL's buffer-source conversion only *references* the value,
  and the bytes are not read until the operation consumes them, which is after
  the whole argument list has been converted. This prelude copies during the
  conversion, so `TextDecoder.decode` returns the 10000 bytes it already holds.
  The deviation is recorded in `idl/encoding/TextDecoder.webidl` and pinned by
  `tenta-js-v8`'s
  `a_getter_that_detaches_the_input_runs_after_the_bytes_are_copied`; closing it
  means giving the conversion a borrowing `BufferSource` representation, which
  is the representation change the bullet above declines.
- `BufferSource` accepts an `ArrayBufferView` over a `SharedArrayBuffer` and
  refuses a bare `SharedArrayBuffer`. The split is in the declaration, not in
  the conversion: `AllowSharedBufferSource` — the typedef the Encoding Standard
  actually names, annotated `[AllowShared]` — cannot be written at all, because
  weedle 0.4 has no `SharedArrayBuffer` keyword (it is not in
  `weedle4::types::Type`), so the spelling would fall through to the typedef
  resolver and become an uninhabited `Type::Interface("SharedArrayBuffer")`. The
  consequences are spelled out in `idl/encoding/TextDecoder.webidl`: a view over
  a `SharedArrayBuffer` converts (the brand check is on the view's class, which
  is what `encoding/sharedarraybuffer.https.html` asserts), a bare one is a
  `TypeError` — which `encoding/textdecoder-copy.any.js` is the only file in the
  corpus to observe, because it passes `createBuffer(...)` itself to `decode`;
  `encoding/textdecoder-streaming.any.js` wraps every buffer in a `Uint8Array`
  and so sees nothing of this decision. Both obtain the constructor from
  `new WebAssembly.Memory({shared: true})`, which is not lowered here, so
  neither file runs in this engine regardless. Both cases are pinned by
  `tenta-js-v8`'s `decode_rejects_a_value_that_is_not_a_buffer_source`.
- The brand check is per type: `Value::is_data_view`, `Value::is_uint8_array` and
  their siblings for the named view types, and
  `Local::<ArrayBuffer>::try_from` for `ArrayBuffer`. So a `Uint8ClampedArray`
  is not a `Uint8Array`, a `Float64Array` is not a `DataView`, and no
  `ArrayBufferView` satisfies `ArrayBuffer`. A value that is not the named type
  is a `TypeError`, as WebIDL's buffer-source conversion requires. The two
  typedefs are the union of their members: `ArrayBufferView` accepts any of the
  ten view types, `BufferSource` accepts those plus `ArrayBuffer` itself.
- `BigInt64Array`, `BigUint64Array` and `Float16Array` have no weedle keyword
  arm, so `idl/` still cannot declare them and they stay opaque identifiers.
  A value of one of those types does *not* fail as a `BufferSource` or
  `ArrayBufferView`: it is an `ArrayBufferView`, and the runtime conversions
  accept it. Only the two BigInt spellings cannot be named in a declaration.
- Handing bytes back to script produces a native V8 typed array or `DataView`
  over a fresh `ArrayBuffer` of its own, never a host wrapper with an internal
  field: `Object.prototype.toString.call(...)` is `[object Uint8Array]` and
  `ArrayBuffer.isView(...)` is true, which is what the platform's own classes
  report and what script branches on. A multi-byte typed array whose byte
  length is not a whole number of elements is a `RangeError`; the length it
  reports is `bytes.len() / element_size`. That case is a precondition in the
  standard rather than an exception it names — *create one of the
  ArrayBufferView types from a byte sequence* opens with an **assert** that the
  length is a multiple of the element size, and only then calls the typed
  array's constructor with `!`, the constructor being what throws a
  `RangeError` — so the explicit check exists to turn a host-side mistake into
  a diagnosable error rather than a violated assertion. `DataView` is exempt,
  as the standard's assert is.
- The write path allocates the `ArrayBuffer` and copies with
  `std::ptr::copy_nonoverlapping` into its `data()`, rather than using
  `new_backing_store_from_vec`, which is compiled out when the Rust `v8`
  crate's `v8_enable_sandbox` feature is on. The copy is what makes the
  conversion independent of which crate features the host builds with.
- A unit's own `typedef (ArrayBufferView or ArrayBuffer) BufferSource;` never
  reaches the model: weedle's keyword arms are matched before its `Identifier`
  arm, so the spelling *is* the type and no typedef lookup happens. The
  shadowing is harmless because the two agree, and it is why `BufferSource`
  carries its own model variant whose conversions are written by hand.
- The QuickJS glue does not use these conversions at all: it never names
  `convert.rs`, and it hands a buffer source to `rquickjs` as the `Vec<u8>` the
  type maps to. `rquickjs`'s own `FromJs`/`IntoJs` for `Vec<u8>` then reads and
  writes a JavaScript `Array` of numbers, not a typed array or an
  `ArrayBuffer`. The gap is real but dormant: it is a shape difference rather
  than a compile error, the `quickjs` feature has no `rquickjs` dependency to
  build against (its `cargo` feature is `quickjs = []`), and no `idl/`
  declaration names a buffer source. A QuickJS host that needed one would need
  the byte conversions written for it.
- A union with a buffer source member carries its own test too, because an arm
  with none is the union's fallback: the shape is
  `typedef (BufferSource or USVString) BodyInit;`, and without the test the
  buffer arm is dropped and the argument is stringified through the `USVString`
  arm instead of being read as bytes. The view types are tested per class —
  WebIDL selects the `Uint8Array` arm only for an object whose
  `[[TypedArrayName]]` is `"Uint8Array"` — `ArrayBufferView` is
  `is_array_buffer_view()`, and `ArrayBuffer` and `BufferSource` admit a
  `SharedArrayBuffer`, because WebIDL's `ArrayBuffer` type *is* the
  `[[ArrayBufferData]]` internal slot a shared buffer also has. That term does
  not soften the conversion, which still refuses a bare `SharedArrayBuffer` with
  a `TypeError`; it is what turns that refusal into a thrown exception instead
  of a `"[object SharedArrayBuffer]"` body. The QuickJS arm is `is_object()`:
  every buffer source is an object, and the feature has no `rquickjs` to check a
  narrower predicate against. `idl/toy/PayloadBox.webidl` is the fixture for
  the `BufferSource` arm; the twelve others are pinned by
  `every_buffer_union_arm_is_guarded_by_the_class_it_converts` rather than by a
  declaration.

Fast-API-eligible members return their value directly; every other member
returns `Result<_, JsError>` so the glue can turn a failure into a JavaScript
exception.

## Generated files

`tenta_idl::generate(&model)` returns, in this order:

| File | Contents |
|------|----------|
| `prelude.rs` | Runtime support: `Handle`, `Promise`, `Callback`, `AnyValue`, `JsError`, the WebIDL integer conversions (`to_*`, `clamp_*`, `enforce_*`), the buffer-source conversions (`*_from_v8` / `*_to_v8` for `ArrayBuffer`, `DataView`, the nine typed arrays and the two typedefs), the registry types, and the engine helpers behind `cfg(feature = "v8")` / `cfg(feature = "quickjs")`. |
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
`fn iterable_entries(&self) -> Result<IterableEntries<K, V>, JsError>`, which is
the snapshot the three generated iterator views and the generated `forEach` all
read. `IterableEntries<K, V>` is the prelude's name for the `Vec<(K, V)>` it
returns, so the spelling stays short when a pair is itself a container: an
`iterable<ByteString, ByteString>` is a `Vec` of `(Vec<u8>, Vec<u8>)`. Doc
comments reproduce the IDL declaration, the fast-API verdict and, for
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
- `ArrayBuffer::{new, data, byte_length}`, `ArrayBufferView::{copy_contents, byte_length}`,
  `v8::Local::<v8::ArrayBufferView>::try_from`, `Value::{is_data_view, is_int8_array, …}`,
  and `v8::DataView` / the nine `v8::*Array` types with their `new` and `data`

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
  trait gains one hook, `fn iterable_entries(&self) -> Result<IterableEntries<K,
  V>, JsError>`, and the generated `entries`, `keys` and `values` members each
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

### JS-owned host objects and the promise seam (P2)

An interface whose instances are host records rather than wrapper-world nodes —
`URL`/`URLSearchParams`, `Headers`, `AbortController`/`AbortSignal`,
`TextEncoder`/`TextDecoder`, `Response`, `XMLHttpRequest` — uses the same
generated glue as a node-backed interface, and differs from it in three places
outside the generator:

- the record lives in the host's slab and every wrapper for it carries
  `TAG_HOST`, a tag shared by all of them; the `HostRecord` variant is what
  names the interface, so `HostRecord::Response` and `HostRecord::Headers` are
  distinguishable through one tag. A new record variant must be added to the
  wrapper path in `crates/tenta-js-v8/src/world.rs` and to the reclaim arms in
  `crates/tenta-js/src/bindings/host.rs` as well as to `HostRecord` itself.
- a member that hands a JavaScript value back needs the engine, and the record
  has no isolate. `ScriptHost` (`crates/tenta-js/src/dom/events/mod.rs`) is that
  boundary: `build_value` interns a value and returns bits, `release_value`
  gives the reference back, `throw_value` raises a value that is not a fresh
  `JsError`, and `invoke_listener` runs a callback.
- a member that has to return a promise *before* its work is done — `Body.text()`
  and `Body.json()` are the first two — needs a promise to exist, which the host
  cannot make either. The trait's two promise members are
  `new_promise() -> Option<PromiseRef>` and
  `settle_promise(resolver: u64, outcome: PromiseOutcome<'_>) -> bool`, with
  `PromiseRef { promise: u64, resolver: u64 }`. Both halves of a `PromiseRef`
  carry one stored reference and the *settlement* returns them, not the caller.

`HostValue`, the enum of values the host asks the engine to build, is
`Undefined`, `DomException { name, message }`, `String`, `TypeError { message }`.
The last two arrived with `Response`: `text()` is the first member to resolve a
promise with a string, and `consume body` rejects with a real `TypeError` —
which `JsError::type_error` cannot produce, because it can only *throw*.
`PromiseOutcome` is `Resolve(HostValue)`, `Reject(HostValue)` and
`ParseJson(&[u8])`; the third is the standard's own "parse JSON from bytes"
step, kept as one step because neither half of it is the host's to take: the
UTF-8 decode needs the engine's decoder and the parse produces an arbitrary
object graph. The V8 implementation (`crates/tenta-js-v8/src/promise.rs`) builds
the settlement's value at `settle_promise` time — a scope is available then —
stores the pointer-free job in a slot table, and enqueues a
`v8::Function` whose *function data* is the job's index into that table with
`Scope::enqueue_microtask`. V8's microtask queue is the page-visible one
(`queueMicrotask` is the same call), so a settlement requested from inside a
microtask lands in the same checkpoint pass; an immediate `resolve` would not
be wrong, but a settlement drained once before
`perform_microtask_checkpoint` would. The queued jobs hold
`v8::Global<PromiseResolver>` handles, so `Engine::Drop` clears the table while
the isolate is still alive.

`Response`'s `BodyInit` is the first *shipped* union with a buffer-source arm,
and its arm order is load-bearing: the union's first arm with no test of its own
becomes the fallback, so `(BufferSource or USVString)` converts by
`is_array_buffer() || is_shared_array_buffer() || is_array_buffer_view()` and
falls back to `USVString` (`ToString`) for everything else. Only three of the
standard's six arm types exist here — `ReadableStream`, `Blob` and `FormData` do
not — and dropping `URLSearchParams` costs nothing observable, because it is an
object with no buffer-source shape, so it reaches the `USVString` arm whose
conversion calls its stringifier, which for `URLSearchParams` *is* its
form-encoded serialization. The union is declared in
`idl/fetch/Response.webidl`, whose header records what the interface leaves out.

`Headers`' guard is part of the same slice. `HeadersGuard`
(`crates/tenta-js/src/bindings/host.rs`) now has `None` and `Response`, split
into the question of whether a mutator may be attempted at all
(`permits_mutation`) and whether it may change *this* name (`permits_name`); the
`"response"` guard rejects `Set-Cookie`/`Set-Cookie2` silently, as Fetch's
*validate a header* step does — not with a throw. The name-level rule lives
outside the generator, but the *fill* step a constructor runs goes through the
same helper, so `new Response(null, {headers: {"Set-Cookie": "a=b"}})` and
`new Headers({"Set-Cookie": "a=b"})` differ in exactly the guard they install.

### The `fetch()` delivery seam (P2 slice 4a-1a)

`fetch` is the first *global function*, so it is hand-written
(`crates/tenta-js-v8/src/host.rs`, installed by `install` beside
`requestAnimationFrame`) rather than generated: the global object is the window,
and no generated glue serves it. It creates the promise before it resolves the
URL or looks for a driver, so everything from that point on is a rejection
rather than an abrupt completion — which is the Fetch Standard's own shape.

The seam between the page and a network driver is `tenta_js::fetch`:
`FetchRequest` (resolved URL, method, headers, body, initiator, provenance),
`FetchOutcome::{Response, Failed}`, `FetchedResponse`, `FetchCompletion`,
the `FetchDriver` trait, the `FetchCompletions` table and `FetchStartError`.
A driver holds no engine and a host record holds no isolate, so the two meet
over owned data and `resolver: u64` stays opaque bits for the same layering
reason `PromiseRef` does. `PageEngine::set_fetch_driver` installs one driver per
navigation and `PageEngine::settle_fetches` applies a batch of completions;
`tenta_page::runtime`'s `turn` drains `FetchCompletions` into the latter before
it pumps timers, and since the loop's `select!` is its only await point, a
completion a task pushes is always drained by the next turn. The settlement is
queued as a microtask (never applied where it is handed over), so the drain sets
`microtasks_pending` and `next_wakeup` then reports zero.

Deliberate deviations in this slice, each with the slice that removes it:

- `RequestInfo`'s first arm is not implemented: `Request` is not a global, so the
  argument is always read through the `USVString` arm. That arm's conversion is
  lossy rather than checked, which makes two inputs *reject* where the standard
  throws a `TypeError` before the body runs — `fetch()` with no argument
  (stringified to `"undefined"`) and `fetch({})` (stringified to
  `"[object Object]"`), both of which then fail URL resolution. The difference is
  a synchronous throw versus a rejected promise, so a page wrapping the call in
  `try`/`catch` sees it differently. `Request` arrives with the request-body
  work.
- `RequestInit` (slice b-6.5) is the Fetch Standard's dictionary, declared in
  full in `idl/fetch/Request.webidl`, with **three of its fifteen members
  served** — `method`, `headers` and `body` — and the other twelve refused *by
  name* (`"the RequestInit member 'signal' is not implemented"`,
  `tenta_js::bindings::host::refused_request_init_member`). Two of the twelve
  are worth naming on their own: `signal` is not waiting on the interface —
  `AbortSignal` landed with `d-3` — but on the plumbing, because the driver
  would have to take a signal handle and abort the request it is already
  running, which is `4a-abort`; and `credentials`/`mode`/`cache`/`redirect` are
  CORS and redirect policy this engine has not built.
  Three deviations this slice makes from the *old* refusal, each deliberate:
  (1) a key the dictionary does not name is **ignored**, because that is what a
  dictionary conversion does, where the refusal refused every key;
  (2) a member is read with `Get`, so an inherited or non-enumerable property
  *is* a member, where the refusal enumerated own enumerable keys only
  (<https://webidl.spec.whatwg.org/#js-dictionary>); and (3) `method`,
  `headers` and `body` are no longer refused, so `fetch(url, {method: "POST"})`
  is a `POST`.
  Deviations that remain, each deliberate. (a) A member whose value the declared
  type rejects throws that type's own `TypeError` before the refusal runs
  (`{credentials: "bogus"}`) — WebIDL's order, and what a browser does. (b) Every
  throw on the argument-conversion path is a synchronous throw where a browser
  *rejects*: the standard runs the `Request` constructor inside `fetch()`'s
  promise and rejects with whatever it throws, and this engine has no `Request`
  to construct. (c) The refusal is by **presence, not value**, so it
  over-refuses: a page that writes out the standard's default explicitly
  (`{keepalive: false}`, `{signal: null}`, `{window: null}`, `{integrity: ""}`,
  `{cache: "default"}`) asks for exactly the request this engine would have sent
  and is refused where a browser proceeds. That is the deliberately chosen error
  direction — a wrong acceptance sends a request the page did not ask for, a
  wrong refusal says so (`tenta_js::bindings::host::refused_request_init_member`).
- With no driver installed, `fetch` rejects with
  `TypeError: fetch is not available` instead of throwing or leaving the promise
  pending. A page that has loaded always has a driver once the `tenta-net`
  driver lands (`4a-1b`).
- A fetched response's `statusText` is derived from the status
  (`http::StatusCode::canonical_reason()`), not taken from the wire; a status
  with no canonical reason gives the empty string. Reason phrases as bytes, and
  the standard's rule that a reason phrase is not a status, arrive with the
  driver.
- `insert_fetched_response` drops `Set-Cookie`/`Set-Cookie2` before the header
  record exists, and corrects the body of a null-body status (101/103/204/205/
  304) to `None`. Both are the Fetch rules applied at the boundary, so a driver
  cannot get them wrong.
- A response's `url` and `redirected` are the driver's report; no redirect is
  followed in the engine layer (the driver's transport is what follows them —
  see the next section), and CORS and `Response.type` other than `basic` are
  absent. The forbidden request-header names are **not** absent, and an earlier
  revision of this bullet said they were: `is_forbidden_request_header` is
  applied on two paths — through `HeadersGuard::Request` for `RequestInit`'s
  `headers` (slice b-6.5), and directly by `XMLHttpRequest.setRequestHeader`
  (b-5a3), which predates the guard — and the rule is the standard's, which is
  why `x-http-method-override` is dropped only alongside a forbidden *method*
  (`crates/tenta-js/src/bindings/host.rs`).

### The network driver behind `fetch()` (P2 slice 4a-1b)

`crates/tenta-page/src/fetch.rs` holds `NetFetchDriver`, the one implementation
of `tenta_js::fetch::FetchDriver` in the workspace. It lives in `tenta-page`
because that is the only crate that can see `tenta-net`, the seam, the
`SettleTracker` and `page::next_request_id` at once while keeping the
dependency direction: a driver holds no engine, and neither `tenta-js` nor
`tenta-js-v8` gains an edge to the *driver*. (The V8 backend does name
`tenta-net` since d-4b, which holds the type in a field and a parameter and no
cookie logic — see the next section.) `PageRuntime::create` builds
one per navigation and installs it with `PageEngine::set_fetch_driver`, so a
completion always names a resolver of the engine current when the page drains
the table.

A request maps to a `Fetch`-resource `tenta-net` `Request`: priority `Medium`,
mode `Cors`, `initiator` set to the document URL (which supplies
`Sec-Fetch-Site`, `Origin` and the default `Referer`), `first_party` from the
document rather than the request URL, and the page's own `extra_headers`
appended after the request's. `limits.total` is deliberately left unset so
`ClientConfig::request_timeout` applies: there is no defensible per-request
deadline here, because `Goto::timeout` is the navigation's and
`settle.foreground_request_max` is the settle machine's *reclassification*
bound, and using the latter would fail legitimately slow fetches. The task is
bounded by the driver's `Drop` instead.

Settlement depends on one ordering the driver owns: the completion is **pushed
into `FetchCompletions` first, then `note_request_done`**. The push alone is
invisible until a turn drains the table; the wake is what makes the loop take
that turn, because `note_request_done` signals the notification the loop's
`select!` parks on. Neither can be lost in between — the page thread is a
single-threaded `LocalSet` whose `select!` is its only await point, so the task
runs only while the loop is parked and `notify_one` leaves a permit behind.
Removing the wake does not hang: the request stays registered until
`record.started + foreground_request_max` (`crates/tenta-page/src/settle.rs:592`,
five seconds for `SettleConfig::library()`), is reclassified as background, and
the navigation reports `Settled { busy: true }` about five seconds later. That
distinction is what the end-to-end test asserts on.

Cross-origin is refused at **two** gates, and both are required:

1. **Before anything is registered or sent**, in `start`. A URL that is not
   same-origin (RFC 6454, `same_origin` in `tenta-net`, compared against
   `FetchRequest.initiator` — the document — and never against
   `FetchRequest.origin`, which is task provenance) becomes
   `Err(FetchStartError)`, which the seam turns into a `TypeError` *rejection*:
   no synchronous throw, no network, no settle registration.
2. **On the final URL, before the body is read.** `Client::send` follows
   redirects internally, so a same-origin URL can redirect off-origin and the
   transport would then hand script a cross-origin body and header list. Gate 1
   alone would leave that leak behind the refusal it performs.

Neither gate is CORS, and there is no CORS or preflight code anywhere in the
workspace, so a page can only fetch its own origin. Keeping the refusal
engine-authored and stable means a page can branch on "the promise rejected
with a `TypeError`" and not on this engine's internal names.

Deliberate deviations in this slice, each with the slice that removes it:

- Because the seam delivers head and body together, **a body that fails
  mid-stream** — connection reset, `max_body_bytes` exceeded — **rejects the
  whole `fetch()`**, whereas the standard resolves a `Response` whose `text()`
  later rejects. Streaming `Response.body` (`d-5b`) removes this; the seam's
  `body: Option<Vec<u8>>` is frozen until then.
- Every transport failure rejects with one fixed message, `Failed to fetch`,
  mirroring Fetch's own network error. The `NetError` detail
  (`error.error_text()`, `error.message`) goes to `tracing` and never reaches
  script: `net::ERR_*` is this project's event vocabulary, not a stable script
  API.
- A header that cannot go on the wire — reachable through the initializer's
  `headers` (b-6.5) or through `PageConfig::extra_headers` — is a *start*
  refusal rather than a `Failed` outcome: the request cannot be built at all,
  so there is nothing to send and nothing to wait for.
- The request's body is sent as `Body::Bytes` and **`Content-Length` is left to
  the transport**: `headers::build` derives it from the body's own length and
  inserts it only when the caller has not, so a second one would be a
  smuggling-shaped bug rather than a preference. A page cannot reach that
  parameter with one anyway — the initializer's header list is filled under the
  `"request"` guard, which drops the forbidden request-header names.
- Response headers are passed through untouched. `insert_fetched_response`
  already drops the forbidden names and the NUL/CR/LF values and corrects
  null-body statuses, so a driver that filtered too would be a second, drifting
  copy of the rule. `as_bytes()` is used, never `to_str()`, so an obs-text
  header value survives rather than silently blanking.
- There is no `AbortSignal`-driven cancellation. Cancellation exists only as
  `Drop`, which aborts the in-flight handles.
- There is no per-hop event for a script fetch.

**Under `Settle::Load` a page's `fetch()` never completes, and this is
product-visible.** `Settle::Load` is `Goto::default().wait_until`
(`crates/tenta-page/src/page.rs:105`), and its machine is
`event_state(self.load_fired)` (`crates/tenta-page/src/settle.rs:701`), which
reads `Settled` purely from that flag and ignores open requests. So the loop
returns the moment `load` fires, settling tears the engine and the driver down,
`Drop` aborts the fetch, and the promise can never settle. Both in-tree callers
name it explicitly — `crates/tentabrowser-cli/src/scrape.rs:519` and
`crates/tenta-browser/src/context.rs:481` — but both open their pages with
`PageOptions::tier1()`, so no script runs and the limitation is not reachable
through them today. It is reachable through the library API, where
`Goto::default()` is the default rather than a choice. **A caller that wants a
page's `fetch` to land must ask for `Settle::NetworkIdle`.** `NetworkIdle`'s
machine settles as soon as
`load_fired && network_sources_empty() && tracked_tasks == 0`; it does not use
the 500 ms `network_idle_window`, so the loop waits for the fetch rather than
for a quiet period.

### The cookie jar behind `document.cookie` (P2 slice d-4b b-1)

`attribute USVString cookie` on `Document` (`idl/dom/dom.webidl`) was the last
framework-visible IDL gap in the window tests. `Document` is a node-backed
interface, so the member needs no new seam and no new host variant: the
generated `DocumentOps::cookie` / `set_cookie` are implemented on `HostObject`
(`crates/tenta-js/src/bindings/host.rs`), which reaches the jar through the
`HostContext` every host already carries.

Both directions are the HTML Standard's cookie getter and setter
(<https://html.spec.whatwg.org/multipage/dom.html#dom-document-cookie>), which
delegate to RFC 6265bis's "cookie-string" and "receive a set-cookie-string" with
the **non-HTTP API flag** set. That flag is one argument on each call and it is
the whole security boundary of the member:

- The getter calls `CookieJar::cookie_header(url, site, is_http = false, ..)`.
  `cookies_for`'s `is_http || !cookie.http_only` filter is what hides every
  `HttpOnly` cookie from script. The join (`name=value`, `"; "` separated) and
  the ordering (RFC 6265 §5.4: longer path first, then earlier creation time)
  are `tenta-net`'s and are deliberately not re-implemented.
- The setter calls `CookieJar::set_cookie(value, url, is_http = false, ..)`.
  `CookieJar::store` returns `RejectedHttpOnly` for `!is_http && (cookie.http_only
  || existing.http_only)` (RFC 6265bis §5.7 steps 15 and 21), so a script can
  neither create nor overwrite an `HttpOnly` cookie. The whole assignment —
  including the parser that splits at the first `;` into the name-value pair and
  the attribute list — is delegated, so `document.cookie = 'a=b; Path=/sub'`
  stores `a` with path `/sub`. The `Option<StoreOutcome>` is discarded on
  purpose: every refusal path is silent in the standard, and surfacing one would
  invent an error a real browser does not throw.

Two further values the binding chooses rather than derives:

- `SameSiteContext::same_site()` on both calls. The site for cookies *is* the
  document's site, so a non-HTTP API acting on the document's own URL makes a
  trivially same-site request; the constant is the value the standard's
  computation yields, not a shortcut.
- The cookie-averse rule. The standard's cookie-averse `Document` is one with a
  null browsing context or a URL whose scheme is neither `http` nor `https`;
  this tree has no null-context case, so `is_cookie_averse_scheme` is the scheme
  test written out rather than inherited from `canonical_host`. The two
  observable consequences are the getter returning `""` and the setter being a
  silent no-op, and both are pinned for the no-URL and `file:` cases.

**Unreachable deviation.** The standard's getter and setter both throw a
`SecurityError` `DOMException` when the origin is opaque. This tree cannot
represent an opaque origin, so neither throw is implemented and neither is
reachable.

**The jar is context-level, and that is what the end-to-end test pins.** The
plumbing chain, so it need not be re-derived:

1. `ContextInner.cookies` (`crates/tenta-browser/src/context.rs`) is the
   context's `CookieJar`, handed to the network client as
   `cookie_jar: Some(cookies.clone())`.
2. `BrowserContext::new_page` clones it into `PageConfig::cookies`
   (`crates/tenta-page/src/page.rs`).
3. `runtime::engine_config` forwards it into `PageEngineConfig::cookies`
   (`crates/tenta-js/src/engine.rs`).
4. `V8EngineFactory::create` passes it to `Engine::with_document_on_clock`,
   which hands it to `HostState::new` and on to `HostContext::new`.

`CookieJar` is `#[derive(Clone)]` over an `Arc`-shared inner, so every link is a
clone of one handle and all of them are the same store. A per-engine copy would
make `document.cookie = ...` invisible to the next request; the end-to-end test
sets a cookie and then `fetch`es `/api/cookie`, which echoes the `Cookie` header
the request carried, so only the shared jar passes. The jar's policy is the
context's policy — this slice adds none.

`tenta-js-v8` therefore names `tenta-net` in `[dependencies]`. The backend holds
the type in a field and a parameter and contains no cookie logic; `tenta-net`
sits below `tenta-js-v8` in `xtask/src/deps.rs`'s foundation order, so
`cargo xtask lint-deps` permits the edge.

The two halves of the test split follow the `Response` precedent:
`crates/tenta-js-v8/tests/dom_cookie.rs` covers the binding against a jar only
Rust can see (the join, the `HttpOnly` filter, the refusal to overwrite, the
attribute parser, the cookie-averse rule, and the "no own property left behind"
regression), and `crates/tenta-browser/tests/cookie_js.rs` is the end-to-end
half, where the cookie has to survive into a real request.

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
module (`cargo nextest run -p tenta-js --features v8 --tests`; the module is
gated on the `v8` feature, so a run without it executes none of these tests),
which runs `sequence_from_v8`, `record_from_v8`, `byte_string_from_v8`/`_to_v8`,
`array_iterator` and `call_js_function` against a real isolate. Those tests are
what reaches a conversion no declaration reaches — nothing in `idl/` declares a
`ByteString` yet — and the failure paths a shipped member cannot produce, such
as an abrupt descriptor read or a failing `done` read. The same module covers
the `TextDecoder` and `TextEncoder` *algorithms*, which need a `HostContext` but
no wrapper world; the half that does need one — installation, branding, the
argument conversions and the typed array `encode` returns — is in
`crates/tenta-js-v8/tests/dom_encoding.rs`. `Response` splits the same way:
`prelude_helpers` covers the algorithms through a recording `ScriptHost`
(the status and status-text validation, the null body status, the `Content-Type`
append, the forbidden-name drop under guard `"response"` and the defaults), and
`crates/tenta-js-v8/tests/dom_response.rs` covers what only a real isolate
shows — the promise actually settling in a microtask checkpoint, the UTF-8
decode, the `SyntaxError` rejection, the `Uint8Array` body taken as bytes, and
the wrapper's reclamation. The `fetch()` seam splits the same way: the table's
push/drain contract is a unit test module in `crates/tenta-js/src/fetch.rs`,
the page layer's drain is `crates/tenta-page/src/runtime.rs`'s test module
(which drives a recording engine, so no isolate is involved), and
`crates/tenta-js-v8/tests/dom_fetch.rs` runs a recording `FetchDriver` against a
real isolate to show the resolved URL, the microtask-ordered settlement, the
`Response` a resolution carries, the null-body status, and both failure shapes —
a rejection rather than a throw.

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
- `idl/encoding/TextDecoder.webidl` and `idl/encoding/TextEncoder.webidl` are
  not fixtures — `tenta-js`'s build script generates them — but they are the
  first shipped declarations to use `interface mixin` with `includes`
  (`TextDecoderCommon`, `TextEncoderCommon`), so they are what exercises step 3
  of the lowering above outside a fixture.

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
- `TextEncoder.encodeInto` and its `TextEncoderEncodeIntoResult` dictionary are
  absent from `idl/encoding/TextEncoder.webidl`. The operation takes a
  `BufferSource` it *writes* into, and a buffer source maps to an owned `Vec<u8>`
  in the argument position, so a generated member has no way to hand the bytes
  it wrote back to the caller's view — the generator has no out-parameter
  mechanism at all. `TextDecoderStream` and `TextEncoderStream` are absent for a
  different reason: they are `TransformStream` subclasses and the Streams
  interfaces have not been lowered yet.
- `Response` declares the part of `Body` a constructor-built response can
  serve — `bodyUsed`, `json()`, `text()` — and leaves out `body`,
  `arrayBuffer()`, `blob()`, `bytes()`, `formData()` and `textStream()`, because
  every one of them reads a `ReadableStream` (or a `Blob`/`FormData` that does
  not exist here). `Response.body` arrives with Streams (`d-5b`). Its static
  `error()`, `redirect()` and `json()` and its `clone()` are absent too:
  `clone()` needs a body tee and the statics need the request and redirect
  machinery. `Response.body`'s stand-in in the record is a `Vec<u8>`, which is
  what makes *unusable* "the body has already been read".
- `XMLHttpRequest` is declared down to the whole of its state machine
  (`UNSENT`, `OPENED`, `HEADERS_RECEIVED`, `LOADING`, `DONE`), the request
  attributes `open()` sets up, and the response the machine stores with its
  accessors (`status`, `statusText`, `getResponseHeader`,
  `getAllResponseHeaders`, `overrideMimeType`, `responseType`, `responseText`)
  and its events. Nothing is sent: a completion is delivered to the object
  through the b-4 seam, and `send()`/`setRequestHeader()` with the driver seam
  arrive next (`b-5a3`), then the `tenta-net` driver that makes either reachable
  from a page (`b-6`). `abort()`, `[SameObject] upload` / `XMLHttpRequestUpload`
  and `responseURL`/`response`/`responseXML` are absent — an inert upload object
  would invite handlers that can never fire, there is nothing in flight to
  abort, and the three response readers need conversions this engine has no
  value for yet. Four narrowings are this engine's own and each refuses
  synchronously: `open(…, async = false)` throws `NotSupportedError` (the driver
  is asynchronous and a blocking request would deadlock the page's turn loop), a
  non-null `username`/`password` throws it (this engine sends no credentialed
  request, and what the network layer does with URL userinfo is unverified),
  `withCredentials = true` throws it (the shared cookie jar decides on its own),
  and `responseType` accepts only `""` and `"text"`. One further deviation is a
  transport limit rather than a narrowing: `statusText` reads back as the status
  code's canonical reason text, because the response has no reason phrase of its
  own to come from. `idl/xhr/XMLHttpRequest.webidl`'s header carries the same
  list beside the members it applies to.
- A *throwing getter* on a dictionary member is treated as an absent member: the
  generated `dictionary_member` maps an empty `Object::get` — the shape a throw
  takes — to `Ok(None)` and the next member lookup then drops the pending
  exception. An exception from the *last* member read does survive to the
  caller. Pre-existing and interface-agnostic (`TextDecoderOptions` behaves the
  same way); the fix belongs in `dictionary_member` and is not made here.

## References

- WebIDL: <https://webidl.spec.whatwg.org/>
- `weedle4`: <https://docs.rs/weedle4/0.4.0/weedle/>
- rusty_v8 152.2.0: <https://docs.rs/v8/152.2.0/v8/>
- `rquickjs` 0.12.2: <https://docs.rs/rquickjs/0.12.2/rquickjs/>
- TentaBrowser spec: `docs/spec/05-javascript.md` §2–§3,
  `docs/spec/02-architecture.md` §2.1
