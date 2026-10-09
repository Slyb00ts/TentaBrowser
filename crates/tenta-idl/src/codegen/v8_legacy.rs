//! V8 interceptors for WebIDL legacy indexed and named properties.

use std::fmt::Write as _;

use super::convert::{self, IntegerMode};
use super::legacy::{LegacyProperties, SpecialOperation};
use super::rust_type::{Position, rust_type};
use crate::model::{Interface, Member, Model, Type};
use crate::naming;

fn expand(template: &str, replacements: &[(&str, &str)]) -> String {
    replacements
        .iter()
        .fold(template.to_owned(), |text, (name, value)| {
            text.replace(name, value)
        })
}

fn append(out: &mut String, template: &str, replacements: &[(&str, &str)]) {
    out.push_str(&expand(template, replacements));
}

pub(super) fn install(model: &Model, interface: &Interface, out: &mut String) {
    let properties = LegacyProperties::resolve(model, interface);
    let snake = naming::snake_stem(&interface.name);
    if properties.indexed_getter.is_some() {
        append(
            out,
            "    instance.set_indexed_property_handler(\n        v8::IndexedPropertyHandlerConfiguration::new()\n            .getter(indexed_get_@S@::<T>)\n            .setter(indexed_set_@S@::<T>)\n            .query(indexed_query_@S@::<T>)\n            .deleter(indexed_delete_@S@::<T>)\n            .enumerator(indexed_enumerate_@S@::<T>)\n            .definer(indexed_define_@S@::<T>)\n            .descriptor(indexed_descriptor_@S@::<T>)\n            .flags(v8::PropertyHandlerFlags::WEB_IDL_LEGACY),\n    );\n",
            &[("@S@", &snake)],
        );
    } else if properties.named_getter.is_some() {
        // V8 dispatches canonical array-index keys only to indexed handlers.
        // A Web IDL named-only interface still treats those keys as names.
        // Enumeration remains on the named handler so one supported-name
        // sequence is emitted, without a second numeric batch.
        append(
            out,
            "    instance.set_indexed_property_handler(\n        v8::IndexedPropertyHandlerConfiguration::new()\n            .getter(indexed_get_@S@::<T>)\n            .setter(indexed_set_@S@::<T>)\n            .query(indexed_query_@S@::<T>)\n            .deleter(indexed_delete_@S@::<T>)\n            .definer(indexed_define_@S@::<T>)\n            .descriptor(indexed_descriptor_@S@::<T>)\n            .flags(v8::PropertyHandlerFlags::WEB_IDL_LEGACY),\n    );\n",
            &[("@S@", &snake)],
        );
    }
    if properties.named_getter.is_some() {
        let flags = if properties.named_setter.is_none() && !properties.override_builtins {
            "v8::PropertyHandlerFlags::ONLY_INTERCEPT_STRINGS | v8::PropertyHandlerFlags::NON_MASKING | v8::PropertyHandlerFlags::WEB_IDL_LEGACY"
        } else {
            "v8::PropertyHandlerFlags::ONLY_INTERCEPT_STRINGS | v8::PropertyHandlerFlags::WEB_IDL_LEGACY"
        };
        append(
            out,
            "    instance.set_named_property_handler(\n        v8::NamedPropertyHandlerConfiguration::new()\n            .getter(named_get_@S@::<T>)\n            .setter(named_set_@S@::<T>)\n            .query(named_query_@S@::<T>)\n            .deleter(named_delete_@S@::<T>)\n            .enumerator(named_enumerate_@S@::<T>)\n            .definer(named_define_@S@::<T>)\n            .descriptor(named_descriptor_@S@::<T>)\n            .flags(@FLAGS@),\n    );\n",
            &[("@S@", &snake), ("@FLAGS@", flags)],
        );
    }
}

pub(super) fn callbacks(model: &Model, interface: &Interface, bound: &str, out: &mut String) {
    let properties = LegacyProperties::resolve(model, interface);
    if properties.indexed_getter.is_none() && properties.named_getter.is_none() {
        return;
    }
    let snake = naming::snake_stem(&interface.name);
    descriptor_helper(&snake, out);
    if let Some(getter) = properties.indexed_getter {
        indexed_callbacks(&snake, bound, getter, properties.indexed_setter, out);
    }
    if let Some(getter) = properties.named_getter {
        let unforgeable = unforgeable_names(model, interface);
        named_callbacks(
            &snake,
            bound,
            &properties,
            getter,
            &unforgeable,
            interface.attrs.has("Global"),
            out,
        );
        if properties.indexed_getter.is_none() {
            named_only_numeric_adapters(&snake, bound, out);
        }
    }
}

