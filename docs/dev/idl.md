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
- Each arm of a union carries its own WebIDL test — a V8 interface-template brand
  check for interface arms, `@@iterator` for a sequence,
  `is_object() && !is_array()` for a record, `is_number()` for numeric arms, and
  the buffer-source predicates below — and the arms are tried in declaration
  order. The first arm with no test of its own is the fallback; declaring one
  earlier cannot shadow a later arm of the same kind.
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
- The QuickJS glue uses `quickjs_buffer_bytes` and `quickjs_buffer_value` in
  the generated prelude. Input dispatch checks native buffer brands and copies
  the selected view's byte range. DataView accessors and output constructors
  are saved before page script runs, so replacing globals or shadowing view
  properties does not change conversion. `BufferSource` and `ArrayBufferView`
  results use a native `Uint8Array`. Shared buffers remain unavailable until
  the QuickJS cross-runtime sharing hooks and isolation policy are implemented.
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
  of a `"[object SharedArrayBuffer]"` body. The QuickJS arm also checks native
  buffer and view brands; it does not accept an ordinary object just because
  its prototype resembles a buffer. `idl/toy/PayloadBox.webidl` is the fixture for
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

One exception: an interface carrying `[HTMLConstructor]` gets a callback that
calls `v8_html_constructor`, which asks the world
(`V8CallbackAdmission::html_constructor`) what construction returns and throws
`Illegal constructor` if the world has no answer. HTML writes the attribute on
a `constructor()` member; here it stands on the interface, because there is no
host constructor operation to generate: the result depends on the page's
custom element definitions and on `new.target`, not on the interface.
`HTMLElement` is the one interface that carries it, so `class X extends
HTMLElement` can call `super()`. The QuickJS glue ignores the attribute and
throws.

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
  cannot be observed half-way.
  **The consequence is that a mutation *during* one iteration is not observed
  either: the generated iterator is a snapshot, where WebIDL's pair iterator
  prototype object wraps the live list.** The whole result of
  `iterable_entries()` is materialized into an array before the first entry is
  yielded, and every subsequent `next()` reads that array, so a `delete` further
  down the list does not skip an entry — the deleted pair is still yielded —
  and an `append` is never reached, where
  <https://webidl.spec.whatwg.org/#es-iterator-prototype-object> steps 4.6–4.7
  require the iterator to re-read the underlying list's index on each step and
  its `[[ArrayLike]]` to be the list itself.
  `wpt/checkout/xhr/formdata/iteration.any.js` is the operational contract for
  the three behaviors a snapshot cannot produce, and all three fail here:
  "Iteration skips elements removed while iterating", "Removing elements already
  iterated over causes an element to be skipped during iteration", and
  "Appending a value pair during iteration causes it to be reached during
  iteration". This is not `FormData`'s: `Headers` and `URLSearchParams` carry the
  same snapshot, and the same three behaviors are pinned for them by
  `wpt/checkout/fetch/api/headers/headers-basic.any.js` and by the
  `urlsearchparams-*` files. Making it live means one live iterator object per
  pair `iterable` in `crates/tenta-idl/src/codegen/v8.rs`, which is a slice of
  its own; until then the snapshot is deliberate and
  `crates/tenta-js-v8/tests/dom_formdata.rs`'s
  `iteration_views_a_snapshot_so_a_mutation_during_it_is_not_observed` pins it,
  so the live-iterator slice has to come back and flip that test. The returned object is an Array Iterator, not an
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

The runtime pins the APIs documented at
<https://docs.rs/rquickjs/0.12.2/rquickjs/>:

- `rquickjs::class::{JsClass, Trace, Tracer, Readable}`, `rquickjs::JsLifetime`
- `rquickjs::Class::instance`
- `rquickjs::function::Constructor::new_prototype`
- `rquickjs::Object::{new, prop, set, set_prototype, into_value}`,
  `rquickjs::object::{Accessor, Property}`
- `rquickjs::function::{Func, This, Opt, Rest}`
- `rquickjs::atom::PredefinedAtom::{SymbolToStringTag, SymbolIterator}`
- `rquickjs::Exception::{throw_type, throw_range, throw_syntax}`
- `rquickjs::{FromJs, IntoJs}`

Every interface has an `install_{snake}` function that creates its native
prototype, links the parent prototype, and installs accessors, methods and its
constructor. Ordinary wrappers use `QuickJsHost`; interfaces with indexed or
named special operations use the native `LegacyHost` exotic class. Both carry
immutable interface ancestry separately from script-visible prototypes. An inherited
method therefore accepts a derived wrapper while rejecting forged receivers.
QuickJS has no V8 fast-call path; every member uses the regular conversion.

Generated callbacks and handwritten globals enter `quickjs_callback_boundary`.
Native panics set sticky runtime failure and throw an uncatchable error; both
the outer callback and the interrupt handler observe that failure. A class
finalizer catches native destructor panics and records generation-tagged dead
wrappers without borrowing the DOM during collection. The runtime applies
reclamation at safe points and releases all roots and minted document contexts
before teardown.

`cargo nextest run -p tenta-js-qjs` exercises the generated DOM bindings in a
real engine. Indexed/named property hooks have focused collection and Storage
coverage. Prototypes are currently installed eagerly; lazy interfaces, full
overload resolution and the broader WebIDL conformance gates remain outstanding.

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
`TextEncoder`/`TextDecoder`, `Response`, `XMLHttpRequest`, `Blob`/`File`,
`FormData` — uses
the same generated glue as a node-backed interface, and differs from it in three
places outside the generator:

- the record lives in the host's slab and every wrapper for it carries
  `TAG_HOST`, a tag shared by all of them; the `HostRecord` variant is what
  names the interface, so `HostRecord::Response` and `HostRecord::Headers` are
  distinguishable through one tag. A new record variant has to be added
  everywhere the existing ones are matched exhaustively, which the compiler
  enumerates rather than this paragraph: `HostRecord` itself, `interface_name()`,
  both `host_pairs` arms, the `HostLink` enum and its `reclaim_host_record` arm
  (all `crates/tenta-js/src/bindings/host.rs`), and the wrapper path in
  `crates/tenta-js-v8/src/world.rs`. An earlier revision named three of those and
  left the rest to the compiler; the compiler does catch them, so the cost is a
  broken build rather than a wrong record, but a slice's author should not have
  to discover the list. `Blob`/`File` also needed a `with_*` reader on
  `HostObject` — one per shape a member reads — which is a design choice rather
  than a site the compiler forces.
- **A record the host *owns* needs an owner list, not just `has_wrapper`.**
  `Response` and its `Headers` were the first pair where one host record holds
  another's key, and the protocol was: the wrapper is the owner, so a record is
  freed when its wrapper dies. `FormData` breaks that assumption — the entry
  list holds a `File` that the page *also* holds, so the record must outlive
  both, and either may die first. `FileRecord` therefore carries two independent
  facts (`crates/tenta-js/src/bindings/host.rs`): `has_wrapper: bool`, set when
  a wrapper is created for it and cleared only when that wrapper is finalized,
  and `owners: Vec<HostKey>`, the records that hold the key in an entry list.
  `FileRecord::is_free()` is `!has_wrapper && owners.is_empty()` and is the only
  reclamation test. `owners` is a `Vec`, not an `Option<HostKey>`, because a
  file may sit in two `FormData`s at once, and it is a *set* (insertion
  idempotent) because one list may hold the same file twice under one name.
  `HostLink::File` carries the owner list and `HostLink::FormData` carries the
  files its entry list references, so the two records can be reclaimed in either
  order: a dying `FormData` frees itself and then drops its ownership from each
  file (freeing any that has no wrapper and no other owner), and a dying `File`
  wrapper clears `has_wrapper` and frees itself only if nothing still owns it.
  Every mutation point is a reclamation point — `delete`, `set`'s
  replace-and-remove-others, and the entry list's own death — and
  `HostObject::mutate_form_data` is the single function that computes the before
  and after key sets and does both halves, so a new mutating member cannot
  forget one.
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
becomes the fallback. It is `XMLHttpRequestBodyInit` — `(Blob or BufferSource or
FormData or URLSearchParams or USVString)` — so the three interfaces are selected
by brand, a buffer source by `is_array_buffer() || is_shared_array_buffer() ||
is_array_buffer_view()`, and everything else falls back to `USVString`
(`ToString`). The standard's sixth arm, `ReadableStream`, does not exist in this
repository. Each arm's bytes and the `Content-Type` it infers are Fetch's
*extract a body with type*, in one function (`extract_body`,
`crates/tenta-js/src/bindings/host.rs`) that the `Response` constructor,
`fetch()`'s initializer and `XMLHttpRequest.send()` all call: a `Blob` gives its
bytes and its `type`, a `FormData` the `multipart/form-data` encoding with a
boundary made for that body, a `URLSearchParams` its serialization as
`application/x-www-form-urlencoded;charset=UTF-8`, a string its UTF-8 as
`text/plain;charset=UTF-8`, and a buffer source its bytes and no type.

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

