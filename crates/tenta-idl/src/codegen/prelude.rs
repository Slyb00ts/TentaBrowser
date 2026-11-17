//! The prelude module: runtime support shared by every generated binding.

use std::fmt::Write as _;

use crate::model::BufferType;

/// The eight WebIDL integer types: helper suffix, Rust type, bit width.
const INTEGERS: &[(&str, &str, u32, bool)] = &[
    ("byte", "i8", 8, true),
    ("octet", "u8", 8, false),
    ("short", "i16", 16, true),
    ("unsigned_short", "u16", 16, false),
    ("long", "i32", 32, true),
    ("unsigned_long", "u32", 32, false),
    ("long_long", "i64", 64, true),
    ("unsigned_long_long", "u64", 64, false),
];

/// The typed arrays: model member, V8 brand predicate, V8 type name, bytes per
/// element.
///
/// `DataView` is not here. A typed array's constructor returns an `Option`
/// where `DataView::new` cannot fail, so its pair is written out with the other
/// hand-written conversions instead of being generated from this table.
const TYPED_ARRAYS: &[(BufferType, &str, &str, usize)] = &[
    (BufferType::Int8Array, "is_int8_array", "Int8Array", 1),
    (BufferType::Int16Array, "is_int16_array", "Int16Array", 2),
    (BufferType::Int32Array, "is_int32_array", "Int32Array", 4),
    (BufferType::Uint8Array, "is_uint8_array", "Uint8Array", 1),
    (BufferType::Uint16Array, "is_uint16_array", "Uint16Array", 2),
    (BufferType::Uint32Array, "is_uint32_array", "Uint32Array", 4),
    (
        BufferType::Uint8ClampedArray,
        "is_uint8_clamped_array",
        "Uint8ClampedArray",
        1,
    ),
    (
        BufferType::Float32Array,
        "is_float32_array",
        "Float32Array",
        4,
    ),
    (
        BufferType::Float64Array,
        "is_float64_array",
        "Float64Array",
        8,
    ),
];

/// Generates `prelude.rs`.
pub fn prelude() -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push_str(CORE);
    integer_conversions(&mut out);
    out.push_str(V8_SUPPORT);
    buffer_conversions(&mut out);
    out.push_str(BUFFER_REF_V8);
    out.push_str(QUICKJS_SUPPORT);
    out.push_str(BUFFER_REF_QJS);
    out
}

fn integer_conversions(out: &mut String) {
    out.push_str(
        "\n// WebIDL integer conversions (https://webidl.spec.whatwg.org/#abstract-opdef-converttoint).\n\
         // The plain form wraps modulo 2^n, `clamp_*` implements `[Clamp]` and\n\
         // `enforce_*` implements `[EnforceRange]`.\n",
    );
    for (suffix, rust, bits, signed) in INTEGERS {
        let modulus = format!("{:.1}", 2f64.powi(*bits as i32));
        // The modulo result is non-negative, so it converts through the
        // unsigned type of the same width; a signed target then reinterprets
        // the bits. For an unsigned target one cast is enough.
        let wrap_cast = if *signed {
            format!("as u{bits} as {rust}")
        } else {
            format!("as {rust}")
        };
        let _ = write!(
            out,
            "
/// WebIDL `{idl}` conversion: truncate, then wrap modulo 2^{bits}.
pub fn to_{suffix}(value: f64) -> {rust} {{
    if !value.is_finite() {{
        return 0;
    }}
    let wrapped = value.trunc().rem_euclid({modulus});
    wrapped {wrap_cast}
}}

/// WebIDL `[Clamp] {idl}` conversion: round half to even, then clamp.
pub fn clamp_{suffix}(value: f64) -> {rust} {{
    if value.is_nan() {{
        return 0;
    }}
    let rounded = value.round_ties_even();
    if rounded <= {rust}::MIN as f64 {{
        {rust}::MIN
    }} else if rounded >= {rust}::MAX as f64 {{
        {rust}::MAX
    }} else {{
        rounded as {rust}
    }}
}}

/// WebIDL `[EnforceRange] {idl}` conversion: `TypeError` outside the range.
pub fn enforce_{suffix}(value: f64) -> Result<{rust}, JsError> {{
    if !value.is_finite() {{
        return Err(JsError::type_error(\"value is not a finite number\"));
    }}
    let truncated = value.trunc();
    if truncated < {rust}::MIN as f64 || truncated > {rust}::MAX as f64 {{
        return Err(JsError::type_error(\"value is outside the range of `{idl}`\"));
    }}
    Ok(truncated as {rust})
}}
",
            idl = suffix.replace('_', " "),
            suffix = suffix,
            rust = rust,
            bits = bits,
            modulus = modulus,
            wrap_cast = wrap_cast,
        );
    }
}

/// Emits the buffer-source conversions.
///
/// The nine typed arrays differ only in their V8 brand, their V8 type name and
/// their element width, so they are generated from [`TYPED_ARRAYS`]. The two
/// typedefs and `ArrayBuffer`/`DataView` are written out in
/// [`BUFFER_SUPPORT`]/[`BUFFER_SUPPORT_TYPEDEFS`], because how they are read
/// and built differs in kind rather than in a parameter.
fn buffer_conversions(out: &mut String) {
    out.push_str(BUFFER_SUPPORT);
    for (kind, brand, v8_type, element_size) in TYPED_ARRAYS {
        let suffix = kind.conversion_suffix();
        let idl = kind.idl_name();
        // A one-byte element count *is* the byte count, so the narrowing check
        // and the division are not emitted for it.
        let (alignment, elements) = if *element_size == 1 {
            (String::new(), "bytes.len()".to_string())
        } else {
            (
                format!(
                    "    if !bytes.len().is_multiple_of({element_size}) {{\n        \
                     return Err(JsError::range_error(\n            \
                     \"a `{idl}` holds {element_size}-byte elements, so its byte length must be a multiple of {element_size}\",\n        \
                     ));\n    }}\n"
                ),
                format!("bytes.len() / {element_size}"),
            )
        };
        let _ = write!(
            out,
            "
/// WebIDL `{idl}` conversion.
///
/// The bytes are those of the view's own `byteOffset .. byteOffset +
/// byteLength` range and not of its backing store, and a view whose buffer has
/// been detached reads as the empty sequence.
#[cfg(feature = \"v8\")]
pub fn {suffix}_from_v8(
    _scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<Vec<u8>, JsError> {{
    brand_checked_view_bytes(value, value.{brand}(), \"{idl}\")
}}

/// Converts bytes to a fresh `{idl}` over an `ArrayBuffer` of its own.
///
/// The bytes are copied into the buffer the view covers, so the array the
/// script side receives owns its storage.
#[cfg(feature = \"v8\")]
pub fn {suffix}_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: Vec<u8>,
) -> Result<v8::Local<'s, v8::Value>, JsError> {{
{alignment}    let buffer = array_buffer_with_bytes(scope, &bytes)?;
    match v8::{v8_type}::new(scope, buffer, 0, {elements}) {{
        Some(view) => Ok(view.into()),
        None => Err(JsError::range_error(\"the bytes do not fit a `{idl}`\")),
    }}
}}
"
        );
    }
    out.push_str(BUFFER_SUPPORT_TYPEDEFS);
}

const HEADER: &str = r#"// Runtime support for the generated WebIDL bindings.
//
// Generated by `tenta-idl`; do not edit. Include this file once per binding
// module, before the generated type and interface files:
//
// ```ignore
// mod bindings {
//     include!(concat!(env!("OUT_DIR"), "/prelude.rs"));
//     include!(concat!(env!("OUT_DIR"), "/types.rs"));
//     include!(concat!(env!("OUT_DIR"), "/node_ops.rs"));
//     include!(concat!(env!("OUT_DIR"), "/node_register.rs"));
//     #[cfg(feature = "v8")]
//     include!(concat!(env!("OUT_DIR"), "/node_v8.rs"));
//     #[cfg(feature = "quickjs")]
//     include!(concat!(env!("OUT_DIR"), "/node_qjs.rs"));
// }
// ```
//
// Every file goes into the same module: the generated code refers to the
// prelude items by bare name and carries no `use` of its own.
//
// The prelude's engine helpers live behind `cfg(feature = "v8")` and
// `cfg(feature = "quickjs")`, so a crate that includes the prelude declares
// features with those names and gates the `_v8.rs` and `_qjs.rs` includes the
// same way.
"#;

const CORE: &str = r#"
use core::marker::PhantomData;

/// A handle to a host object that a wrapper exposes to JavaScript.
///
/// The bits are opaque to generated code: the runtime chooses them when it
/// creates a wrapper and maps them back through [`WrapperWorld`].
pub struct Handle<T: ?Sized> {
    bits: u64,
    interface: PhantomData<fn() -> T>,
}

impl<T: ?Sized> Handle<T> {
    /// Builds a handle from its opaque bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self {
            bits,
            interface: PhantomData,
        }
    }

    /// The opaque bits of this handle.
    pub const fn bits(self) -> u64 {
        self.bits
    }
}

impl<T: ?Sized> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for Handle<T> {}

/// The zero handle, which is what a dictionary's `required` members start at
/// while its conversion is still checking which of them were passed.
///
/// A dictionary derives `Default` because its *optional* members need one, and a
/// `required` member of interface type then needs a value before the presence
/// check: `StaticRangeInit`'s four members are all required, so its `from_v8`
/// builds the default and overwrites each field only once the member has been
/// found — a missing one makes the whole conversion a `TypeError`, which is what
/// `StaticRange-constructor.html`'s last block asserts. The zero handle is never
/// the value an operation receives through that path, and it is not a sentinel:
/// it is a well-formed `TAG_NODE` handle naming node 0 of document slot 0, whose
/// arena generation starts at zero.
impl<T: ?Sized> Default for Handle<T> {
    fn default() -> Self {
        Self::from_bits(0)
    }
}

impl<T: ?Sized> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.bits == other.bits
    }
}

impl<T: ?Sized> Eq for Handle<T> {}

impl<T: ?Sized> core::hash::Hash for Handle<T> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.bits.hash(state);
    }
}

impl<T: ?Sized> core::fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Handle({:#x})", self.bits)
    }
}

/// Links an interface marker type to its WebIDL name.
///
/// Every interface named by this binding module gets an uninhabited marker
/// type so that `Handle<Node>` is distinct from `Handle<Event>`.
pub trait InterfaceMarker {
    /// The WebIDL name, which is also the key of the per-world wrapper map.
    const INTERFACE_NAME: &'static str;
}

/// The per-world host state a constructor receives. In `tenta-js` this is the
/// `Rc<HostContext>`; it is kept opaque here so the generated `*Constructor`
/// traits stay engine-independent. A constructor downcasts it to the concrete
/// context to reach the document and the event slab.
pub type Realm = std::rc::Rc<dyn std::any::Any>;

/// A promise the host settles later. `T` is the resolution type.
pub struct Promise<T> {
    bits: u64,
    value: PhantomData<fn() -> T>,
}

impl<T> Promise<T> {
    /// Builds a promise handle from its opaque bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self {
            bits,
            value: PhantomData,
        }
    }

    /// The opaque bits of this promise handle.
    pub const fn bits(self) -> u64 {
        self.bits
    }
}

impl<T> Clone for Promise<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Promise<T> {}

impl<T> PartialEq for Promise<T> {
    fn eq(&self, other: &Self) -> bool {
        self.bits == other.bits
    }
}

impl<T> core::fmt::Debug for Promise<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Promise({:#x})", self.bits)
    }
}

/// The entries a pair iterator's host hook hands over.
///
/// `iterable<K, V>` has no indexed accessor to drive an iterator from — the
/// spec gives neither `Headers` nor `URLSearchParams` a `length` — so the host
/// returns the whole collection at once, and the three generated members
/// (`entries`, `keys`, `values`) are views over that one snapshot.
///
/// The alias exists because a pair's Rust spelling can itself be a container:
/// an `iterable<ByteString, ByteString>` is a `Vec` of `(Vec<u8>, Vec<u8>)`,
/// which every signature would otherwise repeat and which reads as an
/// unreviewable blob of nesting. Naming it here says what the hook returns
/// once, for every interface that has a pair iterator.
pub type IterableEntries<K, V> = Vec<(K, V)>;

/// A JavaScript callable the host holds on behalf of the page.
///
/// `Args` is a tuple of the argument types and `Ret` the return type, both in
/// their owned Rust spelling.
pub struct Callback<Args, Ret> {
    bits: u64,
    signature: PhantomData<fn(Args) -> Ret>,
}

impl<Args, Ret> Callback<Args, Ret> {
    /// Builds a callback handle from its opaque bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self {
            bits,
            signature: PhantomData,
        }
    }

    /// The opaque bits of this callback handle.
    pub const fn bits(self) -> u64 {
        self.bits
    }
}

impl<Args, Ret> Clone for Callback<Args, Ret> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Args, Ret> Copy for Callback<Args, Ret> {}

impl<Args, Ret> PartialEq for Callback<Args, Ret> {
    fn eq(&self, other: &Self) -> bool {
        self.bits == other.bits
    }
}

impl<Args, Ret> core::fmt::Debug for Callback<Args, Ret> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Callback({:#x})", self.bits)
    }
}

/// The WebIDL `any` type: an arbitrary JavaScript value held by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AnyValue(pub u64);

impl AnyValue {
    /// The sentinel bits for the JavaScript `null` value, distinct from every
    /// value-table index (which the world assigns from 0 upward). `value_to_v8`
    /// and `value_from_v8` map it to and from `null` without touching the table.
    pub const NULL: AnyValue = AnyValue(u64::MAX);
}

/// The WebIDL `object` type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct JsObject(pub u64);

/// The WebIDL `symbol` type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct JsSymbol(pub u64);

/// A callback-scoped reference to the original JavaScript buffer or view.
///
/// The token does not own bytes or a permanent root. The generated binding's
/// temporary stored-value lease owns the corresponding world value until the
/// callback returns. Backend access revalidates the live value on each use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferSourceRef { bits: u64, lease_id: u64 }

impl BufferSourceRef {
    fn new(bits: u64, lease_id: u64) -> Self { Self { bits, lease_id } }
    /// The existing world-value key, for the backend's checked bridge.
    pub const fn bits(self) -> u64 { self.bits }
    fn lease_id(self) -> u64 { self.lease_id }
}

/// A callback-scoped reference that is known to be an ArrayBufferView.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArrayBufferViewRef { bits: u64, lease_id: u64 }

impl ArrayBufferViewRef {
    fn new(bits: u64, lease_id: u64) -> Self { Self { bits, lease_id } }
    /// The existing world-value key, for the backend's checked bridge.
    pub const fn bits(self) -> u64 { self.bits }
    fn lease_id(self) -> u64 { self.lease_id }
}

// One ID space per generated binding module. A callback lease is never
// recycled, even if an engine reuses a value-table index or creates a new
// world. Exhaustion refuses the operation instead of wrapping to an old ID.
static NEXT_BUFFER_LEASE_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn next_buffer_lease_id() -> Option<u64> {
    next_buffer_lease_id_from(&NEXT_BUFFER_LEASE_ID)
}

fn next_buffer_lease_id_from(counter: &std::sync::atomic::AtomicU64) -> Option<u64> {
    use std::sync::atomic::Ordering;
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(1))
        .ok()
        .map(|previous| previous + 1)
}

/// Exact native buffer/view kind of a checked reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    ArrayBuffer, DataView,
    Int8, Uint8, Uint8Clamped, Int16, Uint16, Int32, Uint32,
    BigInt64, BigUint64, Float16, Float32, Float64,
}

/// Current view span; re-read after page-controlled conversions or resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferMetadata {
    pub kind: BufferKind,
    pub byte_offset: usize,
    pub byte_length: usize,
}

// Write helpers require independent caller-owned bytes. A source slice into
// the destination backing could alias native mutation even if its view span
// differs, so reject the entire backing range before the first write.
fn source_overlaps_backing(source: &[u8], backing: *const u8, backing_len: usize) -> bool {
    if source.is_empty() || backing_len == 0 {
        return false;
    }
    let source_start = source.as_ptr() as usize;
    let backing_start = backing as usize;
    let Some(source_end) = source_start.checked_add(source.len()) else {
        return true;
    };
    let Some(backing_end) = backing_start.checked_add(backing_len) else {
        return true;
    };
    source_start < backing_end && backing_start < source_end
}

/// The DOM exception or JavaScript error a binding method reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsError {
    kind: JsErrorKind,
    message: String,
    /// The `DOMException` name (e.g. `IndexSizeError`), set only when `kind` is
    /// `DomException`. Carried separately so the throw path can build a real
    /// `DOMException` object with the right `name`/`code`.
    dom_name: Option<String>,
    /// The engine already has the page exception or fatal stop pending.
    /// Generated glue must propagate it without creating another exception.
    pending_exception: bool,
}

/// The kind of error [`JsError`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsErrorKind {
    /// `TypeError`
    Type,
    /// `RangeError`
    Range,
    /// `SyntaxError`
    Syntax,
    /// A `DOMException` whose name is carried in the message.
    DomException,
}

impl JsError {
    /// A `TypeError` with this message.
    pub fn type_error(message: impl Into<String>) -> Self {
        Self {
            kind: JsErrorKind::Type,
            message: message.into(),
            dom_name: None,
            pending_exception: false,
        }
    }

    /// A `RangeError` with this message.
    pub fn range_error(message: impl Into<String>) -> Self {
        Self {
            kind: JsErrorKind::Range,
            message: message.into(),
            dom_name: None,
            pending_exception: false,
        }
    }

    /// A `SyntaxError` with this message.
    pub fn syntax_error(message: impl Into<String>) -> Self {
        Self {
            kind: JsErrorKind::Syntax,
            message: message.into(),
            dom_name: None,
            pending_exception: false,
        }
    }

    /// A `DOMException` with this name and message. The name is kept separate so
    /// the throw path can build a real `DOMException` object.
    pub fn dom_exception(name: &str, message: &str) -> Self {
        Self {
            kind: JsErrorKind::DomException,
            message: message.to_string(),
            dom_name: Some(name.to_string()),
            pending_exception: false,
        }
    }

    /// Marks an abrupt conversion whose original JavaScript exception is pending.
    /// The marker creates no JS root and allocates no message.
    fn pending_exception() -> Self {
        Self {
            kind: JsErrorKind::Type,
            message: String::new(),
            dom_name: None,
            pending_exception: true,
        }
    }

    /// The error kind.
    pub fn kind(&self) -> JsErrorKind {
        self.kind
    }

    /// The error message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The `DOMException` name, present only for a `DomException` error.
    pub fn dom_name(&self) -> Option<&str> {
        self.dom_name.as_deref()
    }
}

impl core::fmt::Display for JsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.pending_exception {
            return f.write_str("pending JavaScript exception");
        }
        let prefix = match self.kind {
            JsErrorKind::Type => "TypeError",
            JsErrorKind::Range => "RangeError",
            JsErrorKind::Syntax => "SyntaxError",
            JsErrorKind::DomException => self.dom_name.as_deref().unwrap_or("DOMException"),
        };
        write!(f, "{prefix}: {}", self.message)
    }
}

impl std::error::Error for JsError {}

/// Convert an ECMAScript Number to a restricted WebIDL float. Check after
/// narrowing because a finite double can overflow to an infinite f32.
pub fn restricted_f32(number: f64) -> Result<f32, JsError> {
    let narrowed = number as f32;
    if narrowed.is_finite() {
        Ok(narrowed)
    } else {
        Err(JsError::type_error("non-finite number"))
    }
}