fn named_only_numeric_adapters(snake: &str, bound: &str, out: &mut String) {
    for (verb, extra, return_type) in [
        ("get", "", "Value"),
        ("set", "value: v8::Local<'s, v8::Value>,", "Boolean"),
        ("query", "", "Integer"),
        ("delete", "", "Boolean"),
        ("define", "descriptor: &v8::PropertyDescriptor,", "Boolean"),
        ("descriptor", "", "Value"),
    ] {
        let forwarded = match verb {
            "set" => "value, ",
            "define" => "descriptor, ",
            _ => "",
        };
        let body = format!(
            "        let property = match v8_string(scope, &index.to_string()) {{\n            Ok(property) => property,\n            Err(error) => {{\n                legacy_error_{snake}(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }};\n        if native_callback_failure(scope).is_some() {{\n            scope.terminate_execution();\n            return v8::Intercepted::kYes;\n        }}\n        named_{verb}_{snake}::<T>(scope, property.into(), {forwarded}args, rv)"
        );
        indexed_callback(snake, bound, verb, extra, return_type, &body, out);
    }
}

fn unforgeable_names(model: &Model, interface: &Interface) -> Vec<String> {
    model
        .prototype_chain(&interface.name)
        .iter()
        .filter_map(|name| model.interface(name))
        .flat_map(|ancestor| ancestor.members.iter())
        .filter_map(|member| match member {
            Member::Attribute(attribute) if attribute.legacy_unforgeable() => {
                Some(attribute.name.clone())
            }
            _ => None,
        })
        .collect()
}

fn descriptor_helper(snake: &str, out: &mut String) {
    append(
        out,
        r#"
fn legacy_error_@S@(scope: &mut v8::PinScope<'_, '_>, error: &JsError) {
    if native_callback_failure(scope).is_some() {
        scope.terminate_execution();
    } else {
        throw_js_error(scope, error);
    }
}

fn legacy_descriptor_@S@<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    writable: bool,
    enumerable: bool,
) -> Result<v8::Local<'s, v8::Object>, JsError> {
    let descriptor = v8::Object::new(scope);
    let fields = [
        ("value", value),
        ("writable", v8::Boolean::new(scope, writable).into()),
        ("enumerable", v8::Boolean::new(scope, enumerable).into()),
        ("configurable", v8::Boolean::new(scope, true).into()),
    ];
    for (name, value) in fields {
        let key = v8_string(scope, name)?;
        if descriptor.create_data_property(scope, key.into(), value) != Some(true) {
            return Err(JsError::type_error("could not create a WebIDL property descriptor"));
        }
    }
    Ok(descriptor)
}
"#,
        &[("@S@", snake)],
    );
}

fn call(operation: SpecialOperation<'_>, args: &str) -> String {
    let invocation = format!(
        "<T as {}Ops>::{}(host, {args})",
        operation.interface, operation.operation.rust_name
    );
    if operation.operation.fast.is_eligible() {
        format!("Ok::<_, JsError>({invocation})")
    } else {
        invocation
    }
}

fn getter_value_helper(
    snake: &str,
    bound: &str,
    operation: SpecialOperation<'_>,
    indexed: bool,
    out: &mut String,
) {
    let kind = if indexed { "index" } else { "name" };
    let key_type = if indexed { "u32" } else { "&str" };
    let key = if indexed { "index" } else { "name" };
    let invocation = call(operation, key);
    let owned = rust_type(&operation.operation.return_type, Position::Owned);
    let conversion = convert::to_v8(&operation.operation.return_type, "scope", "value");
    let _ = writeln!(
        out,
        "\nfn legacy_{kind}_value_{snake}<'s, T: {bound} + 'static>(\n    scope: &mut v8::PinScope<'s, '_>,\n    host: &T,\n    {key}: {key_type},\n) -> Result<v8::Local<'s, v8::Value>, JsError> {{\n    let value: {owned} = {invocation}?;\n    {conversion}\n}}"
    );
}

