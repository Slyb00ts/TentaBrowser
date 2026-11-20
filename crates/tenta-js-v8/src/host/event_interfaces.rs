//! The native half of `tenta_js::event_interfaces`: the names of the event
//! interfaces that are script, defined when the engine is built, and the
//! script behind them, run when a page first uses one or the engine first
//! fires such an event.

use tenta_js::bindings::v8_registered_slow_callback_boundary;

use super::lazy;
use super::observers::key;

/// The private property of the global that holds the script's exports, or
/// `false` once the script has failed.
const EXPORTS: &str = "tenta.eventInterfaces";

/// The names the script exports.
const INTERFACES: [&str; 16] = [
    "InputEvent",
    "PointerEvent",
    "PopStateEvent",
    "HashChangeEvent",
    "PageTransitionEvent",
    "TransitionEvent",
    "AnimationEvent",
    "SubmitEvent",
    "CloseEvent",
    "StorageEvent",
    "ClipboardEvent",
    "DragEvent",
    "ToggleEvent",
    "FormDataEvent",
    "MediaQueryListEvent",
    "BeforeUnloadEvent",
];

/// Defines the names.
pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    for name in INTERFACES {
        lazy::define_interface(scope, global, name, lazy_interface);
    }
}

fn exports<'s>(scope: &mut v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Object>> {
    lazy::exports(
        scope,
        EXPORTS,
        tenta_js::event_interfaces::EVENT_INTERFACES_PRELUDE_SOURCE,
        |_| Some(Vec::new()),
    )
}

/// Makes an event of the scripted interface `interface` for the engine to
/// fire: `new Interface(type, init)` with the script's own constructor, so a
/// page that replaced the global does not change what the engine fires.
/// `None` in a realm where the script failed, or when construction threw.
pub(crate) fn construct<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    interface: &str,
    kind: &str,
    init: &[(&str, v8::Local<'s, v8::Value>)],
) -> Option<v8::Local<'s, v8::Object>> {
    let exports = exports(scope)?;
    let name = key(scope, interface);
    let constructor = exports.get(scope, name.into())?;
    let constructor = v8::Local::<v8::Function>::try_from(constructor).ok()?;
    let dictionary = v8::Object::new(scope);
    for (member, value) in init {
        let member = key(scope, member);
        dictionary.set(scope, member.into(), *value);
    }
    let kind = key(scope, kind);
    constructor.new_instance(scope, &[kind.into(), dictionary.into()])
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
    unsafe { v8_registered_slow_callback_boundary(scope, "lazy_event_interface", (), call) };
}