/// Convert an ECMAScript Number to a restricted WebIDL double.
pub fn restricted_f64(number: f64) -> Result<f64, JsError> {
    if number.is_finite() {
        Ok(number)
    } else {
        Err(JsError::type_error("non-finite number"))
    }
}

/// Whether a descriptor describes an interface or a namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionKind {
    /// A regular interface with a prototype chain.
    Interface,
    /// A namespace object.
    Namespace,
}

/// The value of a WebIDL constant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConstantValue {
    /// `true` or `false`.
    Boolean(bool),
    /// An integer constant.
    Integer(i64),
    /// A floating-point constant.
    Float(f64),
    /// `null`.
    Null,
}

/// One constant of an interface or namespace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConstantDescriptor {
    /// The property name.
    pub name: &'static str,
    /// The constant value.
    pub value: ConstantValue,
}

/// One accessor property of an interface or namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeDescriptor {
    /// The property name.
    pub name: &'static str,
    /// True when the attribute has no setter.
    pub readonly: bool,
    /// True for `static attribute`.
    pub is_static: bool,
    /// True when the getter has a V8 fast-API callback.
    pub fast_getter: bool,
    /// True when the setter has a V8 fast-API callback.
    pub fast_setter: bool,
    /// The extended attribute names, in source order.
    pub extended_attributes: &'static [&'static str],
}

/// One method of an interface or namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationDescriptor {
    /// The property name.
    pub name: &'static str,
    /// True for `static` operations.
    pub is_static: bool,
    /// The `length` property: the smallest required argument count.
    pub length: usize,
    /// How many overloads share the name.
    pub overloads: usize,
    /// True when the operation has a V8 fast-API callback.
    pub fast: bool,
    /// The extended attribute names of the first overload, in source order.
    pub extended_attributes: &'static [&'static str],
}

/// Everything the runtime needs to expose one interface or namespace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InterfaceDescriptor {
    /// The WebIDL name, which is also the `Symbol.toStringTag` value.
    pub name: &'static str,
    /// Interface or namespace.
    pub kind: DefinitionKind,
    /// The inherited interface, if any.
    pub parent: Option<&'static str>,
    /// The `[Exposed]` globals, or `["*"]` for `[Exposed=*]`.
    pub exposure: &'static [&'static str],
    /// The `length` of the constructor, or `None` when the IDL declares none.
    pub constructor_length: Option<usize>,
    /// Constants, in definition order.
    pub constants: &'static [ConstantDescriptor],
    /// Attributes, in definition order.
    pub attributes: &'static [AttributeDescriptor],
    /// Operations, in definition order.
    pub operations: &'static [OperationDescriptor],
    /// The property enumeration order of the prototype, ending with
    /// `Symbol.toStringTag`.
    pub enumeration_order: &'static [&'static str],
    /// The value type of an `iterable<>` declaration, if there is one.
    pub iterable_value: Option<&'static str>,
}

/// The registry a feature crate hands to `register_*`.
///
/// Registration is cheap: it stores a descriptor and the lazy installer that
/// materialises the prototype on first access in a context
/// (`docs/spec/02-architecture.md` §2.1).
pub trait InterfaceRegistry {
    /// Records one interface or namespace.
    fn register(&mut self, descriptor: InterfaceDescriptor);
}
"#;

const V8_SUPPORT: &str = r#"
// ---------------------------------------------------------------------------
// V8 support (rusty_v8 152.x).
// ---------------------------------------------------------------------------

/// Sticky failure state, allocated when templates are installed rather than
/// while recovering from a native failure. A callback may have changed host
/// state before failing, so the owning engine must be discarded after this is set.
#[cfg(feature = "v8")]
struct NativeCallbackState {
    callback: std::cell::Cell<Option<&'static str>>,
    payload: std::cell::Cell<Option<Box<dyn std::any::Any + Send>>>,
}

#[cfg(feature = "v8")]
impl Drop for NativeCallbackState {
    fn drop(&mut self) {
        if let Some(payload) = self.payload.take() {
            drop_native_panic_payload(payload);
        }
    }
}

#[cfg(feature = "v8")]
fn drop_native_panic_payload(payload: Box<dyn std::any::Any + Send>) {
    // A user-defined panic payload may panic in Drop. Contain that secondary
    // panic as well, forgetting only its arbitrary payload rather than risk
    // an unbounded chain of panicking destructors. Ordinary payloads are freed.
    if let Err(nested) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload))) {
        std::mem::forget(nested);
    }
}

#[cfg(feature = "v8")]
fn install_native_callback_state(scope: &mut v8::PinScope<'_, '_>) {
    install_native_callback_state_for_isolate(scope);
}

/// Preallocates the failure marker before an embedder registers native V8
/// callbacks, including callbacks that can run before a context exists.
#[cfg(feature = "v8")]
pub fn install_native_callback_state_for_isolate(isolate: &mut v8::Isolate) {
    if isolate.get_slot::<NativeCallbackState>().is_none() {
        isolate.set_slot(NativeCallbackState {
            callback: std::cell::Cell::new(None),
            payload: std::cell::Cell::new(None),
        });
    }
}

/// The first native callback or recovery step that failed in this isolate.
///
/// The marker is never cleared: even if a caller cancels V8 termination, host
/// state may be inconsistent. Embedders must report a crash and discard this
/// engine, rather than treat the failure as a catchable JavaScript exception.
#[cfg(feature = "v8")]
pub fn native_callback_failure(isolate: &v8::Isolate) -> Option<&'static str> {
    isolate.get_slot::<NativeCallbackState>().and_then(|state| state.callback.get())
}

#[cfg(feature = "v8")]
fn mark_native_callback_failure(isolate: &v8::Isolate, callback: &'static str) {
    let Some(state) = isolate.get_slot::<NativeCallbackState>() else {
        return;
    };
    if state.callback.get().is_none() {
        state.callback.set(Some(callback));
    }
}

/// Poisons an isolate after a native recovery failure and stops JavaScript.
///
/// Call this only when execution cannot safely resume. Like a contained native
/// panic, this failure is sticky and cannot become a catchable JS exception.
#[cfg(feature = "v8")]
pub fn poison_native_callback(isolate: &v8::Isolate, callback: &'static str) {
    mark_native_callback_failure(isolate, callback);
    isolate.terminate_execution();
}

/// Carries an already pending V8 termination through a generated Result path
/// without allocating or replacing it with a catchable JavaScript exception.
#[cfg(feature = "v8")]
pub fn v8_pending_exception_error() -> JsError {
    JsError::pending_exception()
}

/// Propagates an existing exception in the current engine callback.
pub fn pending_js_exception_error() -> JsError {
    JsError::pending_exception()
}

/// A slow native callback's admission boundary. The owning engine may run a
/// closed wrapper graph checkpoint before argument conversion or host effects.
/// Fast API callbacks deliberately do not enter this allocating path.
#[cfg(feature = "v8")]
pub enum V8AdmissionError {
    /// A consistent, catchable allocation refusal before host effects.
    Throw(JsError),
    /// Ordinary watchdog termination or a sticky native failure is active.
    Terminated,
}

#[cfg(feature = "v8")]
pub trait V8CallbackAdmission: 'static {
    fn enter(&self, scope: &mut v8::PinScope<'_, '_>) -> Result<(), V8AdmissionError>;
    /// Recheck after page-controlled constructor argument conversion, before
    /// the host constructor can mutate a slab. No collection runs in this frame.
    fn before_constructor_host(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
    ) -> Result<(), V8AdmissionError>;
    /// Track stored JS values acquired by the current constructor or listener call.
    fn begin_stored_value_lease(&self);
    /// Begin an annotated buffer lease only inside a live callback frame.
    fn begin_checked_stored_value_lease(&self, _lease_id: u64) -> bool { false }
    /// ID of the innermost active annotated buffer lease.
    fn current_buffer_reference_lease(&self) -> Option<u64> { None }
    /// Whether a token's original lease remains active, including an outer frame.
    fn buffer_reference_lease_active(&self, _lease_id: u64) -> bool { false }
    /// Poll a hard stop only while native buffer work runs, recording it for
    /// the outer registered slow callback after its frame has been released.
    fn poll_native_buffer_work_termination(&self) -> bool { false }
    /// Read the recorded stop after frame cleanup. Nested direct boundaries
    /// must not consume the outer registered callback's decision.
    fn take_native_buffer_work_termination(&self) -> bool { false }
    fn track_stored_value(&self, bits: u64);
    /// The successful host operation has consumed those stored references.
    fn transfer_stored_value_lease(&self);
    /// Release values if conversion or the host operation returned early.
    fn end_stored_value_lease(&self, isolate: &v8::Isolate);
    fn exit(&self);
    /// Runs after a callback frame has been released and its result checked,
    /// still inside the callback's scope. A world uses it to act on native
    /// state the callback left behind, such as queueing a microtask that must
    /// be ordered with the script that caused it, or firing the events the
    /// callback owed (an element removed while focused). It runs after the
    /// callback's own effects are complete, so it may enter script but must not
    /// change what the callback returns.
    fn after_callback(&self, _scope: &mut v8::PinScope<'_, '_>) {}
    /// Runs the constructor of an interface declared `[HTMLConstructor]`
    /// (HTML's "HTML element constructor"): what it returns depends on the
    /// world's custom element definitions, not on the interface. Returns
    /// whether the world handled the call, by setting the return value or by
    /// throwing; a world without custom elements leaves the default, and the
    /// caller throws.
    fn html_constructor<'s>(
        &self,
        _scope: &mut v8::PinScope<'s, '_>,
        _args: &v8::FunctionCallbackArguments<'s>,
        _rv: &mut v8::ReturnValue<v8::Value>,
    ) -> bool {
        false
    }
}

/// The constructor body of an interface declared `[HTMLConstructor]`.
#[cfg(feature = "v8")]
pub fn v8_html_constructor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let admission = scope
        .get_slot::<V8CallbackAdmissionSlot>()
        .map(|slot| std::rc::Rc::clone(&slot.0));
    let handled = admission
        .is_some_and(|admission| admission.html_constructor(scope, &args, &mut rv));
    if !handled {
        throw_type_error(scope, "Illegal constructor");
    }
}

#[cfg(feature = "v8")]
pub fn v8_begin_stored_value_lease(scope: &v8::Isolate) {
    if let Some(slot) = scope.get_slot::<V8CallbackAdmissionSlot>() {
        slot.0.begin_stored_value_lease();
    }
}

/// Reject a reference-bearing operation before converting any argument when
/// no admitted callback frame can own its temporary roots.
#[cfg(feature = "v8")]
pub fn v8_checked_begin_stored_value_lease(scope: &v8::Isolate) -> Result<(), JsError> {
    let slot = scope.get_slot::<V8CallbackAdmissionSlot>()
        .ok_or_else(|| JsError::type_error("buffer reference callback has no admission world"))?;
    let lease_id = next_buffer_lease_id()
        .ok_or_else(|| JsError::range_error("buffer reference lease IDs exhausted"))?;
    if slot.0.begin_checked_stored_value_lease(lease_id) {
        Ok(())
    } else {
        Err(JsError::type_error("buffer reference callback has no active frame"))
    }
}

#[cfg(feature = "v8")]
fn v8_current_buffer_reference_lease(scope: &v8::Isolate) -> Result<u64, JsError> {
    scope.get_slot::<V8CallbackAdmissionSlot>()
        .and_then(|slot| slot.0.current_buffer_reference_lease())
        .ok_or_else(|| JsError::type_error("buffer reference has no active lease"))
}

#[cfg(feature = "v8")]
fn v8_validate_buffer_reference_lease(scope: &v8::Isolate, lease_id: u64) -> Result<(), JsError> {
    if scope.get_slot::<V8CallbackAdmissionSlot>()
        .is_some_and(|slot| slot.0.buffer_reference_lease_active(lease_id)) {
        Ok(())
    } else {
        Err(JsError::type_error("buffer reference escaped its callback"))
    }
}

#[cfg(feature = "v8")]
pub fn v8_check_native_buffer_work(scope: &v8::Isolate) -> Result<(), JsError> {
    if v8_buffer_work_interrupted(scope) {
        Err(JsError::pending_exception())
    } else {
        Ok(())
    }
}

#[cfg(feature = "v8")]
fn v8_buffer_work_interrupted(scope: &v8::Isolate) -> bool {
    scope.is_execution_terminating()
        || native_callback_failure(scope).is_some()
        || scope.get_slot::<V8CallbackAdmissionSlot>()
            .is_some_and(|slot| slot.0.poll_native_buffer_work_termination())
}

#[cfg(feature = "v8")]
pub fn v8_transfer_stored_value_lease(scope: &v8::Isolate) {
    if let Some(slot) = scope.get_slot::<V8CallbackAdmissionSlot>() {
        slot.0.transfer_stored_value_lease();
    }
}

#[cfg(feature = "v8")]
pub fn v8_end_stored_value_lease(scope: &v8::Isolate) {
    if let Some(slot) = scope.get_slot::<V8CallbackAdmissionSlot>() {
        slot.0.end_stored_value_lease(scope);
    }
}

/// A page getter in argument conversion may publish many wrappers after the
/// callback's initial admission. Refuse before host effects if it spent the
/// hard budget; the active conversion frame cannot collect safely.
#[cfg(feature = "v8")]
pub fn v8_before_constructor_host(
    scope: &mut v8::PinScope<'_, '_>,
) -> Result<(), V8AdmissionError> {
    if native_callback_failure(scope).is_some() {
        scope.terminate_execution();
        return Err(V8AdmissionError::Terminated);
    }
    let admission = scope
        .get_slot::<V8CallbackAdmissionSlot>()
        .map(|slot| std::rc::Rc::clone(&slot.0));
    let result = match admission {
        Some(admission) => admission.before_constructor_host(scope),
        None => Ok(()),
    };
    if native_callback_failure(scope).is_some() {
        scope.terminate_execution();
        Err(V8AdmissionError::Terminated)
    } else {
        result
    }
}

#[cfg(feature = "v8")]
struct V8CallbackAdmissionSlot(std::rc::Rc<dyn V8CallbackAdmission>);

#[cfg(feature = "v8")]
pub fn install_v8_callback_admission(
    isolate: &mut v8::Isolate,
    admission: std::rc::Rc<dyn V8CallbackAdmission>,
) {
    isolate.set_slot(V8CallbackAdmissionSlot(admission));
}

/// Removes the admission state before the V8 world is drained at teardown.
#[cfg(feature = "v8")]
pub fn clear_v8_callback_admission(isolate: &mut v8::Isolate) {
    drop(isolate.remove_slot::<V8CallbackAdmissionSlot>());
}

#[cfg(feature = "v8")]
struct V8CallbackFrame(std::rc::Rc<dyn V8CallbackAdmission>);

#[cfg(feature = "v8")]
impl Drop for V8CallbackFrame {
    fn drop(&mut self) {
        self.0.exit();
    }
}

#[cfg(feature = "v8")]
fn terminate_native_callback(
    isolate: &v8::Isolate,
    callback: &'static str,
    payload: Box<dyn std::any::Any + Send>,
) {
    mark_native_callback_failure(isolate, callback);
    if let Some(state) = isolate.get_slot::<NativeCallbackState>() {
        // Keep the payload until isolate teardown, after V8 has returned. An
        // already-active outer callback can also panic after an inner one has
        // failed; dispose the previous payload under its own guard in that case.
        if let Some(previous) = state.payload.replace(Some(payload)) {
            drop_native_panic_payload(previous);
        }
    } else {
        // Generated installers always preallocate this slot. Keep the native
        // boundary contained even if an embedder bypasses those installers.
        drop_native_panic_payload(payload);
    }
    // This uses V8's termination API without allocating handles, constructing
    // an exception or running page script. JavaScript cannot catch it.
    isolate.terminate_execution();
}

/// Contains Rust callback work, or a direct embedder invocation, without
/// unwinding across the V8 boundary.
///
/// A native panic poisons the isolate and requests termination. Registered
/// slow callbacks use [`v8_registered_slow_callback_boundary`] around this
/// core to install the termination sentinel before returning to JavaScript.
#[cfg(feature = "v8")]
pub fn v8_callback_boundary<'s, 'i, R: Copy>(
    scope: &mut v8::PinScope<'s, 'i>,
    callback: &'static str,
    fallback: R,
    call: impl FnOnce(&mut v8::PinScope<'s, 'i>) -> R,
) -> R {
    let hard_stop_requested = std::cell::Cell::new(false);
    v8_callback_boundary_inner(scope, callback, fallback, call, &hard_stop_requested)
}

#[cfg(feature = "v8")]
fn v8_callback_boundary_inner<'s, 'i, R: Copy>(
    scope: &mut v8::PinScope<'s, 'i>,
    callback: &'static str,
    fallback: R,
    call: impl FnOnce(&mut v8::PinScope<'s, 'i>) -> R,
    hard_stop_requested: &std::cell::Cell<bool>,
) -> R {
    if native_callback_failure(scope).is_some() {
        scope.terminate_execution();
        return fallback;
    }
    // The mutable scope and interior-mutable hosts are not unwind-safe to
    // resume. AssertUnwindSafe is justified by the sticky failure marker and
    // uncatchable termination: neither this operation nor its host is retried.
    let admission = scope
        .get_slot::<V8CallbackAdmissionSlot>()
        .map(|slot| std::rc::Rc::clone(&slot.0));
    let poll = admission.clone();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _frame = if let Some(admission) = admission {
            match admission.enter(scope) {
                Ok(()) => {}
                Err(V8AdmissionError::Throw(error)) => {
                    // No frame was entered. Preserve a stop observed by an
                    // outer native buffer loop even if this nested admission
                    // reports an ordinary refusal first.
                    hard_stop_requested.set(poll.as_ref().is_some_and(|admission| {
                        admission.take_native_buffer_work_termination()
                    }));
                    if native_callback_failure(scope).is_some() {
                        scope.terminate_execution();
                        return fallback;
                    }
                    throw_js_error(scope, &error);
                    return fallback;
                }
                Err(V8AdmissionError::Terminated) => {
                    hard_stop_requested.set(true);
                    scope.terminate_execution();
                    return fallback;
                }
            }
            Some(V8CallbackFrame(admission))
        } else {
            None
        };
        let value = call(scope);
        drop(_frame);
        if let Some(admission) = &poll {
            hard_stop_requested.set(admission.take_native_buffer_work_termination());
        }
        if native_callback_failure(scope).is_some() {
            scope.terminate_execution();
            return fallback;
        }
        if let Some(admission) = &poll {
            admission.after_callback(scope);
        }
        value
    })) {
        Ok(value) => value,
        Err(payload) => {
            terminate_native_callback(scope, callback, payload);
            fallback
        }
    }
}