fn setter_call(operation: SpecialOperation<'_>, key: &str, value: &str) -> (String, String) {
    let argument = &operation.operation.arguments[1];
    let conversion = convert::legacy_null_to_empty(
        &argument.ty,
        &argument.attrs,
        value,
        convert::from_v8(&argument.ty, "scope", value, IntegerMode::of(argument)),
    );
    let passed = match &argument.ty {
        Type::DomString | Type::UsvString => "&converted".to_owned(),
        Type::ByteString => "&converted".to_owned(),
        Type::Nullable(inner) if matches!(&**inner, Type::DomString | Type::UsvString) => {
            "converted.as_deref()".to_owned()
        }
        _ => "converted".to_owned(),
    };
    (conversion, call(operation, &format!("{key}, {passed}")))
}

fn indexed_callbacks(
    snake: &str,
    bound: &str,
    getter: SpecialOperation<'_>,
    setter: Option<SpecialOperation<'_>>,
    out: &mut String,
) {
    getter_value_helper(snake, bound, getter, true, out);
    let declaring = getter.interface;
    append(
        out,
        r#"
fn legacy_index_supported_@S@<T: @B@ + 'static>(host: &T, index: u32) -> Result<bool, JsError> {
    <T as @D@Ops>::is_supported_property_index(host, index)
}
"#,
        &[("@S@", snake), ("@B@", bound), ("@D@", declaring)],
    );
    indexed_get_query_descriptor(snake, bound, setter.is_some(), out);
    indexed_set_define(snake, bound, setter, out);
    indexed_delete_enumerate(snake, bound, getter, out);
}

fn named_callbacks(
    snake: &str,
    bound: &str,
    properties: &LegacyProperties<'_>,
    getter: SpecialOperation<'_>,
    unforgeable: &[String],
    is_global: bool,
    out: &mut String,
) {
    getter_value_helper(snake, bound, getter, false, out);
    let declaring = getter.interface;
    let unforgeable_match = if unforgeable.is_empty() {
        "false".to_owned()
    } else {
        format!(
            "matches!(name, {})",
            unforgeable
                .iter()
                .map(|name| format!("{name:?}"))
                .collect::<Vec<_>>()
                .join(" | ")
        )
    };
    append(
        out,
        r#"
fn legacy_name_supported_@S@<T: @B@ + 'static>(host: &T, name: &str) -> Result<bool, JsError> {
    <T as @D@Ops>::is_supported_property_name(host, name)
}

fn legacy_name_visible_@S@<T: @B@ + 'static>(
    scope: &mut v8::PinScope<'_, '_>,
    holder: v8::Local<v8::Object>,
    property: v8::Local<v8::Name>,
    host: &T,
    name: &str,
) -> Result<bool, Option<JsError>> {
    if @UNFORGEABLE@ || !legacy_name_supported_@S@(host, name).map_err(Some)? {
        return Ok(false);
    }
    // V8's HasRealNamedProperty is own-only. WebIDL checks own properties
    // first even for [LegacyOverrideBuiltIns], then walks the prototypes only
    // when that attribute is absent.
    match holder.has_real_named_property(scope, property) {
        Some(true) => return Ok(false),
        Some(false) => {}
        None => return Err(None),
    }
    if native_callback_failure(scope).is_some() {
        scope.terminate_execution();
        return Err(None);
    }
    if @OVERRIDE@ {
        return Ok(true);
    }
    let mut seen = vec![holder];
    let mut current = Some(holder.get_prototype(scope).ok_or(None)?);
    while let Some(prototype) = current {
        if native_callback_failure(scope).is_some() {
            scope.terminate_execution();
            return Err(None);
        }
        let Ok(prototype) = v8::Local::<v8::Object>::try_from(prototype) else {
            break;
        };
        if seen.iter().any(|prior: &v8::Local<v8::Object>| prior.strict_equals(prototype.into())) {
            return Err(Some(JsError::type_error("cyclic prototype chain")));
        }
        seen.push(prototype);
        // This is an actual [[GetOwnProperty]] check, including Proxy traps.
        // A returned None leaves the original JS exception pending.
        match prototype.has_own_property(scope, property) {
            Some(true) => return Ok(false),
            Some(false) => {}
            None => return Err(None),
        }
        if native_callback_failure(scope).is_some() {
            scope.terminate_execution();
            return Err(None);
        }
        current = Some(prototype.get_prototype(scope).ok_or(None)?);
    }
    Ok(true)
}
"#,
        &[
            ("@S@", snake),
            ("@B@", bound),
            ("@D@", declaring),
            ("@UNFORGEABLE@", &unforgeable_match),
            (
                "@OVERRIDE@",
                if properties.override_builtins {
                    "true"
                } else {
                    "false"
                },
            ),
        ],
    );
    named_get_query_descriptor(
        snake,
        bound,
        properties.named_setter.is_some(),
        properties.unenumerable_names,
        out,
    );
    named_set_define(
        snake,
        bound,
        properties.named_setter,
        properties.override_builtins,
        &unforgeable_match,
        is_global,
        out,
    );
    named_delete_enumerate(snake, bound, getter, properties.named_deleter, out);
}

