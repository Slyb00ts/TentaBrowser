//! The native half of `tenta_js::selection`: `Selection`,
//! `window.getSelection()` and `Document.prototype.getSelection()`, defined
//! when the engine is built, and the script behind them, run when a page
//! first uses one.
//!
//! As with streams (`super::streams`), only the names cost anything up front:
//! one lazy data property and two native methods. The script's exports are
//! kept in a private property of the global; a realm where the script fails
//! has the names, `undefined` behind `Selection` and `null` from
//! `getSelection()`.

use tenta_js::bindings::{
    v8_begin_stored_value_lease, v8_end_stored_value_lease, v8_registered_slow_callback_boundary,
};

use super::observers::{get, get_private, key, set_private};
use super::{define, guarded_queue_task_function};

guarded_callback!(guarded_window_selection => window_selection);
guarded_callback!(guarded_document_selection => document_selection);

/// The private property of the global that holds the script's exports, or
/// `false` once the script has failed.
const EXPORTS: &str = "tenta.selection";

/// Defines the names. `Document` is looked up on `global`, so the generated
/// interfaces have to be installed already.
pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    let undefined = v8::undefined(scope);
    let name = key(scope, "Selection");
    global.set_lazy_data_property_with_data(
        scope,
        name.into(),
        lazy_interface,
        undefined.into(),
        v8::PropertyAttribute::DONT_ENUM,
        v8::SideEffectType::HasSideEffect,
        v8::SideEffectType::HasSideEffect,
    );
    define(scope, global, "getSelection", guarded_window_selection);
    let prototype = get(scope, global, "Document")
        .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
        .and_then(|constructor| get(scope, constructor, "prototype"))
        .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok());
    if let Some(prototype) = prototype {
        define(scope, prototype, "getSelection", guarded_document_selection);
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
        let built = v8::String::new(tc, tenta_js::selection::SELECTION_PRELUDE_SOURCE)
            .and_then(|source| v8::Script::compile(tc, source, None))
            .and_then(|script| script.run(tc))
            .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
            .and_then(|build| {
                let queue = v8::Function::builder(guarded_queue_task_function).build(tc)?;
                let receiver = v8::undefined(tc).into();
                build.call(tc, receiver, &[queue.into()])
            })
            .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
            .map(|object| v8::Global::new(tc, object));
        // A termination is the page's to see; anything else the script threw
        // is this realm going without a selection.
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
    unsafe { v8_registered_slow_callback_boundary(scope, "lazy_selection_interface", (), call) };
}

/// Calls the exported function `name` with `arguments` and returns what it
/// returns, or `null` in a realm without the script.
fn call_export<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
    arguments: &[v8::Local<'s, v8::Value>],
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    rv.set_null();
    let Some(exports) = exports(scope) else {
        return;
    };
    let Some(function) =
        get(scope, exports, name).and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
    else {
        return;
    };
    let undefined = v8::undefined(scope).into();
    if let Some(value) = function.call(scope, undefined, arguments) {
        rv.set(value);
    }
}

/// `window.getSelection()`.
fn window_selection<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    call_export(scope, "windowSelection", &[], rv);
}

/// `Document.prototype.getSelection()`.
fn document_selection<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    let this = args.this().into();
    call_export(scope, "documentSelection", &[this], rv);
}