/// Contains a callback registered through V8's slow function or property API
/// and makes a sticky native failure uncatchable before returning to JavaScript.
///
/// The ordinary boundary owns admission, Rust unwind containment, and callback
/// frame cleanup. Only after it returns may this tail synchronously install
/// V8's termination sentinel. The generic boundary remains safe for embedder
/// calls made outside a V8 callback.
///
/// # Safety
/// Call only from the owning isolate's active slow V8 function or property
/// callback, on its thread, where V8 allocation is permitted. Native callback
/// state must have been installed before registration. Never call this from a
/// Fast API callback, GC/heap hook, or a plain embedder scope.
#[cfg(feature = "v8")]
pub unsafe fn v8_registered_slow_callback_boundary<'s, 'i, R: Copy>(
    scope: &mut v8::PinScope<'s, 'i>,
    callback: &'static str,
    fallback: R,
    call: impl FnOnce(&mut v8::PinScope<'s, 'i>) -> R,
) -> R {
    let hard_stop_requested = std::cell::Cell::new(false);
    let value = v8_callback_boundary_inner(scope, callback, fallback, call, &hard_stop_requested);
    if native_callback_failure(scope).is_some() || hard_stop_requested.get() {
        // SAFETY: The caller guarantees an active allocating slow API callback.
        // A native failure, hard memory refusal, or watchdog request dominates
        // an ordinary pending exception. Engine classification keeps their
        // typed causes separate after termination.
        unsafe { scope.terminate_execution_now_from_callback() };
        fallback
    } else {
        value
    }
}

/// Contains a native V8 hook that receives an isolate rather than a callback
/// scope. Heap-limit and GC hooks use this entry point and share the same
/// sticky failure marker as generated and handwritten function callbacks.
#[cfg(feature = "v8")]
pub fn v8_isolate_callback_boundary<R: Copy>(
    isolate: &v8::Isolate,
    callback: &'static str,
    fallback: R,
    call: impl FnOnce() -> R,
) -> R {
    if native_callback_failure(isolate).is_some() {
        isolate.terminate_execution();
        return fallback;
    }
    // A panic can follow a partial host mutation, so the isolate is poisoned
    // and terminated before control returns to V8.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
        Ok(value) => {
            if native_callback_failure(isolate).is_some() {
                isolate.terminate_execution();
                fallback
            } else {
                value
            }
        }
        Err(payload) => {
            terminate_native_callback(isolate, callback, payload);
            fallback
        }
    }
}

/// Fast calls have no PinScope, but V8 supplies the current isolate in the
/// final CallbackOptions argument. No slow-call fallback may retry host work
/// after a panic; the ABI return value is only a placeholder during termination.
#[cfg(feature = "v8")]
fn v8_fast_callback_boundary<R: Copy>(
    options: &v8::fast_api::FastApiCallbackOptions<'_>,
    callback: &'static str,
    fallback: R,
    call: impl FnOnce() -> R,
) -> R {
    // SAFETY: only generated fast extern C callbacks call this helper, using
    // the options V8 supplied for the current call. The shared isolate view
    // never escapes this callback and does not create a handle scope.
    if native_callback_failure(unsafe { options.isolate_unchecked() }).is_some() {
        // SAFETY: same active callback, with no host operation in progress.
        unsafe { options.isolate_unchecked() }.terminate_execution();
        return fallback;
    }
    // As above: failure poisons and terminates the engine, never resumes it.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
        Ok(value) => {
            // A finalizer reached during the fast host body can poison the
            // isolate without unwinding this callback itself.
            if native_callback_failure(unsafe { options.isolate_unchecked() }).is_some() {
                unsafe { options.isolate_unchecked() }.terminate_execution();
                fallback
            } else {
                value
            }
        }
        Err(payload) => {
            // SAFETY: the caught host operation has finished unwinding. No
            // isolate reference is retained across that operation or escapes.
            terminate_native_callback(unsafe { options.isolate_unchecked() }, callback, payload);
            fallback
        }
    }
}

/// The `v8::EmbedderDataTypeTag` used for wrapper internal fields.
///
/// V8 152 only guarantees `kEmbedderDataTypeTagDefault` to be inside
/// `0..V8_EMBEDDER_DATA_TAG_COUNT`, so every interface uses it and relies on
/// `v8::Signature` for the brand check instead.
#[cfg(feature = "v8")]
pub const WRAPPER_TAG: u16 = 0;

/// The internal field index that holds the host pointer.
#[cfg(feature = "v8")]
pub const WRAPPER_HOST_FIELD: i32 = 0;

/// Per-isolate cache of the constructor templates installed so far.
///
/// `install_*` looks the parent interface up here before calling
/// `FunctionTemplate::inherit`, so parents must be installed first. Each entry
/// also keeps a no-op function template carrying the interface signature. The
/// pinned rusty_v8 release does not expose `FunctionTemplate::HasInstance`, so
/// calling that no-op under a `TryCatch` is the equivalent native brand test:
/// V8 accepts instances of this template (including inherited templates) and
/// rejects unrelated objects before the callback runs.
#[cfg(feature = "v8")]
#[derive(Default)]
pub struct TemplateCache {
    templates: Vec<(
        &'static str,
        v8::Global<v8::FunctionTemplate>,
        v8::Global<v8::FunctionTemplate>,
    )>,
}

#[cfg(feature = "v8")]
impl TemplateCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Stores a constructor template under its interface name.
#[cfg(feature = "v8")]
pub fn cache_template(
    scope: &mut v8::PinScope<'_, '_>,
    name: &'static str,
    template: v8::Local<v8::FunctionTemplate>,
) {
    install_native_callback_state(scope);
    let global = v8::Global::new(scope.as_ref(), template);
    let signature = v8::Signature::new(scope, template);
    let brand_probe = v8::FunctionTemplate::builder(probe_interface_brand)
        .signature(signature)
        .build(scope);
    let brand_probe = v8::Global::new(scope.as_ref(), brand_probe);
    if scope.get_slot::<TemplateCache>().is_none() {
        scope.set_slot(TemplateCache::new());
    }
    if let Some(cache) = scope.get_slot_mut::<TemplateCache>() {
        cache.templates.retain(|(cached, _, _)| *cached != name);
        cache.templates.push((name, global, brand_probe));
    }
}

/// The callback behind a cached interface brand probe.
///
/// A matching receiver reaches this empty body. V8 rejects every other
/// receiver through the function template's signature before entering it.
#[cfg(feature = "v8")]
fn probe_interface_brand<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<v8::Value>,
) {
    // Signature validation is an internal, no-op brand query. It neither
    // allocates a wrapper nor enters host code, so the wrapper-pressure
    // admission limit must not turn a valid object into a failed brand check.
    v8_isolate_callback_boundary(scope, "probe_interface_brand", (), || {});
}

/// Looks a previously installed constructor template up by interface name.
#[cfg(feature = "v8")]
pub fn cached_template<'s>(
    scope: &v8::PinScope<'s, '_>,
    name: &str,
) -> Option<v8::Local<'s, v8::FunctionTemplate>> {
    let cache = scope.get_slot::<TemplateCache>()?;
    let global = cache
        .templates
        .iter()
        .find(|(cached, _, _)| *cached == name)
        .map(|(_, template, _)| template)?;
    Some(v8::Local::new(scope, global))
}

/// Looks up the native receiver check paired with an interface template.
#[cfg(feature = "v8")]
fn cached_brand_probe<'s>(
    scope: &v8::PinScope<'s, '_>,
    name: &str,
) -> Option<v8::Local<'s, v8::FunctionTemplate>> {
    let cache = scope.get_slot::<TemplateCache>()?;
    let global = cache
        .templates
        .iter()
        .find(|(cached, _, _)| *cached == name)
        .map(|(_, _, brand_probe)| brand_probe)?;
    Some(v8::Local::new(scope, global))
}

/// Runs one cached `FunctionTemplate` receiver check without exposing its
/// expected TypeError to script.
#[cfg(feature = "v8")]
fn matches_interface_template(
    scope: &mut v8::PinScope<'_, '_>,
    name: &str,
    value: v8::Local<v8::Value>,
) -> bool {
    let Some(probe) = cached_brand_probe(scope, name) else {
        return false;
    };
    let Some(probe) = probe.get_function(scope) else {
        return false;
    };
    v8::tc_scope!(let try_catch, scope);
    probe.call(try_catch, value, &[]).is_some()
}

/// Whether `value` is an instance of `T`'s WebIDL interface.
///
/// This is a native template test rather than JavaScript `instanceof`, so an
/// object whose prototype was merely set to `T.prototype` does not acquire the
/// platform brand. V8's template inheritance makes genuine derived interface
/// wrappers satisfy their parent interface.
#[cfg(feature = "v8")]
fn matches_interface_brand<T: InterfaceMarker>(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> bool {
    let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
        return false;
    };
    let interface = T::INTERFACE_NAME;

    // The page global is the Window wrapper and deliberately has no API-object
    // internal field. This IDL slice models Window's inherited event surface as
    // EventTarget. The opaque Window and WindowProxy markers identify this same
    // global; no unrelated interface acquires its brand by sharing the object.
    if matches!(interface, "EventTarget" | "Window" | "WindowProxy") {
        let global = scope.get_current_context().global(scope);
        if object.strict_equals(global.into()) {
            return true;
        }
    }

    if matches_interface_template(scope, interface, value) {
        return true;
    }

    // This tree stores attributes outside its node arena, so `Attr` is a
    // standalone interface even though the DOM Standard makes it a Node. Keep
    // the existing narrow compatibility path: Node-typed range endpoints must
    // reach the host's InvalidNodeTypeError check, while every other unrelated
    // platform wrapper is rejected here as a TypeError.
    interface == "Node" && matches_interface_template(scope, "Attr", value)
}

/// Maps interface handles to their wrapper objects for one world.
///
/// `tenta-js` implements this over the per-world wrapper maps described in
/// `docs/spec/05-javascript.md` §3 and stores it in an isolate slot.
#[cfg(feature = "v8")]
pub trait WrapperWorld: 'static {
    /// Returns the wrapper for `handle`, creating it if the world allows.
    fn wrapper<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        interface: &'static str,
        handle: u64,
    ) -> Result<Option<v8::Local<'s, v8::Object>>, JsError>;

    /// Returns the handle a wrapper stands for.
    ///
    /// Takes the scope because not every object that stands for a handle is an
    /// interface instance: the global object *is* the window, and recognising
    /// it means comparing it against the current context's global. A world
    /// whose interfaces never include such an object may ignore the scope;
    /// [`handle_from_v8`] passes it for the worlds that need it.
    fn handle(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        interface: &'static str,
        wrapper: v8::Local<v8::Object>,
    ) -> Option<u64>;

    /// Stores an arbitrary value and returns the bits that stand for it.
    ///
    /// This backs the `any`, `object`, `symbol`, `Promise<T>` and callback
    /// types, whose Rust forms are opaque bit patterns.
    fn store_value(&self, scope: &mut v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> u64;

    /// Returns the value `bits` stands for.
    fn load_value<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        bits: u64,
    ) -> Option<v8::Local<'s, v8::Value>>;

    /// Reads the pristine ArrayBuffer resizable flag in this world's context.
    /// The getter is world-owned, never stored in a Context-owned slot.
    fn array_buffer_resizable<'s>(
        &self,
        _scope: &mut v8::PinScope<'s, '_>,
        _buffer: v8::Local<'s, v8::ArrayBuffer>,
    ) -> Result<bool, JsError> {
        Err(JsError::type_error("buffer reference intrinsic was not captured"))
    }

    /// Reads an optional pristine immutable flag. A missing intrinsic is
    /// allowed only when that feature was absent in the world's bootstrap.
    fn array_buffer_immutable<'s>(
        &self,
        _scope: &mut v8::PinScope<'s, '_>,
        _buffer: v8::Local<'s, v8::ArrayBuffer>,
    ) -> Result<bool, JsError> {
        Err(JsError::type_error("buffer reference intrinsic was not captured"))
    }

    /// Returns the realm (host context) a constructor downcasts to reach the
    /// document and event slab. See [`Realm`].
    fn realm(&self) -> Realm;

    /// Takes ownership of `host` for a `wrapper` object V8 allocated for a
    /// `new` call: writes the host pointer into the wrapper's internal field,
    /// registers the GC finalizer, and records the wrapper's identity — the
    /// constructor counterpart of [`WrapperWorld::wrapper`]. `interface` is the
    /// WebIDL name of the constructed type.
    fn adopt(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        interface: &'static str,
        wrapper: v8::Local<v8::Object>,
        host: Box<dyn std::any::Any>,
    ) -> Result<(), JsError>;
}

#[cfg(feature = "v8")]
struct WrapperWorldSlot(Box<dyn WrapperWorld>);

/// The realm's original `Reflect.set`, retained before page script can replace it.
#[cfg(feature = "v8")]
struct LegacyReflectSetSlot(v8::Global<v8::Function>);

#[cfg(feature = "v8")]
fn capture_legacy_reflect_set(
    scope: &mut v8::PinScope<'_, '_>,
) -> Result<v8::Global<v8::Function>, JsError> {
    let global = scope.get_current_context().global(scope);
    let reflect_key = v8_string(scope, "Reflect")?;
    let reflect = global
        .get(scope, reflect_key.into())
        .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
        .ok_or_else(|| JsError::type_error("the context has no `Reflect` global"))?;
    let set_key = v8_string(scope, "set")?;
    let set = reflect
        .get(scope, set_key.into())
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
        .ok_or_else(|| JsError::type_error("`Reflect.set` is not callable"))?;
    Ok(v8::Global::new(scope, set))
}

/// Runs the captured `Reflect.set` so its Boolean result and Proxy traps are
/// preserved. `None` leaves the original JavaScript exception pending.
#[cfg(feature = "v8")]
fn legacy_reflect_set(
    scope: &mut v8::PinScope<'_, '_>,
    target: v8::Local<v8::Object>,
    property: v8::Local<v8::Value>,
    value: v8::Local<v8::Value>,
    receiver: v8::Local<v8::Object>,
) -> Option<bool> {
    let set = {
        let Some(slot) = scope.get_slot::<LegacyReflectSetSlot>() else {
            throw_js_error(scope, &JsError::type_error("the wrapper world has no `Reflect.set` intrinsic"));
            return None;
        };
        v8::Local::new(scope, &slot.0)
    };
    let this = v8::undefined(scope);
    let result = set.call(
        scope,
        this.into(),
        &[target.into(), property, value, receiver.into()],
    )?;
    Some(result.is_true())
}

/// Installs the wrapper world for this isolate.
#[cfg(feature = "v8")]
pub fn set_wrapper_world(
    scope: &mut v8::PinScope<'_, '_>,
    world: Box<dyn WrapperWorld>,
) -> Result<(), JsError> {
    let set = capture_legacy_reflect_set(scope)?;
    scope.set_slot(LegacyReflectSetSlot(set));
    scope.set_slot(WrapperWorldSlot(world));
    Ok(())
}

/// Releases the captured legacy setter while its V8 isolate is still alive.
#[cfg(feature = "v8")]
pub fn clear_legacy_reflect_set(isolate: &mut v8::Isolate) {
    drop(isolate.remove_slot::<LegacyReflectSetSlot>());
}

/// Temporarily lends the world and scope to a conversion, restoring the world
/// slot before an unwind reaches the callback boundary. Dropping the removed
/// world during unwind could free hosts whose wrappers still exist in V8.
#[cfg(feature = "v8")]
fn with_wrapper_world<'s, 'i, R>(
    scope: &mut v8::PinScope<'s, 'i>,
    call: impl FnOnce(&dyn WrapperWorld, &mut v8::PinScope<'s, 'i>) -> R,
) -> Result<R, JsError> {
    let world = scope
        .remove_slot::<WrapperWorldSlot>()
        .ok_or_else(|| JsError::type_error("no wrapper world is installed for this isolate"))?;
    // This does not recover or reuse host state: restore ownership, then pass
    // the original panic to the outer callback boundary to poison the isolate.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        call(world.0.as_ref(), scope)
    }));
    scope.set_slot(world);
    match outcome {
        Ok(value) => Ok(value),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Converts a handle to its wrapper object.
#[cfg(feature = "v8")]
pub fn handle_to_v8<'s, T: InterfaceMarker>(
    scope: &mut v8::PinScope<'s, '_>,
    handle: Handle<T>,
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    let interface = T::INTERFACE_NAME;
    let bits = handle.bits();
    let wrapper = with_wrapper_world(scope, |world, scope| world.wrapper(scope, interface, bits))??;
    match wrapper {
        Some(object) => Ok(object.into()),
        None => Err(JsError::type_error("the host object has no wrapper")),
    }
}

/// Converts a wrapper object back to a handle.
#[cfg(feature = "v8")]
pub fn handle_from_v8<T: InterfaceMarker>(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<Handle<T>, JsError> {
    interface_handle_from_v8::<T>(scope, value)?.ok_or_else(|| {
        JsError::type_error(format!(
            "the value is not of type '{}'",
            T::INTERFACE_NAME
        ))
    })
}

/// Returns `T`'s handle when `value` is an actual wrapper carrying its brand.
///
/// Generated union conversion uses `None` to continue to its remaining arms.
/// Requiring both the template signature and the wrapper world's own handle
/// lookup matters for objects such as `Object.create(Node.prototype)` and
/// `Object.create(realNode)`: a signature may find a matching holder in their
/// prototype chain, but neither object is itself a Node wrapper.
#[cfg(feature = "v8")]
fn interface_handle_from_v8<T: InterfaceMarker>(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<Option<Handle<T>>, JsError> {
    if !matches_interface_brand::<T>(scope, value) {
        return Ok(None);
    }
    let interface = T::INTERFACE_NAME;
    let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
        return Ok(None);
    };
    // The world is taken out of the scope for the duration of the call, as in
    // `handle_to_v8`, because `handle` needs the scope mutably to ask the realm
    // whether this object is its global.
    //
    // INVARIANT: nothing on `handle`'s path may call back into this function or
    // `handle_to_v8` while the slot is out — they would find no world. Resolving
    // a receiver only reads: it neither runs script nor creates a wrapper.
    let handle = with_wrapper_world(scope, |world, scope| world.handle(scope, interface, object))?;
    Ok(handle.map(Handle::from_bits))
}

/// Reads the host object out of a wrapper's internal field.
///
/// # Safety
///
/// The receiver must have been created from the interface's instance template
/// and its host field must hold a live `*const T` written by [`attach_host`].
/// Generated callbacks satisfy this by carrying the interface's
/// `v8::Signature`, which makes V8 reject any other receiver before the
/// callback runs.
#[cfg(feature = "v8")]
pub unsafe fn unwrap_this<'a, T>(this: &v8::Local<v8::Object>) -> Result<&'a T, JsError> {
    if this.internal_field_count() <= WRAPPER_HOST_FIELD as usize {
        return Err(JsError::type_error("the receiver is not an interface instance"));
    }
    // SAFETY: the field index is in range and the caller guarantees that the
    // field was written by `attach_host::<T>`.
    let pointer =
        unsafe { this.get_aligned_pointer_from_internal_field(WRAPPER_HOST_FIELD, WRAPPER_TAG) };
    if pointer.is_null() {
        return Err(JsError::type_error("the receiver has no host object"));
    }
    // SAFETY: as above; the host object outlives the wrapper.
    Ok(unsafe { &*(pointer as *const T) })
}

/// Moves `host` into the wrapper's internal field.
///
/// The wrapper owns the allocation from this point; [`detach_host`] gives it
/// back so a weak callback can drop it.
#[cfg(feature = "v8")]
pub fn attach_host<T>(this: &v8::Local<v8::Object>, host: T) {
    let pointer = Box::into_raw(Box::new(host));
    this.set_aligned_pointer_in_internal_field(
        WRAPPER_HOST_FIELD,
        pointer as *const std::ffi::c_void,
        WRAPPER_TAG,
    );
}

/// Takes the host object back out of a wrapper and clears the field.
///
/// # Safety
///
/// The field must hold a `*mut T` written by [`attach_host::<T>`] and must not
/// have been detached already.
#[cfg(feature = "v8")]
pub unsafe fn detach_host<T>(this: &v8::Local<v8::Object>) -> Option<Box<T>> {
    if this.internal_field_count() <= WRAPPER_HOST_FIELD as usize {
        return None;
    }
    // SAFETY: the field index is in range and the caller guarantees the type.
    let pointer =
        unsafe { this.get_aligned_pointer_from_internal_field(WRAPPER_HOST_FIELD, WRAPPER_TAG) };
    if pointer.is_null() {
        return None;
    }
    this.set_aligned_pointer_in_internal_field(
        WRAPPER_HOST_FIELD,
        std::ptr::null(),
        WRAPPER_TAG,
    );
    // SAFETY: the pointer came from `Box::into_raw` in `attach_host::<T>`.
    Some(unsafe { Box::from_raw(pointer as *mut T) })
}

/// Reads the host object out of a fast-call receiver.
///
/// # Safety
///
/// Same invariant as [`unwrap_this`]. Fast callbacks are only reachable
/// through a `FunctionTemplate` that carries the interface signature.
#[cfg(feature = "v8")]
pub unsafe fn unwrap_receiver<'a, T>(receiver: v8::Local<v8::Object>) -> Option<&'a T> {
    if receiver.internal_field_count() <= WRAPPER_HOST_FIELD as usize {
        return None;
    }
    // SAFETY: the field index is in range and the caller guarantees the type.
    let pointer = unsafe {
        receiver.get_aligned_pointer_from_internal_field(WRAPPER_HOST_FIELD, WRAPPER_TAG)
    };
    if pointer.is_null() {
        return None;
    }
    // SAFETY: as above.
    Some(unsafe { &*(pointer as *const T) })
}

/// Interns a Rust string as a V8 string.
#[cfg(feature = "v8")]
pub fn v8_string<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: &str,
) -> Result<v8::Local<'s, v8::String>, JsError> {
    v8::String::new(scope, value)
        .ok_or_else(|| JsError::type_error("the string is too long for this isolate"))
}

