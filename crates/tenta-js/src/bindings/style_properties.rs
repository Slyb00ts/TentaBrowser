//! The per-property attributes of `CSSStyleProperties` (CSSOM, "Extensions to
//! the CSSStyleDeclaration interface").
//!
//! The spec gives the interface one attribute for every supported CSS property
//! in up to three spellings: camel-cased (`backgroundColor`), webkit-cased for
//! `-webkit-` properties (`webkitTransform`) and dashed (`background-color`).
//! The set depends on which properties Stylo supports, so it is read from
//! Stylo at install time instead of being spelled out in the IDL, which would
//! need a generated trait method per attribute. Each attribute is an accessor
//! on the prototype; it forwards to the interface's own `getPropertyValue` and
//! `setProperty`, so the style attribute stays the single source of truth for
//! an inline style and the existing mutation path is the only writer.
//!
//! The prototype is populated when the first `CSSStyleProperties` wrapper of a
//! context is made, not when the interface is installed. Defining about 900
//! accessors through templates costs several milliseconds in every new context
//! (V8) and about two (QuickJS), which pages that never touch `style` would pay
//! for; defining them on a live prototype is cheaper still and is paid once.

use std::sync::OnceLock;

use super::*;

/// A CSS property and every IDL attribute spelling that reaches it.
struct StyleProperty {
    property: &'static str,
    attributes: Vec<String>,
}

/// The properties in a fixed order, so a callback can carry just an index.
fn style_properties() -> &'static [StyleProperty] {
    static PROPERTIES: OnceLock<Vec<StyleProperty>> = OnceLock::new();
    PROPERTIES.get_or_init(|| style_properties_for(&tenta_css::supported_property_names()))
}

fn style_properties_for(properties: &[&'static str]) -> Vec<StyleProperty> {
    properties
        .iter()
        .map(|&property| {
            let mut attributes = Vec::with_capacity(3);
            // `float` is reserved in script; `cssFloat` is declared in the IDL.
            if property != "float" {
                attributes.push(idl_attribute(property, false));
            }
            if property.starts_with("-webkit-") {
                attributes.push(idl_attribute(property, true));
            }
            if property.contains('-') {
                attributes.push(property.to_owned());
            }
            StyleProperty {
                property,
                attributes,
            }
        })
        .collect()
}

/// CSSOM "CSS property to IDL attribute": drop each dash and capitalize the
/// letter after it; with `lowercase_first`, a leading dash is dropped without
/// capitalizing, which turns `-webkit-transform` into `webkitTransform`.
fn idl_attribute(property: &str, lowercase_first: bool) -> String {
    let property = if lowercase_first {
        property.strip_prefix('-').unwrap_or(property)
    } else {
        property
    };
    let mut attribute = String::with_capacity(property.len());
    let mut capitalize = false;
    for character in property.chars() {
        if character == '-' {
            capitalize = true;
        } else if capitalize {
            attribute.extend(character.to_uppercase());
            capitalize = false;
        } else {
            attribute.push(character);
        }
    }
    attribute
}

/// Switches a template-built prototype to dictionary mode.
///
/// Such an object starts in fast mode, where each of the 900 definitions below
/// allocates a new hidden class (about 3 ms in total); in dictionary mode they
/// take about a tenth of that. Deleting a property that is not the newest one
/// is what makes V8 normalize, so two throwaway symbol-keyed properties are
/// added and removed, which is invisible to script.
#[cfg(feature = "v8")]
fn normalize_to_dictionary_mode(
    scope: &mut v8::PinScope<'_, '_>,
    object: v8::Local<'_, v8::Object>,
) {
    let first = v8::Symbol::new(scope, None);
    let second = v8::Symbol::new(scope, None);
    let placeholder = v8::undefined(scope);
    object.set(scope, first.into(), placeholder.into());
    object.set(scope, second.into(), placeholder.into());
    object.delete(scope, first.into());
    object.delete(scope, second.into());
}

/// A host that can tell whether it is a style declaration.
///
/// The accessors are plain functions on the prototype, so V8 does not brand
/// check their receiver the way it does for a template signature.
#[cfg(feature = "v8")]
pub trait StyleDeclarationHost: CSSStyleDeclarationOps {
    fn is_style_declaration(&self) -> bool;
}

/// Defines the attributes on the prototype of `wrapper`, a `CSSStyleProperties`
/// wrapper of the current context, unless an earlier wrapper already did.
#[cfg(feature = "v8")]
pub fn ensure_v8<T: StyleDeclarationHost + 'static>(
    scope: &mut v8::PinScope<'_, '_>,
    wrapper: v8::Local<'_, v8::Object>,
) -> Result<(), JsError> {
    let prototype = wrapper
        .get_prototype(scope)
        .and_then(|prototype| v8::Local::<v8::Object>::try_from(prototype).ok())
        .ok_or_else(|| JsError::type_error("CSSStyleProperties has no prototype"))?;
    // `color` is supported in every configuration, so it marks a populated
    // prototype.
    let marker = v8_string(scope, "color")?;
    if prototype.has_own_property(scope, marker.into()) == Some(true) {
        return Ok(());
    }
    normalize_to_dictionary_mode(scope, prototype);
    for (index, style_property) in style_properties().iter().enumerate() {
        if style_property.attributes.is_empty() {
            continue;
        }
        let index = v8::Integer::new_from_unsigned(scope, index as u32);
        // One accessor pair serves every spelling of a property.
        let getter = v8::Function::builder(get_attribute::<T>)
            .data(index.into())
            .length(0)
            .build(scope)
            .ok_or_else(|| JsError::type_error("could not create a CSS property getter"))?;
        let setter = v8::Function::builder(set_attribute::<T>)
            .data(index.into())
            .length(1)
            .build(scope)
            .ok_or_else(|| JsError::type_error("could not create a CSS property setter"))?;
        for attribute in &style_property.attributes {
            let key = v8_string(scope, attribute)?;
            let mut descriptor =
                v8::PropertyDescriptor::new_from_get_set(getter.into(), setter.into());
            descriptor.set_enumerable(true);
            descriptor.set_configurable(true);
            if prototype.define_property(scope, key.into(), &descriptor) != Some(true) {
                return Err(JsError::type_error(
                    "could not define a CSS property attribute",
                ));
            }
        }
    }
    Ok(())
}