- `RequestInfo` is `(Request or USVString)`: a `Request` is taken by brand and
  fetched as it was made, with `init` applied over it, and anything else is
  read through the `USVString` arm. That arm's conversion is lossy rather than
  checked, which makes two inputs *reject* where the standard throws a
  `TypeError` before the body runs — `fetch()` with no argument (stringified to
  `"undefined"`) and `fetch({})` (stringified to `"[object Object]"`), both of
  which then fail URL resolution. The difference is a synchronous throw versus a
  rejected promise, so a page wrapping the call in `try`/`catch` sees it
  differently. `Request` itself is described in `idl/fetch/Request.webidl`'s
  header: its `signal` is the signal it was given rather than one that follows
  it, and its body is a byte sequence, so `clone()` copies.
- `RequestInit` (slice b-6.5, widened by CO1 on 2026-11-16) is the Fetch
  Standard's dictionary, declared in full in `idl/fetch/Request.webidl`.
  Served: `method`, `headers`, `body`; `credentials` in all three modes and
  `mode` as `cors`, `same-origin` or `no-cors` (the driver runs the CORS
  protocol, see the next section; a `no-cors` request to another origin
  resolves with an opaque `Response`, and only a simple method and
  form-safe headers go out); `redirect` as `follow` or `error`; `cache` as `default`,
  or as `no-store`, `reload` and `no-cache`, which all go to the network;
  `signal` on V8 (aborting rejects the promise with the signal's reason and
  ends the request; QuickJS refuses a non-null `signal`). `keepalive`,
  `priority` and `duplex` are accepted with any value: the first only matters
  to a request that outlives its page, the second is a hint, and the third has
  one value. A value that would send a different request than the page wrote
  is still refused *by name*
  (`"the RequestInit member 'mode' is not implemented"`,
  `tenta_js::bindings::host::refused_request_init_member`): `mode`
  `navigate`, `redirect: "manual"`, `cache`
  `force-cache` and `only-if-cached`, a non-empty `integrity`, a `referrer`
  other than `about:client`, a `referrerPolicy` other than the default, a
  non-null `window`.
  Three deviations the slice made from the *old* refusal, each deliberate:
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
  promise and rejects with whatever it throws, while the hand-written `fetch`
  converts its two arguments before the promise exists. (c) **Changed by CO1, and a decision to record:** the refusal
  was by presence, so a page that wrote out a default (`{signal: null}`,
  `{cache: "default"}`) or named a mode (`{credentials: "include"}`) was
  refused where a browser proceeds. It is now by value: a member is accepted
  when the engine sends the request the page wrote. The earlier error
  direction — refuse rather than risk sending something else — still governs
  every value the engine cannot honour.
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
  see the next section), and `Response.type` other than `basic` is
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

Requests to another origin go through the CORS protocol (CO1, 2026-11-16;
`crates/tenta-page/src/cors.rs` holds the decisions and
`crates/tenta-page/src/fetch.rs` makes the requests). `fetch()` and
`XMLHttpRequest` share it:

1. **Before anything is registered or sent**, in `start`. A URL that is not
   `http:` or `https:`, and a cross-origin URL in `same-origin` mode, become
   `Err(FetchStartError)`, which the seam turns into a `TypeError` *rejection*
   (the *request error steps* for XHR): no network, no settle registration.
   "Cross-origin" is RFC 6454's (`same_origin` in `tenta-net`), compared
   against `FetchRequest.initiator` — the document — and never against
   `FetchRequest.origin`, which is task provenance.
2. **The preflight.** A cross-origin request whose method is not `GET`,
   `HEAD` or `POST`, or which carries an author header that is not
   CORS-safelisted, is preceded by an `OPTIONS` request naming the method and
   the header names. The real request is sent only when the answer has an ok
   status and allows the origin, the method and every such header.
3. **On the final response, before the body is read.** `Client::send`
   follows redirects internally, so the check covers the whole chain: when
   any URL of it is off the document's origin, the response must carry an
   `Access-Control-Allow-Origin` that names the origin (or `*` when the
   credentials mode is not `include`), and `Access-Control-Allow-Credentials:
   true` when it is. A response that fails is dropped unread and script sees
   `TypeError: Failed to fetch`; the reason goes to the log. A shared
   response's headers are cut down to the CORS-safelisted ones and those
   `Access-Control-Expose-Headers` names.

Credentials are cookies here. `tenta_net::CredentialsMode` is evaluated per
hop by the transport: `fetch()`'s default (`same-origin`) and an XHR without
`withCredentials` send and store cookies only while the URL is same-origin
with the document, `include`/`withCredentials = true` always, `omit` never.
`tenta-net` now writes `Origin` on any request that owes one, whether or not a
profile places the header.

`mode: "no-cors"` skips all of the above for a request to another origin:
it is sent, its answer is read and dropped, and script gets an opaque
`Response` (`type` `opaque`, status 0, no headers, no body).

Not built: a preflight cache (every non-simple request is preflighted),
`Response.type` `cors` (a shared cross-origin response still says `basic`),
and a tainted `Origin: null` after a cross-origin redirect chain. A request
that needed a preflight does not follow a redirect to another origin
(`Request::same_origin_redirects`), where a browser would preflight the new
URL.

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

### `Blob` and `File`: the first interface-union brand check (P2 `d-4b`, `FormData`'s prerequisite)