/// Throws a `TypeError` with this message.
#[cfg(feature = "v8")]
pub fn throw_type_error(scope: &mut v8::PinScope<'_, '_>, message: &str) {
    if let Some(message) = v8::String::new(scope, message) {
        let error = v8::Exception::type_error(scope, message);
        scope.throw_exception(error);
    }
}

/// Saves the realm's DOMException prototype behind an engine-private key.
///
/// Call after the realm's DOMException bootstrap and before page script runs.
/// Later exception creation must not consult a mutable global constructor or
/// page-controlled `Object.create`/`Object.defineProperty` functions.
#[cfg(feature = "v8")]
pub fn capture_v8_dom_exception_prototype(scope: &mut v8::PinScope<'_, '_>) -> bool {
    let context = scope.get_current_context();
    let global = context.global(scope);
    let Some(key) = v8::String::new(scope, "DOMException") else {
        return false;
    };
    let Some(constructor) = global.get(scope, key.into()) else {
        return false;
    };
    let Ok(constructor) = v8::Local::<v8::Object>::try_from(constructor) else {
        return false;
    };
    let Some(key) = v8::String::new(scope, "prototype") else {
        return false;
    };
    let Some(prototype) = constructor.get(scope, key.into()) else {
        return false;
    };
    if !prototype.is_object() {
        return false;
    }
    let Some(key) = v8::String::new(scope, "TentaBrowser#DOMException.prototype") else {
        return false;
    };
    let private = v8::Private::for_api(scope, Some(key));
    global.set_private(scope, private, prototype) == Some(true)
}

#[cfg(feature = "v8")]
fn v8_dom_exception<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
    message: v8::Local<'s, v8::String>,
) -> Option<v8::Local<'s, v8::Value>> {
    let global = scope.get_current_context().global(scope);
    let key = v8::String::new(scope, "TentaBrowser#DOMException.prototype")?;
    let private = v8::Private::for_api(scope, Some(key));
    let prototype = global.get_private(scope, private)?;
    if !prototype.is_object() {
        return None;
    }
    // The native Error factory captures the stack without invoking a mutable
    // page constructor. Keep that stack when applying the DOMException brand.
    let exception = v8::Exception::error(scope, message);
    let exception = v8::Local::<v8::Object>::try_from(exception).ok()?;
    if exception.set_prototype(scope, prototype) != Some(true) {
        return None;
    }
    let name_value = v8::String::new(scope, name)?;
    let name_key = v8::String::new(scope, "name")?;
    if exception.define_own_property(
        scope,
        name_key.into(),
        name_value.into(),
        v8::PropertyAttribute::DONT_ENUM | v8::PropertyAttribute::READ_ONLY,
    ) != Some(true) {
        return None;
    }
    let message_key = v8::String::new(scope, "message")?;
    if exception.define_own_property(
        scope,
        message_key.into(),
        message.into(),
        v8::PropertyAttribute::DONT_ENUM | v8::PropertyAttribute::READ_ONLY,
    ) != Some(true) {
        return None;
    }
    Some(exception.into())
}

/// Throws the JavaScript error a binding method reported.
#[cfg(feature = "v8")]
pub fn throw_js_error(scope: &mut v8::PinScope<'_, '_>, error: &JsError) {
    if error.pending_exception {
        return;
    }
    let Some(message) = v8::String::new(scope, error.message()) else {
        return;
    };
    let exception = match error.kind() {
        JsErrorKind::Type => v8::Exception::type_error(scope, message),
        JsErrorKind::Range => v8::Exception::range_error(scope, message),
        JsErrorKind::Syntax => v8::Exception::syntax_error(scope, message),
        JsErrorKind::DomException => {
            // The saved prototype and native own-data definitions avoid page
            // constructor replacement or prototype setters during a throw.
            let name = error.dom_name().unwrap_or("Error");
            // Standalone generated-binding contexts may omit the browser's
            // bootstrap; retain their previous safe TypeError fallback.
            v8_dom_exception(scope, name, message)
                .unwrap_or_else(|| v8::Exception::type_error(scope, message))
        }
    };
    scope.throw_exception(exception);
}

/// WebIDL `ToNumber` for an argument.
#[cfg(feature = "v8")]
pub fn number_from_v8(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<f64, JsError> {
    value.number_value(scope).ok_or_else(JsError::pending_exception)
}

/// WebIDL `boolean` conversion.
#[cfg(feature = "v8")]
pub fn bool_from_v8(scope: &mut v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> bool {
    value.boolean_value(scope)
}

/// WebIDL `unsigned long` conversion straight through V8's `ToUint32`.
#[cfg(feature = "v8")]
pub fn u32_from_v8(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<u32, JsError> {
    value
        .to_uint32(scope)
        .map(|number| number.value())
        .ok_or_else(JsError::pending_exception)
}

/// WebIDL `long` conversion straight through V8's `ToInt32`.
#[cfg(feature = "v8")]
pub fn i32_from_v8(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<i32, JsError> {
    value.int32_value(scope).ok_or_else(JsError::pending_exception)
}

/// WebIDL `DOMString` conversion.
#[cfg(feature = "v8")]
pub fn string_from_v8(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<String, JsError> {
    let string = value.to_string(scope).ok_or_else(JsError::pending_exception)?;
    Ok(string.to_rust_string_lossy(scope.as_ref()))
}

/// Converts a Rust string to a JavaScript value.
#[cfg(feature = "v8")]
pub fn string_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: &str,
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    Ok(v8_string(scope, value)?.into())
}

/// WebIDL `ByteString` conversion: one code unit per byte, isomorphically.
///
/// WebIDL defines the conversion as `ToString` followed by isomorphic encoding,
/// so `"\u{FF}"` is the single byte `0xFF` and not the two bytes its UTF-8 form
/// needs. The string is therefore read through `write_v2` and each code unit is
/// narrowed by hand: the one-byte writer would truncate a code unit above
/// `0xFF` to its low byte, while WebIDL requires a `TypeError` for one.
#[cfg(feature = "v8")]
pub fn byte_string_from_v8(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<Vec<u8>, JsError> {
    let string = value.to_string(scope).ok_or_else(JsError::pending_exception)?;
    let mut units = vec![0u16; string.length()];
    string.write_v2(scope.as_ref(), 0, &mut units, v8::WriteFlags::empty());
    let mut bytes = Vec::with_capacity(units.len());
    for unit in units {
        let byte = u8::try_from(unit).map_err(|_| {
            JsError::type_error("the argument is not a valid ByteString: it has a code unit above 0xFF")
        })?;
        bytes.push(byte);
    }
    Ok(bytes)
}

/// Converts a WebIDL `ByteString` to a JavaScript value, isomorphically.
///
/// The mirror image of [`byte_string_from_v8`]: one byte per code unit, through
/// the one-byte string constructor, so `0xFF` becomes `"\u{FF}"` rather than the
/// replacement characters a UTF-8 round trip would produce.
#[cfg(feature = "v8")]
pub fn byte_string_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: &[u8],
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    let string = v8::String::new_from_one_byte(scope, value, v8::NewStringType::Normal)
        .ok_or_else(|| JsError::type_error("the string is too long for this isolate"))?;
    Ok(string.into())
}

/// Converts a sequence to a JavaScript array.
#[cfg(feature = "v8")]
pub fn sequence_to_v8<'s, T, F>(
    scope: &mut v8::PinScope<'s, '_>,
    items: Vec<T>,
    mut convert: F,
) -> Result<v8::Local<'s, v8::Value>, JsError>
where
    F: FnMut(&mut v8::PinScope<'s, '_>, T) -> Result<v8::Local<'s, v8::Value>, JsError>,
{
    let mut elements = Vec::with_capacity(items.len());
    for item in items {
        elements.push(convert(scope, item)?);
    }
    Ok(v8::Array::new_with_elements(scope, &elements).into())
}

/// Converts a record to a JavaScript object with own data properties.
///
/// `key` is the *declared* key type's conversion and `convert` the value
/// type's, so a record's key type is as faithful here as it is on the way in.
/// A `ByteString` key of `0xE9` is therefore defined under the one-code-unit
/// name `"\u{E9}"`, not the two characters its UTF-8 form would spell.
#[cfg(feature = "v8")]
pub fn record_to_v8<'s, K, V, F, G>(
    scope: &mut v8::PinScope<'s, '_>,
    entries: Vec<(K, V)>,
    mut key: G,
    mut convert: F,
) -> Result<v8::Local<'s, v8::Value>, JsError>
where
    G: FnMut(&mut v8::PinScope<'s, '_>, K) -> Result<v8::Local<'s, v8::Value>, JsError>,
    F: FnMut(&mut v8::PinScope<'s, '_>, V) -> Result<v8::Local<'s, v8::Value>, JsError>,
{
    let object = v8::Object::new(scope);
    for (entry, value) in entries {
        let key = key(scope, entry)?;
        let key = v8::Local::<v8::String>::try_from(key)
            .map_err(|_| JsError::type_error("the record key must be a string"))?;
        let value = convert(scope, value)?;
        let defined = object
            .create_data_property(scope, key.into(), value)
            .ok_or_else(JsError::pending_exception)?;
        if !defined {
            return Err(JsError::type_error("the record entry cannot be defined"));
        }
    }
    Ok(object.into())
}

/// The isolate slot holding the captured `%Array.prototype.values%`.
#[cfg(feature = "v8")]
struct ArrayValuesSlot(v8::Global<v8::Function>);

/// Returns `array` as an Array Iterator.
///
/// This is the whole of WebIDL's `iterable<K, V>` machinery for a collection
/// the host can only hand over as a whole. A pair iterator has no `length` and
/// no indexed accessor to drive an iterator from, so the generated `entries`,
/// `keys` and `values` members each build a plain array from one snapshot and
/// hand it here; one intrinsic serves all three, and no per-interface iterator
/// object has to exist.
///
/// The returned object is an Array Iterator, not an interface-branded one:
/// `Object.prototype.toString.call(it)` is `"[object Array Iterator]"`, and it
/// is not an `instanceof` any DOM interface.
#[cfg(feature = "v8")]
pub fn array_iterator<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    array: v8::Local<'s, v8::Array>,
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    let values = array_values(scope)?;
    call_js_function(scope, values, array.into(), &[])
}

/// This isolate's `%Array.prototype.values%`, captured on first use.
#[cfg(feature = "v8")]
fn array_values<'s>(
    scope: &mut v8::PinScope<'s, '_>,
) -> Result<v8::Local<'s, v8::Function>, JsError> {
    if let Some(slot) = scope.get_slot::<ArrayValuesSlot>() {
        return Ok(v8::Local::new(scope, &slot.0));
    }
    let values = array_prototype_values(scope)?;
    scope.set_slot(ArrayValuesSlot(v8::Global::new(scope.as_ref(), values)));
    Ok(values)
}

/// Reads `Array.prototype.values` out of the current context.
#[cfg(feature = "v8")]
fn array_prototype_values<'s>(
    scope: &mut v8::PinScope<'s, '_>,
) -> Result<v8::Local<'s, v8::Function>, JsError> {
    let context = scope.get_current_context();
    let global = context.global(scope);
    let key = v8_string(scope, "Array")?;
    let array = global
        .get(scope, key.into())
        .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
        .ok_or_else(|| JsError::type_error("the context has no `Array` global"))?;
    let key = v8_string(scope, "prototype")?;
    let prototype = array
        .get(scope, key.into())
        .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
        .ok_or_else(|| JsError::type_error("`Array.prototype` is not an object"))?;
    let key = v8_string(scope, "values")?;
    prototype
        .get(scope, key.into())
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
        .ok_or_else(|| JsError::type_error("`Array.prototype.values` is not callable"))
}

/// Calls `function` with `receiver` as its `this` and `arguments` as its
/// argument list.
///
/// An exception the call throws is rethrown into `scope`, and the returned
/// error is only the pending-exception marker. The caller must not turn it into
/// a new host TypeError. Relying on the `TryCatch` destructor instead would
/// swallow the original page value.
#[cfg(feature = "v8")]
pub fn call_js_function<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    function: v8::Local<'s, v8::Function>,
    receiver: v8::Local<'s, v8::Value>,
    arguments: &[v8::Local<'s, v8::Value>],
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    v8::tc_scope!(let tc, scope);
    match function.call(tc, receiver, arguments) {
        Some(value) => Ok(value),
        None => {
            tc.rethrow();
            Err(JsError::pending_exception())
        }
    }
}

/// Stores an arbitrary JavaScript value in the world's value table.
#[cfg(feature = "v8")]
pub fn value_from_v8(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<u64, JsError> {
    // The JavaScript `null` maps to the sentinel bits, never into the value
    // table, so a null `any` never allocates or holds a reference.
    if value.is_null() {
        return Ok(AnyValue::NULL.0);
    }
    let bits = with_wrapper_world(scope, |world, scope| world.store_value(scope, value))?;
    if let Some(slot) = scope.get_slot::<V8CallbackAdmissionSlot>() {
        slot.0.track_stored_value(bits);
    }
    Ok(bits)
}

/// Reads an arbitrary JavaScript value back out of the world's value table.
#[cfg(feature = "v8")]
pub fn value_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bits: u64,
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    // The null sentinel maps straight back to `null` without a table lookup.
    if bits == AnyValue::NULL.0 {
        return Ok(v8::null(scope).into());
    }
    let value = with_wrapper_world(scope, |world, scope| world.load_value(scope, bits))?;
    value.ok_or_else(|| JsError::type_error("the value is no longer live"))
}

/// Returns the current isolate's realm for a constructor callback.
#[cfg(feature = "v8")]
pub fn world_realm(scope: &mut v8::PinScope<'_, '_>) -> Result<Realm, JsError> {
    let world = scope
        .get_slot::<WrapperWorldSlot>()
        .ok_or_else(|| JsError::type_error("no wrapper world is installed for this isolate"))?;
    Ok(world.0.realm())
}

/// Adopts a constructor's `this` object into the world. The world needs
/// `&mut scope` for the finalizer, so the slot is removed for the call and
/// restored after, exactly as in [`handle_to_v8`].
#[cfg(feature = "v8")]
pub fn adopt_host<T: std::any::Any>(
    scope: &mut v8::PinScope<'_, '_>,
    interface: &'static str,
    this: v8::Local<v8::Object>,
    host: T,
) -> Result<(), JsError> {
    with_wrapper_world(scope, |world, scope| world.adopt(scope, interface, this, Box::new(host)))?
}

/// Converts a JavaScript value to a sequence through `@@iterator`.
///
/// WebIDL's sequence conversion is `GetMethod(V, @@iterator)` followed by
/// `GetIterator`, not an array check: `new URLSearchParams(new Map([["a",
/// "b"]]))` has to see the map's entries, and so does any other iterable. An
/// array takes the same path, because `%Array.prototype[@@iterator]%` is just a
/// method, so there is no special case to get wrong. A `value` that is not an
/// object, or whose `@@iterator` is absent or not callable, is the `TypeError`
/// the spec prescribes.
#[cfg(feature = "v8")]
pub fn sequence_from_v8<'s, T, F>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    mut convert: F,
) -> Result<Vec<T>, JsError>
where
    F: FnMut(&mut v8::PinScope<'s, '_>, v8::Local<'s, v8::Value>) -> Result<T, JsError>,
{
    let iterator = iterator_from_v8(scope, value)?
        .ok_or_else(|| JsError::type_error("the value is not iterable, so it is not a sequence"))?;
    let mut items = Vec::new();
    while let Some(element) = iterator_next(scope, iterator)? {
        items.push(convert(scope, element)?);
    }
    Ok(items)
}

/// `GetMethod(value, @@iterator)` as a callable, or `None` when `value` has no
/// usable one.
///
/// The `Get` is the observable one the spec performs, which is why a union arm
/// distinguishes a sequence by probing here rather than checking `is_array`: a
/// proxy sees the read.
#[cfg(feature = "v8")]
fn iteration_method<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<Option<v8::Local<'s, v8::Function>>, JsError> {
    let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
        return Ok(None);
    };
    let key = v8::Symbol::get_iterator(scope);
    let method = object
        .get(scope, key.into())
        .ok_or_else(JsError::pending_exception)?;
    if method.is_null_or_undefined() {
        return Ok(None);
    }
    v8::Local::<v8::Function>::try_from(method)
        .map(Some)
        .map_err(|_| JsError::type_error("the `@@iterator` of the value is not callable"))
}

/// Whether `value` has a callable `@@iterator`, which is what selects a union's
/// `sequence<T>` arm.
///
/// The probe performs the `Get` the spec performs, so a proxy or an accessor
/// observes it. An abrupt getter or a non-callable value is an abrupt union
/// conversion too; only `null` or `undefined` means that the remaining arms
/// should be tried.
#[cfg(feature = "v8")]
pub fn has_iterator<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<bool, JsError> {
    iteration_method(scope, value).map(|method| method.is_some())
}

/// `value`'s iterator object, as WebIDL's `GetIterator` obtains it.
#[cfg(feature = "v8")]
fn iterator_from_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<Option<v8::Local<'s, v8::Object>>, JsError> {
    let Some(method) = iteration_method(scope, value)? else {
        return Ok(None);
    };
    let iterator = call_js_function(scope, method, value, &[])?;
    Ok(v8::Local::<v8::Object>::try_from(iterator).ok())
}