// The callback families below use the same V8 boundary for every native entry.
// A callback error sets a JS exception; a Rust unwind poisons the isolate and
// terminates execution, never reporting a catchable fallback after mutation.

fn indexed_callback(
    snake: &str,
    bound: &str,
    verb: &str,
    extra: &str,
    return_type: &str,
    body: &str,
    out: &mut String,
) {
    let function = format!("indexed_{verb}_{snake}");
    let body = body.replace(
        "throw_js_error(scope, &error)",
        &format!("legacy_error_{snake}(scope, &error)"),
    );
    let rv_binding = if body.contains("rv.set") {
        "mut rv"
    } else {
        "rv"
    };
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!("    {extra}\n")
    };
    append(
        out,
        r#"
fn @FUNCTION@<'s, T: @BOUND@ + 'static>(
    scope: &mut v8::PinScope<'s, '_>,
    index: u32,
@EXTRA@    args: v8::PropertyCallbackArguments<'s>,
    @RV@: v8::ReturnValue<v8::@RETURN@>,
) -> v8::Intercepted {
    let call = |scope: &mut v8::PinScope<'s, '_>| -> v8::Intercepted {
@BODY@
    };
    // SAFETY: V8 invokes this registered slow property callback.
    unsafe { v8_registered_slow_callback_boundary(scope, "@FUNCTION@", v8::Intercepted::kYes, call) }
}
"#,
        &[
            ("@FUNCTION@", &function),
            ("@BOUND@", bound),
            ("@EXTRA@", &extra),
            ("@RETURN@", return_type),
            ("@RV@", rv_binding),
            ("@BODY@", &body),
        ],
    );
}

fn named_callback(
    snake: &str,
    bound: &str,
    verb: &str,
    extra: &str,
    return_type: &str,
    body: &str,
    out: &mut String,
) {
    let function = format!("named_{verb}_{snake}");
    let body = body.replace(
        "throw_js_error(scope, &error)",
        &format!("legacy_error_{snake}(scope, &error)"),
    );
    let rv_binding = if body.contains("rv.set") {
        "mut rv"
    } else {
        "rv"
    };
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!("    {extra}\n")
    };
    append(
        out,
        r#"
fn @FUNCTION@<'s, T: @BOUND@ + 'static>(
    scope: &mut v8::PinScope<'s, '_>,
    property: v8::Local<'s, v8::Name>,
@EXTRA@    args: v8::PropertyCallbackArguments<'s>,
    @RV@: v8::ReturnValue<v8::@RETURN@>,
) -> v8::Intercepted {
    let call = |scope: &mut v8::PinScope<'s, '_>| -> v8::Intercepted {
@BODY@
    };
    // SAFETY: V8 invokes this registered slow property callback.
    unsafe { v8_registered_slow_callback_boundary(scope, "@FUNCTION@", v8::Intercepted::kYes, call) }
}
"#,
        &[
            ("@FUNCTION@", &function),
            ("@BOUND@", bound),
            ("@EXTRA@", &extra),
            ("@RETURN@", return_type),
            ("@RV@", rv_binding),
            ("@BODY@", &body),
        ],
    );
}