`idl/fileapi/Blob.webidl` declares `Blob`, `BlobPropertyBag`, `EndingType`, the
`BlobPart` typedef, and `File : Blob` with `FilePropertyBag`. `idl/fileapi/` is a
new entry in `tenta-js/build.rs`'s directory list, so this is the first
declaration lowered from a directory the list did not already name (`idl/toy/`
is the generator's own fixture and is not in it). `Blob` and `File`
are also the first declarations whose *union arm* needs an interface brand
check. Both host records are new variants of
`HostRecord`, so they follow the recipe in "JS-owned host objects and the promise
seam" above: a variant, an `interface_name()` arm, a `host_pairs`
arm, a `HostLink` arm, a `reclaim_host_record` arm, a `with_*` reader on
`HostObject`, and the `has_wrapper` arm in `crates/tenta-js-v8/src/world.rs`.

**Why the union needs a check at all.** `BlobPart` is `(BufferSource or Blob or
USVString)`, and the generator's arm order is the declaration's. Its V8
conversion uses a cached `FunctionTemplate` with a receiver signature to test the `Blob` brand and
then asks `WrapperWorld` for the receiver's native handle. The second check
matters because a signature alone can accept a matching holder in the prototype
chain, such as `Object.create(realBlob)`. A real `Blob` or derived `File` selects
the interface arm; other platform wrappers, ordinary objects and prototype
spoofs fall through to `USVString`. The host's `blob_bytes_of` record check
remains as a defensive invariant.

**Two deviations this slice records**, beside the members in
`idl/fileapi/Blob.webidl`:

1. `BlobPropertyBag.endings` is refused **by name, by presence** — the same
   error direction `RequestInit`'s twelve unserved members take from slice
   b-6.5 — because the CRLF branch of *convert line endings to native*
   (<https://w3c.github.io/FileAPI/#convert-line-endings-to-native>) is
   unreachable on a Unix-like host and only `endings: "native"` selects it. By
   presence and not by value, so `{endings: "transparent"}` is over-refused.
2. `endings` is declared **without** the standard's `= "transparent"`. A
   dictionary member with a default lowers to a plain field and one without a
   default to an `Option<T>`, so a default makes presence unobservable to the
   host and deviation 1 unimplementable. The host applies the standard's default
   itself. `type` keeps its `= ""`.

**One editorial defect in File API, read around rather than reproduced.** Both
constructors guard the `type` normalization on `type` being non-empty and then
read the guarded local `t` from outside that guard — `Blob` in its step 4 ("its
`type` set to the value of `t` from the substeps above") and `File` in its
sub-step 3.2 — leaving `t` unbound when `type` was absent or empty. Since
`BlobPropertyBag` declares `type = ""`, the empty string is the only input the
two readings can disagree about, and the implementation answers the empty string
for it. `idl/fileapi/Blob.webidl` records the same reading.

**Facts the declaration relies on, verified rather than assumed.** A dictionary
member is read with `Get` and not by own-enumerable lookup, so
`new Blob([], Object.create({endings: "native"}))` is refused. `sequence_from_v8`
requires an object with a callable `@@iterator` (`iteration_method` refuses a
non-object, which is WebIDL's own `GetV`), so `new Blob("ab")` and
`new Blob({})` both throw where the standard throws too. Optional arguments with
no IDL default lower to
`Option<T>` and are `None` when the argument is absent *or* explicitly
`undefined`, which is WebIDL's own rule, so `slice(undefined)` and `slice()` take
the same branch and `slice(0)` does not. `[Clamp]` leaves the sign intact, which
is what lets `slice(-3)` reach the standard's `originalSize - 3` branch.
`File` inherits `Blob`'s constructor-visibility *not* by inheritance but by being
a supertrait: the generated `FileOps: BlobOps` bound is what makes
`file.slice()` resolve, and `install_file<T: FileOps + BlobOps + FileConstructor
+ BlobConstructor>` is the same fact restated for the installer. `File` is
installed after `Blob`, because `constructor.inherit` reads the parent template
out of the isolate's template cache (`crates/tenta-idl/src/codegen/v8.rs`).

**A third deviation, and it is the generator's rather than the declaration's.**
An absent or explicitly `undefined` optional `sequence<>` argument lowers to
`None`, but an explicit `null` does not: the generated argument check is
`args.length() > 0 && !args.get(0).is_undefined()`, so `null` reaches
`sequence_from_v8`, whose `iteration_method` refuses a non-object and throws.
WebIDL's sequence conversion returns the empty sequence for `null` as well as
for `undefined` (<https://webidl.spec.whatwg.org/#es-sequence> step 2), so
`new Blob(null)` is an empty `Blob` in a browser and a `TypeError` here. `Blob`
and `File` are the corpus's first optional `sequence<>` parameters, so this is
where it became reachable; the fix is in the codegen's optional-argument test,
not in `idl/fileapi/Blob.webidl`, and it is not made here.

`lastModified` defaults to the system clock, not to the event loop's `Clock`:
`Date.now()` in this engine is V8's, which is the system clock, and a value
measured from a virtual origin would not be comparable with it.

**Two further properties `File` exposed, both the generator's rather than the
declaration's.** `File` is the corpus's first *derived* interface, so both are
first observable here and both are pinned in
`crates/tenta-js-v8/tests/dom_blob.rs`:

- `Object.getPrototypeOf(File) === Blob` is `false` where a browser says `true`.
  V8's `FunctionTemplate::inherit` (`crates/tenta-idl/src/codegen/v8.rs`'s
  `constructor.inherit(parent)`) sets the **instance** prototype chain, which is
  the one WebIDL requires of `File.prototype`; the constructor *function's* own
  `[[Prototype]]` has no template API at all and would have to be patched on the
  instantiated function object per context. So `File.prototype` chains correctly
  and `File` does not.
- `File.length` is `0` where the standard's is `2` (`fileBits` and `fileName` are
  required). The generator computes the number —
  `InterfaceDescriptor.constructor_length` is `Some(2)` — and **nothing reads
  it**: no `*_v8.rs` sets the template's `length`, so every constructor in this
  engine answers `0`. `Blob`'s `0` is correct by accident, since both of its
  arguments are `optional` and the existing `Response.length` assertion in
  `dom_response.rs` passes for the same reason.

### `FormData`: a host record owned in two directions (P2 `d-4b`)

`idl/xhr/FormData.webidl` declares `FormData`, the `FormDataEntryValue`
typedef, and nothing else. `idl/xhr/` is already in `tenta-js/build.rs`'s
directory list (`XMLHttpRequest.webidl` is there), so this is a second
declaration lowered from a named directory rather than a new entry in it. The
generated names follow `naming::snake_stem`, which splits `FormData` into
`form_data` (not `formdata`): `form_data_ops.rs`, `form_data_v8.rs`,
`form_data_register.rs`, and the installer `install_form_data`, called in
`install_dom` (`crates/tenta-js-v8/src/lib.rs`) immediately after `install_file`
because this interface mints a `File`.

The entry list is the record, and the record is host-side:

```rust
struct FormDataRecord { list: Vec<(String, EntryValue)> }
enum EntryValue { String(String), File(HostKey) }
```

`FormDataRecord::interface_name()` answers `"FormData"` through the shared
`TAG_HOST`, so the new variant follows the recipe in "JS-owned host objects and
the promise seam" above: a `HostRecord` variant, an `interface_name()` arm, both
`host_pairs` arms (it is `None` — a `FormData` has no paired record), a
`HostLink` arm, a `reclaim_host_record` arm, a `with_form_data` reader, and no
marking in either of `crates/tenta-js-v8/src/world.rs`'s two `has_wrapper`
sites (`WrapperWorld::wrapper`'s `TAG_HOST` arm and `WrapperWorld::adopt`):
a `FormData` is never freed by anything but its own wrapper's death, so neither
match assigns it a flag — the record is *owned*, not *owning*.

**Two-way ownership is the whole of this slice's new machinery.** `Response`
introduced the one-way case (a `Headers` record's validity depended on its
wrapper), `Blob`/`File` added `has_wrapper`, and `FormData` needs the second
half: a `File` record can be reached through *its own* wrapper or through an
entry list, and either may be finalized first. `FileRecord` carries both facts —
`has_wrapper: bool` (set by `WrapperWorld::wrapper` and `WrapperWorld::adopt` in
`crates/tenta-js-v8/src/world.rs`, cleared when the wrapper's finalizer runs)
and `owners: Vec<HostKey>` (insertion-idempotent, one entry per owning
`FormData` record) — and `FileRecord::is_free()` is
`!self.has_wrapper && self.owners.is_empty()`. `owners` is a `Vec` because one
file can be an entry of two lists at once and of one list twice.
`HostLink::File(Vec<HostKey>)` carries the owners out of a dying `File` wrapper
and `HostLink::FormData(Vec<HostKey>)` carries the files out of a dying entry
list, so `reclaim_host_record` reclaims in either order and both orders converge
on "every record with no wrapper and no owner is freed". Both are pinned:
`one_file_can_be_owned_by_two_entry_lists` and
`a_collected_entry_list_frees_the_files_it_minted`
(`crates/tenta-js-v8/tests/dom_formdata.rs`). Every mutating member is also a
reclamation point, and `HostObject::mutate_form_data`
(`crates/tenta-js/src/bindings/host.rs`) is the single function that computes the
before/after key sets, re-registers the surviving owners and relinquishes the
dropped ones, so `delete`, `set`'s replace-and-remove-others and the list's own
death cannot diverge.

**The identity table, as implemented** (HTML's *create an entry*, which the IDL
header quotes verbatim). The free functions `blob_source_of` and `entry_file`
(`crates/tenta-js/src/bindings/host.rs`) are the two that decide it:

| value argument | `filename` | resulting entry value |
|----------------|-----------|-----------------------|
| `USVString` | absent | the string, via `ToString` (so `undefined` → `"undefined"`, `null` → `"null"`) |
| `File` | absent | **the argument itself** — `FormData` records the file's key, so `get()` returns the same object (`set-blob.any.js`'s `assert_file_identity`) |
| `File` | given | a **new** `File`: bytes and `type` copied from the source, `name` = `filename`, `lastModified` **copied from the source** |
| `Blob` (not a `File`) | absent | a **new** `File`: bytes and `type` copied, `name` = `"blob"`, `lastModified` = **now** |
| `Blob` (not a `File`) | given | a **new** `File`: bytes and `type` copied, `name` = `filename`, `lastModified` = **now** |
| any other value | absent | the string produced by `ToString` |
| non-`Blob` value | given | `TypeError` from the three-argument Blob overload |

`lastModified` is a `File`-only attribute, so it is not "copied" on the `Blob`
rows — it is minted, and `now_epoch_ms()` is the system clock, consistent with
`File`'s own default. The two WPT windows agreed on this before it was written:
`append.any.js`'s `testFormDataAppendEmptyBlob` uses `>= now - 2000 && <= now`
and `set-blob.any.js`'s file-blob case narrows the same window to 200 ms.
`lastModified` is *carried* on the `File`-with-filename row because
`set-blob.any.js` requires `file2.lastModified === 123` for a source constructed
with that value; a browser's *create an entry* does not say so explicitly, but
the standard's `new File(bytes, name, { type, lastModified })` step is what makes
it true there too. The empty-`Blob` row is pinned exactly as
`append.any.js` writes it — `name === "blank.txt"`, `type === ""`, `size === 0`,
`instanceof File` — in `a_filename_mints_a_file_copying_the_sources_last_modified`.

**The overloads, and the one departure from the standard's IDL text.** The
generated names are `append_2`/`append_3` and `set_2`/`set_3`, and the
both backends use `codegen/mod.rs::overload_dispatch`: argument count selects
the first declared matching arm, with excess arguments ignored by the longest
finite signature and insufficient arguments rejected with `TypeError`. That is why the file declares the *value* parameter of the
two-argument form as `(Blob or USVString)` instead of the standard's separate
`USVString` and `Blob blobValue` overloads: WebIDL's own algorithm
(<https://webidl.spec.whatwg.org/#es-overloads>) chooses between them on the
*distinguishing argument's value* — a platform object at that index picks the
`Blob` arm — and an arity dispatcher cannot, so it would send
`append("file-1", file)` to the `USVString` overload and store
`"[object File]"`. Four of `set-blob.any.js`'s six subtests fail under the
standard's pair; the union declaration restores the standard's semantics for
every input a browser distinguishes. If the generator grows value-based
overload resolution, this file can return to the standard's overload text.

`FormDataEntryValue` appears only in return positions, where the host chooses
the `File` or `USVString` arm. The argument positions use `(Blob or USVString)`:
generated interface-brand selection keeps real `Blob` and derived `File`
wrappers on the interface arm and sends every other value through `ToString`.

The deviations the declarations create are recorded in
`idl/xhr/FormData.webidl`'s header and restated here for a reader who has only
this file:

1. **`new FormData(form, submitter)` is refused with a `NotSupportedError`.**
   The form arm needs HTML's *constructing the entry list*, which walks a form's
   submittable elements; `HTMLFormElement` exists in this repository's IDL as a
   name only, with no `type`, `files` or `value` to read. `new FormData()` is
   the standard's and is served. Values without the `HTMLFormElement` brand,
   including unrelated node wrappers, fail argument conversion with the
   `TypeError` WebIDL requires.
2. **A mutation during iteration is not observed: a pair iterator is a
   snapshot.** Shared with every landed pair `iterable` and recorded under
   "Notes on the shape of the generated code" above, with
   `wpt/checkout/xhr/formdata/iteration.any.js`'s three subtests named. Pinned
   by `iteration_views_a_snapshot_so_a_mutation_during_it_is_not_observed`,
   which a live-iterator slice has to come back and flip.
3. **An entry's value can be collected while the `FormData` still holds it, so
   a later read can mint a new wrapper for the same record.** A browser's entry
   list holds the `File` object, which keeps it uncollectable; here the entry
   holds the record's key and the record outlives its wrapper (that is what
   makes a read work at all), but the *wrapper* is collectable once the page
   drops it. Narrow and latent — observing the difference requires holding the
   first object, which keeps it alive — and the property `set-blob.any.js`
   actually pins (`assert_file_identity`: two `get`s are the same object) holds.
   Recorded, not worked around.

`FormData` is deliberately **not** added to `BodyInit` or
`XMLHttpRequestBodyInit`, so `fetch(url, { body: fd })` and
`xhr.send(fd)` are unchanged by this slice; multipart serialization does not
exist here and the union would have to grow an arm and a serializer together.
`Response.formData()` is likewise absent (`HostValue` has no entry-list value).

### A second document: the handle slot (P2 slice d-9)

`DOMImplementation.createHTMLDocument`/`createDocument` return a document that
did not exist before, so the engine has to hold more than one document. This is
the slice that made `NodeRef`'s lack of a document id visible: a handle is
`tag:4 | doc_slot:12 | node_id:32 | generation:16`, and the slot — the field next
to the tag, which every encoder used to pass a literal `0` for — is what says
*which* document a node id belongs to.

- **The slot is twelve bits, and the tag paid for them.** The field was eight
  bits while a world could hold one document, and eight is not enough: the widest
  `dom/nodes` file, `DOMImplementation-createDocument.html`, mints over four
  hundred documents in one page (its 214-row table runs three `createDocument`
  calls for each of the 105 rows that expect no exception). The four bits came
  off the tag because that is the one field nothing outside the handle's own
  encoders pins: a node's `node_id` is the arena's `u32` index and its
  `generation` is a `u16` in both the arena and `NodeRef`, while ten tags are in
  use and `TAG_HOST` already multiplexes the host-owned classes by a record
  field rather than by a tag. `MAX_DOCUMENT_SLOT` is the ceiling, so a caller
  does not have to know the layout to know the limit.
- **Running out is an error, not an abort.** `DocumentSet::next_slot` returns
  `Option<u16>` and `HostContext::new_child`/`HostObject::mint_document` are
  fallible, so the document past the ceiling is a catchable
  `QuotaExceededError` `DOMException`. The eight-bit version panicked on
  `u8::try_from`, and it panicked *inside a V8 callback*, where the unwinder
  cannot start: `fatal runtime error: failed to initiate panic, error 5`,
  `abort()`, and every test the WPT driver had in flight died with the process.
  A page that loops over `createHTMLDocument` is a page, not a crash.
  `exhausting_the_document_slots_is_a_catchable_error_and_not_an_abort` asserts
  the exact ceiling, and `a_page_can_hold_more_documents_than_a_byte_of_slot_would_allow`
  asserts that six hundred of them stay *usable* — a wider field whose bits
  collided would resolve handles in the wrong arena.
- **`DocumentSet` is the registry.** One per wrapper world, shared by `Rc`
  (`crates/tenta-js/src/bindings/host.rs`). Slot 0 is the page's context, held
  **weakly** through the `Rc::new_cyclic` that built it, so the set never keeps
  the page alive; slots 1 and up are held **strongly** and nothing points back
  at the set from them, so the graph is a tree with no cycle. A minted context is
  retained for the world's life: freeing a slot would let a stale handle resolve
  to whatever took the slot, and the slot cannot also carry a generation.
- **`HostContext` gained `slot` and `documents`,** and `HostObject::slot()` reads
  the slot off `self.context()`. That is deliberate: the slot is a property of
  the *context* a host was built in, not of the host, so all ~126 host
  constructors stayed unchanged and only the encoders (`encode_node`,
  `encode_collection`, `encode_host`, …) take it. A set of host constructors that
  each had to remember a slot would be a defect waiting to happen.
- **Receiving a foreign handle is `HostObject::anchor_of`,** which resolves
  `decoded.slot` through the set and validates the generation against that
  document's arena. A slot that names no context is a `TypeError` — the same
  answer the wrapper world gives when it refuses to instantiate a wrapper. The
  three members that take a node *argument* each layer their own rule over it
  (`same_document_node`, `child_argument`, and `contains`'s `false`), because a
  node of another document means something different to each of them; see the
  adoption deviation below.
- **`V8World::wrapper` routes by `decoded.slot`** and looks its host up in that
  document's slabs (snapshots, events, host records), and
  `reclaim_dead_bookkeeping` takes the `DocumentSet` rather than one context for
  the same reason: `WorldState.dead` is a list of `(slot, NodeId)` pairs, because
  a finalizer cannot borrow a document and only the slot can say which document
  to unmark. Teardown's `wrapped_node_ids` loop follows the same per-slot rule.
- **`Document.createElement` lowercases only in an HTML document.** Its step is
  "if this is an HTML document, set localName to localName in ASCII lowercase",
  and `DOMImplementation-createDocument.html` reads
  `doc.createElement("DIV").localName` back as `"DIV"` from the XML document it
  minted. The namespace is the HTML namespace either way, which the same step
  also says.
- **Interfaces named for the first time here.** `DocumentType`,
  `ProcessingInstruction` and `DOMImplementation` are ordinary generated
  interfaces; the world's node arm had to learn to name the first two
  (`NodeKind::DocType => "DocumentType"`,
  `NodeKind::ProcessingInstruction => "ProcessingInstruction"`) because both
  node kinds already existed in the arena and both were being wrapped as the
  `Node` shell. `ProcessingInstruction` needed `tenta_dom` work too: the six
  `CharacterData` splice entry points guarded on "is a text-ish node", so a
  processing instruction's `data` was unreachable. They now guard on
  `NodeKind::is_character_data_subclass`, which is `Text | Comment | CData |
  ProcessingInstruction`.
- **`DOMImplementation` is a `HostRecord` variant with no state,** and that is
  the reason the slot exists at all: the record's key alone cannot say which
  document an implementation belongs to, because two documents' slabs are
  independent and their first key is the same number.
  `HostContext::implementation` caches the key per context, which is what makes
  `[SameObject]` hold and a second document's implementation a different object.
- **`WindowProxy` is deliberately left undeclared.** `Document.defaultView` is
  typed `WindowProxy?` in the DOM Standard and `WindowProxy` is not an IDL
  interface there either. The generator turns an undeclared name into an opaque
  interface marker with a trait method and `Handle<WindowProxy>`, and
  `V8World::wrapper` ignores the interface name it is handed, so
  `handle_to_v8::<WindowProxy>(encode_window())` reaches the `TAG_WINDOW` arm and
  answers the global object — which is what makes `document.defaultView ===
  window` true rather than merely truthy. The cost is that `window instanceof
  WindowProxy` is `false`; no test in `dom/nodes` asks for it.
- **`defaultView` is non-null exactly on slot 0.** The world was built over the
  one document the engine navigated, so the page's document is its browsing
  context's active document and every minted document is detached. When a second
  browsing context arrives the test has to become "is this the document a window
  was installed over".

Deviations, all recorded in `idl/dom/dom.webidl`:

1. **`createDocument` returns `Document`, not `XMLDocument`.** There is no
   `XMLDocument` interface and no XML-specific document prototype, so the
   standard's `Object.getPrototypeOf(doc) === XMLDocument.prototype` cannot hold.
2. **`createDocument`'s `qualifiedName` is `DOMString?`,** where the standard says
   `[LegacyNullToEmptyString] DOMString`. The generator supports that annotation;
   this older nullable compatibility signature still needs alignment with its
   host conversion. `null` takes the empty-name path, but explicit `undefined`
   can differ from the standard conversion.
3. **No adoption: a node cannot move between documents.** The third deviation is
   the one that reaches beyond `DOMImplementation`. Here a node's document *is*
   the arena the node lives in and a handle names that arena by its slot, so
   adopting would mean moving a subtree into another arena — assigning it new
   node ids there — and re-keying every wrapper over the moved nodes. Two
   consequences:

   - A `doctype` argument to `createDocument` is **reproduced, not moved**. The
     new document gets a doctype with the same `name`/`publicId`/`systemId` owned
     by itself, but `doc.doctype === doctype` is false and the argument doctype
     stays where it was, even when it already had a parent — the standard's step
     is an append, which adopts and is not fallible, so refusing would fail every
     assertion in a row of `DOMImplementation-createDocument.html` rather than
     the single identity one.
   - An insertion of a **node from another document** is refused with a
     `NotSupportedError`, which is the same refusal the `FormData` constructor
     already makes for a `form` argument. `Node.contains` needs no refusal: a
     node of another tree is not an inclusive ancestor of anything here, so the
     answer is `false`.

   The refusal is not politeness, it is the only safe answer. A `NodeRef` carries
   no document id of its own beyond the slot, so a foreign node id used in this
   arena names an unrelated node — a wrong-node write — or nothing at all, which
   is a panic out of the arena's own accessors. `dom/nodes` found both: `contains`
   walked `parent` with a foreign id and the abort below killed the WPT driver's
   whole process, four seconds in, and the insertion path was refused only by the
   accident that the foreign id happened not to be live here.

   ```
   panicked at crates/tenta-dom/src/document.rs:716:32:
   fatal runtime error: failed to initiate panic, error 5, aborting
   ```

   So `HostObject::same_document_node` (insertions), `HostObject::child_argument`
   (a `removeChild`/`insertBefore`/`replaceChild` reference that claims to be a
   child — a foreign one is a `NotFoundError`, which is what the standard says
   too) and `contains` each test the slot before any id reaches the arena. Pinned
   by `a_node_of_another_document_is_never_indexed_in_this_arena`, which also
   checks the page's tree for corruption rather than only the return value.
   A later adopt slice changes that test deliberately;
   `Node-mutation-adoptNode.html`'s "simple append of foreign div with text" is
   the subtest it will turn green.

### The Element attribute surface (P2 slice d-10)

`Element.prefix`, the namespace-aware attribute operations
(`setAttributeNS`/`getAttributeNS`/`removeAttributeNS`/`hasAttributeNS`),
`Element.attributes` with `NamedNodeMap` and `Attr`, the attribute-node
operations, `Document.createAttribute`/`createAttributeNS`,
`getElementsByTagNameNS` and the ASCII-whitespace rule
`getElementsByClassName` splits on. The deviations and the deliberate gaps:

- **`Attr : Node` uses a host record rather than an arena node.** Attribute
  storage remains packed in each element. An attached Attr record names its
  element and `(namespace, localName)` and pins that element while its wrapper
  is live. The inherited Node operations that need an arena tree position
  handle Attr explicitly. A value write keeps an existing attribute's prefix;
  `setAttributeNode` replaces the attribute object and can change its prefix.
  Removal detaches the same Attr object with its last value.
- **Attached Attr identity is canonical within a binding world.**
  `getAttributeNode`, `NamedNodeMap.item`, and its named getters reuse the
  wrapper for the same element and `(namespace, localName)`. The weak index
  is cleared when the wrapper is finalized or the attribute is detached.
  `Element.attributes` remains `[SameObject]` via its separate
  `TAG_NAMED_NODE_MAP` handle.
- **Two name rules, not one.** `createElement`/`createElementNS` use XML's
  `Name` production as the WPT tables pin it (`Document-createElement.html`,
  `Document-createElementNS.js`): an ASCII letter, `_`, `:` or a non-ASCII
  scalar to start, and no ASCII whitespace or `>` after it. Attribute names use
  a different, looser predicate — non-empty, and free of `=`, `>` and `/` —
  because `dom/nodes/attributes.html`'s `valid_names` requires
  `setAttribute("0")`, `setAttribute("~")` and `setAttribute("\"")` to be
  accepted, while `dom/nodes/processing-instruction-attributes.html` requires
  `$`, `_` and `a$b` to be accepted and `=`, `>` and `/` to be refused wherever
  they sit. No single first-character rule satisfies both tables.
- **Plain name operations use the first matching qualified name.** The host
  shares that lookup across Element and NamedNodeMap, applying ASCII lowercase
  only for HTML-namespace elements in HTML documents. Writes to an existing
  namespaced attribute preserve its namespace, prefix and Attr identity;
  missing names create null-namespace attributes. Direct native mutations of a
  shared `Document` outside the binding host do not yet synchronize held Attr
  records with external attribute removals.
- **Attr integration remains partial across document and range boundaries.**
  Attaching an Attr from another document currently reports
  `NotSupportedError` rather than adopting it. Live Range boundary methods
  still resolve only arena-backed `TAG_NODE` handles, so a Node-typed Attr
  endpoint does not yet receive the DOM Standard's Range behavior. QuickJS
  indexed and named collection hooks now have focused real-engine coverage on
  both backends; this does not establish full collection conformance.
- **`getElementsByTagNameNS` returns a snapshot**, not the standard's live
  collection, exactly as `getElementsByTagName` already did, so a mutation after
  the call is not visible through it. `[NewObject]` requires a fresh object;
  this API's DOM algorithm separately requires that collection to remain live.

Deliberately **not** taken in this slice, each with its reason:

- `MutationObserver` (114 non-passing `dom/nodes` subtests). `tenta-dom` has a
  mutation *log* (`MutationObserverSink`), but the interface needs a delivery
  order — a microtask queue, an observation-target set and `takeRecords`'s
  interplay with it — that the host has no mechanism for. A stub whose callbacks
  never fire would be exactly the masking the working rules forbid.
- `DOMParser` was absent at this historical checkpoint (41 subtests).
  Native XML parsing and DOMParser/XMLSerializer bindings have since landed
  with focused tests; the old count is not a current conformance result.
- The `Attr` surface on `ProcessingInstruction` (`pi.setAttribute`,
  `pi.getAttribute`, `pi.hasAttributes`, `pi.toggleAttribute` — 90 subtests of
  `processing-instruction-attributes.html`). Those members belong to the WICG
  *declarative partial updates* proposal, where a PI's attributes are parsed out
  of its `data` string and a mutation rewrites that string; that is a
  serialization subsystem of its own, not an `Element` member.

### The ParentNode and ChildNode mutation methods

`ParentNode.append`/`prepend`/`replaceChildren` (Element, Document,
DocumentFragment) and `ChildNode.before`/`after`/`replaceWith` (Element,
CharacterData, DocumentType) take `(Node or DOMString)...`. The algorithms live
in `tenta-dom` (`Document::*_nodes`, `Document::replace_children`) and the host
only resolves the arguments, so both engines share one implementation.

- *Convert nodes into a node*: a lone argument is used as it is, anything else
  is gathered into a new fragment. Each string is its own Text node and
  adjacent Text nodes are never merged; the standard has no such step.
- `before`/`after`/`replaceWith` pick the viable sibling before converting, so
  arguments that are siblings of the context object do not anchor the
  insertion; a node without a parent does nothing.
- `replaceChildren` adopts the node (a child of the same parent included, which
  queues its own removal record), then replaces all children in one record.
  The hierarchy check ignores the children being removed, as
  `ParentNode-replaceChildren.html` expects of a document that holds a doctype
  and an element.
- `ReplaceAllWith::Detached` is the replace-all source for one detached Text
  node, or a doctype when the parent is a document. Replace-all now accepts a
  Document parent and checks the replacement's own children (no Text, at most
  one element, doctype only under a document).

Not covered: nodes of another document are refused with `NotSupportedError`
(no adoption), and `Node.moveBefore` is absent.

### `window.onload` and the two lifecycle dispatches (P2 slice d-8)

The window is the global object, so none of its members can come from the
generated glue — a generator emits a template per interface, and the global uses
the one `install_dom` builds. `window.onload` is therefore hand-written, and it
is an **accessor** rather than a data property: HTML's `on*` attributes are
`EventHandler` attributes, so a read returns what script assigned and an
assignment registers a listener in the table `addEventListener` writes to
(`HostObject::set_event_handler`). Three consequences worth knowing:

- The accessor goes on the **global template**
  (`host::install_window_event_handlers`, called before `Context::new`), because
  `Context::global` hands back the global *proxy*: a property installed on the
  proxy is not the global object's own property, which is what `window.onload`
  reads.
- The handler is an ordinary listener carrying HTML's internal callback, so an
  `onload` handler and the same function passed to `addEventListener('load', …)`
  are **two independent listeners**: `addEventListener` never dedups against one
  and `removeEventListener` can never remove one.
- `[LegacyTreatNonObjectAsNull]` is implemented, so `window.onload = 0`
  deactivates the handler rather than storing `0`.

Deviation: `GlobalEventHandlers` has many more names and only `onload` is
installed, because nothing else in the engine fires the event a name would
receive. `window.onload` is also the first member installed as an accessor
through the global template; a later one adds its key to the same function.

The events themselves are fired through `PageEngine::dispatch_lifecycle`, whose
one entry takes a `LifecycleEvent`:

- `DOMContentLoaded` at the `Document`, `bubbles` true and `cancelable` false
  (HTML "the end");
- `load` at the `Window` **with the legacy target override flag** (HTML "the
  load event"), both flags false. DOM's flag makes `targetOverride` the target's
  associated `Document`, so `event.target` at that dispatch is the document while
  the listener table and the path are still the window's — which is what makes
  `window.addEventListener('load', …)` fire.

`DispatchTarget::with_target_override` is the only place that flag is spelled,
and it is deliberately separate from the target's own handle: the table the
listeners are keyed by and the path `composedPath()` reports come from the target,
not from what `event.target` says. `crates/tenta-js/src/dom/events/tests.rs`
pins the split.

`event.isTrusted` is true for both: they are dispatched through the b-4
native-dispatch path (`HostState::enter_native_dispatch`), which is also what
lets them enter V8 with no callback on the stack.

### `MutationObserver`: host records over native observer state (P3 `JS1`)

`MutationObserver` and `MutationRecord` (`idl/dom/MutationObserver.webidl`) are
host records in the sense above, with three differences worth knowing before
touching them (`crates/tenta-js/src/bindings/host/mutation_observer.rs`):

- **Native state is released after the slab borrow ends.** `HostRecord::MutationObserver`
  holds a `tenta_dom::NativeObserver` and `HostRecord::MutationRecord` a
  `PublishedObserverRecord`; both walk native registrations when dropped. `free_record`
  therefore does not drop them: it moves them into `ReclaimedHost::retired`, and
  `HostContext::reclaim_host` finishes them (`NativeObserver::forget`, record release) once
  `host_records` is no longer borrowed. A new host record that owns native state follows
  the same route.
- **The observer owns a reference to itself while it observes.** `observe()` stores one
  stored-value reference to the observer's own wrapper (`ScriptHost::retain_existing_target`)
  and `disconnect()` releases it, so an observer the page no longer references keeps
  delivering. `reclaim_host_record` returns the callback's reference (and the self
  reference, if any) in `ReclaimedHost::values`.
- **Delivery is a microtask queued from the callback boundary.** The V8 prelude calls
  `V8CallbackAdmission::after_callback(scope)` (default no-op) after each generated
  callback's frame is released and its result checked, still inside the callback's scope.
  The world's `Admission` uses it to turn a `Requested` notify demand into one V8 microtask
  (`tenta-js-v8/src/observer.rs`), which is how the notification stays ordered with promise
  reactions queued after the mutation. Native-origin mutations are picked up by
  `Engine::checkpoint`. `ScriptHost::invoke_mutation_callback` has a refusing default so a
  backend without the microtask (QuickJS today) compiles and never delivers.

### `location`, `atob` and `btoa`: handwritten window members (P2 `LOC1`)

These are installed by the engine hosts, like `navigator` and `performance`,
not generated: every `Location` attribute is `[LegacyUnforgeable]`, so the
members are own properties of the one instance and there is no prototype
surface for the generator to emit. The shared logic is
`crates/tenta-js/src/location.rs` (component getters, also used by the `URL`
host) and `crates/tenta-js/src/base64.rs`; the hosts are `install_location`
and the `btoa`/`atob` functions in `crates/tenta-js-v8/src/host.rs` and
`crates/tenta-js-qjs/src/host.rs`. Only the read side exists; the members that
navigate are absent until session history is built.

### Geometry, computed style and document sheets (P3 `CSS1`)

Four more host records answer from the page rather than from the arena
(`crates/tenta-js/src/bindings/host/{geometry,computed_style,doc_sheets}.rs`):

- `DOMRectReadOnly`/`DOMRect`/`DOMRectList` (`idl/dom/DOMRect.webidl`;
  `HostRecord::{DomRect, DomRectList}`). The generator emits an inherited member's
  accessor in both the base and the derived `Ops` trait, which makes every call
  of it ambiguous, so `x`, `y`, `width`, `height` are declared once on
  `DOMRectReadOnly` with setters and the record's `read_only` flag drops a write.
  `DOMRect` is an empty derived interface. Static operations and `toJSON` cannot
  be declared (no static installer; `object` return), so
  `tenta_js::geometry::GEOMETRY_PRELUDE_SOURCE` defines `fromRect`, `toJSON` and
  the `DOMRectList` iterator; a backend runs it from
  `PageEngine::install_preludes` (V8 only).
- `getComputedStyle` returns a `CSSStyleProperties` whose record is
  `HostRecord::ComputedStyle` (a `NodeRef` and the pseudo string). The
  declaration ops in `cssom.rs` branch on it first; every read asks the world's
  `GeometryHost` through `ScriptHost::geometry_host()`, so a page below the render
  tier, which installs none, reads `""` everywhere. The function itself is
  hand-installed on the V8 global (`host.rs` `get_computed_style`), like `fetch`;
  there is no `partial interface Window` in the model.
- The element members (`getBoundingClientRect`, `client*`, `scroll*`, `offset*`,
  `elementFromPoint`, ...) are ordinary generated members on `Element`,
  `HTMLElement` and `Document` whose bodies call `HostObject::geo_*`, which read
  the same `GeometryHost` and answer as a box-less element when there is none.
- `StyleSheetList` is a stateless record: `length` and `item()` recompute the
  owners (`<style>` and loaded `<link>`) from the document on every call, and
  the `CSSStyleSheet` of an owner is cached in `SharedContext::doc_sheets` under
  a fingerprint of its text. A link's text comes from
  `tenta_js::stylesheets::StyleHost`, installed per navigation next to the
  image and geometry hosts (`PageEngine::set_style_host`).

`HTMLLinkElement` and `HTMLStyleElement` (`idl/dom/dom.webidl`) reflect their
content attributes and carry `onload`/`onerror` handler attributes; the loader
fires the events through the image-event entry (`settle_images`).

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
  the golden files with `UPDATE_GOLDEN=1 cargo nextest run --locked -p tenta-idl --test codegen`.
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
- QuickJS pair iterators currently enumerate a snapshot. Value iterators use
  the native Array methods with generated indexed collection hooks; full
  iterable conformance still needs separate validation.
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
- The generator installs native indexed and named property hooks for interfaces
  that declare special operations. `NamedNodeMap` and `HTMLCollection` declare
  named getters, with focused tests for case rules, supported-name ordering,
  unenumerable properties and ordinary own/prototype shadowing.
- `[LegacyNullToEmptyString]` converts null to the empty string for annotated
  DOMString attribute setters, operation arguments and dictionary members.
  `[LegacyLenientThis]` and other unsupported legacy attributes remain model-only.
- `TextEncoder.encodeInto` and its `TextEncoderEncodeIntoResult` dictionary are
  absent from `idl/encoding/TextEncoder.webidl`. The operation takes a
  `BufferSource` it *writes* into, and a buffer source maps to an owned `Vec<u8>`
  in the argument position, so a generated member has no way to hand the bytes
  it wrote back to the caller's view — the generator has no out-parameter
  mechanism at all. `TextDecoderStream` and `TextEncoderStream` are absent for a
  different reason: they are `TransformStream` subclasses, and the streams
  prelude has `ReadableStream` only.
- `Response` declares the part of `Body` that reads the whole body —
  `bodyUsed`, `arrayBuffer()`, `blob()`, `bytes()`, `json()`, `text()` — and
  `clone()`. `body`, `formData()` and `textStream()` are not declared: the
  first and the last are a `ReadableStream`, and `formData()` needs a
  multipart parser. On V8 `body` exists all the same, defined by the streams
  prelude (`crates/tenta-js/src/streams.rs`) as a default stream that delivers
  the body in one chunk; it is not generated from WebIDL. The body's stand-in
  in the record is a `Vec<u8>`, which is
  what makes *unusable* "the body has already been read" and what makes
  `clone()` a copy of the bytes where the standard tees a stream. `blob()`'s
  type is the first `Content-Type` value that parses, lower-cased and
  otherwise as the header spelled it, where the standard re-serializes the
  MIME type. The static `error()`, `redirect()` and `json()` are absent.
- `XMLHttpRequest` is declared down to the whole of its state machine
  (`UNSENT`, `OPENED`, `HEADERS_RECEIVED`, `LOADING`, `DONE`), the request
  attributes `open()` sets up, and the response the machine stores with its
  accessors (`status`, `statusText`, `getResponseHeader`,
  `getAllResponseHeaders`, `overrideMimeType`, `responseType`, `responseText`)
  and its events. It is no longer inert: `send()`/`setRequestHeader()` hand a
  request to the driver seam (`b-5a3`), the `tenta-net` driver behind
  `PageEngine::net_request` carries it (`b-6`, `b-6.5`), `abort()` and the
  `timeout` attribute end one before its answer arrives (`b-7`), and `send()`
  carries a request body (`b-9`).
  `[SameObject] upload` / `XMLHttpRequestUpload` and `responseXML` are the
  members still absent — an inert upload object would invite handlers that can
  never fire, and `responseXML` needs a document the parsed body would build.
  `response` is the text for `""` and `"text"` and, for `arraybuffer`, `blob`
  and `json`, a value built once per finished response and the same on every
  read. Five narrowings are this engine's own and each refuses synchronously:
  `open(…, async = false)` throws `NotSupportedError` (the driver is
  asynchronous and a blocking request would deadlock the page's turn loop), a
  non-null `username`/`password` throws it (this engine sends no credentialed
  request, and what the network layer does with URL userinfo is unverified),
  `withCredentials = true` threw it until CO1 and is now stored and honoured,
  `responseType` refuses `"document"`, and `send()`'s body is declared
  as `XMLHttpRequestBodyInit` without the standard's `Document` arm, which needs
  a `tenta-dom` entry point that serialises a whole document plus the
  HTML-versus-XML choice. `b-9` is what makes the body move:
  the converted bytes reach the driver (`XhrRequest.body`), `GET`/`HEAD` drop the
  body at step 3 before the conversion's own step, and the type the body infers is
  written into the author request headers only when the author set none — the
  `Otherwise` of step 4's sub-step 5, with `set_inferred_content_type` shared
  with the `Response` constructor and `fetch()`'s initializer. One deviation comes
  with the body. Step 4's `charset` rewrite is not implemented: when the author
  *has* set a `Content-Type` and the body is a string, the standard parses it and
  re-serialises the whole type with its `charset` replaced by `UTF-8`, which
  `parse_mime_type` cannot do because it reduces a MIME type to its essence and
  its `charset` and drops every other parameter — so an author
  `Content-Type: text/plain;charset=iso-8859-1` survives as written here where a
  browser rewrites it. Two further
  deviations belong to `timeout`, and neither is a narrowing: the deadline is
  armed **once**, by `send()`, from the value the record holds then, where the
  standard's wait re-reads `this`'s *timeout* as a live condition — so a value
  changed mid-flight governs the request already in flight in a browser, and
  only the next one here;
  and the standard's *timed out* and *aborted* flags are not held, because the
  name of the failing event is passed from the one place the cause is known
  (`xhr_request_error_steps`). One more deviation is a transport limit rather
  than a narrowing: `statusText` reads back as the status code's canonical
  reason text, because the response has no reason phrase of its own to come
  from. The `timeout` setter is declared without `[Throws]`, because its one
  standard throw needs `synchronous` true, which `open()` never leaves
  reachable. `idl/xhr/XMLHttpRequest.webidl`'s header carries the same list
  beside the members it applies to.
- A *throwing getter* on a dictionary member is treated as an absent member: the
  generated `dictionary_member` maps an empty `Object::get` — the shape a throw
  takes — to `Ok(None)` and the next member lookup then drops the pending
  exception. An exception from the *last* member read does survive to the
  caller. Pre-existing and interface-agnostic (`TextDecoderOptions` behaves the
  same way); the fix belongs in `dictionary_member` and is not made here.
- A *failing string conversion* runs the operation anyway. `string_from_v8`
  (`crates/tenta-idl/src/codegen/prelude.rs`) is infallible — it reads V8's
  `to_rust_string_lossy`, so a `to_string` that fails yields the empty string and
  leaves the exception pending, which V8 then throws at the call site when the
  binding returns. WebIDL converts an operation's arguments *before* its steps
  run, so the standard never reaches the steps at all. `send(Symbol())` is where
  `b-9` made the pair reachable: it starts a real request with an empty body and
  `text/plain;charset=UTF-8` and *then* throws the `TypeError`. Pre-existing and
  interface-agnostic — `document.title = Symbol()` throws its `TypeError` and
  stores the empty string for the same reason — so the fix is `string_from_v8`
  becoming fallible, which rewrites every generated call site and needs a
  generator slice with its own goldens; it is not made here, and
  `a_number_body_is_stringified_and_a_symbol_starts_a_request_before_it_throws`
  (`crates/tenta-js-v8/tests/dom_xhr.rs`) pins the pair until it is.
- `Blob` declares the half a constructor and a byte sequence can serve — both
  constructors, `size`, `type`, `slice()`, `text()`, `arrayBuffer()` and
  `bytes()` — and leaves out `stream()`, `textStream()` and the `Serializable`
  attribute. The two stream members need the Streams interfaces (`d-5a`);
  on V8 the streams prelude defines `stream()` over `bytes()`, undeclared
  here, and `textStream()` stays absent. `Serializable` needs
  a structured clone. `FileList`, `FileReader`, `FileReaderSync` and
  `URL.createObjectURL()` are absent for reasons of their own and are listed in
  `idl/fileapi/Blob.webidl`'s header beside the deviations the declarations
  create.

## Native panic containment (V8)

Generated constructors, methods, overload dispatchers, accessors (including
static, `[Replaceable]` and `[PutForwards]` members), indexed getters and pair
iterators run inside `catch_unwind` before returning through V8's C++ callback
trampoline. The internal interface-brand probe is guarded too. Fast getters,
setters and methods catch inside their `extern "C"` bodies; their ABI tables
include a final `FastApiCallbackOptions` argument so recovery can terminate the
current isolate without allocating V8 handles. Value iterators use V8's array
intrinsics and introduce no Rust callback.

A panic records the first callback name in an isolate slot allocated during
template installation and requests uncatchable V8 termination. Primitive fast
return values and the indexed handler's `kYes` are ABI placeholders during that
termination. No slow fallback retries a possibly partial host mutation.
`native_callback_failure(&v8::Isolate)` exposes the sticky marker: embedders
must inspect it after every entry, even if a short `Script::run` returned a
value before V8 consumed the termination interrupt, and discard the engine
when it is set. `Engine` refuses later evaluation, and
the page runtime reports a crash after the native entry has returned, through
its existing page-task crash boundary. Deadline termination remains resumable.

Conversions that temporarily remove the wrapper-world slot restore it before
resuming the unwind, preserving teardown ownership. The first panic payload is
retained until isolate teardown and dropped under another unwind guard; only a
secondary payload from a panicking destructor is intentionally leaked, avoiding
an unbounded chain of destructor panics. An already-active outer callback that
also panics replaces the retained payload after guarded disposal of the older
one. `AssertUnwindSafe` is justified by permanent engine poisoning, not a claim
that partly mutated host state can be reused.

`cargo nextest run -p tenta-idl --features v8 --test panic_v8` runs generated
callback injection cases in subprocesses, including forced V8 optimization,
unwind-time wrapper ownership and panic-payload destruction. The backend's
`native_panic` tests exercise real DOM borrow panics through evaluation,
microtasks and timers. This containment covers generated V8 glue; hand-written
V8 callbacks, QuickJS and the other native dependencies need their own audit.

## References

- WebIDL: <https://webidl.spec.whatwg.org/>
- `weedle4`: <https://docs.rs/weedle4/0.4.0/weedle/>
- rusty_v8 152.2.0: <https://docs.rs/v8/152.2.0/v8/>
- `rquickjs` 0.12.2: <https://docs.rs/rquickjs/0.12.2/rquickjs/>
- TentaBrowser spec: `docs/spec/05-javascript.md` §2–§3,
  `docs/spec/02-architecture.md` §2.1

### Callback-scoped buffer references

`[TentaBufferReferences]` is an internal operation annotation for native APIs
that must preserve the caller's original `BufferSource` or `ArrayBufferView`
identity. It changes only direct buffer arguments and a direct buffer return of
that operation to `BufferSourceRef` or `ArrayBufferViewRef`; ordinary unannotated
buffer conversions still copy bytes into `Vec<u8>`. The reference is an opaque
world-value key owned by the generated callback's temporary stored-value lease.
It is not a byte slice, a persistent root, or permission to keep a JS object in
an asynchronous job. The backend bridge revalidates the native brand, backing
and current span before metadata, copy or write access.

The initial lowering accepts only required, non-nullable, non-variadic direct
`BufferSource` and `ArrayBufferView` positions on named interface operations.
Namespace operations and mixed annotated/unannotated overload groups are
rejected in this slice. Other ordinary optional and variadic arguments keep
their existing lowering.
Optional, nested, named concrete-buffer and direct nullable forms fail model
lowering. A `Promise<ArrayBuffer>` return stays an opaque Promise as before; it
is not converted into a buffer reference. The separate Promise rejection policy
is required before a WebCrypto `digest()` binding can be installed. No Crypto
interface is exposed by this annotation package.
