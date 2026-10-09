//! The WebIDL-to-Rust type mapping documented in `docs/dev/idl.md`.

use crate::model::{DefaultValue, Type};

/// Whether a type appears where a value is produced or where one is consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// Return types, dictionary fields, sequence elements: owned values.
    Owned,
    /// Operation and setter arguments: strings are borrowed.
    Argument,
}

/// The Rust spelling of a WebIDL type.
pub fn rust_type(ty: &Type, position: Position) -> String {
    match ty {
        Type::Undefined => "()".to_string(),
        Type::Any => "AnyValue".to_string(),
        Type::Boolean => "bool".to_string(),
        Type::Byte => "i8".to_string(),
        Type::Octet => "u8".to_string(),
        Type::Short => "i16".to_string(),
        Type::UnsignedShort => "u16".to_string(),
        Type::Long => "i32".to_string(),
        Type::UnsignedLong => "u32".to_string(),
        Type::LongLong => "i64".to_string(),
        Type::UnsignedLongLong => "u64".to_string(),
        Type::Float { .. } => "f32".to_string(),
        Type::Double { .. } => "f64".to_string(),
        Type::DomString | Type::UsvString => match position {
            Position::Owned => "String".to_string(),
            Position::Argument => "&str".to_string(),
        },
        Type::ByteString => match position {
            Position::Owned => "Vec<u8>".to_string(),
            Position::Argument => "&[u8]".to_string(),
        },
        Type::Object => "JsObject".to_string(),
        Type::Symbol => "JsSymbol".to_string(),
        Type::Interface(name) => format!("Handle<{name}>"),
        // A buffer source crosses the host boundary as the bytes it holds: a
        // `Handle<Uint8Array>` would be a wrapper object rather than the native
        // typed array the script side has to receive, and only the bytes are
        // what a host can use. Both positions are the owned `Vec<u8>` because
        // the argument binding owns the copy the conversion made, so the
        // borrowed form would not be the expression the caller passes.
        Type::Buffer(_) => "Vec<u8>".to_string(),
        Type::Dictionary(name)
        | Type::Enum(name)
        | Type::Callback(name)
        | Type::CallbackInterface(name)
        | Type::Union(name) => name.clone(),
        Type::Sequence(inner) | Type::FrozenArray(inner) | Type::ObservableArray(inner) => {
            format!("Vec<{}>", rust_type(inner, Position::Owned))
        }
        Type::Record(key, value) => format!(
            "Vec<({}, {})>",
            rust_type(key, Position::Owned),
            rust_type(value, Position::Owned)
        ),
        Type::Promise(inner) => format!("Promise<{}>", rust_type(inner, Position::Owned)),
        Type::Nullable(inner) => format!("Option<{}>", rust_type(inner, position)),
    }
}

/// The Rust expression for a WebIDL default value at `ty`.
pub fn default_expr(ty: &Type, default: &DefaultValue) -> String {
    match default {
        DefaultValue::Boolean(value) => value.to_string(),
        DefaultValue::Integer(value) => match ty.unwrap_nullable() {
            Type::Float { .. } | Type::Double { .. } => format!("{value}.0"),
            _ => value.to_string(),
        },
        DefaultValue::Float(value) => float_literal(*value),
        DefaultValue::String(value) => match ty.unwrap_nullable() {
            Type::Enum(name) => format!("{name}::{}", enum_variant(ty, value)),
            // A `ByteString` is a `Vec<u8>` in Rust, so the string literal the
            // IDL writes has to become its bytes. Emitting `String::from` here
            // would produce a default of the one type the field cannot hold,
            // which is a generator defect rather than a spelling the caller
            // could work around (`ByteString statusText = "";` is the member
            // that surfaced it).
            Type::ByteString => format!("{value:?}.as_bytes().to_vec()"),
            _ => format!("String::from({value:?})"),
        },
        DefaultValue::Null => "None".to_string(),
        DefaultValue::EmptySequence => "Vec::new()".to_string(),
        DefaultValue::EmptyDictionary => "Default::default()".to_string(),
    }
}

/// The Rust variant name a string default maps to, for enumeration defaults.
///
/// The lowering keeps the WebIDL spelling, so the variant is recomputed here
/// with the same rule the enumeration generator uses.
fn enum_variant(_ty: &Type, value: &str) -> String {
    crate::naming::pascal_case(value)
}