const INDEX_HOST: &str = r#"        let Some(host) = (unsafe { unwrap_receiver::<T>(args.holder()) }) else {
            return v8::Intercepted::kNo;
        };"#;

const INDEX_SUPPORT: &str = r#"        let supported = match legacy_index_supported_@S@(host, index) {
            Ok(supported) => supported,
            Err(error) => {
                throw_js_error(scope, &error);
                return v8::Intercepted::kYes;
            }
        };"#;

const NAME_HOST: &str = r#"        let Ok(property_string) = v8::Local::<v8::String>::try_from(property) else {
            return v8::Intercepted::kNo;
        };
        let name = property_string.to_rust_string_lossy(scope);
        let holder = args.holder();
        let Some(host) = (unsafe { unwrap_receiver::<T>(holder) }) else {
            return v8::Intercepted::kNo;
        };
        let _ = holder;"#;

const NAME_VISIBLE: &str = r#"        let visible = match legacy_name_visible_@S@(scope, holder, property, host, &name) {
            Ok(visible) => visible,
            Err(Some(error)) => {
                throw_js_error(scope, &error);
                return v8::Intercepted::kYes;
            }
            Err(None) => return v8::Intercepted::kYes,
        };"#;

fn indexed_get_query_descriptor(snake: &str, bound: &str, writable: bool, out: &mut String) {
    let support = expand(INDEX_SUPPORT, &[("@S@", snake)]);
    let body = format!(
        "{INDEX_HOST}\n{support}\n        if !supported {{\n            return v8::Intercepted::kNo;\n        }}\n        let value = match legacy_index_value_{snake}(scope, host, index) {{\n            Ok(value) => value,\n            Err(error) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }};\n        rv.set(value);\n        v8::Intercepted::kYes"
    );
    indexed_callback(snake, bound, "get", "", "Value", &body, out);

    let attr = if writable {
        "v8::PropertyAttribute::NONE"
    } else {
        "v8::PropertyAttribute::READ_ONLY"
    };
    let body = format!(
        "{INDEX_HOST}\n{support}\n        if !supported {{\n            return v8::Intercepted::kNo;\n        }}\n        rv.set_int32({attr}.as_u32() as i32);\n        v8::Intercepted::kYes"
    );
    indexed_callback(snake, bound, "query", "", "Integer", &body, out);

    let body = format!(
        "{INDEX_HOST}\n{support}\n        if !supported {{\n            return v8::Intercepted::kNo;\n        }}\n        let value = match legacy_index_value_{snake}(scope, host, index) {{\n            Ok(value) => value,\n            Err(error) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }};\n        let descriptor = match legacy_descriptor_{snake}(scope, value, {writable}, true) {{\n            Ok(descriptor) => descriptor,\n            Err(error) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }};\n        rv.set(descriptor.into());\n        v8::Intercepted::kYes"
    );
    indexed_callback(snake, bound, "descriptor", "", "Value", &body, out);
}

fn named_get_query_descriptor(
    snake: &str,
    bound: &str,
    writable: bool,
    unenumerable: bool,
    out: &mut String,
) {
    let visible = expand(NAME_VISIBLE, &[("@S@", snake)]);
    let body = format!(
        "{NAME_HOST}\n{visible}\n        if !visible {{\n            return v8::Intercepted::kNo;\n        }}\n        let value = match legacy_name_value_{snake}(scope, host, &name) {{\n            Ok(value) => value,\n            Err(error) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }};\n        rv.set(value);\n        v8::Intercepted::kYes"
    );
    named_callback(snake, bound, "get", "", "Value", &body, out);

    let attr = match (writable, unenumerable) {
        (true, true) => "v8::PropertyAttribute::DONT_ENUM",
        (true, false) => "v8::PropertyAttribute::NONE",
        (false, true) => "v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_ENUM",
        (false, false) => "v8::PropertyAttribute::READ_ONLY",
    };
    let body = format!(
        "{NAME_HOST}\n{visible}\n        if !visible {{\n            return v8::Intercepted::kNo;\n        }}\n        rv.set_int32(({attr}).as_u32() as i32);\n        v8::Intercepted::kYes"
    );
    named_callback(snake, bound, "query", "", "Integer", &body, out);

    let body = format!(
        "{NAME_HOST}\n{visible}\n        if !visible {{\n            return v8::Intercepted::kNo;\n        }}\n        let value = match legacy_name_value_{snake}(scope, host, &name) {{\n            Ok(value) => value,\n            Err(error) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }};\n        let descriptor = match legacy_descriptor_{snake}(scope, value, {writable}, {}) {{\n            Ok(descriptor) => descriptor,\n            Err(error) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }};\n        rv.set(descriptor.into());\n        v8::Intercepted::kYes",
        !unenumerable
    );
    named_callback(snake, bound, "descriptor", "", "Value", &body, out);
}

