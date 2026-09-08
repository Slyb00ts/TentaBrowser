//! Expression builders for the JavaScript-value conversions the glue needs.
//!
//! Every builder returns a Rust *expression*; the callers wrap it in a `let`
//! binding and propagate the error. Conversions are uniformly fallible so that
//! the callback bodies have one shape.

use crate::codegen::rust_type::integer_suffix;
use crate::model::{Argument, Type};

/// `[Clamp]` / `[EnforceRange]` state of an integer conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntegerMode {
    /// Plain modulo conversion.
    #[default]
    Modulo,
    /// `[Clamp]`
    Clamp,
    /// `[EnforceRange]`
    EnforceRange,
}

impl IntegerMode {
    /// The mode an argument's extended attributes select.
    pub fn of(argument: &Argument) -> Self {
        if argument.clamp() {
            IntegerMode::Clamp
        } else if argument.enforce_range() {
            IntegerMode::EnforceRange
        } else {
            IntegerMode::Modulo
        }
    }
}

/// Builds an expression of type `Result<T, JsError>` converting `value` from V8.
pub fn from_v8(ty: &Type, scope: &str, value: &str, mode: IntegerMode) -> String {
    match ty {
        Type::Undefined => "Ok::<(), JsError>(())".to_string(),
        Type::Any => format!("value_from_v8({scope}, {value}).map(AnyValue)"),
        Type::Object => format!("value_from_v8({scope}, {value}).map(JsObject)"),
        Type::Symbol => format!("value_from_v8({scope}, {value}).map(JsSymbol)"),
        Type::Boolean => format!("Ok::<bool, JsError>(bool_from_v8({scope}, {value}))"),
        Type::Long if mode == IntegerMode::Modulo => format!("i32_from_v8({scope}, {value})"),
        Type::UnsignedLong if mode == IntegerMode::Modulo => {
            format!("u32_from_v8({scope}, {value})")
        }
        ty if ty.is_integer() => {
            let suffix = integer_suffix(ty).expect("integer type");
            match mode {
                IntegerMode::Modulo => {
                    format!("number_from_v8({scope}, {value}).map(to_{suffix})")
                }
                IntegerMode::Clamp => {
                    format!("number_from_v8({scope}, {value}).map(clamp_{suffix})")
                }
                IntegerMode::EnforceRange => {
                    format!("number_from_v8({scope}, {value}).and_then(enforce_{suffix})")
                }
            }
        }
        Type::Float { .. } => format!("number_from_v8({scope}, {value}).map(|n| n as f32)"),
        Type::Double { .. } => format!("number_from_v8({scope}, {value})"),
        Type::DomString | Type::UsvString => {
            format!("Ok::<String, JsError>(string_from_v8({scope}, {value}))")
        }
        Type::ByteString => {
            format!("Ok::<Vec<u8>, JsError>(string_from_v8({scope}, {value}).into_bytes())")
        }
        Type::Interface(name) => {
            format!("handle_from_v8::<{name}>({scope}, {value})")
        }
        Type::Dictionary(name) | Type::Enum(name) | Type::Union(name) => {
            format!("{name}::from_v8({scope}, {value})")
        }
        Type::Callback(name) | Type::CallbackInterface(name) => {
            format!("value_from_v8({scope}, {value}).map({name}::from_bits)")
        }
        Type::Promise(_) => format!("value_from_v8({scope}, {value}).map(Promise::from_bits)"),
        Type::Sequence(inner) | Type::FrozenArray(inner) | Type::ObservableArray(inner) => {
            let element = from_v8(inner, "scope", "item", IntegerMode::Modulo);
            format!("sequence_from_v8({scope}, {value}, |scope, item| {element})")
        }
        Type::Record(_, inner) => {
            let element = from_v8(inner, "scope", "item", IntegerMode::Modulo);
            format!("record_from_v8({scope}, {value}, |scope, item| {element})")
        }
        Type::Nullable(inner) => {
            let inner_expr = from_v8(inner, scope, value, mode);
            format!(
                "if {value}.is_null_or_undefined() {{ Ok(None) }} else {{ ({inner_expr}).map(Some) }}"
            )
        }
        other => unreachable!("integer type {other:?} is handled by the guarded arm above"),
    }
}

