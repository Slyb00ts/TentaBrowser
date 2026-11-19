//! The native half of `tenta_js::streams`: the names a page can reach a
//! stream through, defined when the engine is built, and the script behind
//! them, run when one of the names is first used.
//!
//! Most pages never touch a stream, and the script is not small, so building
//! it for every page would charge all of them for it (0.41 ms a page when it
//! was measured, against 1.52 ms for the other preludes together). What is
//! defined up front costs a handful of property definitions:
//!
//!  - the five interface objects, as lazy data properties of the global, the
//!    way `customElements` and the observers are;
//!  - `Response.prototype.body`, an accessor whose getter is native;
//!  - `Blob.prototype.stream()`, a native method.
//!
//! Each of them asks for the script's exports — built once, kept in a private
//! property of the global — and hands over to the script. A realm where the
//! script fails has the names and `undefined` behind them.

use tenta_js::bindings::host::{realm_context, response_has_body};
use tenta_js::bindings::{
    Response, handle_from_v8, throw_type_error, v8_begin_stored_value_lease,
    v8_end_stored_value_lease, v8_registered_slow_callback_boundary, world_realm,
};

use super::lazy;
use super::observers::{get, key};

guarded_callback!(guarded_response_has_body => response_has_body_callback);
guarded_callback!(guarded_response_body => response_body);
guarded_callback!(guarded_blob_stream => blob_stream);

/// The private property of the global that holds the script's exports, or
/// `false` once the script has failed.
const EXPORTS: &str = "tenta.streams";

/// The interface objects the script exports under their own names.
const INTERFACES: [&str; 5] = [
    "ReadableStream",
    "ReadableStreamDefaultReader",
    "ReadableStreamDefaultController",
    "CountQueuingStrategy",
    "ByteLengthQueuingStrategy",
];

/// Defines the names. `Response` and `Blob` are looked up on `global`, so the
/// generated interfaces have to be installed already; a realm without one of
/// them goes without that member.
pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    let undefined = v8::undefined(scope);
    for name in INTERFACES {
        lazy::define_interface(scope, global, name, lazy_interface);
    }
    if let Some(prototype) = prototype_of(scope, global, "Response")
        && let Some(getter) = v8::Function::builder(guarded_response_body)
            .length(0)
            .build(scope)
    {
        let name = key(scope, "get body");
        getter.set_name(name);
        let mut descriptor =
            v8::PropertyDescriptor::new_from_get_set(getter.into(), undefined.into());
        descriptor.set_enumerable(true);
        descriptor.set_configurable(true);
        let body = key(scope, "body");
        prototype.define_property(scope, body.into(), &descriptor);
    }
    if let Some(prototype) = prototype_of(scope, global, "Blob")
        && let Some(method) = v8::Function::builder(guarded_blob_stream)
            .length(0)
            .build(scope)
    {
        let name = key(scope, "stream");
        method.set_name(name);
        prototype.set(scope, name.into(), method.into());
    }
}

/// `global[interface].prototype`, when the interface is installed.
pub(super) fn prototype_of<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    global: v8::Local<v8::Object>,
    interface: &str,
) -> Option<v8::Local<'s, v8::Object>> {
    let constructor = v8::Local::<v8::Object>::try_from(get(scope, global, interface)?).ok()?;
    v8::Local::<v8::Object>::try_from(get(scope, constructor, "prototype")?).ok()
}

/// The script's exports, built on first use. `None` in a realm where the
/// script failed, now or earlier.
fn exports<'s>(scope: &mut v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Object>> {
    lazy::exports(
        scope,
        EXPORTS,
        tenta_js::streams::STREAMS_PRELUDE_SOURCE,
        |scope| {
            let has_body = v8::Function::builder(guarded_response_has_body).build(scope)?;
            Some(vec![has_body.into()])
        },
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
    unsafe { v8_registered_slow_callback_boundary(scope, "lazy_stream_interface", (), call) };
}

/// Calls the exported function `name` with `receiver` as its one argument and
/// returns what it returns; what it throws is the caller's exception.
fn call_export<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
    receiver: v8::Local<'s, v8::Object>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(exports) = exports(scope) else {
        return;
    };
    let Some(function) =
        get(scope, exports, name).and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
    else {
        return;
    };
    let undefined = v8::undefined(scope).into();
    if let Some(value) = function.call(scope, undefined, &[receiver.into()]) {
        rv.set(value);
    }
}

/// The getter of `Response.prototype.body`.
fn response_body<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    call_export(scope, "responseBody", args.this(), rv);
}

/// `Blob.prototype.stream()`.
fn blob_stream<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    call_export(scope, "blobStream", args.this(), rv);
}

/// What the script asks before it makes a body stream: whether its argument is
/// a `Response` with a body. Anything that is not a `Response` is an illegal
/// invocation of the getter.
fn response_has_body_callback<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut retval: v8::ReturnValue<'s, v8::Value>,
) {
    let answer = handle_from_v8::<Response>(scope, args.get(0))
        .ok()
        .and_then(|response| {
            let realm = world_realm(scope).ok()?;
            let ctx = realm_context(&realm).ok()?;
            response_has_body(&ctx, response)
        });
    match answer {
        Some(has_body) => retval.set_bool(has_body),
        None => throw_type_error(scope, "Illegal invocation"),
    }
}