fn indexed_set_define(
    snake: &str,
    bound: &str,
    setter: Option<SpecialOperation<'_>>,
    out: &mut String,
) {
    let Some(operation) = setter else {
        // With no indexed setter, WebIDL still runs OrdinarySetWithOwnDescriptor.
        // V8 bypasses the indexed definer when an interceptor setter declines.
        // Delegate the prototype's full [[Set]] (including Proxy traps) with
        // the original receiver, or reject the unsupported own index.
        append(
            out,
            r#"
fn legacy_index_prototype_set_@S@(
    scope: &mut v8::PinScope<'_, '_>,
    holder: v8::Local<v8::Object>,
    index: u32,
    value: v8::Local<v8::Value>,
) -> Result<bool, Option<JsError>> {
    let property = v8_string(scope, &index.to_string()).map_err(Some)?;
    if native_callback_failure(scope).is_some() {
        scope.terminate_execution();
        return Err(None);
    }
    let prototype = holder.get_prototype(scope).ok_or(None)?;
    if native_callback_failure(scope).is_some() {
        scope.terminate_execution();
        return Err(None);
    }
    let Ok(prototype) = v8::Local::<v8::Object>::try_from(prototype) else {
        return Ok(false);
    };
    let result = legacy_reflect_set(scope, prototype, property.into(), value, holder).ok_or(None)?;
    if native_callback_failure(scope).is_some() {
        scope.terminate_execution();
        return Err(None);
    }
    Ok(result)
}
"#,
            &[("@S@", snake)],
        );
        let support = expand(INDEX_SUPPORT, &[("@S@", snake)]);
        let body = format!(
            "{INDEX_HOST}\n{support}\n        if supported {{\n            rv.set_bool(false);\n            return v8::Intercepted::kYes;\n        }}\n        let succeeded = match legacy_index_prototype_set_{snake}(scope, args.holder(), index, value) {{\n            Ok(succeeded) => succeeded,\n            Err(Some(error)) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n            Err(None) => return v8::Intercepted::kYes,\n        }};\n        rv.set_bool(succeeded);\n        v8::Intercepted::kYes"
        );
        indexed_callback(
            snake,
            bound,
            "set",
            "value: v8::Local<'s, v8::Value>,",
            "Boolean",
            &body,
            out,
        );
        let body = "        let _ = (scope, index, descriptor, args);\n        rv.set_bool(false);\n        v8::Intercepted::kYes";
        indexed_callback(
            snake,
            bound,
            "define",
            "descriptor: &v8::PropertyDescriptor,",
            "Boolean",
            body,
            out,
        );
        return;
    };
    let (conversion, invocation) = setter_call(operation, "index", "value");
    let body = format!(
        "{INDEX_HOST}\n        let converted = match {conversion} {{\n            Ok(converted) => converted,\n            Err(error) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }};\n        match {invocation} {{\n            Ok(_) => rv.set_bool(true),\n            Err(error) => throw_js_error(scope, &error),\n        }}\n        v8::Intercepted::kYes"
    );
    indexed_callback(
        snake,
        bound,
        "set",
        "value: v8::Local<'s, v8::Value>,",
        "Boolean",
        &body,
        out,
    );
    let body = format!(
        "{INDEX_HOST}\n        if descriptor.has_get() || descriptor.has_set() || (!descriptor.has_value() && !descriptor.has_writable()) {{\n            rv.set_bool(false);\n            return v8::Intercepted::kYes;\n        }}\n        let value = if descriptor.has_value() {{ descriptor.value() }} else {{ v8::undefined(scope).into() }};\n        let converted = match {conversion} {{\n            Ok(converted) => converted,\n            Err(error) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }};\n        match {invocation} {{\n            Ok(_) => rv.set_bool(true),\n            Err(error) => throw_js_error(scope, &error),\n        }}\n        v8::Intercepted::kYes"
    );
    indexed_callback(
        snake,
        bound,
        "define",
        "descriptor: &v8::PropertyDescriptor,",
        "Boolean",
        &body,
        out,
    );
}

