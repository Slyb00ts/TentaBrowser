//! Small window members a page expects to find: the `Window` interface
//! object, dialogs, `screen`, the facts about the browsing context
//! (`isSecureContext`, `origin`, `closed`, ...), `document.location`, and the
//! user-timing part of `performance`.
//!
//! - **Dialogs.** Nothing can answer a dialog, so each is dismissed at once,
//!   as an automated browser does: `alert` returns, `confirm` answers false,
//!   `prompt` answers null. The message is written to the console
//!   (`[alert] ...`), where a host reads it.
//! - **`screen`.** The screen is the viewport: its size, 24-bit colour, no
//!   reserved areas.
//! - **`performance`.** The legacy `timing` object reports the time origin
//!   for every milestone: the page does not record them individually. The
//!   timeline members (`mark`, `measure`, `getEntries*`, `clear*`) are a
//!   script, named here (`super::performance_timeline`).

use tenta_js::bindings::{
    string_from_v8, throw_js_error, throw_type_error, v8_begin_stored_value_lease,
    v8_end_stored_value_lease, v8_registered_slow_callback_boundary,
};
use tenta_js::location::LocationPart;

use super::{define, host_state, set_bool, set_number, window_host};

guarded_callback!(guarded_window_constructor => window_constructor);
guarded_callback!(guarded_alert => alert);
guarded_callback!(guarded_confirm => confirm);
guarded_callback!(guarded_prompt => prompt);
guarded_callback!(guarded_nothing => nothing);
guarded_callback!(guarded_origin => origin);
guarded_callback!(guarded_is_secure_context => is_secure_context);
guarded_callback!(guarded_screen_metric => screen_metric);
guarded_callback!(guarded_document_location => document_location);
guarded_callback!(guarded_set_document_location => set_document_location);

fn key<'s>(scope: &mut v8::PinScope<'s, '_>, name: &str) -> v8::Local<'s, v8::String> {
    v8::String::new(scope, name).expect("a member name fits V8")
}

/// The attributes of the legacy `performance.timing` object.
const TIMING_MILESTONES: [&str; 21] = [
    "navigationStart",
    "unloadEventStart",
    "unloadEventEnd",
    "redirectStart",
    "redirectEnd",
    "fetchStart",
    "domainLookupStart",
    "domainLookupEnd",
    "connectStart",
    "connectEnd",
    "secureConnectionStart",
    "requestStart",
    "responseStart",
    "responseEnd",
    "domLoading",
    "domInteractive",
    "domContentLoadedEventStart",
    "domContentLoadedEventEnd",
    "domComplete",
    "loadEventStart",
    "loadEventEnd",
];

/// Installs the members of this module. `document`'s interface must already
/// be defined; `performance` must already be on `global`.
pub(super) fn install(
    scope: &mut v8::PinScope,
    global: v8::Local<v8::Object>,
    time_origin_ms: f64,
) {
    define(scope, global, "alert", guarded_alert);
    define(scope, global, "confirm", guarded_confirm);
    define(scope, global, "prompt", guarded_prompt);
    // No printer, nothing to stop, and no other window to give focus to.
    for name in ["print", "stop", "focus", "blur"] {
        define(scope, global, name, guarded_nothing);
    }

    // The page is the only browsing context: no opener, no frame, no children.
    let name_key = key(scope, "name");
    let empty = key(scope, "");
    global.set(scope, name_key.into(), empty.into());
    set_bool(scope, global, "closed", false);
    set_number(scope, global, "length", 0.0);
    for name in ["opener", "frameElement"] {
        let name = key(scope, name);
        let null = v8::null(scope);
        global.set(scope, name.into(), null.into());
    }
    accessor(scope, global, "origin", guarded_origin, 0);
    accessor(
        scope,
        global,
        "isSecureContext",
        guarded_is_secure_context,
        0,
    );
    set_bool(scope, global, "crossOriginIsolated", false);

    install_screen(scope, global);
    install_document_location(scope, global);
    install_performance(scope, global, time_origin_ms);
}