/// Advances `iterator` once and returns the yielded value; `None` ends it.
///
/// `next` is read on every step, the way `IteratorStep` does, so an iterator
/// that replaces its own `next` is followed rather than captured once.
#[cfg(feature = "v8")]
fn iterator_next<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    iterator: v8::Local<'s, v8::Object>,
) -> Result<Option<v8::Local<'s, v8::Value>>, JsError> {
    let key = v8_string(scope, "next")?;
    let next = iterator
        .get(scope, key.into())
        .ok_or_else(JsError::pending_exception)?;
    let next = v8::Local::<v8::Function>::try_from(next)
        .map_err(|_| JsError::type_error("the iterator has no callable `next`"))?;
    let result = call_js_function(scope, next, iterator.into(), &[])?;
    let result = v8::Local::<v8::Object>::try_from(result)
        .map_err(|_| JsError::type_error("the iterator result is not an object"))?;
    let key = v8_string(scope, "done")?;
    // `IteratorComplete` is `ToBoolean(Get(result, "done"))`, and an abrupt
    // `Get` propagates. Reading it as absent-instead-of-throwing would end the
    // loop at the *first* entry and hand back a truncated sequence as if the
    // iterator had finished, which is a wrong answer rather than a failure.
    let done = result
        .get(scope, key.into())
        .ok_or_else(JsError::pending_exception)?;
    if done.boolean_value(scope) {
        return Ok(None);
    }
    let key = v8_string(scope, "value")?;
    let value = result
        .get(scope, key.into())
        .ok_or_else(JsError::pending_exception)?;
    Ok(Some(value))
}

/// Converts a JavaScript object to a record, in own-property order.
///
/// This is WebIDL's `es-to-record`: the own keys are enumerated first, then
/// each one's property descriptor decides whether it is enumerable, and only an
/// enumerable property is read. The descriptor read is not an optimisation — a
/// proxy observes it and the `Get` that follows, in that order, and a
/// non-enumerable own property has to be skipped rather than treated as data.
///
/// `key` is the *declared* key type's conversion and `convert` the value
/// type's, and they run in that order — `ConvertByType(key, K)` before
/// `ConvertByType(value, V)`, which is observable because either can throw. The
/// key conversion is handed the key as `[[OwnPropertyKeys]]` reported it rather
/// than a lossy `String` of it. A `ByteString` key is the clearest case: read
/// through a Rust `String` first, the legal key `0xFF` would become the two
/// characters of its UTF-8 form, and the code unit above `0xFF` that the key
/// conversion is what rejects would become an ordinary two-byte key.
///
/// `get_own_property_names` is own-only by construction, so an inherited
/// enumerable property never reaches this loop.
#[cfg(feature = "v8")]
pub fn record_from_v8<'s, K, V, F, G>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    mut key: G,
    mut convert: F,
) -> Result<Vec<(K, V)>, JsError>
where
    G: FnMut(&mut v8::PinScope<'s, '_>, v8::Local<'s, v8::Value>) -> Result<K, JsError>,
    F: FnMut(&mut v8::PinScope<'s, '_>, v8::Local<'s, v8::Value>) -> Result<V, JsError>,
{
    let object: v8::Local<v8::Object> = value
        .try_into()
        .map_err(|_| JsError::type_error("expected a record"))?;
    let args = v8::GetPropertyNamesArgs {
        // `[[OwnPropertyKeys]]` reports non-enumerable own properties too; the
        // descriptor read below is what filters them out, and it is the read
        // the spec makes. Symbol keys never reach a record: every record key
        // type is a string type.
        property_filter: v8::PropertyFilter::ALL_PROPERTIES
            | v8::PropertyFilter::SKIP_SYMBOLS,
        // An integer-like own key would otherwise be reported as a Number, and
        // the descriptor read below needs a `Name`; without this the entry
        // would be dropped rather than read.
        key_conversion: v8::KeyConversionMode::ConvertToString,
        ..Default::default()
    };
    let names = object
        .get_own_property_names(scope, args)
        .ok_or_else(JsError::pending_exception)?;
    let enumerable_key = v8_string(scope, "enumerable")?;
    let mut entries = Vec::with_capacity(names.length() as usize);
    for index in 0..names.length() {
        let property = names
            .get_index(scope, index)
            .ok_or_else(JsError::pending_exception)?;
        // `? V.[[GetOwnProperty]](key)` — an abrupt descriptor read propagates,
        // so a proxy that throws from its `getOwnPropertyDescriptor` trap fails
        // the conversion instead of quietly dropping the key. Skipping is for
        // the one case the spec skips: a descriptor that is `undefined`, which
        // is a key a proxy's `ownKeys` reported without it being own.
        let name = v8::Local::<v8::Name>::try_from(property)
            .map_err(|_| JsError::type_error("the record key is not a property name"))?;
        let descriptor = object
            .get_own_property_descriptor(scope, name)
            .ok_or_else(JsError::pending_exception)?;
        if descriptor.is_undefined() {
            continue;
        }
        let descriptor = v8::Local::<v8::Object>::try_from(descriptor)
            .map_err(|_| JsError::type_error("the record key descriptor is not an object"))?;
        let enumerable = descriptor
            .get(scope, enumerable_key.into())
            .ok_or_else(JsError::pending_exception)?;
        if !enumerable.boolean_value(scope) {
            continue;
        }
        let entry = object
            .get(scope, property)
            .ok_or_else(JsError::pending_exception)?;
        let key = key(scope, property)?;
        entries.push((key, convert(scope, entry)?));
    }
    Ok(entries)
}

/// Reads a dictionary member, treating `undefined` as absent.
#[cfg(feature = "v8")]
pub fn dictionary_member<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<v8::Object>,
    name: &str,
) -> Result<Option<v8::Local<'s, v8::Value>>, JsError> {
    let key = v8_string(scope, name)?;
    // `None` means V8 already has the page getter's exception pending. A
    // required dictionary member must not turn that abrupt completion into a
    // synthetic "missing member" TypeError.
    let value = object
        .get(scope, key.into())
        .ok_or_else(JsError::pending_exception)?;
    Ok((!value.is_undefined()).then_some(value))
}

/// The property attributes WebIDL gives a constant.
#[cfg(feature = "v8")]
pub fn constant_attributes() -> v8::PropertyAttribute {
    v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_DELETE
}

/// The property attributes WebIDL gives `Symbol.toStringTag`.
#[cfg(feature = "v8")]
pub fn to_string_tag_attributes() -> v8::PropertyAttribute {
    v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_ENUM
}

/// The property attributes of a `[LegacyUnforgeable]` member.
#[cfg(feature = "v8")]
pub fn unforgeable_attributes() -> v8::PropertyAttribute {
    v8::PropertyAttribute::DONT_DELETE
}
"#;

const BUFFER_SUPPORT: &str = r#"
// ---------------------------------------------------------------------------
// Buffer sources: `ArrayBuffer`, `DataView`, the typed arrays, and the
// `ArrayBufferView` and `BufferSource` typedefs that name them.
//
// A buffer source is not a host object, so none of these go through
// `handle_from_v8`: the script side owns a native V8 array buffer or view and
// the host side receives the bytes it holds. Reading a view copies its own
// `byteOffset .. byteOffset + byteLength` range and never the whole backing
// store, and a detached buffer or view reads as the empty sequence, which is
// the WebIDL conversion rather than an error.
// ---------------------------------------------------------------------------

/// The bytes a V8 view holds.
///
/// `ArrayBufferView::copy_contents` copies exactly the view's own range, so a
/// `Uint8Array` over a larger backing store yields its own bytes and no more.
/// A view over a detached buffer reports a zero byte length and no data, and
/// the length check keeps `copy_contents` from being asked about one at all.
#[cfg(feature = "v8")]
fn view_bytes(view: &v8::ArrayBufferView) -> Vec<u8> {
    let length = view.byte_length();
    if length == 0 {
        return Vec::new();
    }
    let mut bytes = vec![0u8; length];
    let written = view.copy_contents(&mut bytes);
    // `copy_contents` writes `min(dest.len(), byte_length)` bytes, and no script
    // runs between the two calls, so a short copy would mean the view changed
    // under us.
    debug_assert_eq!(
        written,
        bytes.len(),
        "the view copied fewer bytes than it holds"
    );
    bytes
}

/// The value as a V8 view, or a `TypeError` naming what was expected.
#[cfg(feature = "v8")]
fn view_of_v8<'s>(
    value: v8::Local<'s, v8::Value>,
    idl_name: &str,
) -> Result<v8::Local<'s, v8::ArrayBufferView>, JsError> {
    v8::Local::<v8::ArrayBufferView>::try_from(value)
        .map_err(|_| JsError::type_error(format!("the value is not a `{idl_name}`")))
}

/// The bytes of a V8 view, after the brand check that makes it the thing the
/// declaration names.
///
/// The two steps are separate because every typed array is an
/// `ArrayBufferView` as well: `brand` is the named type's own predicate, while
/// the conversion that follows is the general one yielding the `byteOffset`
/// aware reader.
#[cfg(feature = "v8")]
fn brand_checked_view_bytes(
    value: v8::Local<v8::Value>,
    brand: bool,
    idl_name: &str,
) -> Result<Vec<u8>, JsError> {
    if !brand {
        return Err(JsError::type_error(format!(
            "the value is not a `{idl_name}`"
        )));
    }
    let view = view_of_v8(value, idl_name)?;
    Ok(view_bytes(&view))
}

/// The bytes a V8 `ArrayBuffer` holds, which is its whole data block.
///
/// The zero-length check covers a detached `ArrayBuffer`, which reports a zero
/// byte length: it holds no bytes either way, and that is the WebIDL
/// conversion rather than an error.
#[cfg(feature = "v8")]
fn array_buffer_bytes(buffer: &v8::ArrayBuffer) -> Result<Vec<u8>, JsError> {
    let length = buffer.byte_length();
    if length == 0 {
        return Ok(Vec::new());
    }
    let Some(data) = buffer.data() else {
        return Err(JsError::type_error("the `ArrayBuffer` has no backing store"));
    };
    let mut bytes = vec![0u8; length];
    // SAFETY: `data` is the start of the buffer's data block and `length` is
    // that block's length in bytes, as V8 reports them, so the source range is
    // exactly `length` bytes and cannot run past the block. `bytes` is an
    // embedder-owned allocation of the same length that cannot overlap it, and
    // no script runs between the reads and the copy, so neither can go stale.
    unsafe {
        std::ptr::copy_nonoverlapping(data.as_ptr().cast::<u8>(), bytes.as_mut_ptr(), length);
    }
    Ok(bytes)
}

/// An `ArrayBuffer` holding a copy of `bytes`.
#[cfg(feature = "v8")]
fn array_buffer_with_bytes<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: &[u8],
) -> Result<v8::Local<'s, v8::ArrayBuffer>, JsError> {
    let buffer = v8::ArrayBuffer::new(scope, bytes.len());
    if bytes.is_empty() {
        return Ok(buffer);
    }
    let Some(data) = buffer.data() else {
        return Err(JsError::type_error("the `ArrayBuffer` has no backing store"));
    };
    // SAFETY: the buffer was allocated a moment ago with exactly `bytes.len()`
    // bytes and nothing has detached it, so `data` is the start of a block of
    // at least that length. V8 zero-fills the block and every byte of it is
    // overwritten here, and `bytes` is an embedder-side Rust slice that cannot
    // overlap it.
    //
    // `new_backing_store_from_vec` would hand the `Vec` over without the copy,
    // but it is compiled out when V8 is built with the sandbox
    // (`v8_enable_sandbox`), and V8's own documentation names this path as the
    // sandbox-compatible alternative, so the generated glue takes the one that
    // compiles under every feature set.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data.as_ptr().cast::<u8>(), bytes.len());
    }
    Ok(buffer)
}

/// WebIDL `ArrayBuffer` conversion.
///
/// A `SharedArrayBuffer` is rejected: it is a different type, and no
/// declaration in the corpus carries the `[AllowShared]` extended attribute
/// that would admit one.
#[cfg(feature = "v8")]
pub fn array_buffer_from_v8(
    _scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<Vec<u8>, JsError> {
    let buffer = v8::Local::<v8::ArrayBuffer>::try_from(value)
        .map_err(|_| JsError::type_error("the value is not an `ArrayBuffer`"))?;
    array_buffer_bytes(&buffer)
}

/// Converts bytes to an `ArrayBuffer`.
#[cfg(feature = "v8")]
pub fn array_buffer_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: Vec<u8>,
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    Ok(array_buffer_with_bytes(scope, &bytes)?.into())
}

/// WebIDL `DataView` conversion.
///
/// A `DataView` has no element width of its own, so its bytes are the range it
/// covers, exactly as for a typed array.
#[cfg(feature = "v8")]
pub fn data_view_from_v8(
    _scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<Vec<u8>, JsError> {
    brand_checked_view_bytes(value, value.is_data_view(), "DataView")
}

/// Converts bytes to a `DataView` over a fresh `ArrayBuffer`.
#[cfg(feature = "v8")]
pub fn data_view_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: Vec<u8>,
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    let buffer = array_buffer_with_bytes(scope, &bytes)?;
    Ok(v8::DataView::new(scope, buffer, 0, bytes.len()).into())
}
"#;

const BUFFER_SUPPORT_TYPEDEFS: &str = r#"
/// WebIDL `ArrayBufferView` conversion: any of the ten view types.
///
/// The view's own range is what is read, so the backing store it is a window
/// into is never touched beyond it.
#[cfg(feature = "v8")]
pub fn array_buffer_view_from_v8(
    _scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<Vec<u8>, JsError> {
    let view = view_of_v8(value, "ArrayBufferView")?;
    Ok(view_bytes(&view))
}

/// Converts bytes to an `ArrayBufferView`.
///
/// The value is a `Uint8Array` over a fresh `ArrayBuffer`. Any of the ten view
/// types satisfies the typedef, and the one-element-wide view is the only one
/// that needs no element width to interpret the bytes, so it is the one a byte
/// sequence is already spelled in.
#[cfg(feature = "v8")]
pub fn array_buffer_view_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: Vec<u8>,
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    uint8_array_to_v8(scope, bytes)
}

/// WebIDL `BufferSource` conversion: an `ArrayBuffer` or any `ArrayBufferView`.
///
/// The two cases are tried in the order the typedef declares them. A
/// `SharedArrayBuffer` is neither, so it is a `TypeError`, as for `ArrayBuffer`.
#[cfg(feature = "v8")]
pub fn buffer_source_from_v8(
    _scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> Result<Vec<u8>, JsError> {
    if let Ok(buffer) = v8::Local::<v8::ArrayBuffer>::try_from(value) {
        return array_buffer_bytes(&buffer);
    }
    if let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(value) {
        return Ok(view_bytes(&view));
    }
    Err(JsError::type_error(
        "the value is not an `ArrayBuffer` or an `ArrayBufferView`",
    ))
}

/// Converts bytes to a `BufferSource`.
///
/// As in `array_buffer_view_to_v8`, the value is a `Uint8Array`: both cases the
/// typedef admits satisfy it, and this is the spelling a byte sequence is
/// already in.
#[cfg(feature = "v8")]
pub fn buffer_source_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: Vec<u8>,
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    uint8_array_to_v8(scope, bytes)
}
"#;

const QUICKJS_SUPPORT: &str = r#"
// ---------------------------------------------------------------------------
// QuickJS support (rquickjs 0.12).
// ---------------------------------------------------------------------------

/// Maps interface handles to their wrapper objects for one QuickJS context.
///
/// `tenta-js-qjs` implements this and stores it with `Ctx::store_userdata`.
#[cfg(feature = "quickjs")]
pub trait QuickJsWrapperWorld: 'static {
    /// Returns the wrapper for `handle`, creating it if the world allows.
    fn wrapper<'js>(
        &self,
        ctx: &rquickjs::Ctx<'js>,
        interface: &'static str,
        handle: u64,
    ) -> Option<rquickjs::Object<'js>>;

    /// Returns the handle a wrapper stands for.
    fn handle(&self, interface: &'static str, wrapper: &rquickjs::Object<'_>) -> Option<u64>;

    /// Opens a nested scope for interface values consumed by native conversion.
    fn begin_callback_roots(&self, ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<u64>;

    /// Begin a stored-value lease in the existing callback frame.
    fn begin_stored_value_lease(&self, ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()>;
    /// Starts a temporary annotated buffer lease in the current callback frame.
    fn begin_buffer_reference_lease(&self, ctx: &rquickjs::Ctx<'_>, lease_id: u64) -> rquickjs::Result<()>;
    fn current_buffer_reference_lease(&self) -> Option<u64>;
    fn buffer_reference_lease_active(&self, lease_id: u64) -> bool;
    /// Track one newly stored reference; failures poison rather than lose it.
    fn track_stored_value(&self, ctx: &rquickjs::Ctx<'_>, bits: u64) -> rquickjs::Result<()>;
    /// Transfer tracked references after the host operation succeeds.
    fn transfer_stored_value_lease(&self, ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()>;
    /// Recheck hard admission after page-controlled constructor conversion.
    fn before_constructor_host(&self, ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()>;

    /// Retains a successfully branded argument until its callback returns.
    fn retain_callback_argument<'js>(&self, ctx: &rquickjs::Ctx<'js>, value: rquickjs::Value<'js>) -> rquickjs::Result<()>;

    /// Ends exactly the latest scope, dropping roots outside native borrows.
    /// An invariant failure must poison the world and retain its remaining roots.
    fn end_callback_roots(&self, frame: u64);

    /// Stores an arbitrary value and returns the bits that stand for it.
    fn store_value(&self, ctx: &rquickjs::Ctx<'_>, value: rquickjs::Value<'_>) -> u64;

    /// Returns the value `bits` stands for.
    fn load_value<'js>(&self, ctx: &rquickjs::Ctx<'js>, bits: u64) -> Option<rquickjs::Value<'js>>;

    /// Returns the realm (host context) a constructor downcasts to reach the
    /// document and event slab. See [`Realm`].
    fn realm(&self) -> Realm;
    fn adopt<'js>(&self, ctx: &rquickjs::Ctx<'js>, interface: &'static str, host: Box<dyn std::any::Any>) -> Option<rquickjs::Object<'js>>;
    fn prototype<'js>(&self, ctx: &rquickjs::Ctx<'js>, interface: &'static str) -> Option<rquickjs::Object<'js>>;
    fn cache_prototype<'js>(&self, ctx: &rquickjs::Ctx<'js>, interface: &'static str, prototype: rquickjs::Object<'js>, ancestors: &'static [&'static str]);
    /// Returns an intrinsic saved before page script can replace it.
    fn intrinsic<'js>(&self, ctx: &rquickjs::Ctx<'js>, name: &str) -> Option<rquickjs::Value<'js>>;
    /// Saves generated legacy property callbacks alongside the native prototype.
    fn cache_legacy(&self, interface: &'static str, methods: QuickJsLegacyMethods);
    fn native_failure(&self) -> Option<&'static str>;
    /// Whether page-native accounting already recorded a sticky memory failure.
    fn native_memory_failure(&self) -> bool { false }
    /// Whether a watchdog or sticky native memory failure forbids more work.
    fn script_interrupted(&self) -> bool;
    fn record_native_failure(&self, name: &'static str);

    fn native_host(&self, interface: &'static str, value: &rquickjs::Value<'_>) -> Option<std::rc::Rc<dyn std::any::Any>>;

}

/// Converts and assigns a value through an interface's indexed setter.
#[cfg(feature = "quickjs")]
pub type QuickJsIndexedSetter = for<'js> fn(&rquickjs::Ctx<'js>, &rquickjs::Object<'js>, u32, rquickjs::Value<'js>) -> rquickjs::Result<()>;
/// Converts and assigns a value through an interface's named setter.
#[cfg(feature = "quickjs")]
pub type QuickJsNamedSetter = for<'js> fn(&rquickjs::Ctx<'js>, &rquickjs::Object<'js>, &str, rquickjs::Value<'js>) -> rquickjs::Result<()>;
/// Deletes a supported named property and reports whether deletion succeeded.
#[cfg(feature = "quickjs")]
pub type QuickJsNamedDeleter = for<'js> fn(&rquickjs::Ctx<'js>, &rquickjs::Object<'js>, &str) -> rquickjs::Result<bool>;

/// Native dispatch for an interface's indexed properties. Support is explicit:
/// a nullable getter result is not used to infer whether an index exists.
#[cfg(feature = "quickjs")]
#[derive(Clone, Copy)]
pub struct QuickJsIndexedProperties {
    pub indices: for<'js> fn(&rquickjs::Ctx<'js>, &rquickjs::Object<'js>) -> rquickjs::Result<Vec<u32>>,
    pub supports: for<'js> fn(&rquickjs::Ctx<'js>, &rquickjs::Object<'js>, u32) -> rquickjs::Result<bool>,
    pub get: for<'js> fn(&rquickjs::Ctx<'js>, &rquickjs::Object<'js>, u32) -> rquickjs::Result<rquickjs::Value<'js>>,
    pub set: Option<QuickJsIndexedSetter>,
}
/// Native dispatch for an interface's named properties.
#[cfg(feature = "quickjs")]
#[derive(Clone, Copy)]
pub struct QuickJsNamedProperties {
    pub names: for<'js> fn(&rquickjs::Ctx<'js>, &rquickjs::Object<'js>) -> rquickjs::Result<Vec<String>>,
    pub supports: for<'js> fn(&rquickjs::Ctx<'js>, &rquickjs::Object<'js>, &str) -> rquickjs::Result<bool>,
    pub get: for<'js> fn(&rquickjs::Ctx<'js>, &rquickjs::Object<'js>, &str) -> rquickjs::Result<rquickjs::Value<'js>>,
    pub set: Option<QuickJsNamedSetter>,
    pub delete: Option<QuickJsNamedDeleter>,
}
/// Immutable generated metadata used by a native legacy platform wrapper.
#[cfg(feature = "quickjs")]
#[derive(Clone, Copy)]
pub struct QuickJsLegacyMethods {
    pub indexed: Option<QuickJsIndexedProperties>,
    pub named: Option<QuickJsNamedProperties>,
    pub override_builtins: bool,
    pub unenumerable_names: bool,
    pub is_global: bool,
    pub unforgeable_names: &'static [&'static str],
}
/// Registers the generated native callbacks without consulting page globals.
#[cfg(feature = "quickjs")]
pub fn quickjs_cache_legacy(ctx: &rquickjs::Ctx<'_>, interface: &'static str, methods: QuickJsLegacyMethods) -> rquickjs::Result<()> {
    let world = ctx.userdata::<QuickJsWorldSlot>().ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world"))?;
    world.0.cache_legacy(interface, methods);
    Ok(())
}