fn indexed_delete_enumerate(
    snake: &str,
    bound: &str,
    getter: SpecialOperation<'_>,
    out: &mut String,
) {
    let support = expand(INDEX_SUPPORT, &[("@S@", snake)]);
    let body = format!(
        "{INDEX_HOST}\n{support}\n        rv.set_bool(!supported);\n        v8::Intercepted::kYes"
    );
    indexed_callback(snake, bound, "delete", "", "Boolean", &body, out);
    append(
        out,
        r#"
fn indexed_enumerate_@S@<'s, T: @B@ + 'static>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<v8::Array>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        let Some(host) = (unsafe { unwrap_receiver::<T>(args.holder()) }) else {
            return;
        };
        let mut indices = match <T as @D@Ops>::supported_property_indices(host) {
            Ok(indices) => indices,
            Err(error) => {
                legacy_error_@S@(scope, &error);
                return;
            }
        };
        indices.sort_unstable();
        indices.dedup();
        let keys: Vec<v8::Local<v8::Value>> = indices
            .into_iter()
            .map(|index| v8::Integer::new_from_unsigned(scope, index).into())
            .collect();
        rv.set(v8::Array::new_with_elements(scope, &keys));
    };
    // SAFETY: V8 invokes this registered slow property callback.
    unsafe { v8_registered_slow_callback_boundary(scope, "indexed_enumerate_@S@", (), call) };
}
"#,
        &[("@S@", snake), ("@B@", bound), ("@D@", getter.interface)],
    );
}

fn named_set_define(
    snake: &str,
    bound: &str,
    setter: Option<SpecialOperation<'_>>,
    override_builtins: bool,
    unforgeable_match: &str,
    is_global: bool,
    out: &mut String,
) {
    let existing_own = if override_builtins {
        ""
    } else {
        r#"        // A real own property keeps precedence if its name later becomes supported.
        match holder.has_real_named_property(scope, property) {
            Some(true) => return v8::Intercepted::kNo,
            Some(false) => {}
            None => return v8::Intercepted::kYes,
        }
"#
    };
    let define_guard = if unforgeable_match == "false" {
        String::new()
    } else {
        format!(
            "        if {unforgeable_match} {{\n            return v8::Intercepted::kNo;\n        }}\n"
        )
    };
    if let Some(operation) = setter {
        let (conversion, invocation) = setter_call(operation, "&name", "value");
        let body = format!(
            "{NAME_HOST}\n        let converted = match {conversion} {{\n            Ok(converted) => converted,\n            Err(error) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }};\n        match {invocation} {{\n            Ok(_) => rv.set_bool(true),\n            Err(error) => throw_js_error(scope, &error),\n        }}\n        v8::Intercepted::kYes"
        );
        named_callback(
            snake,
            bound,
            "set",
            "value: v8::Local<'s, v8::Value>,",
            "Boolean",
            &body,
            out,
        );
        let body = if is_global {
            "        let _ = (property, descriptor, args, rv);\n        v8::Intercepted::kNo"
                .to_owned()
        } else {
            format!(
                "{NAME_HOST}\n{define_guard}{existing_own}        if descriptor.has_get() || descriptor.has_set() || (!descriptor.has_value() && !descriptor.has_writable()) {{\n            rv.set_bool(false);\n            return v8::Intercepted::kYes;\n        }}\n        let value = if descriptor.has_value() {{ descriptor.value() }} else {{ v8::undefined(scope).into() }};\n        let converted = match {conversion} {{\n            Ok(converted) => converted,\n            Err(error) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }};\n        match {invocation} {{\n            Ok(_) => rv.set_bool(true),\n            Err(error) => throw_js_error(scope, &error),\n        }}\n        v8::Intercepted::kYes"
            )
        };
        named_callback(
            snake,
            bound,
            "define",
            "descriptor: &v8::PropertyDescriptor,",
            "Boolean",
            &body,
            out,
        );
    } else {
        let support = expand(
            r#"        let supported = match legacy_name_supported_@S@(host, &name) {
            Ok(supported) => supported,
            Err(error) => {
                throw_js_error(scope, &error);
                return v8::Intercepted::kYes;
            }
        };"#,
            &[("@S@", snake)],
        );
        // Without a named setter, [[Set]] ignores virtual named properties
        // and runs OrdinarySetWithOwnDescriptor. That path can invoke an
        // inherited accessor before [[DefineOwnProperty]] is considered.
        let body =
            "        let _ = (scope, property, value, args, rv);\n        v8::Intercepted::kNo";
        named_callback(
            snake,
            bound,
            "set",
            "value: v8::Local<'s, v8::Value>,",
            "Boolean",
            body,
            out,
        );
        let body = if is_global {
            "        let _ = (property, descriptor, args, rv);\n        v8::Intercepted::kNo"
                .to_owned()
        } else {
            format!(
                "{NAME_HOST}\n{define_guard}{existing_own}{support}\n        if !supported {{\n            return v8::Intercepted::kNo;\n        }}\n        let _ = descriptor;\n        rv.set_bool(false);\n        v8::Intercepted::kYes"
            )
        };
        named_callback(
            snake,
            bound,
            "define",
            "descriptor: &v8::PropertyDescriptor,",
            "Boolean",
            &body,
            out,
        );
    }
}

