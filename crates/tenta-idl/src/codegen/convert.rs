//! Expression builders for the JavaScript-value conversions the glue needs.
//!
//! Every builder returns a Rust *expression*; the callers wrap it in a `let`
//! binding and propagate the error. Conversions are uniformly fallible so that
//! the callback bodies have one shape.

use crate::codegen::rust_type::{Position, integer_suffix, rust_type};
use crate::model::{Argument, ExtendedAttributes, Model, Type};

/// Applies the DOMString/USVString annotation without changing ordinary
/// undefined or object coercion. Nullable strings cannot carry this annotation.
pub(super) fn legacy_null_to_empty(
    ty: &Type,
    attrs: &ExtendedAttributes,
    value: &str,
    conversion: String,
) -> String {
    if matches!(ty, Type::DomString | Type::UsvString) && attrs.has("LegacyNullToEmptyString") {
        format!("if {value}.is_null() {{ Ok(String::new()) }} else {{ {conversion} }}")
    } else {
        conversion
    }
}

/// The extended attribute WebIDL gives its loose callback conversion to; see
/// [`from_v8_for_attribute_setter`].
const LEGACY_TREAT_NON_OBJECT_AS_NULL: &str = "LegacyTreatNonObjectAsNull";

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
        Type::Float { unrestricted: true } => {
            format!("number_from_v8({scope}, {value}).map(|number| number as f32)")
        }
        Type::Float {
            unrestricted: false,
        } => format!("number_from_v8({scope}, {value}).and_then(restricted_f32)"),
        Type::Double { unrestricted: true } => format!("number_from_v8({scope}, {value})"),
        Type::Double {
            unrestricted: false,
        } => format!("number_from_v8({scope}, {value}).and_then(restricted_f64)"),
        Type::DomString | Type::UsvString => {
            format!("string_from_v8({scope}, {value})")
        }
        Type::ByteString => {
            format!("byte_string_from_v8({scope}, {value})")
        }
        Type::Interface(name) => {
            format!("handle_from_v8::<{name}>({scope}, {value})")
        }
        // A buffer source is not a host object: it is a native V8 typed array,
        // data view or array buffer, and the prelude converts it by its bytes.
        // The name comes from the type, so the call and the prelude's definition
        // of it are generated from one source.
        Type::Buffer(kind) => {
            format!("{}_from_v8({scope}, {value})", kind.conversion_suffix())
        }
        Type::Dictionary(name) | Type::Enum(name) | Type::Union(name) => {
            format!("{name}::from_v8({scope}, {value})")
        }
        // A callback interface converts as an object and nothing more: the
        // whole algorithm is "if V is not an Object, throw a TypeError"
        // (callback interface types, §3.2.16). Whether the object is callable
        // is deliberately not asked here — that is the `handleEvent` lookup at
        // *invocation* time, which the bindings already perform.
        Type::CallbackInterface(name) => format!(
            "if {value}.is_object() {{ value_from_v8({scope}, {value}).map({name}::from_bits) }} \
             else {{ Err::<{name}, JsError>(JsError::type_error(\"the value is not an object, so it is not an `{name}`\")) }}"
        ),
        // A callback function converts as a callable: the conversion throws
        // unless `IsCallable(V)` (callback function types, §3.2.19).
        // `is_function` is that test — a callable `Proxy` passes it, where the
        // stricter `TryFrom<Local<Function>>` would not. The one escape,
        // `[LegacyTreatNonObjectAsNull]`, is a property of the *assignment* and
        // not of the type, so it is applied by
        // [`from_v8_for_attribute_setter`] and never here.
        Type::Callback(name) => format!(
            "if {value}.is_function() {{ value_from_v8({scope}, {value}).map({name}::from_bits) }} \
             else {{ Err::<{name}, JsError>(JsError::type_error(\"the value is not callable, so it is not an `{name}`\")) }}"
        ),
        Type::Promise(_) => format!("value_from_v8({scope}, {value}).map(Promise::from_bits)"),
        Type::Sequence(inner) | Type::FrozenArray(inner) | Type::ObservableArray(inner) => {
            let element = from_v8_converter(inner);
            format!("sequence_from_v8({scope}, {value}, {element})")
        }
        Type::Record(key, inner) => {
            let key = record_key_from_v8(key);
            let element = from_v8_converter(inner);
            format!("record_from_v8({scope}, {value}, {key}, {element})")
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

/// The converter passed to sequence and record helpers.
///
/// String conversions are function items now that every V8 `ToString` path is
/// fallible. Other types still need an expression generated for each element.
fn from_v8_converter(ty: &Type) -> String {
    match ty {
        Type::DomString | Type::UsvString => "string_from_v8".to_owned(),
        Type::ByteString => "byte_string_from_v8".to_owned(),
        other => format!(
            "|scope, item| {}",
            from_v8(other, "scope", "item", IntegerMode::Modulo)
        ),
    }
}

/// Builds the expression converting a value that is being **assigned to an
/// attribute**.
///
/// This is the one conversion position where `[LegacyTreatNonObjectAsNull]`
/// applies, and every one of the rule's conditions is about that position: the
/// value must be assigned to an *attribute*, the attribute's type must be a
/// *nullable* callback function, and the callback must carry the annotation
/// (§3.4.8; the escape clause in §3.2.19 step 1 and the early return in
/// §3.2.20 step 1 both restate all three).
///
/// Anywhere else — an operation argument, a dictionary member, an optional
/// argument — [`from_v8`]'s strict gate is what WebIDL asks for, so the plain
/// entry point keeps it.
///
/// The annotation is looked up on the *lowered* callback the nullable wraps,
/// which is what makes both spellings reach it: `attribute ErrorHandler?
/// handler2;` names the annotated callback directly, and the HTML Standard's
/// `typedef EventHandlerNonNull? EventHandler;` resolves through the typedef to
/// the same `Nullable(Callback(..))` shape.
pub fn from_v8_for_attribute_setter(
    model: &Model,
    ty: &Type,
    scope: &str,
    value: &str,
    mode: IntegerMode,
) -> String {
    if let Type::Nullable(inner) = ty
        && let Type::Callback(name) = inner.as_ref()
        && model
            .callback(name)
            .is_some_and(|callback| callback.attrs.has(LEGACY_TREAT_NON_OBJECT_AS_NULL))
    {
        // The loose path is the nullable conversion with a wider early return:
        // a non-object — `null` and `undefined` included — becomes `null`, and
        // **any** object is stored as itself, callable or not, so the inner
        // conversion is the plain reference with no callable gate.
        let reference = format!("value_from_v8({scope}, {value}).map({name}::from_bits)");
        return format!(
            "if !{value}.is_object() {{ Ok(None) }} else {{ ({reference}).map(Some) }}"
        );
    }
    from_v8(ty, scope, value, mode)
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
        Type::Buffer(kind) => format!("{}_to_v8({scope}, {value})", kind.conversion_suffix()),
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
/// All three string key types name their fallible helper directly, avoiding
/// redundant closures in generated call sites.
///
/// Any other key type is not valid WebIDL and has no key conversion to apply,
/// so the emitted converter fails with a `TypeError` rather than quietly
/// reusing the type's ordinary conversion — the record would then be keyed by
/// something the declaration does not name. No such declaration exists in the
/// corpus; this arm is what keeps one from passing silently if it appears.
fn record_key_from_v8(ty: &Type) -> String {
    match ty {
        Type::DomString | Type::UsvString => "string_from_v8".to_owned(),
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
    use crate::model::{BufferType, Member};

    /// The WebIDL §3.4.8 contrastive pair: one nullable callback attribute
    /// whose callback is unannotated, one whose callback carries
    /// `[LegacyTreatNonObjectAsNull]`.
    fn handlers() -> Model {
        crate::parse_webidl(
            "callback OccurrenceHandler = undefined (DOMString details);\n\
             [LegacyTreatNonObjectAsNull]\n\
             callback ErrorHandler = undefined (DOMString details);\n\
             interface Manager {\n  \
             attribute OccurrenceHandler? handler1;\n  \
             attribute ErrorHandler? handler2;\n\
             };\n",
        )
        .expect("the snippet parses")
    }

    /// The lowered type of one attribute, which is what the conversion sees.
    fn attribute_type(model: &Model, interface_name: &str, name: &str) -> Type {
        let interface = model.interface(interface_name).expect("the interface");
        match interface.member(name) {
            Some(Member::Attribute(attribute)) => attribute.ty.clone(),
            other => panic!("`{interface_name}.{name}` is an attribute, not {other:?}"),
        }
    }

    /// The expression for one attribute's setter on `model`.
    fn setter_conversion(model: &Model, interface_name: &str, name: &str) -> String {
        from_v8_for_attribute_setter(
            model,
            &attribute_type(model, interface_name, name),
            "scope",
            "raw",
            IntegerMode::Modulo,
        )
    }

    /// A callback interface converts as an object — no callable test, because
    /// `handleEvent` is looked up when the callback is *invoked*.
    #[test]
    fn a_callback_interface_is_gated_on_being_an_object() {
        assert_eq!(
            from_v8(
                &Type::CallbackInterface("EventListener".to_string()),
                "scope",
                "raw",
                IntegerMode::Modulo
            ),
            "if raw.is_object() { value_from_v8(scope, raw).map(EventListener::from_bits) } \
             else { Err::<EventListener, JsError>(JsError::type_error(\"the value is not an object, so it is not an `EventListener`\")) }"
        );
    }

    /// A callback function converts as a callable. This is the arm every
    /// position other than an attribute setter gets, and it is deliberately
    /// blind to `[LegacyTreatNonObjectAsNull]`: the annotation escapes the gate
    /// only for an attribute assignment, never for an argument.
    #[test]
    fn a_callback_function_is_gated_on_being_callable() {
        let strict = "if raw.is_function() { value_from_v8(scope, raw).map(OccurrenceHandler::from_bits) } \
             else { Err::<OccurrenceHandler, JsError>(JsError::type_error(\"the value is not callable, so it is not an `OccurrenceHandler`\")) }";
        assert_eq!(
            from_v8(
                &Type::Callback("OccurrenceHandler".to_string()),
                "scope",
                "raw",
                IntegerMode::Modulo
            ),
            strict
        );

        // Even the annotated callback keeps the strict gate where the
        // conversion is not an attribute assignment.
        assert_eq!(
            from_v8(
                &Type::Callback("ErrorHandler".to_string()),
                "scope",
                "raw",
                IntegerMode::Modulo
            ),
            "if raw.is_function() { value_from_v8(scope, raw).map(ErrorHandler::from_bits) } \
             else { Err::<ErrorHandler, JsError>(JsError::type_error(\"the value is not callable, so it is not an `ErrorHandler`\")) }"
        );
    }

    /// An unannotated nullable callback attribute keeps the strict gate, which
    /// the nullable wrapper now carries into its non-null branch. Assigning it
    /// `123` or `{}` throws, as §3.4.8's `handler1` does.
    #[test]
    fn an_unannotated_nullable_callback_attribute_keeps_the_strict_gate() {
        let model = handlers();
        assert_eq!(
            setter_conversion(&model, "Manager", "handler1"),
            "if raw.is_null_or_undefined() { Ok(None) } else { (if raw.is_function() { value_from_v8(scope, raw).map(OccurrenceHandler::from_bits) } else { Err::<OccurrenceHandler, JsError>(JsError::type_error(\"the value is not callable, so it is not an `OccurrenceHandler`\")) }).map(Some) }"
        );
    }

    /// An annotated nullable callback attribute converts loosely: a non-object
    /// becomes `null`, and **any** object is stored as itself — there is no
    /// callable test on that branch, so `handler2 = {}` keeps the object.
    #[test]
    fn an_annotated_nullable_callback_attribute_takes_the_loose_conversion() {
        let model = handlers();
        assert_eq!(
            setter_conversion(&model, "Manager", "handler2"),
            "if !raw.is_object() { Ok(None) } else { (value_from_v8(scope, raw).map(ErrorHandler::from_bits)).map(Some) }"
        );
    }

    /// The rule needs all three conditions, so dropping the nullability drops
    /// the loose path: a non-nullable annotated callback attribute throws on a
    /// non-callable exactly as an argument does.
    #[test]
    fn a_non_nullable_annotated_callback_attribute_keeps_the_strict_gate() {
        let model = crate::parse_webidl(
            "[LegacyTreatNonObjectAsNull]\n\
             callback ErrorHandler = undefined (DOMString details);\n\
             interface Manager { attribute ErrorHandler handler; };\n",
        )
        .expect("the snippet parses");
        assert_eq!(
            setter_conversion(&model, "Manager", "handler"),
            from_v8(
                &Type::Callback("ErrorHandler".to_string()),
                "scope",
                "raw",
                IntegerMode::Modulo
            )
        );
    }

    /// The annotation is read off the *lowered* callback, so a typedef over the
    /// callback reaches the loose rule exactly as naming it under `?` does. The
    /// HTML Standard's `typedef EventHandlerNonNull? EventHandler;` is the
    /// spelling this pins.
    #[test]
    fn an_annotated_nullable_attribute_reaches_the_loose_rule_through_a_typedef() {
        let direct = handlers();
        let via_typedef = crate::parse_webidl(
            "[LegacyTreatNonObjectAsNull]\n\
             callback ErrorHandler = undefined (DOMString details);\n\
             typedef ErrorHandler? Handler;\n\
             interface Manager { attribute Handler handler; };\n",
        )
        .expect("the snippet parses");

        assert_eq!(
            attribute_type(&via_typedef, "Manager", "handler"),
            Type::Nullable(Box::new(Type::Callback("ErrorHandler".to_string()))),
            "the typedef is substituted, so the attribute's type is the nullable callback"
        );
        assert_eq!(
            setter_conversion(&via_typedef, "Manager", "handler"),
            setter_conversion(&direct, "Manager", "handler2")
        );
    }

    /// A buffer source is converted by its bytes. `Handle<T>` is what the
    /// previous lowering produced, and it is what a native V8 typed array can
    /// never satisfy, so both directions are pinned per type.
    #[test]
    fn a_buffer_source_converts_by_its_bytes() {
        for kind in BufferType::ALL {
            let ty = Type::Buffer(kind);
            let suffix = kind.conversion_suffix();
            assert_eq!(
                from_v8(&ty, "scope", "value", IntegerMode::Modulo),
                format!("{suffix}_from_v8(scope, value)")
            );
            assert_eq!(
                to_v8(&ty, "scope", "value"),
                format!("{suffix}_to_v8(scope, value)")
            );
        }
    }

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
            "record_from_v8(scope, value, byte_string_from_v8, string_from_v8)"
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
                "record_from_v8(scope, value, string_from_v8, string_from_v8)"
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
            "record_from_v8(scope, value, |_scope, _key| Err::<i32, JsError>(JsError::type_error(\"a record key must be a string type\")), string_from_v8)"
        );
        assert_eq!(
            to_v8(&record(Type::Long), "scope", "value"),
            "record_to_v8(scope, value, |_scope, _key| Err::<v8::Local<v8::Value>, JsError>(JsError::type_error(\"a record key must be a string type\")), |scope, item| string_to_v8(scope, &item))"
        );
    }
}
