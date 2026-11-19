//! The native half of `tenta_js::matrix`: the names of the geometry
//! interfaces that are script - `DOMMatrix`, `DOMMatrixReadOnly`, `DOMPoint`,
//! `DOMPointReadOnly`, `DOMQuad` and the legacy aliases - defined when the
//! engine is built, and the script behind them, run when a page first uses
//! one.
//!
//! The script asks nothing of the host: it is arithmetic. What is here is
//! the lazy names (`super::lazy`).

use tenta_js::bindings::v8_registered_slow_callback_boundary;

use super::lazy;

/// The private property of the global that holds the script's exports, or
/// `false` once the script has failed.
const EXPORTS: &str = "tenta.matrix";

/// The names the script exports. `WebKitCSSMatrix` and `SVGMatrix` are
/// `DOMMatrix` and `SVGPoint` is `DOMPoint`, as the standard's legacy window
/// aliases have it.
const INTERFACES: [&str; 8] = [
    "DOMMatrix",
    "DOMMatrixReadOnly",
    "DOMPoint",
    "DOMPointReadOnly",
    "DOMQuad",
    "WebKitCSSMatrix",
    "SVGMatrix",
    "SVGPoint",
];

/// Defines the names.
pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    for name in INTERFACES {
        lazy::define_interface(scope, global, name, lazy_interface);
    }
}

fn lazy_interface<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: v8::Local<'s, v8::Name>,
    _args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'_, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        let exports = lazy::exports(
            scope,
            EXPORTS,
            tenta_js::matrix::MATRIX_PRELUDE_SOURCE,
            |_| Some(Vec::new()),
        );
        if let Some(exports) = exports
            && let Some(interface) = exports.get(scope, name.into())
        {
            rv.set(interface);
        }
    };
    // SAFETY: a registered V8 property callback, where allocation is allowed.
    unsafe { v8_registered_slow_callback_boundary(scope, "lazy_matrix_interface", (), call) };
}