fn named_delete_enumerate(
    snake: &str,
    bound: &str,
    getter: SpecialOperation<'_>,
    deleter: Option<SpecialOperation<'_>>,
    out: &mut String,
) {
    let visible = expand(NAME_VISIBLE, &[("@S@", snake)]);
    let body = if let Some(operation) = deleter {
        let invocation = call(operation, "&name");
        let success = if operation.operation.return_type == Type::Boolean {
            "Ok(deleted) => rv.set_bool(deleted),"
        } else {
            "Ok(_) => rv.set_bool(true),"
        };
        format!(
            "{NAME_HOST}\n{visible}\n        if !visible {{\n            return v8::Intercepted::kNo;\n        }}\n        match {invocation} {{\n            {success}\n            Err(error) => {{\n                throw_js_error(scope, &error);\n                return v8::Intercepted::kYes;\n            }}\n        }}\n        v8::Intercepted::kYes"
        )
    } else {
        format!(
            "{NAME_HOST}\n{visible}\n        if !visible {{\n            return v8::Intercepted::kNo;\n        }}\n        rv.set_bool(false);\n        v8::Intercepted::kYes"
        )
    };
    named_callback(snake, bound, "delete", "", "Boolean", &body, out);
    append(
        out,
        r#"
fn named_enumerate_@S@<'s, T: @B@ + 'static>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<v8::Array>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        let holder = args.holder();
        let Some(host) = (unsafe { unwrap_receiver::<T>(holder) }) else {
            return;
        };
        let names = match <T as @D@Ops>::supported_property_names(host) {
            Ok(names) => names,
            Err(error) => {
                legacy_error_@S@(scope, &error);
                return;
            }
        };
        let mut keys = Vec::with_capacity(names.len());
        for name in names {
            let property = match v8_string(scope, &name) {
                Ok(property) => property,
                Err(error) => {
                    legacy_error_@S@(scope, &error);
                    return;
                }
            };
            let visible = match legacy_name_visible_@S@(scope, holder, property.into(), host, &name) {
                Ok(visible) => visible,
                Err(Some(error)) => {
                    legacy_error_@S@(scope, &error);
                    return;
                }
                Err(None) => return,
            };
            if visible {
                keys.push(property.into());
            }
        }
        rv.set(v8::Array::new_with_elements(scope, &keys));
    };
    // SAFETY: V8 invokes this registered slow property callback.
    unsafe { v8_registered_slow_callback_boundary(scope, "named_enumerate_@S@", (), call) };
}
"#,
        &[("@S@", snake), ("@B@", bound), ("@D@", getter.interface)],
    );
}