/// The declaration behind `this`, or the `TypeError` WebIDL requires for any
/// other receiver, including the prototype itself.
#[cfg(feature = "v8")]
fn receiver<'a, T: StyleDeclarationHost + 'static>(
    this: &v8::Local<v8::Object>,
) -> Result<&'a T, JsError> {
    // SAFETY: every wrapper in a context carries a host written by
    // `attach_host::<T>` in its first internal field; `unwrap_this` rejects an
    // object without one. The pointer outlives this callback because the
    // receiver is alive for its duration.
    let host = unsafe { unwrap_this::<T>(this) }?;
    if host.is_style_declaration() {
        Ok(host)
    } else {
        Err(JsError::type_error("Illegal invocation"))
    }
}

#[cfg(feature = "v8")]
fn property_of<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
) -> Result<&'static str, JsError> {
    args.data()
        .uint32_value(scope)
        .and_then(|index| style_properties().get(index as usize))
        .map(|style_property| style_property.property)
        .ok_or_else(|| JsError::type_error("unknown CSS property attribute"))
}

#[cfg(feature = "v8")]
fn get_attribute<'s, T: StyleDeclarationHost + 'static>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        body_get_attribute::<T>(scope, args, rv);
    };
    // SAFETY: V8 invokes this registered slow FunctionTemplate callback.
    unsafe { v8_registered_slow_callback_boundary(scope, "css_style_property_getter", (), call) };
}

#[cfg(feature = "v8")]
fn body_get_attribute<'s, T: StyleDeclarationHost + 'static>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let this = args.this();
    let result = receiver::<T>(&this).and_then(|host| {
        let property = property_of(scope, &args)?;
        host.get_property_value(property)
    });
    match result.and_then(|value| string_to_v8(scope, &value)) {
        Ok(value) => rv.set(value),
        Err(error) => throw_js_error(scope, &error),
    }
}