/// Builds an expression of type `Result<v8::Local<v8::Value>, JsError>` that
/// converts the Rust value `value` back to V8.
pub fn to_v8(ty: &Type, scope: &str, value: &str) -> String {
    let wrap = |expr: String| format!("Ok::<v8::Local<v8::Value>, JsError>({expr})");
    match ty {
        Type::Undefined => wrap(format!("v8::undefined({scope}).into()")),
        Type::Boolean => wrap(format!("v8::Boolean::new({scope}, {value}).into()")),
        Type::Byte | Type::Short | Type::Long => {
            wrap(format!("v8::Integer::new({scope}, {value} as i32).into()"))
        }
        Type::Octet | Type::UnsignedShort | Type::UnsignedLong => wrap(format!(
            "v8::Integer::new_from_unsigned({scope}, {value} as u32).into()"
        )),
        Type::LongLong | Type::UnsignedLongLong | Type::Float { .. } | Type::Double { .. } => {
            wrap(format!("v8::Number::new({scope}, {value} as f64).into()"))
        }
        Type::DomString | Type::UsvString => format!("string_to_v8({scope}, &{value})"),
        Type::ByteString => {
            format!("string_to_v8({scope}, &String::from_utf8_lossy(&{value}))")
        }
        Type::Any | Type::Object | Type::Symbol => format!("value_to_v8({scope}, {value}.0)"),
        Type::Interface(name) => format!("handle_to_v8::<{name}>({scope}, {value})"),
        Type::Dictionary(_) | Type::Enum(_) | Type::Union(_) => {
            format!("{value}.to_v8({scope})")
        }
        Type::Callback(_) | Type::CallbackInterface(_) | Type::Promise(_) => {
            format!("value_to_v8({scope}, {value}.bits())")
        }
        Type::Sequence(inner) | Type::FrozenArray(inner) | Type::ObservableArray(inner) => {
            let element = to_v8(inner, "scope", "item");
            format!("sequence_to_v8({scope}, {value}, |scope, item| {element})")
        }
        Type::Record(_, inner) => {
            let element = to_v8(inner, "scope", "item");
            format!("record_to_v8({scope}, {value}, |scope, item| {element})")
        }
        Type::Nullable(inner) => {
            let inner_expr = to_v8(inner, scope, "inner");
            format!(
                "match {value} {{ Some(inner) => {inner_expr}, None => Ok(v8::null({scope}).into()) }}"
            )
        }
    }
}

/// The `ReturnValue` setter for a type, when one of the specialised setters
/// applies. Returns `None` when the value has to go through [`to_v8`].
pub fn return_value_setter(ty: &Type) -> Option<&'static str> {
    Some(match ty {
        Type::Boolean => "set_bool",
        Type::Byte | Type::Short | Type::Long => "set_int32",
        Type::Octet | Type::UnsignedShort | Type::UnsignedLong => "set_uint32",
        Type::LongLong | Type::UnsignedLongLong | Type::Float { .. } | Type::Double { .. } => {
            "set_double"
        }
        _ => return None,
    })
}

/// The cast a specialised `ReturnValue` setter needs, e.g. `as i32`.
pub fn return_value_cast(ty: &Type) -> &'static str {
    match ty {
        Type::Byte | Type::Short | Type::Long => " as i32",
        Type::Octet | Type::UnsignedShort | Type::UnsignedLong => " as u32",
        Type::LongLong | Type::UnsignedLongLong | Type::Float { .. } | Type::Double { .. } => {
            " as f64"
        }
        _ => "",
    }
}
