//! The native half of `tenta_js::subtle`: `SubtleCrypto` and `crypto.subtle`,
//! defined when the engine is built; the script behind them, run when a page
//! first uses one; and the hashing `digest()` asks the host for.
//!
//! As with streams (`super::streams`), only the names cost anything up front:
//! one lazy data property and one accessor. The script's exports are kept in
//! a private property of the global; a realm where the script fails has the
//! names and `undefined` behind them.

use tenta_core::NativeMemoryComponent;
use tenta_js::bindings::{
    Crypto, JsError, array_buffer_to_v8, buffer_source_from_v8, handle_from_v8,
    pending_js_exception_error, string_from_v8, throw_js_error, throw_type_error,
    v8_begin_stored_value_lease, v8_check_native_buffer_work, v8_end_stored_value_lease,
    v8_registered_slow_callback_boundary,
};
use tenta_net::digest::{Digest, DigestAlgorithm};

use super::observers::{get, get_private, key, set_private};
use super::streams::prototype_of;
use super::{guarded_queue_task_function, host_state};

guarded_callback!(guarded_subtle => subtle);
guarded_callback!(guarded_digest_bytes => digest_bytes);

/// The private property of the global that holds the script's exports, or
/// `false` once the script has failed.
const EXPORTS: &str = "tenta.subtle";

/// How much of the input is hashed between two looks at the page's deadline.
const DIGEST_PIECE: usize = 1 << 20;

/// Defines the names. `Crypto` is looked up on `global`, so the generated
/// interfaces have to be installed already.
pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    let undefined = v8::undefined(scope);
    let name = key(scope, "SubtleCrypto");
    global.set_lazy_data_property_with_data(
        scope,
        name.into(),
        lazy_interface,
        undefined.into(),
        v8::PropertyAttribute::DONT_ENUM,
        v8::SideEffectType::HasSideEffect,
        v8::SideEffectType::HasSideEffect,
    );
    if let Some(prototype) = prototype_of(scope, global, "Crypto")
        && let Some(getter) = v8::Function::builder(guarded_subtle).length(0).build(scope)
    {
        let name = key(scope, "get subtle");
        getter.set_name(name);
        let mut descriptor =
            v8::PropertyDescriptor::new_from_get_set(getter.into(), undefined.into());
        descriptor.set_enumerable(true);
        descriptor.set_configurable(true);
        let subtle = key(scope, "subtle");
        prototype.define_property(scope, subtle.into(), &descriptor);
    }
}

/// The script's exports, built on first use. `None` in a realm where the
/// script failed, now or earlier.
fn exports<'s>(scope: &mut v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Object>> {
    let global = scope.get_current_context().global(scope);
    if let Some(known) = get_private(scope, global, EXPORTS) {
        return v8::Local::<v8::Object>::try_from(known).ok();
    }
    let built = {
        v8::tc_scope!(let tc, scope);
        let built = v8::String::new(tc, tenta_js::subtle::SUBTLE_PRELUDE_SOURCE)
            .and_then(|source| v8::Script::compile(tc, source, None))
            .and_then(|script| script.run(tc))
            .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
            .and_then(|build| {
                let digest = v8::Function::builder(guarded_digest_bytes).build(tc)?;
                let queue = v8::Function::builder(guarded_queue_task_function).build(tc)?;
                let receiver = v8::undefined(tc).into();
                build.call(tc, receiver, &[digest.into(), queue.into()])
            })
            .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
            .map(|object| v8::Global::new(tc, object));
        // A termination is the page's to see; anything else the script threw
        // is this realm going without `subtle`.
        if !tc.has_terminated() {
            tc.reset();
        }
        built
    };
    let built = built.map(|object| v8::Local::new(scope, object));
    let stored: v8::Local<v8::Value> = match built {
        Some(object) => object.into(),
        None => v8::Boolean::new(scope, false).into(),
    };
    set_private(scope, global, EXPORTS, stored);
    built
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
    unsafe { v8_registered_slow_callback_boundary(scope, "lazy_subtle_interface", (), call) };
}

