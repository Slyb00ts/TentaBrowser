//! `CSSStyleSheet.prototype.replace()`: `replaceSync()` with its outcome in a
//! promise.
//!
//! The generated glue has no promise-returning operations (it would throw
//! where WebIDL says to reject), so this one is defined by hand. It is the
//! native `replaceSync` of the same prototype, captured when the engine is
//! built and so not something a page can swap out, called with the receiver
//! and the argument as they came. Whatever that throws (a receiver that is
//! not a sheet, a sheet that was not constructed) is what the promise is
//! rejected with; otherwise the promise is fulfilled with the sheet.
//!
//! CSSOM parses the text off the main thread and sets the rules when that is
//! done. Here the rules are set before `replace()` returns, which a page can
//! only tell by reading `cssRules` in the same task.

use tenta_js::bindings::{
    v8_begin_stored_value_lease, v8_end_stored_value_lease, v8_registered_slow_callback_boundary,
};

use super::observers::{get, key};
use super::streams::prototype_of;

guarded_callback!(guarded_replace => replace);

/// Defines `replace` on `CSSStyleSheet.prototype`, which has to be installed.
pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    let Some(prototype) = prototype_of(scope, global, "CSSStyleSheet") else {
        return;
    };
    let Some(replace_sync) = get(scope, prototype, "replaceSync") else {
        return;
    };
    let Some(function) = v8::Function::builder(guarded_replace)
        .data(replace_sync)
        .length(1)
        .build(scope)
    else {
        return;
    };
    let name = key(scope, "replace");
    function.set_name(name);
    prototype.set(scope, name.into(), function.into());
}

fn replace<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        return;
    };
    rv.set(resolver.get_promise(scope).into());
    let Ok(replace_sync) = v8::Local::<v8::Function>::try_from(args.data()) else {
        return;
    };
    let this: v8::Local<v8::Value> = args.this().into();
    let arguments: Vec<v8::Local<v8::Value>> = (0..args.length()).map(|i| args.get(i)).collect();
    v8::tc_scope!(let tc, scope);
    if replace_sync.call(tc, this, &arguments).is_some() {
        resolver.resolve(tc, this);
        return;
    }
    // A termination is the page's to see, not a reason to reject with.
    if tc.has_terminated() {
        return;
    }
    if let Some(reason) = tc.exception() {
        tc.reset();
        resolver.reject(tc, reason);
    }
}