/// The userdata slot that carries the wrapper world of a context.
#[cfg(feature = "quickjs")]
pub struct QuickJsWorldSlot(pub Box<dyn QuickJsWrapperWorld>);

#[cfg(feature = "quickjs")]
unsafe impl<'js> rquickjs::JsLifetime<'js> for QuickJsWorldSlot {
    type Changed<'to> = QuickJsWorldSlot;
}

/// Throws `error` in `ctx` and returns the matching `rquickjs` error.
///
/// The V8 counterpart is `throw_js_error`; the two live in the same module
/// when a crate enables both engines, so they carry different names.
#[cfg(feature = "quickjs")]
pub fn throw_quickjs_error(ctx: &rquickjs::Ctx<'_>, error: &JsError) -> rquickjs::Error {
    if error.pending_exception {
        return rquickjs::Error::Exception;
    }
    let message = error.message();
    match error.kind() {
        JsErrorKind::Type => rquickjs::Exception::throw_type(ctx, message),
        JsErrorKind::Range => rquickjs::Exception::throw_range(ctx, message),
        JsErrorKind::Syntax => rquickjs::Exception::throw_syntax(ctx, message),
        JsErrorKind::DomException => {
            let name = error.dom_name().unwrap_or("Error");
            match quickjs_dom_exception(ctx, name, message) {
                Ok(value) => ctx.throw(value),
                Err(error) => error,
            }
        }
    }
}

/// Creates a DOMException using the saved native prototype, even if script
/// replaces the global constructor.
#[cfg(feature = "quickjs")]
pub fn quickjs_dom_exception<'js>(ctx: &rquickjs::Ctx<'js>, name: &str, message: &str) -> rquickjs::Result<rquickjs::Value<'js>> {
    let value = rquickjs::Object::new(ctx.clone())?;
    value.set_prototype(Some(&quickjs_prototype(ctx, "DOMException")?))?;
    let code = match name {
        "IndexSizeError" => 1, "DOMStringSizeError" => 2,
        "HierarchyRequestError" => 3, "WrongDocumentError" => 4,
        "InvalidCharacterError" => 5, "NoDataAllowedError" => 6,
        "NoModificationAllowedError" => 7, "NotFoundError" => 8,
        "NotSupportedError" => 9, "InUseAttributeError" => 10,
        "InvalidStateError" => 11, "SyntaxError" => 12,
        "InvalidModificationError" => 13, "NamespaceError" => 14,
        "InvalidAccessError" => 15, "ValidationError" => 16,
        "TypeMismatchError" => 17, "SecurityError" => 18,
        "NetworkError" => 19, "AbortError" => 20, "URLMismatchError" => 21,
        "QuotaExceededError" => 22, "TimeoutError" => 23,
        "InvalidNodeTypeError" => 24, "DataCloneError" => 25, _ => 0,
    };
    value.prop("name", rquickjs::object::Property::from(name).enumerable().configurable())?;
    value.prop("message", rquickjs::object::Property::from(message).enumerable().configurable())?;
    value.prop("code", rquickjs::object::Property::from(code).enumerable().configurable())?;
    Ok(value.into_value())
}

/// Converts a handle to its wrapper object.
#[cfg(feature = "quickjs")]
pub fn handle_to_quickjs<'js, T: InterfaceMarker>(
    ctx: &rquickjs::Ctx<'js>,
    handle: Handle<T>,
) -> rquickjs::Result<rquickjs::Value<'js>> {
    let interface = T::INTERFACE_NAME;
    let world = ctx
        .userdata::<QuickJsWorldSlot>()
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world for this context"))?;
    match world.0.wrapper(ctx, interface, handle.bits()) {
        Some(object) => Ok(object.into_value()),
        None => Err(quickjs_wrapper_error(ctx, "the host object has no wrapper")),
    }
}

// Wrapper publication can already have thrown an allocation-limit exception.
// Preserve it instead of replacing a failed publication with a brand error.
#[cfg(feature = "quickjs")]
fn quickjs_wrapper_error(ctx: &rquickjs::Ctx<'_>, message: &str) -> rquickjs::Error {
    // SAFETY: the context belongs to the current native callback.
    if unsafe { rquickjs::qjs::JS_HasException(ctx.as_raw().as_ptr()) } {
        rquickjs::Error::Exception
    } else {
        rquickjs::Exception::throw_type(ctx, message)
    }
}

/// Converts a wrapper object back to a handle.
#[cfg(feature = "quickjs")]
pub fn handle_from_quickjs<'js, T: InterfaceMarker>(
    ctx: &rquickjs::Ctx<'js>,
    value: rquickjs::Value<'js>,
) -> rquickjs::Result<Handle<T>> {
    let interface = T::INTERFACE_NAME;
    let not_an_instance = || {
        rquickjs::Exception::throw_type(ctx, &format!("the value is not of type '{interface}'"))
    };
    let object = value.as_object().ok_or_else(not_an_instance)?;
    let world = ctx
        .userdata::<QuickJsWorldSlot>()
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world for this context"))?;
    let handle = world
        .0
        .handle(interface, object)
        .ok_or_else(not_an_instance)?;
    // A later dictionary getter or iterator step can reenter JavaScript after
    // this temporary Value is consumed. Keep the original wrapper, not only
    // its native bits, until the enclosing host call has produced its result.
    world.0.retain_callback_argument(ctx, value)?;
    Ok(Handle::from_bits(handle))
}

/// Stores an arbitrary JavaScript value in the context's value table.
#[cfg(feature = "quickjs")]
pub fn value_from_quickjs(ctx: &rquickjs::Ctx<'_>, value: rquickjs::Value<'_>) -> rquickjs::Result<u64> {
    if value.is_null() {
        return Ok(AnyValue::NULL.0);
    }
    let world = ctx
        .userdata::<QuickJsWorldSlot>()
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world for this context"))?;
    let bits = world.0.store_value(ctx, value);
    world.0.track_stored_value(ctx, bits)?;
    Ok(bits)
}

/// Reads an arbitrary JavaScript value back out of the value table.
#[cfg(feature = "quickjs")]
pub fn value_to_quickjs<'js>(ctx: &rquickjs::Ctx<'js>, bits: u64) -> rquickjs::Result<rquickjs::Value<'js>> {
    if bits == AnyValue::NULL.0 {
        return Ok(rquickjs::Value::new_null(ctx.clone()));
    }
    let world = ctx
        .userdata::<QuickJsWorldSlot>()
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world for this context"))?;
    world
        .0
        .load_value(ctx, bits)
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "the value is no longer live"))
}

/// Returns the current context's realm for a constructor.
#[cfg(feature = "quickjs")]
pub fn world_realm_quickjs(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<Realm> {
    let world = ctx
        .userdata::<QuickJsWorldSlot>()
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world for this context"))?;
    Ok(world.0.realm())
}

#[cfg(feature = "quickjs")]
impl<'js, T: InterfaceMarker> rquickjs::FromJs<'js> for Handle<T> {
    fn from_js(ctx: &rquickjs::Ctx<'js>, value: rquickjs::Value<'js>) -> rquickjs::Result<Self> {
        handle_from_quickjs::<T>(ctx, value)
    }
}

#[cfg(feature = "quickjs")]
impl<'js, T: InterfaceMarker> rquickjs::IntoJs<'js> for Handle<T> {
    fn into_js(self, ctx: &rquickjs::Ctx<'js>) -> rquickjs::Result<rquickjs::Value<'js>> {
        handle_to_quickjs(ctx, self)
    }
}

#[cfg(feature = "quickjs")]
impl<'js, T> rquickjs::FromJs<'js> for Promise<T> {
    fn from_js(ctx: &rquickjs::Ctx<'js>, value: rquickjs::Value<'js>) -> rquickjs::Result<Self> {
        value_from_quickjs(ctx, value).map(Promise::from_bits)
    }
}

#[cfg(feature = "quickjs")]
impl<'js, T> rquickjs::IntoJs<'js> for Promise<T> {
    fn into_js(self, ctx: &rquickjs::Ctx<'js>) -> rquickjs::Result<rquickjs::Value<'js>> {
        value_to_quickjs(ctx, self.bits())
    }
}

#[cfg(feature = "quickjs")]
impl<'js, Args, Ret> rquickjs::FromJs<'js> for Callback<Args, Ret> {
    fn from_js(ctx: &rquickjs::Ctx<'js>, value: rquickjs::Value<'js>) -> rquickjs::Result<Self> {
        value_from_quickjs(ctx, value).map(Callback::from_bits)
    }
}

#[cfg(feature = "quickjs")]
impl<'js, Args, Ret> rquickjs::IntoJs<'js> for Callback<Args, Ret> {
    fn into_js(self, ctx: &rquickjs::Ctx<'js>) -> rquickjs::Result<rquickjs::Value<'js>> {
        value_to_quickjs(ctx, self.bits())
    }
}

#[cfg(feature = "quickjs")]
impl<'js> rquickjs::FromJs<'js> for AnyValue {
    fn from_js(ctx: &rquickjs::Ctx<'js>, value: rquickjs::Value<'js>) -> rquickjs::Result<Self> {
        value_from_quickjs(ctx, value).map(AnyValue)
    }
}

#[cfg(feature = "quickjs")]
impl<'js> rquickjs::IntoJs<'js> for AnyValue {
    fn into_js(self, ctx: &rquickjs::Ctx<'js>) -> rquickjs::Result<rquickjs::Value<'js>> {
        value_to_quickjs(ctx, self.0)
    }
}

#[cfg(feature = "quickjs")]
impl<'js> rquickjs::FromJs<'js> for JsObject {
    fn from_js(ctx: &rquickjs::Ctx<'js>, value: rquickjs::Value<'js>) -> rquickjs::Result<Self> {
        value_from_quickjs(ctx, value).map(JsObject)
    }
}

#[cfg(feature = "quickjs")]
impl<'js> rquickjs::IntoJs<'js> for JsObject {
    fn into_js(self, ctx: &rquickjs::Ctx<'js>) -> rquickjs::Result<rquickjs::Value<'js>> {
        value_to_quickjs(ctx, self.0)
    }
}

#[cfg(feature = "quickjs")]
impl<'js> rquickjs::FromJs<'js> for JsSymbol {
    fn from_js(ctx: &rquickjs::Ctx<'js>, value: rquickjs::Value<'js>) -> rquickjs::Result<Self> {
        value_from_quickjs(ctx, value).map(JsSymbol)
    }
}

#[cfg(feature = "quickjs")]
impl<'js> rquickjs::IntoJs<'js> for JsSymbol {
    fn into_js(self, ctx: &rquickjs::Ctx<'js>) -> rquickjs::Result<rquickjs::Value<'js>> {
        value_to_quickjs(ctx, self.0)
    }
}
/// A native host uses a single class id; the immutable brand is independent of
/// script-visible prototypes. This makes inherited WebIDL calls safe.
#[cfg(feature = "quickjs")]
pub struct QuickJsHost {
    pub host: Option<std::rc::Rc<dyn std::any::Any>>,
    pub handle: u64,
    pub generation: u64,
    pub native_failure: std::rc::Rc<std::cell::Cell<Option<&'static str>>>,
    pub interfaces: &'static [&'static str],
    pub dead: std::rc::Rc<std::cell::Cell<Vec<(u64, u64)>>>,
}
#[cfg(feature = "quickjs")]
impl Drop for QuickJsHost {
    fn drop(&mut self) {
        // Finalization records ids only. Document and value-table borrows are
        // deferred until the engine reaches a safe point outside QuickJS GC.
        let mut dead = self.dead.take();
        dead.push((self.handle, self.generation));
        self.dead.set(dead);
        // rquickjs calls the class finalizer from C. Taking the payload means
        // automatic field destruction cannot run arbitrary native Drop code.
        if let Some(host) = self.host.take()
            && let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(host))) {
            if self.native_failure.get().is_none() { self.native_failure.set(Some("native host finalizer")); }
            if let Err(nested) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload))) { std::mem::forget(nested); }
        }
    }
}
#[cfg(feature = "quickjs")]
impl<'js> rquickjs::class::Trace<'js> for QuickJsHost {
    fn trace<'a>(&self, _: rquickjs::class::Tracer<'a, 'js>) {}
}
// SAFETY: the native host carries no lifetime-branded QuickJS values.
#[cfg(feature = "quickjs")]
unsafe impl<'js> rquickjs::JsLifetime<'js> for QuickJsHost { type Changed<'to> = Self; }
#[cfg(feature = "quickjs")]
impl<'js> rquickjs::class::JsClass<'js> for QuickJsHost {
    const NAME: &'static str = "TentaHost";
    type Mutable = rquickjs::class::Readable;
    fn constructor(_: &rquickjs::Ctx<'js>) -> rquickjs::Result<Option<rquickjs::function::Constructor<'js>>> { Ok(None) }
}
#[cfg(feature = "quickjs")]
pub fn quickjs_host<'js, T: 'static>(ctx: &rquickjs::Ctx<'js>, value: &rquickjs::Value<'js>, interface: &'static str) -> rquickjs::Result<std::rc::Rc<T>> {
    // Web IDL regular operation/attribute receivers substitute the method
    // realm's global object for a nullish this value before brand checking.
    // Generated callers use this helper only for receivers; interface
    // argument conversion goes through the separate handle conversion path.
    let global = (value.is_null() || value.is_undefined()).then(|| ctx.globals());
    let value = global.as_ref().map_or(value, rquickjs::Object::as_value);
    if let Some(host) = ctx.userdata::<QuickJsWorldSlot>().and_then(|world| world.0.native_host(interface, value)) {
        return host.downcast().map_err(|_| rquickjs::Exception::throw_type(ctx, "native host type mismatch"));
    }
    let class = value.as_object().and_then(|o| o.as_class::<QuickJsHost>()).ok_or_else(|| rquickjs::Exception::throw_type(ctx, "Illegal invocation"))?;
    let host = class.borrow();
    if !host.interfaces.contains(&interface) { return Err(rquickjs::Exception::throw_type(ctx, "Illegal invocation")); }
    host.host.as_ref().ok_or_else(|| rquickjs::Exception::throw_type(ctx, "collected host"))?.clone().downcast().map_err(|_| rquickjs::Exception::throw_type(ctx, "native host type mismatch"))
}
#[cfg(feature = "quickjs")]
pub fn quickjs_begin_stored_value_lease(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    let world = ctx.userdata::<QuickJsWorldSlot>()
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world"))?;
    world.0.begin_stored_value_lease(ctx)
}

#[cfg(feature = "quickjs")]
pub fn quickjs_begin_buffer_reference_lease(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    let lease_id = next_buffer_lease_id()
        .ok_or_else(|| rquickjs::Exception::throw_range(ctx, "buffer reference lease IDs exhausted"))?;
    let world = ctx.userdata::<QuickJsWorldSlot>()
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world"))?;
    world.0.begin_buffer_reference_lease(ctx, lease_id)
}

#[cfg(feature = "quickjs")]
fn quickjs_current_buffer_reference_lease(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<u64> {
    ctx.userdata::<QuickJsWorldSlot>()
        .and_then(|world| world.0.current_buffer_reference_lease())
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "buffer reference has no active lease"))
}

#[cfg(feature = "quickjs")]
fn quickjs_validate_buffer_reference_lease(ctx: &rquickjs::Ctx<'_>, lease_id: u64) -> rquickjs::Result<()> {
    if ctx.userdata::<QuickJsWorldSlot>()
        .is_some_and(|world| world.0.buffer_reference_lease_active(lease_id)) {
        Ok(())
    } else {
        Err(rquickjs::Exception::throw_type(ctx, "buffer reference escaped its callback"))
    }
}

#[cfg(feature = "quickjs")]
pub fn quickjs_before_constructor_host(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    let world = ctx.userdata::<QuickJsWorldSlot>()
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world"))?;
    world.0.before_constructor_host(ctx)
}

#[cfg(feature = "quickjs")]
pub fn quickjs_transfer_stored_value_lease(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    let world = ctx.userdata::<QuickJsWorldSlot>()
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world"))?;
    world.0.transfer_stored_value_lease(ctx)
}

#[cfg(feature = "quickjs")]
pub fn quickjs_adopt<'js>(ctx: &rquickjs::Ctx<'js>, interface: &'static str, host: Box<dyn std::any::Any>) -> rquickjs::Result<rquickjs::Object<'js>> {
    let world = ctx.userdata::<QuickJsWorldSlot>().ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world"))?;
    world.0.adopt(ctx, interface, host).ok_or_else(|| quickjs_wrapper_error(ctx, "cannot wrap native host"))
}
#[cfg(feature = "quickjs")]
pub fn quickjs_prototype<'js>(ctx: &rquickjs::Ctx<'js>, interface: &'static str) -> rquickjs::Result<rquickjs::Object<'js>> {
    let world = ctx.userdata::<QuickJsWorldSlot>().ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world"))?;
    world.0.prototype(ctx, interface).ok_or_else(|| rquickjs::Exception::throw_type(ctx, "missing interface prototype"))
}
/// Resolve a constructed Web IDL object's prototype after its arguments have
/// been converted. The current QuickJS page path has one realm per Engine.
#[cfg(feature = "quickjs")]
pub fn quickjs_constructor_prototype<'js>(ctx: &rquickjs::Ctx<'js>, target: &rquickjs::Function<'js>, interface: &'static str) -> rquickjs::Result<rquickjs::Object<'js>> {
    let value: rquickjs::Value<'js> = target.get(rquickjs::atom::PredefinedAtom::Prototype)?;
    quickjs_check_native_failure(ctx)?;
    if let Some(prototype) = value.as_object() { Ok(prototype.clone()) } else { quickjs_prototype(ctx, interface) }
}
#[cfg(feature = "quickjs")]
pub fn quickjs_cache_prototype<'js>(ctx: &rquickjs::Ctx<'js>, interface: &'static str, prototype: rquickjs::Object<'js>, ancestors: &'static [&'static str]) -> rquickjs::Result<()> {
    let world = ctx.userdata::<QuickJsWorldSlot>().ok_or_else(|| rquickjs::Exception::throw_type(ctx, "no wrapper world"))?;
    world.0.cache_prototype(ctx, interface, prototype, ancestors);
    Ok(())
}