/// A Rust float literal that round-trips, including the non-finite values
/// WebIDL allows for `unrestricted` types.
pub fn float_literal(value: f64) -> String {
    if value.is_nan() {
        "f64::NAN".to_string()
    } else if value == f64::INFINITY {
        "f64::INFINITY".to_string()
    } else if value == f64::NEG_INFINITY {
        "f64::NEG_INFINITY".to_string()
    } else if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.1}")
    } else {
        format!("{value:?}")
    }
}

/// The name of the integer conversion helpers in the prelude for `ty`.
///
/// Returns `None` for types that are not WebIDL integers.
pub fn integer_suffix(ty: &Type) -> Option<&'static str> {
    Some(match ty {
        Type::Byte => "byte",
        Type::Octet => "octet",
        Type::Short => "short",
        Type::UnsignedShort => "unsigned_short",
        Type::Long => "long",
        Type::UnsignedLong => "unsigned_long",
        Type::LongLong => "long_long",
        Type::UnsignedLongLong => "unsigned_long_long",
        _ => return None,
    })
}

/// The V8 `fast_api::Type` variant that carries `ty` in a fast call.
pub fn fast_api_type(ty: &Type) -> &'static str {
    match ty {
        Type::Undefined => "Void",
        Type::Boolean => "Bool",
        Type::Byte | Type::Short | Type::Long => "Int32",
        Type::Octet | Type::UnsignedShort | Type::UnsignedLong => "Uint32",
        Type::LongLong => "Int64",
        Type::UnsignedLongLong => "Uint64",
        Type::Float { .. } => "Float32",
        Type::Double { .. } => "Float64",
        Type::DomString | Type::UsvString | Type::ByteString => "SeqOneByteString",
        _ => "V8Value",
    }
}

/// The Rust type a V8 fast-API callback receives or returns for `ty`.
pub fn fast_api_rust_type(ty: &Type) -> String {
    match ty {
        Type::Undefined => "()".to_string(),
        Type::Boolean => "bool".to_string(),
        Type::Byte | Type::Short | Type::Long => "i32".to_string(),
        Type::Octet | Type::UnsignedShort | Type::UnsignedLong => "u32".to_string(),
        Type::LongLong => "i64".to_string(),
        Type::UnsignedLongLong => "u64".to_string(),
        Type::Float { .. } => "f32".to_string(),
        Type::Double { .. } => "f64".to_string(),
        Type::DomString | Type::UsvString | Type::ByteString => {
            "*const v8::fast_api::FastApiOneByteString".to_string()
        }
        other => rust_type(other, Position::Owned),
    }
}

/// Converts a fast-call argument to the Rust type the trait method expects.
///
/// V8 carries every integer narrower than 32 bits as `Int32`/`Uint32`, so the
/// remaining WebIDL narrowing happens here: `[Clamp]` clamps to the target
/// range, the plain form wraps modulo 2^n like a Rust `as` cast.
pub fn fast_argument_convert(ty: &Type, clamp: bool, name: &str) -> String {
    let target = rust_type(ty, Position::Owned);
    if fast_api_rust_type(ty) == target {
        return name.to_string();
    }
    match (ty.is_integer(), clamp) {
        (true, true) => {
            let suffix = integer_suffix(ty).expect("integer type");
            format!("clamp_{suffix}({name} as f64)")
        }
        (true, false) => format!("{name} as {target}"),
        _ => name.to_string(),
    }
}

/// Widens a trait return value to the type a fast callback must return.
pub fn fast_return_convert(ty: &Type, expr: &str) -> String {
    let fast = fast_api_rust_type(ty);
    if fast == rust_type(ty, Position::Owned) || matches!(ty, Type::Undefined) {
        expr.to_string()
    } else {
        format!("{expr} as {fast}")
    }
}

/// The operation-local Rust spelling of a direct buffer reference.
/// All unannotated buffer positions retain the ordinary copied `Vec<u8>` form.
pub fn operation_buffer_type(ty: &Type, references: bool, position: Position) -> String {
    if references {
        match ty {
            Type::Buffer(crate::model::BufferType::BufferSource) => {
                return "BufferSourceRef".into();
            }
            Type::Buffer(crate::model::BufferType::ArrayBufferView) => {
                return "ArrayBufferViewRef".into();
            }
            _ => {}
        }
    }
    rust_type(ty, position)
}