/// A read-only accessor on `target` whose getter carries `data`.
fn accessor(
    scope: &mut v8::PinScope,
    target: v8::Local<v8::Object>,
    name: &str,
    getter: impl v8::MapFnTo<v8::FunctionCallback>,
    data: i32,
) {
    let data = v8::Integer::new(scope, data);
    let Some(getter) = v8::Function::builder(getter)
        .data(data.into())
        .length(0)
        .build(scope)
    else {
        return;
    };
    let setter = v8::undefined(scope);
    let mut descriptor = v8::PropertyDescriptor::new_from_get_set(getter.into(), setter.into());
    descriptor.set_enumerable(true);
    descriptor.set_configurable(true);
    let name = key(scope, name);
    target.define_property(scope, name.into(), &descriptor);
}

/// Defines the `Window` interface object and puts `Window.prototype` between
/// the global and `prototype`, which is `EventTarget.prototype`: `window
/// instanceof Window` holds and the window's chain is the one Web IDL gives
/// a `[Global]` interface (without the named-properties object).
pub(super) fn install_window_interface<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    global: v8::Local<'s, v8::Object>,
    event_target: v8::Local<'s, v8::Object>,
    event_target_prototype: v8::Local<'s, v8::Value>,
) -> Option<v8::Local<'s, v8::Value>> {
    let interface = v8::FunctionTemplate::new(scope, guarded_window_constructor);
    let name = key(scope, "Window");
    interface.set_class_name(name);
    let tag = v8::Symbol::get_to_string_tag(scope);
    interface.prototype_template(scope).set_with_attr(
        tag.into(),
        name.into(),
        v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_ENUM,
    );
    let constructor = interface.get_function(scope)?;
    let prototype_key = key(scope, "prototype");
    let prototype = constructor.get(scope, prototype_key.into())?;
    let prototype_object = v8::Local::<v8::Object>::try_from(prototype).ok()?;
    prototype_object.set_prototype(scope, event_target_prototype);
    let constructor_object: v8::Local<v8::Object> = constructor.into();
    constructor_object.set_prototype(scope, event_target.into());
    global.define_own_property(
        scope,
        name.into(),
        constructor.into(),
        v8::PropertyAttribute::DONT_ENUM,
    );
    Some(prototype)
}

/// `new Window()`: the interface has no constructor.
fn window_constructor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    throw_type_error(scope, "Illegal constructor");
}

fn nothing<'s>(
    _scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
}

// ---- dialogs -------------------------------------------------------------

/// Writes `[kind] message` to the realm's console, where a host reads what a
/// dialog said.
fn log_dialog(scope: &mut v8::PinScope, kind: &str, args: &v8::FunctionCallbackArguments) {
    let message = if args.length() == 0 {
        String::new()
    } else {
        match string_from_v8(scope, args.get(0)) {
            Ok(message) => message,
            Err(error) => return throw_js_error(scope, &error),
        }
    };
    let global = scope.get_current_context().global(scope);
    let console_key = key(scope, "console");
    let Some(console) = global
        .get(scope, console_key.into())
        .and_then(|console| v8::Local::<v8::Object>::try_from(console).ok())
    else {
        return;
    };
    let info_key = key(scope, "info");
    let Some(info) = console
        .get(scope, info_key.into())
        .and_then(|info| v8::Local::<v8::Function>::try_from(info).ok())
    else {
        return;
    };
    let Some(line) = v8::String::new(scope, &format!("[{kind}] {message}")) else {
        return;
    };
    let _ = info.call(scope, console.into(), &[line.into()]);
}

fn alert<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    log_dialog(scope, "alert", &args);
}

fn confirm<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    log_dialog(scope, "confirm", &args);
    rv.set_bool(false);
}

fn prompt<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    log_dialog(scope, "prompt", &args);
    rv.set_null();
}

// ---- the browsing context ------------------------------------------------

fn location_part(scope: &mut v8::PinScope, part: LocationPart) -> String {
    match window_host(scope) {
        Ok(host) => host.location_part(part),
        Err(_) => part.read(&tenta_js::location::document_location(None)),
    }
}

fn origin<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let origin = location_part(scope, LocationPart::Origin);
    if let Some(origin) = v8::String::new(scope, &origin) {
        rv.set(origin.into());
    }
}