/// Looks up a native intrinsic without consulting script-mutable globals.
#[cfg(feature = "quickjs")]
pub fn quickjs_intrinsic<'js>(ctx: &rquickjs::Ctx<'js>, name: &str) -> rquickjs::Result<rquickjs::Value<'js>> {
    ctx.userdata::<QuickJsWorldSlot>().and_then(|world| world.0.intrinsic(ctx, name))
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "missing native intrinsic"))
}

#[cfg(feature = "quickjs")]
struct QuickJsArgumentFrame<'js> {
    ctx: rquickjs::Ctx<'js>,
    frame: u64,
    callback: &'static str,
}

#[cfg(feature = "quickjs")]
impl<'js> QuickJsArgumentFrame<'js> {
    fn enter(ctx: &rquickjs::Ctx<'js>, callback: &'static str) -> rquickjs::Result<Option<Self>> {
        // Pure static callbacks can operate without a wrapper world. Typed
        // interface conversion already requires one, and never takes this path.
        let frame = match ctx.userdata::<QuickJsWorldSlot>() {
            Some(world) => world.0.begin_callback_roots(ctx)?,
            None => return Ok(None),
        };
        Ok(Some(Self { ctx: ctx.clone(), frame, callback }))
    }
}

#[cfg(feature = "quickjs")]
impl Drop for QuickJsArgumentFrame<'_> {
    fn drop(&mut self) {
        // This guard also runs during a native unwind. A cleanup failure must
        // not start a second unwind through QuickJS's C callback.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(world) = self.ctx.userdata::<QuickJsWorldSlot>() {
                // The outer boundary records a panic only after Rust has run
                // this guard. Preserve that primary callback before cleanup
                // could discover a second native failure.
                if std::thread::panicking() {
                    world.0.record_native_failure(self.callback);
                }
                world.0.end_callback_roots(self.frame);
            }
        }));
        if let Err(payload) = result {
            if let Some(world) = self.ctx.userdata::<QuickJsWorldSlot>() {
                world.0.record_native_failure("callback argument cleanup");
            }
            if let Err(nested) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload))) { std::mem::forget(nested); }
        }
    }
}

/// Panic containment for every generated and handwritten QuickJS callback.
/// The runtime's sticky failure flag forbids further entries after a panic.
#[cfg(feature = "quickjs")]
pub fn quickjs_callback_boundary<'js, T>(ctx: &rquickjs::Ctx<'js>, name: &'static str, f: impl FnOnce() -> rquickjs::Result<T>) -> rquickjs::Result<T> {
    quickjs_check_native_failure(ctx)?;
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _arguments = QuickJsArgumentFrame::enter(ctx, name)?;
        f()
    })) {
        Ok(result) => {
            quickjs_check_native_failure(ctx)?;
            // Host APIs can throw an existing stored value (AbortSignal.reason).
            // SAFETY: the context belongs to this active callback.
            if unsafe { rquickjs::qjs::JS_HasException(ctx.as_raw().as_ptr()) } { Err(rquickjs::Error::Exception) } else { result }
        },
        Err(payload) => {
            if let Some(world) = ctx.userdata::<QuickJsWorldSlot>() { world.0.record_native_failure(name); }
            // Arbitrary panic payloads can panic during Drop as well.
            if let Err(nested) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload))) { std::mem::forget(nested); }
            Err(quickjs_fatal_exception(ctx))
        }
    }
}
/// Preserves sticky poison before an error handler could enter more script.
#[cfg(feature = "quickjs")]
pub fn quickjs_check_native_failure(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    if ctx.userdata::<QuickJsWorldSlot>().is_some_and(|w| w.0.native_failure().is_some()) {
        Err(quickjs_fatal_exception(ctx))
    } else { Ok(()) }
}
#[cfg(feature = "quickjs")]
fn quickjs_fatal_exception(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Error {
    let _ = rquickjs::Exception::throw_internal(ctx, "native callback failed");
    let error = ctx.catch();
    // SAFETY: `error` and `ctx` belong to this live runtime. QuickJS-ng marks
    // this Error uncatchable, so page catch/finally blocks cannot resume it.
    unsafe { rquickjs::qjs::JS_SetUncatchableError(ctx.as_raw().as_ptr(), error.as_raw()); }
    ctx.throw(error)
}

#[cfg(feature = "quickjs")]
pub fn quickjs_bytestring<'js>(ctx: &rquickjs::Ctx<'js>, value: rquickjs::Value<'js>) -> rquickjs::Result<Vec<u8>> {
    use rquickjs::FromJs;
    let text = rquickjs::Coerced::<String>::from_js(ctx, value)?.0;
    text.chars().map(|c| u8::try_from(u32::from(c)).map_err(|_| rquickjs::Exception::throw_type(ctx, "ByteString contains a character above 255"))).collect()
}
#[cfg(feature = "quickjs")]
pub fn quickjs_sequence<'js, T>(ctx: &rquickjs::Ctx<'js>, value: rquickjs::Value<'js>, mut convert: impl FnMut(rquickjs::Value<'js>) -> rquickjs::Result<T>) -> rquickjs::Result<Vec<T>> {
    let object = value.as_object().ok_or_else(|| rquickjs::Exception::throw_type(ctx, "sequence requires an iterable object"))?;
    let method: rquickjs::Function = object.get(rquickjs::atom::PredefinedAtom::SymbolIterator)?;
    let iterator: rquickjs::Object = method.call((rquickjs::function::This(object.clone()),))?;
    let next: rquickjs::Function = iterator.get("next")?;
    let mut values = Vec::new();
    loop {
        let step: rquickjs::Object = next.call((rquickjs::function::This(iterator.clone()),))?;
        let done: rquickjs::Coerced<bool> = step.get("done")?;
        if done.0 { return Ok(values); }
        let item = step.get("value")?;
        match convert(item) {
            Ok(item) => values.push(item),
            Err(error) => {
                quickjs_check_native_failure(ctx)?;
                let original = if error.is_exception() { Some(ctx.catch()) } else { None };
                if let Ok(Some(close)) = iterator.get::<_, Option<rquickjs::Function>>("return") { let _ = close.call::<_, rquickjs::Value>((rquickjs::function::This(iterator),)); }
                if let Some(original) = original { ctx.throw(original); }
                return Err(error);
            }
        }
    }
}
#[cfg(feature = "quickjs")]
pub fn quickjs_record<'js, K, V>(ctx: &rquickjs::Ctx<'js>, value: rquickjs::Value<'js>, mut key: impl FnMut(rquickjs::Value<'js>) -> rquickjs::Result<K>, mut item: impl FnMut(rquickjs::Value<'js>) -> rquickjs::Result<V>) -> rquickjs::Result<Vec<(K,V)>> {
    use rquickjs::IntoJs;
    let object = value.as_object().ok_or_else(|| rquickjs::Exception::throw_type(ctx, "record requires an object"))?;
    let mut values = Vec::new();
    for name in object.keys::<String>() { let name = name?; values.push((key(name.clone().into_js(ctx)?)?, item(object.get(name)?)?)); }
    Ok(values)
}
#[cfg(feature = "quickjs")]
pub fn quickjs_array_data_property<'js>(array: &rquickjs::Array<'js>, index: usize, value: rquickjs::Value<'js>) -> rquickjs::Result<()> {
    let ctx = array.as_object().ctx();
    let key = match u32::try_from(index) {
        Ok(index) => rquickjs::Atom::from_u32(ctx.clone(), index)?,
        Err(_) => rquickjs::Atom::from_str(ctx.clone(), &index.to_string())?,
    };
    array.as_object().prop(key, rquickjs::object::Property::from(value).writable().enumerable().configurable())
}
#[cfg(feature = "quickjs")]
pub fn quickjs_sequence_value<'js,T>(ctx:&rquickjs::Ctx<'js>, values:Vec<T>, mut convert:impl FnMut(T)->rquickjs::Result<rquickjs::Value<'js>>) -> rquickjs::Result<rquickjs::Value<'js>> {
    let array=rquickjs::Array::new(ctx.clone())?;
    for (index,value) in values.into_iter().enumerate() { quickjs_array_data_property(&array,index,convert(value)?)?; }
    Ok(array.into_value())
}
#[cfg(feature = "quickjs")]
pub fn quickjs_record_value<'js,K,V>(ctx:&rquickjs::Ctx<'js>, values:Vec<(K,V)>, mut key:impl FnMut(K)->rquickjs::Result<rquickjs::Value<'js>>, mut item:impl FnMut(V)->rquickjs::Result<rquickjs::Value<'js>>) -> rquickjs::Result<rquickjs::Value<'js>> {
    use rquickjs::FromJs;
    let object=rquickjs::Object::new(ctx.clone())?;
    for (k,v) in values { let k=rquickjs::Coerced::<String>::from_js(ctx,key(k)?)?.0; object.prop(k,rquickjs::object::Property::from(item(v)?).writable().enumerable().configurable())?; }
    Ok(object.into_value())
}
#[cfg(feature = "quickjs")]
pub fn quickjs_array_iterator<'js>(ctx:&rquickjs::Ctx<'js>, array:rquickjs::Array<'js>)->rquickjs::Result<rquickjs::Value<'js>> {
    let method:rquickjs::Function=rquickjs::FromJs::from_js(ctx, quickjs_intrinsic(ctx, "Array.values")?)?;
    method.call((rquickjs::function::This(array),))
}

#[cfg(feature = "quickjs")]
pub fn quickjs_buffer_bytes<'js>(ctx:&rquickjs::Ctx<'js>,value:rquickjs::Value<'js>,kind:&str)->rquickjs::Result<Vec<u8>> {
    if !quickjs_is_buffer(&value, kind) { return Err(rquickjs::Exception::throw_type(ctx, "wrong buffer source type")); }
    if quickjs_is_buffer(&value, "ArrayBuffer") { return value.as_object().and_then(|o| o.as_array_buffer()).and_then(|b|b.as_bytes()).map(|b|b.to_vec()).ok_or_else(||rquickjs::Exception::throw_type(ctx,"detached ArrayBuffer")); }
    let mut offset=0; let mut len=0; let mut element=0;
    let buffer = if quickjs_is_buffer(&value, "DataView") {
        let getter = |name| -> rquickjs::Result<rquickjs::Value<'js>> {
            let function: rquickjs::Function = rquickjs::FromJs::from_js(ctx, quickjs_intrinsic(ctx, name)?)?;
            function.call((rquickjs::function::This(value.clone()),))
        };
        let buffer = getter("DataView.buffer")?;
        offset = rquickjs::FromJs::from_js(ctx, getter("DataView.byteOffset")?)?;
        len = rquickjs::FromJs::from_js(ctx, getter("DataView.byteLength")?)?;
        buffer
    } else {
        // SAFETY: QuickJS checks the native typed-array brand and returns a new
        // owned buffer reference. No JS callback runs while its bytes are read.
        let raw=unsafe { rquickjs::qjs::JS_GetTypedArrayBuffer(ctx.as_raw().as_ptr(),value.as_raw(),&mut offset,&mut len,&mut element) };
        // SAFETY: JS_GetTypedArrayBuffer returns one owned reference, including
        // the exception sentinel; this value takes responsibility for it.
        unsafe { rquickjs::Value::from_raw(ctx.clone(),raw) }
    };
    if buffer.is_exception() { return Err(rquickjs::Error::Exception); }
    let bytes=buffer.as_object().and_then(|o| o.as_array_buffer()).and_then(|b|b.as_bytes()).ok_or_else(||rquickjs::Exception::throw_type(ctx,"detached buffer"))?;
    let offset = usize::try_from(offset).map_err(|_| rquickjs::Exception::throw_type(ctx,"invalid buffer offset"))?;
    let len = usize::try_from(len).map_err(|_| rquickjs::Exception::throw_type(ctx,"invalid buffer length"))?;
    let end = offset.checked_add(len).ok_or_else(||rquickjs::Exception::throw_type(ctx,"invalid buffer range"))?;
    bytes.get(offset..end).map(|v|v.to_vec()).ok_or_else(||rquickjs::Exception::throw_type(ctx,"invalid buffer view"))
}
#[cfg(feature = "quickjs")]
pub fn quickjs_buffer_value<'js>(ctx:&rquickjs::Ctx<'js>,bytes:Vec<u8>,kind:&str)->rquickjs::Result<rquickjs::Value<'js>> {
    let buffer=rquickjs::ArrayBuffer::new(ctx.clone(),bytes)?;
    if kind=="ArrayBuffer" {return Ok(buffer.into_value());}
    let kind = match kind { "ArrayBufferView" | "BufferSource" => "Uint8Array", kind => kind };
    let constructor:rquickjs::function::Constructor=rquickjs::FromJs::from_js(ctx, quickjs_intrinsic(ctx, kind)?)?;
    constructor.construct((buffer,))
}

#[cfg(feature = "quickjs")]
pub fn quickjs_is_interface(ctx: &rquickjs::Ctx<'_>, value: &rquickjs::Value<'_>, interface: &'static str) -> bool {
    let Some(object) = value.as_object() else { return false; };
    ctx.userdata::<QuickJsWorldSlot>().is_some_and(|world| world.0.handle(interface, object).is_some())
}
#[cfg(feature = "quickjs")]
pub fn quickjs_has_iterator(ctx: &rquickjs::Ctx<'_>, value: &rquickjs::Value<'_>) -> rquickjs::Result<bool> {
    let Some(object) = value.as_object() else { return Ok(false); };
    let iterator: rquickjs::Value = object.get(rquickjs::atom::PredefinedAtom::SymbolIterator)?;
    if iterator.is_null() || iterator.is_undefined() { return Ok(false); }
    if iterator.is_function() { Ok(true) } else { Err(rquickjs::Exception::throw_type(ctx, "iterator is not callable")) }
}
#[cfg(feature = "quickjs")]
pub fn quickjs_is_buffer(value: &rquickjs::Value<'_>, kind: &str) -> bool {
    // SAFETY: these QuickJS queries read native value brands without running JS.
    unsafe {
        if kind == "ArrayBuffer" { return rquickjs::qjs::JS_IsArrayBuffer(value.as_raw()); }
        if kind == "DataView" { return rquickjs::qjs::JS_IsDataView(value.as_raw()); }
        let tag = rquickjs::qjs::JS_GetTypedArrayType(value.as_raw());
        match kind {
            "BufferSource" => tag >= 0 || rquickjs::qjs::JS_IsArrayBuffer(value.as_raw()) || rquickjs::qjs::JS_IsDataView(value.as_raw()),
            "ArrayBufferView" => tag >= 0 || rquickjs::qjs::JS_IsDataView(value.as_raw()),
            "Uint8ClampedArray" => tag == 0, "Int8Array" => tag == 1,
            "Uint8Array" => tag == 2, "Int16Array" => tag == 3,
            "Uint16Array" => tag == 4, "Int32Array" => tag == 5,
            "Uint32Array" => tag == 6, "Float32Array" => tag == 10,
            "Float64Array" => tag == 11, _ => false,
        }
    }
}
"#;

const BUFFER_REF_V8: &str = r#"
// Callback-scoped reference-preserving BufferSource conversions.
#[cfg(feature = "v8")]
fn v8_buffer_kind(value: v8::Local<v8::Value>) -> Option<BufferKind> {
    Some(if value.is_array_buffer() {
        BufferKind::ArrayBuffer
    } else if value.is_data_view() {
        BufferKind::DataView
    } else if value.is_int8_array() {
        BufferKind::Int8
    } else if value.is_uint8_array() {
        BufferKind::Uint8
    } else if value.is_uint8_clamped_array() {
        BufferKind::Uint8Clamped
    } else if value.is_int16_array() {
        BufferKind::Int16
    } else if value.is_uint16_array() {
        BufferKind::Uint16
    } else if value.is_int32_array() {
        BufferKind::Int32
    } else if value.is_uint32_array() {
        BufferKind::Uint32
    } else if value.is_big_int64_array() {
        BufferKind::BigInt64
    } else if value.is_big_uint64_array() {
        BufferKind::BigUint64
    } else if value.is_float16_array() {
        BufferKind::Float16
    } else if value.is_float32_array() {
        BufferKind::Float32
    } else if value.is_float64_array() {
        BufferKind::Float64
    } else {
        return None;
    })
}

#[cfg(feature = "v8")]
fn v8_buffer_object<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<v8::Local<'s, v8::ArrayBuffer>, JsError> {
    if value.is_array_buffer() {
        return v8::Local::<v8::ArrayBuffer>::try_from(value)
            .map_err(|_| JsError::type_error("not an ArrayBuffer"));
    }
    let view = v8::Local::<v8::ArrayBufferView>::try_from(value)
        .map_err(|_| JsError::type_error("not an ArrayBufferView"))?;
    view.buffer(scope)
        .ok_or_else(|| JsError::type_error("buffer view has no backing object"))
}

#[cfg(feature = "v8")]
fn v8_buffer_metadata<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    allow_buffer: bool,
) -> Result<BufferMetadata, JsError> {
    let kind = v8_buffer_kind(value)
        .ok_or_else(|| JsError::type_error("expected a native buffer source"))?;
    if !allow_buffer && kind == BufferKind::ArrayBuffer {
        return Err(JsError::type_error("expected an ArrayBufferView"));
    }
    let buffer = v8_buffer_object(scope, value)?;
    // `ArrayBufferView::buffer()` is historically typed ArrayBuffer even for
    // shared backing, so inspect both the native value brand and backing.
    let backing = buffer.get_backing_store();
    if buffer.is_shared_array_buffer() || backing.is_shared() {
        return Err(JsError::type_error("shared buffer source is not supported"));
    }
    // The native backing store can become a fresh empty fixed store after
    // detachment. The saved original ArrayBuffer getter sees the original flag.
    if with_wrapper_world(scope, |world, scope| {
        world.array_buffer_resizable(scope, buffer)
    })?? {
        return Err(JsError::type_error(
            "resizable buffer source is not supported",
        ));
    }
    if buffer.was_detached() {
        return Ok(BufferMetadata {
            kind,
            byte_offset: 0,
            byte_length: 0,
        });
    }
    let (byte_offset, byte_length) = if kind == BufferKind::ArrayBuffer {
        (0, buffer.byte_length())
    } else {
        let view = v8::Local::<v8::ArrayBufferView>::try_from(value)
            .map_err(|_| JsError::type_error("expected an ArrayBufferView"))?;
        (view.byte_offset(), view.byte_length())
    };
    let end = byte_offset
        .checked_add(byte_length)
        .ok_or_else(|| JsError::type_error("buffer view range overflow"))?;
    if end > backing.byte_length() {
        return Err(JsError::type_error("buffer view is out of bounds"));
    }
    Ok(BufferMetadata {
        kind,
        byte_offset,
        byte_length,
    })
}