/// The getter of `Crypto.prototype.subtle`: the realm's one `SubtleCrypto`.
fn subtle<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    if handle_from_v8::<Crypto>(scope, args.this().into()).is_err() {
        throw_type_error(scope, "Illegal invocation");
        return;
    }
    let Some(exports) = exports(scope) else {
        return;
    };
    let Some(function) = get(scope, exports, "subtle")
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
    else {
        return;
    };
    let undefined = v8::undefined(scope).into();
    if let Some(value) = function.call(scope, undefined, &[]) {
        rv.set(value);
    }
}

/// What the script's `digest()` asks: `(algorithm, data)` to an `ArrayBuffer`
/// holding the digest. Whatever it throws is what the promise is rejected
/// with.
fn digest_bytes<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    match digest_of(scope, args.get(0), args.get(1)) {
        Ok(buffer) => rv.set(buffer),
        Err(error) => throw_js_error(scope, &error),
    }
}

/// The standard's steps for `digest()` up to the digest itself
/// (<https://w3c.github.io/webcrypto/#SubtleCrypto-method-digest>), in its
/// order, because script can watch it: `algorithm` is converted as
/// `(object or DOMString)`, then `data` as a `BufferSource` and copied, and
/// only then is the algorithm normalized, which reads the object's `name`. A
/// `name` getter that rewrites the data therefore changes nothing that is
/// hashed.
fn digest_of<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    algorithm: v8::Local<'s, v8::Value>,
    data: v8::Local<'s, v8::Value>,
) -> Result<v8::Local<'s, v8::Value>, JsError> {
    let named = match v8::Local::<v8::Object>::try_from(algorithm) {
        Ok(object) => Err(object),
        Err(_) => Ok(string_from_v8(scope, algorithm)?),
    };

    let length = if let Ok(buffer) = v8::Local::<v8::ArrayBuffer>::try_from(data) {
        buffer.byte_length()
    } else if let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(data) {
        view.byte_length()
    } else {
        return Err(JsError::type_error(
            "Failed to execute 'digest' on 'SubtleCrypto': parameter 2 is not of type \
             'BufferSource'.",
        ));
    };
    // The copy is the page's native memory for as long as it is hashed. Data
    // that does not fit is this one call's failure, not the page's.
    let state = host_state(scope)
        .ok_or_else(|| JsError::type_error("the realm has no host to hash with"))?;
    let mut held = state.native_memory.begin(NativeMemoryComponent::WebCrypto);
    if !held.try_charge_optional(length) {
        return Err(JsError::dom_exception(
            "OperationError",
            "the data is larger than this page's memory budget allows to hash",
        ));
    }
    let bytes = buffer_source_from_v8(scope, data)?;

    let name = match named {
        Ok(name) => name,
        Err(object) => {
            let member = key(scope, "name");
            let value = object
                .get(scope, member.into())
                .ok_or_else(pending_js_exception_error)?;
            if value.is_undefined() {
                return Err(JsError::type_error(
                    "Failed to execute 'digest' on 'SubtleCrypto': Algorithm: name: Missing or \
                     not a string",
                ));
            }
            string_from_v8(scope, value)?
        }
    };
    let Some(algorithm) = DigestAlgorithm::from_name(&name) else {
        return Err(JsError::dom_exception(
            "NotSupportedError",
            "Failed to execute 'digest' on 'SubtleCrypto': Algorithm: Unrecognized name",
        ));
    };

    let failed = |_| JsError::dom_exception("OperationError", "the digest could not be computed");
    let mut digest = Digest::new(algorithm).map_err(failed)?;
    for piece in bytes.chunks(DIGEST_PIECE) {
        // A page that hashes a large buffer is still a page with a deadline.
        v8_check_native_buffer_work(scope)?;
        digest.update(piece).map_err(failed)?;
    }
    let digest = digest.finish().map_err(failed)?;
    array_buffer_to_v8(scope, digest)
}
