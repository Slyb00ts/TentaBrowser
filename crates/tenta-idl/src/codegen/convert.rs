//! Expression builders for the JavaScript-value conversions the glue needs.
//!
//! Every builder returns a Rust *expression*; the callers wrap it in a `let`
//! binding and propagate the error. Conversions are uniformly fallible so that
//! the callback bodies have one shape.

use crate::codegen::rust_type::{Position, integer_suffix, rust_type};
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
            format!("byte_string_from_v8({scope}, {value})")
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
        Type::Record(key, inner) => {
            let key = record_key_from_v8(key);
            let element = from_v8(inner, "scope", "item", IntegerMode::Modulo);
            format!("record_from_v8({scope}, {value}, {key}, |scope, item| {element})")
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
        Type::ByteString => format!("byte_string_to_v8({scope}, &{value})"),
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
        Type::Record(key, inner) => {
            let key = record_key_to_v8(key);
            let element = to_v8(inner, "scope", "item");
            format!("record_to_v8({scope}, {value}, {key}, |scope, item| {element})")
        }
        Type::Nullable(inner) => {
            let inner_expr = to_v8(inner, scope, "inner");
            format!(
                "match {value} {{ Some(inner) => {inner_expr}, None => Ok(v8::null({scope}).into()) }}"
            )
        }
    }
}

/// The message the generated key converter raises for a key type WebIDL does
/// not admit.
const RECORD_KEY_TYPE_ERROR: &str = "a record key must be a string type";

/// The converter for a record's declared key type, reading one JavaScript
/// property name into the key type's Rust form.
///
/// WebIDL's `es-to-record` applies the *declared* key type's conversion to each
/// own enumerable key, so which converter runs is decided by `record`'s key type
/// and not by the Rust type the key happens to have. Only `ByteString`,
/// `DOMString` and `USVString` are legal key types, and the prelude has the
/// conversion for each: `string_from_v8` for the two string types, and
/// `byte_string_from_v8` for `ByteString`, which reads one code unit per byte
/// and rejects a code unit above `0xFF` instead of truncating it.
///
/// `ByteString` names that helper directly rather than wrapping it in a
/// closure, so the emitted call site does not read as a `redundant_closure`.
///
/// Any other key type is not valid WebIDL and has no key conversion to apply,
/// so the emitted converter fails with a `TypeError` rather than quietly
/// reusing the type's ordinary conversion — the record would then be keyed by
/// something the declaration does not name. No such declaration exists in the
/// corpus; this arm is what keeps one from passing silently if it appears.
fn record_key_from_v8(ty: &Type) -> String {
    match ty {
        Type::DomString | Type::UsvString => {
            format!(
                "|scope, key| {}",
                from_v8(ty, "scope", "key", IntegerMode::Modulo)
            )
        }
        Type::ByteString => "byte_string_from_v8".to_string(),
        other => format!(
            "|_scope, _key| Err::<{}, JsError>(JsError::type_error(\"{RECORD_KEY_TYPE_ERROR}\"))",
            rust_type(other, Position::Owned)
        ),
    }
}

/// The converter for a record's declared key type, writing the key type's Rust
/// form back to the property name it is defined under.
///
/// The mirror image of [`record_key_from_v8`], with the same three legal key
/// types and the same refusal for every other one.
fn record_key_to_v8(ty: &Type) -> String {
    match ty {
        Type::DomString | Type::UsvString | Type::ByteString => {
            format!("|scope, key| {}", to_v8(ty, "scope", "key"))
        }
        _ => format!(
            "|_scope, _key| Err::<v8::Local<v8::Value>, JsError>(JsError::type_error(\"{RECORD_KEY_TYPE_ERROR}\"))"
        ),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A record whose value type is the string one, so the emitted text differs
    /// only in the key converter under test.
    fn record(key: Type) -> Type {
        Type::Record(Box::new(key), Box::new(Type::DomString))
    }

    /// The converter's key type must be the type `rust_type` names, or the
    /// generated `*Ops`/`*V8` glue does not compile — which is what happened
    /// for `record<ByteString, _>` before the key type reached the conversion.
    #[test]
    fn a_byte_string_record_converts_its_keys_as_bytes() {
        assert_eq!(
            from_v8(
                &record(Type::ByteString),
                "scope",
                "value",
                IntegerMode::Modulo
            ),
            "record_from_v8(scope, value, byte_string_from_v8, |scope, item| Ok::<String, JsError>(string_from_v8(scope, item)))"
        );
        assert_eq!(
            to_v8(&record(Type::ByteString), "scope", "value"),
            "record_to_v8(scope, value, |scope, key| byte_string_to_v8(scope, &key), |scope, item| string_to_v8(scope, &item))"
        );
    }

    /// `URLSearchParams` ships on this one, so it is pinned: the key conversion
    /// is the string key's own, and the value conversion is unchanged by it.
    #[test]
    fn a_string_record_converts_its_keys_as_strings() {
        for key in [Type::DomString, Type::UsvString] {
            assert_eq!(
                from_v8(&record(key), "scope", "value", IntegerMode::Modulo),
                "record_from_v8(scope, value, |scope, key| Ok::<String, JsError>(string_from_v8(scope, key)), |scope, item| Ok::<String, JsError>(string_from_v8(scope, item)))"
            );
        }
        assert_eq!(
            to_v8(&record(Type::DomString), "scope", "value"),
            "record_to_v8(scope, value, |scope, key| string_to_v8(scope, &key), |scope, item| string_to_v8(scope, &item))"
        );
    }

    /// WebIDL admits only the three string key types, so a key of any other
    /// type has no conversion. Emitting a failure is what keeps such a
    /// declaration from being keyed by a conversion it never asked for.
    #[test]
    fn a_non_string_record_key_has_no_conversion() {
        assert_eq!(
            from_v8(&record(Type::Long), "scope", "value", IntegerMode::Modulo),
            "record_from_v8(scope, value, |_scope, _key| Err::<i32, JsError>(JsError::type_error(\"a record key must be a string type\")), |scope, item| Ok::<String, JsError>(string_from_v8(scope, item)))"
        );
        assert_eq!(
            to_v8(&record(Type::Long), "scope", "value"),
            "record_to_v8(scope, value, |_scope, _key| Err::<v8::Local<v8::Value>, JsError>(JsError::type_error(\"a record key must be a string type\")), |scope, item| string_to_v8(scope, &item))"
        );
    }
}