/// A secure context is one delivered over `https`, or from the local machine.
fn is_secure_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let protocol = location_part(scope, LocationPart::Protocol);
    let hostname = location_part(scope, LocationPart::Hostname);
    let local = hostname == "localhost"
        || hostname.ends_with(".localhost")
        || hostname == "127.0.0.1"
        || hostname == "[::1]";
    rv.set_bool(matches!(protocol.as_str(), "https:" | "wss:" | "file:") || local);
}

// ---- screen --------------------------------------------------------------

fn install_screen(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    let screen = v8::Object::new(scope);
    // 0 is the width, 1 the height; the available area is the whole screen.
    for (name, metric) in [
        ("width", 0),
        ("height", 1),
        ("availWidth", 0),
        ("availHeight", 1),
    ] {
        accessor(scope, screen, name, guarded_screen_metric, metric);
    }
    for name in ["availLeft", "availTop"] {
        set_number(scope, screen, name, 0.0);
    }
    for name in ["colorDepth", "pixelDepth"] {
        set_number(scope, screen, name, 24.0);
    }
    let screen_key = key(scope, "screen");
    global.set(scope, screen_key.into(), screen.into());
}

fn screen_metric<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(state) = host_state(scope) else {
        return;
    };
    let geometry = state.geometry_host.borrow().clone();
    let viewport = geometry.map_or_else(|| state.viewport.get(), |host| host.viewport());
    let height = args.data().int32_value(scope) == Some(1);
    rv.set_double(if height {
        viewport.inner_height
    } else {
        viewport.inner_width
    });
}

// ---- document.location ---------------------------------------------------

/// `document.location` is the window's `Location` object, and assigning to
/// it navigates, as assigning to `window.location` does.
fn install_document_location(
    scope: &mut v8::PinScope,
    global: v8::Local<v8::Object>,
) -> Option<()> {
    let document_key = key(scope, "Document");
    let document = global.get(scope, document_key.into())?;
    let document = v8::Local::<v8::Object>::try_from(document).ok()?;
    let prototype_key = key(scope, "prototype");
    let prototype = document.get(scope, prototype_key.into())?;
    let prototype = v8::Local::<v8::Object>::try_from(prototype).ok()?;
    let getter = v8::Function::builder(guarded_document_location)
        .length(0)
        .build(scope)?;
    let setter = v8::Function::builder(guarded_set_document_location)
        .length(1)
        .build(scope)?;
    let mut descriptor = v8::PropertyDescriptor::new_from_get_set(getter.into(), setter.into());
    descriptor.set_enumerable(true);
    descriptor.set_configurable(true);
    let location_key = key(scope, "location");
    prototype.define_property(scope, location_key.into(), &descriptor);
    Some(())
}

fn document_location<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let global = scope.get_current_context().global(scope);
    let location_key = key(scope, "location");
    if let Some(location) = global.get(scope, location_key.into()) {
        rv.set(location);
    }
}

fn set_document_location<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let global = scope.get_current_context().global(scope);
    let location_key = key(scope, "location");
    let _ = global.set(scope, location_key.into(), args.get(0));
}

// ---- performance ---------------------------------------------------------

fn install_performance(
    scope: &mut v8::PinScope,
    global: v8::Local<v8::Object>,
    time_origin_ms: f64,
) -> Option<()> {
    let performance_key = key(scope, "performance");
    let performance = global.get(scope, performance_key.into())?;
    let performance = v8::Local::<v8::Object>::try_from(performance).ok()?;
    // `mark`, `measure`, the entry readers and the `clear*` methods are the
    // timeline script's, defined by name here and built at first use.
    super::performance_timeline::install_methods(scope, performance);

    // The legacy timing object: every milestone is the time origin.
    let timing = v8::Object::new(scope);
    let origin = time_origin_ms.round();
    for name in TIMING_MILESTONES {
        set_number(scope, timing, name, origin);
    }
    let timing_key = key(scope, "timing");
    performance.set(scope, timing_key.into(), timing.into());
    let navigation = v8::Object::new(scope);
    set_number(scope, navigation, "type", 0.0);
    set_number(scope, navigation, "redirectCount", 0.0);
    let navigation_key = key(scope, "navigation");
    performance.set(scope, navigation_key.into(), navigation.into());
    Some(())
}