#[cfg(feature = "v8")]
fn set_attribute<'s, T: StyleDeclarationHost + 'static>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        body_set_attribute::<T>(scope, args, rv);
    };
    // SAFETY: V8 invokes this registered slow FunctionTemplate callback.
    unsafe { v8_registered_slow_callback_boundary(scope, "css_style_property_setter", (), call) };
}

#[cfg(feature = "v8")]
fn body_set_attribute<'s, T: StyleDeclarationHost + 'static>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let this = args.this();
    let result = receiver::<T>(&this).and_then(|host| {
        let property = property_of(scope, &args)?;
        // The attributes are `[LegacyNullToEmptyString] DOMString`.
        let raw = args.get(0);
        let value = if raw.is_null() {
            String::new()
        } else {
            string_from_v8(scope, raw)?
        };
        host.set_property(property, &value, None)
    });
    if let Err(error) = result {
        throw_js_error(scope, &error);
    }
}

/// Defines the attributes on `prototype`, a context's `CSSStyleProperties`
/// prototype, unless an earlier wrapper already did.
#[cfg(feature = "quickjs")]
pub fn ensure_quickjs<'js, T: CSSStyleDeclarationOps + 'static>(
    prototype: &rquickjs::Object<'js>,
) -> rquickjs::Result<()> {
    use rquickjs::FromJs;

    // `color` is supported in every configuration, so it marks a populated
    // prototype.
    if prototype.contains_key("color")? {
        return Ok(());
    }
    for style_property in style_properties() {
        let property = style_property.property;
        for attribute in &style_property.attributes {
            let accessor = rquickjs::object::Accessor::new(
                move |ctx: rquickjs::Ctx<'js>,
                      this: rquickjs::function::This<rquickjs::Value<'js>>|
                      -> rquickjs::Result<rquickjs::Value<'js>> {
                    quickjs_callback_boundary(&ctx, "css_style_property_getter", || {
                        let host = quickjs_host::<T>(&ctx, &this.0, "CSSStyleProperties")?;
                        let value = host
                            .get_property_value(property)
                            .map_err(|error| throw_quickjs_error(&ctx, &error))?;
                        rquickjs::IntoJs::into_js(value, &ctx)
                    })
                },
                move |ctx: rquickjs::Ctx<'js>,
                      this: rquickjs::function::This<rquickjs::Value<'js>>,
                      value: rquickjs::Value<'js>|
                      -> rquickjs::Result<()> {
                    quickjs_callback_boundary(&ctx, "css_style_property_setter", || {
                        let host = quickjs_host::<T>(&ctx, &this.0, "CSSStyleProperties")?;
                        // The attributes are `[LegacyNullToEmptyString] DOMString`.
                        let value = if value.is_null() {
                            String::new()
                        } else {
                            rquickjs::Coerced::<rquickjs::String>::from_js(&ctx, value)
                                .and_then(|coerced| coerced.0.to_string_lossy())?
                        };
                        host.set_property(property, &value, None)
                            .map_err(|error| throw_quickjs_error(&ctx, &error))
                    })
                },
            )
            .enumerable()
            .configurable();
            prototype.prop(attribute.as_str(), accessor)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idl_attribute_follows_the_cssom_algorithm() {
        assert_eq!(idl_attribute("color", false), "color");
        assert_eq!(idl_attribute("background-color", false), "backgroundColor");
        assert_eq!(idl_attribute("-webkit-transform", false), "WebkitTransform");
        assert_eq!(idl_attribute("-webkit-transform", true), "webkitTransform");
    }

    #[test]
    fn each_property_lists_its_spellings_without_float() {
        let properties =
            style_properties_for(&["float", "color", "background-color", "-webkit-transform"]);
        let spellings: Vec<(&str, Vec<&str>)> = properties
            .iter()
            .map(|p| {
                (
                    p.property,
                    p.attributes.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            spellings,
            [
                ("float", vec![]),
                ("color", vec!["color"]),
                (
                    "background-color",
                    vec!["backgroundColor", "background-color"]
                ),
                (
                    "-webkit-transform",
                    vec!["WebkitTransform", "webkitTransform", "-webkit-transform"]
                ),
            ]
        );
    }
}
