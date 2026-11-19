//! The native half of `tenta_js::svg_animated`: `SVGAnimatedString`,
//! `SVGElement.prototype.className` and `SVGAElement.prototype.href`, defined
//! when the engine is built, and the script behind them, run when a page
//! first uses one (`super::lazy`).
//!
//! The two accessors are defined on the SVG prototypes, so they shadow what
//! `Element` declares there: on an SVG element `className` is the object, as
//! in a browser, and has no setter.

use tenta_js::bindings::{
    InterfaceMarker, SVGAElement, SVGElement, handle_from_v8, throw_type_error,
    v8_begin_stored_value_lease, v8_end_stored_value_lease, v8_registered_slow_callback_boundary,
};

use super::lazy;
use super::observers::{get, key};
use super::streams::prototype_of;

guarded_callback!(guarded_class_name => class_name);
guarded_callback!(guarded_href => href);

/// The private property of the global that holds the script's exports, or
/// `false` once the script has failed.
const EXPORTS: &str = "tenta.svgAnimated";

/// Defines the names. `SVGElement` and `SVGAElement` are looked up on
/// `global`, so the generated interfaces have to be installed already.
pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    lazy::define_interface(scope, global, "SVGAnimatedString", lazy_interface);
    getter(scope, global, "SVGElement", "className", guarded_class_name);
    getter(scope, global, "SVGAElement", "href", guarded_href);
}

/// Defines `interface.prototype[name]` as an accessor with a getter and no
/// setter, enumerable and configurable as WebIDL's attributes are.
fn getter(
    scope: &mut v8::PinScope,
    global: v8::Local<v8::Object>,
    interface: &str,
    name: &str,
    callback: impl v8::MapFnTo<v8::FunctionCallback>,
) {
    let Some(prototype) = prototype_of(scope, global, interface) else {
        return;
    };
    let Some(function) = v8::Function::builder(callback).length(0).build(scope) else {
        return;
    };
    let label = key(scope, &format!("get {name}"));
    function.set_name(label);
    let undefined = v8::undefined(scope);
    let mut descriptor =
        v8::PropertyDescriptor::new_from_get_set(function.into(), undefined.into());
    descriptor.set_enumerable(true);
    descriptor.set_configurable(true);
    let name = key(scope, name);
    prototype.define_property(scope, name.into(), &descriptor);
}

/// The script's exports, built on first use. `None` in a realm where the
/// script failed, now or earlier.
fn exports<'s>(scope: &mut v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Object>> {
    lazy::exports(
        scope,
        EXPORTS,
        tenta_js::svg_animated::SVG_ANIMATED_PRELUDE_SOURCE,
        |_| Some(Vec::new()),
    )
}

fn lazy_interface<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: v8::Local<'s, v8::Name>,
    _args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'_, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        if let Some(exports) = exports(scope)
            && let Some(interface) = exports.get(scope, name.into())
        {
            rv.set(interface);
        }
    };
    // SAFETY: a registered V8 property callback, where allocation is allowed.
    unsafe { v8_registered_slow_callback_boundary(scope, "lazy_svg_animated_interface", (), call) };
}

/// The object the exported function `name` makes for the receiver, which has
/// to be a `T`.
fn animated<'s, T: InterfaceMarker>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let this: v8::Local<v8::Value> = args.this().into();
    if handle_from_v8::<T>(scope, this).is_err() {
        throw_type_error(scope, "Illegal invocation");
        return;
    }
    let Some(exports) = exports(scope) else {
        return;
    };
    let Some(function) =
        get(scope, exports, name).and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
    else {
        return;
    };
    let undefined = v8::undefined(scope).into();
    if let Some(value) = function.call(scope, undefined, &[this]) {
        rv.set(value);
    }
}

/// The getter of `SVGElement.prototype.className`.
fn class_name<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    animated::<SVGElement>(scope, "className", args, rv);
}

/// The getter of `SVGAElement.prototype.href`.
fn href<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    animated::<SVGAElement>(scope, "href", args, rv);
}