/// Stores the original validated BufferSource in the current callback lease.
#[cfg(feature = "v8")]
pub fn buffer_source_ref_from_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<BufferSourceRef, JsError> {
    let lease_id = v8_current_buffer_reference_lease(scope)?;
    v8_buffer_metadata(scope, value, true)?;
    value_from_v8(scope, value).map(|bits| BufferSourceRef::new(bits, lease_id))
}

/// Stores the original validated ArrayBufferView in the current callback lease.
#[cfg(feature = "v8")]
pub fn array_buffer_view_ref_from_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<ArrayBufferViewRef, JsError> {
    let lease_id = v8_current_buffer_reference_lease(scope)?;
    v8_buffer_metadata(scope, value, false)?;
    value_from_v8(scope, value).map(|bits| ArrayBufferViewRef::new(bits, lease_id))
}

#[cfg(feature = "v8")]
pub fn buffer_source_ref_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    reference: BufferSourceRef,
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    v8_validate_buffer_reference_lease(scope, reference.lease_id())?;
    let value = value_to_v8(scope, reference.bits())?;
    v8_buffer_metadata(scope, value, true)?;
    Ok(value)
}

#[cfg(feature = "v8")]
pub fn array_buffer_view_ref_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    reference: ArrayBufferViewRef,
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    v8_validate_buffer_reference_lease(scope, reference.lease_id())?;
    let value = value_to_v8(scope, reference.bits())?;
    v8_buffer_metadata(scope, value, false)?;
    Ok(value)
}

/// Re-reads current metadata after all page-controlled conversions.
#[cfg(feature = "v8")]
pub fn v8_buffer_source_info(
    scope: &mut v8::PinScope<'_, '_>,
    reference: BufferSourceRef,
) -> Result<BufferMetadata, JsError> {
    v8_validate_buffer_reference_lease(scope, reference.lease_id())?;
    let value = value_to_v8(scope, reference.bits())?;
    v8_buffer_metadata(scope, value, true)
}

#[cfg(feature = "v8")]
pub fn v8_array_buffer_view_info(
    scope: &mut v8::PinScope<'_, '_>,
    reference: ArrayBufferViewRef,
) -> Result<BufferMetadata, JsError> {
    v8_validate_buffer_reference_lease(scope, reference.lease_id())?;
    let value = value_to_v8(scope, reference.bits())?;
    v8_buffer_metadata(scope, value, false)
}

/// Copies the current view span into an already admitted caller-owned slice.
#[cfg(feature = "v8")]
pub fn v8_copy_buffer_source_into(
    scope: &mut v8::PinScope<'_, '_>,
    reference: BufferSourceRef,
    destination: &mut [u8],
) -> Result<BufferMetadata, JsError> {
    v8_validate_buffer_reference_lease(scope, reference.lease_id())?;
    let value = value_to_v8(scope, reference.bits())?;
    let info = v8_buffer_metadata(scope, value, true)?;
    if destination.len() != info.byte_length {
        return Err(JsError::type_error("buffer destination length changed"));
    }
    let buffer = v8_buffer_object(scope, value)?;
    let backing = buffer.get_backing_store();
    let end = info
        .byte_offset
        .checked_add(info.byte_length)
        .ok_or_else(|| JsError::type_error("buffer range overflow"))?;
    let bytes = backing
        .get(info.byte_offset..end)
        .ok_or_else(|| JsError::type_error("buffer range changed"))?;
    for (source, target) in bytes
        .chunks(64 * 1024)
        .zip(destination.chunks_mut(64 * 1024))
    {
        if v8_buffer_work_interrupted(scope) {
            return Err(JsError::pending_exception());
        }
        for (source, target) in source.iter().zip(target) {
            *target = source.get();
        }
    }
    Ok(info)
}

/// Writes through the original view after rechecking its current span.
#[cfg(feature = "v8")]
pub fn v8_write_array_buffer_view(
    scope: &mut v8::PinScope<'_, '_>,
    reference: ArrayBufferViewRef,
    source: &[u8],
) -> Result<BufferMetadata, JsError> {
    v8_validate_buffer_reference_lease(scope, reference.lease_id())?;
    let value = value_to_v8(scope, reference.bits())?;
    let info = v8_buffer_metadata(scope, value, false)?;
    if source.len() != info.byte_length {
        return Err(JsError::type_error("buffer source length changed"));
    }
    let buffer = v8_buffer_object(scope, value)?;
    if with_wrapper_world(scope, |world, scope| {
        world.array_buffer_immutable(scope, buffer)
    })?? {
        return Err(JsError::type_error("immutable buffer cannot be written"));
    }
    let backing = buffer.get_backing_store();
    let end = info
        .byte_offset
        .checked_add(info.byte_length)
        .ok_or_else(|| JsError::type_error("buffer range overflow"))?;
    let bytes = backing
        .get(info.byte_offset..end)
        .ok_or_else(|| JsError::type_error("buffer range changed"))?;
    if info.byte_length != 0 {
        let data = backing.data()
            .ok_or_else(|| JsError::type_error("buffer backing disappeared"))?;
        if source_overlaps_backing(source, data.as_ptr().cast(), backing.byte_length()) {
            return Err(JsError::type_error("buffer write source overlaps backing"));
        }
    }
    for (target, source) in bytes.chunks(64 * 1024).zip(source.chunks(64 * 1024)) {
        if v8_buffer_work_interrupted(scope) {
            return Err(JsError::pending_exception());
        }
        for (target, source) in target.iter().zip(source) {
            target.set(*source);
        }
    }
    Ok(info)
}
"#;

const BUFFER_REF_QJS: &str = r#"
// Callback-scoped reference-preserving BufferSource conversions.
#[cfg(feature = "quickjs")]
fn quickjs_buffer_kind(value: &rquickjs::Value<'_>) -> Option<BufferKind> {
    // SAFETY: these are exact native class queries and cannot invoke a Proxy.
    unsafe {
        let raw = value.as_raw();
        if rquickjs::qjs::JS_IsArrayBuffer(raw) {
            return Some(BufferKind::ArrayBuffer);
        }
        if rquickjs::qjs::JS_IsDataView(raw) {
            return Some(BufferKind::DataView);
        }
        let tag = rquickjs::qjs::JS_GetTypedArrayType(raw);
        Some(match tag {
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_UINT8C as i32 => {
                BufferKind::Uint8Clamped
            }
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_INT8 as i32 => {
                BufferKind::Int8
            }
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_UINT8 as i32 => {
                BufferKind::Uint8
            }
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_INT16 as i32 => {
                BufferKind::Int16
            }
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_UINT16 as i32 => {
                BufferKind::Uint16
            }
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_INT32 as i32 => {
                BufferKind::Int32
            }
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_UINT32 as i32 => {
                BufferKind::Uint32
            }
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_BIG_INT64 as i32 => {
                BufferKind::BigInt64
            }
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_BIG_UINT64 as i32 => {
                BufferKind::BigUint64
            }
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_FLOAT16 as i32 => {
                BufferKind::Float16
            }
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_FLOAT32 as i32 => {
                BufferKind::Float32
            }
            x if x == rquickjs::qjs::JSTypedArrayEnum_JS_TYPED_ARRAY_FLOAT64 as i32 => {
                BufferKind::Float64
            }
            _ => return None,
        })
    }
}

#[cfg(feature = "quickjs")]
fn quickjs_native_work_stop(ctx: &rquickjs::Ctx<'_>, message: &'static str) -> rquickjs::Error {
    let _ = rquickjs::Exception::throw_internal(ctx, message);
    let error = ctx.catch();
    // SAFETY: the thrown Error belongs to this callback's live runtime.
    unsafe { rquickjs::qjs::JS_SetUncatchableError(ctx.as_raw().as_ptr(), error.as_raw()); }
    ctx.throw(error)
}

#[cfg(feature = "quickjs")]
pub fn quickjs_check_buffer_interrupt(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    quickjs_check_native_failure(ctx)?;
    if let Some(slot) = ctx.userdata::<QuickJsWorldSlot>() {
        if slot.0.native_memory_failure() {
            return Err(quickjs_native_work_stop(ctx, "page memory budget exceeded"));
        }
        if slot.0.script_interrupted() {
            return Err(quickjs_native_work_stop(ctx, "script execution interrupted"));
        }
    }
    Ok(())
}

#[cfg(feature = "quickjs")]
fn quickjs_buffer_backing<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: &rquickjs::Value<'js>,
    kind: BufferKind,
) -> rquickjs::Result<rquickjs::Value<'js>> {
    if kind == BufferKind::ArrayBuffer {
        return Ok(value.clone());
    }
    let name = if kind == BufferKind::DataView {
        "DataView.buffer"
    } else {
        "TypedArray.buffer"
    };
    let getter: rquickjs::Function = rquickjs::FromJs::from_js(ctx, quickjs_intrinsic(ctx, name)?)?;
    let backing = getter.call((rquickjs::function::This(value.clone()),))?;
    quickjs_check_buffer_interrupt(ctx)?;
    Ok(backing)
}

#[cfg(feature = "quickjs")]
fn quickjs_buffer_metadata<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: &rquickjs::Value<'js>,
    allow_buffer: bool,
) -> rquickjs::Result<(BufferMetadata, rquickjs::Value<'js>)> {
    quickjs_check_buffer_interrupt(ctx)?;
    let kind = quickjs_buffer_kind(value)
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "expected a native buffer source"))?;
    if !allow_buffer && kind == BufferKind::ArrayBuffer {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "expected an ArrayBufferView",
        ));
    }
    let backing = quickjs_buffer_backing(ctx, value, kind)?;
    // The trusted native getter may return an SAB; never infer safety from a
    // zero-length pointer or from a page-mutable prototype property.
    if !unsafe { rquickjs::qjs::JS_IsArrayBuffer(backing.as_raw()) } {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "shared buffer source is not supported",
        ));
    }
    if unsafe { rquickjs::qjs::JS_IsResizableArrayBuffer(backing.as_raw()) } {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "resizable buffer source is not supported",
        ));
    }
    if unsafe { rquickjs::qjs::JS_IsDetachedArrayBuffer(backing.as_raw()) } {
        return Ok((
            BufferMetadata {
                kind,
                byte_offset: 0,
                byte_length: 0,
            },
            backing,
        ));
    }
    let (offset, length) = if kind == BufferKind::ArrayBuffer {
        let buffer = backing
            .as_object()
            .and_then(|o| o.as_array_buffer())
            .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "buffer brand changed"))?;
        (0, buffer.len())
    } else if kind == BufferKind::DataView {
        let getter = |name| -> rquickjs::Result<usize> {
            let function: rquickjs::Function =
                rquickjs::FromJs::from_js(ctx, quickjs_intrinsic(ctx, name)?)?;
            let value = function.call((rquickjs::function::This(value.clone()),))?;
            quickjs_check_buffer_interrupt(ctx)?;
            rquickjs::FromJs::from_js(ctx, value)
        };
        (
            getter("DataView.byteOffset")?,
            getter("DataView.byteLength")?,
        )
    } else {
        let mut offset = 0;
        let mut length = 0;
        let mut element = 0;
        // SAFETY: the native brand, fixed backing and attached state have been
        // checked. The function returns an owned backing value or exception.
        let raw = unsafe {
            rquickjs::qjs::JS_GetTypedArrayBuffer(
                ctx.as_raw().as_ptr(),
                value.as_raw(),
                &mut offset,
                &mut length,
                &mut element,
            )
        };
        let returned = unsafe { rquickjs::Value::from_raw(ctx.clone(), raw) };
        if returned.is_exception() {
            return Err(rquickjs::Error::Exception);
        }
        let offset = usize::try_from(offset)
            .map_err(|_| rquickjs::Exception::throw_type(ctx, "invalid view offset"))?;
        let length = usize::try_from(length)
            .map_err(|_| rquickjs::Exception::throw_type(ctx, "invalid view length"))?;
        (offset, length)
    };
    let end = offset
        .checked_add(length)
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "buffer view range overflow"))?;
    let backing_len = backing
        .as_object()
        .and_then(|o| o.as_array_buffer())
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "buffer brand changed"))?
        .len();
    if end > backing_len {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "buffer view is out of bounds",
        ));
    }
    Ok((
        BufferMetadata {
            kind,
            byte_offset: offset,
            byte_length: length,
        },
        backing,
    ))
}

#[cfg(feature = "quickjs")]
pub fn buffer_source_ref_from_quickjs<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: rquickjs::Value<'js>,
) -> rquickjs::Result<BufferSourceRef> {
    let lease_id = quickjs_current_buffer_reference_lease(ctx)?;
    quickjs_buffer_metadata(ctx, &value, true)?;
    value_from_quickjs(ctx, value).map(|bits| BufferSourceRef::new(bits, lease_id))
}

#[cfg(feature = "quickjs")]
pub fn array_buffer_view_ref_from_quickjs<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: rquickjs::Value<'js>,
) -> rquickjs::Result<ArrayBufferViewRef> {
    let lease_id = quickjs_current_buffer_reference_lease(ctx)?;
    quickjs_buffer_metadata(ctx, &value, false)?;
    value_from_quickjs(ctx, value).map(|bits| ArrayBufferViewRef::new(bits, lease_id))
}

#[cfg(feature = "quickjs")]
pub fn buffer_source_ref_to_quickjs<'js>(
    ctx: &rquickjs::Ctx<'js>,
    reference: BufferSourceRef,
) -> rquickjs::Result<rquickjs::Value<'js>> {
    quickjs_validate_buffer_reference_lease(ctx, reference.lease_id())?;
    let value = value_to_quickjs(ctx, reference.bits())?;
    quickjs_buffer_metadata(ctx, &value, true)?;
    Ok(value)
}

#[cfg(feature = "quickjs")]
pub fn array_buffer_view_ref_to_quickjs<'js>(
    ctx: &rquickjs::Ctx<'js>,
    reference: ArrayBufferViewRef,
) -> rquickjs::Result<rquickjs::Value<'js>> {
    quickjs_validate_buffer_reference_lease(ctx, reference.lease_id())?;
    let value = value_to_quickjs(ctx, reference.bits())?;
    quickjs_buffer_metadata(ctx, &value, false)?;
    Ok(value)
}

#[cfg(feature = "quickjs")]
pub fn quickjs_buffer_source_info(
    ctx: &rquickjs::Ctx<'_>,
    reference: BufferSourceRef,
) -> rquickjs::Result<BufferMetadata> {
    quickjs_validate_buffer_reference_lease(ctx, reference.lease_id())?;
    let value = value_to_quickjs(ctx, reference.bits())?;
    quickjs_buffer_metadata(ctx, &value, true).map(|(info, _)| info)
}

#[cfg(feature = "quickjs")]
pub fn quickjs_array_buffer_view_info(
    ctx: &rquickjs::Ctx<'_>,
    reference: ArrayBufferViewRef,
) -> rquickjs::Result<BufferMetadata> {
    quickjs_validate_buffer_reference_lease(ctx, reference.lease_id())?;
    let value = value_to_quickjs(ctx, reference.bits())?;
    quickjs_buffer_metadata(ctx, &value, false).map(|(info, _)| info)
}

#[cfg(feature = "quickjs")]
pub fn quickjs_copy_buffer_source_into(
    ctx: &rquickjs::Ctx<'_>,
    reference: BufferSourceRef,
    destination: &mut [u8],
) -> rquickjs::Result<BufferMetadata> {
    quickjs_validate_buffer_reference_lease(ctx, reference.lease_id())?;
    let value = value_to_quickjs(ctx, reference.bits())?;
    let (info, backing) = quickjs_buffer_metadata(ctx, &value, true)?;
    if destination.len() != info.byte_length {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "buffer destination length changed",
        ));
    }
    if info.byte_length == 0 {
        return Ok(info);
    }
    let buffer = backing
        .as_object()
        .and_then(|o| o.as_array_buffer())
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "buffer brand changed"))?;
    let all = buffer
        .as_bytes()
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "buffer detached"))?;
    let end = info
        .byte_offset
        .checked_add(info.byte_length)
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "buffer range overflow"))?;
    let bytes = all
        .get(info.byte_offset..end)
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "buffer range changed"))?;
    for (source, target) in bytes
        .chunks(64 * 1024)
        .zip(destination.chunks_mut(64 * 1024))
    {
        quickjs_check_buffer_interrupt(ctx)?;
        target.copy_from_slice(source);
    }
    Ok(info)
}

#[cfg(feature = "quickjs")]
pub fn quickjs_write_array_buffer_view(
    ctx: &rquickjs::Ctx<'_>,
    reference: ArrayBufferViewRef,
    source: &[u8],
) -> rquickjs::Result<BufferMetadata> {
    quickjs_validate_buffer_reference_lease(ctx, reference.lease_id())?;
    let value = value_to_quickjs(ctx, reference.bits())?;
    let (info, backing) = quickjs_buffer_metadata(ctx, &value, false)?;
    if source.len() != info.byte_length {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "buffer source length changed",
        ));
    }
    // This native class query does not read a page property or invoke Proxy
    // traps. QuickJS's raw pointer API does not enforce immutability itself.
    if unsafe { rquickjs::qjs::JS_IsImmutableArrayBuffer(backing.as_raw()) } != 0 {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "immutable buffer cannot be written",
        ));
    }
    if info.byte_length == 0 {
        return Ok(info);
    }
    let mut len = 0;
    // SAFETY: backing is a live, attached, exact native ArrayBuffer. No page JS
    // runs while its pointer is used; the saved Value roots it for this scope.
    let ptr = unsafe {
        rquickjs::qjs::JS_GetArrayBuffer(ctx.as_raw().as_ptr(), &mut len, backing.as_raw())
    };
    let len = usize::try_from(len)
        .map_err(|_| rquickjs::Exception::throw_type(ctx, "invalid buffer length"))?;
    let end = info
        .byte_offset
        .checked_add(info.byte_length)
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "buffer range overflow"))?;
    if ptr.is_null() || end > len {
        return Err(rquickjs::Exception::throw_type(ctx, "buffer range changed"));
    }
    if source_overlaps_backing(source, ptr.cast_const(), len) {
        return Err(rquickjs::Exception::throw_type(ctx, "buffer write source overlaps backing"));
    }
    for (index, chunk) in source.chunks(64 * 1024).enumerate() {
        quickjs_check_buffer_interrupt(ctx)?;
        let start = info.byte_offset + index * 64 * 1024;
        // SAFETY: the entire source was checked disjoint from this native
        // backing before mutation, and `start..start+chunk.len()` is in range.
        unsafe {
            std::ptr::copy_nonoverlapping(chunk.as_ptr(), ptr.add(start), chunk.len());
        }
    }
    Ok(info)
}
"#;
