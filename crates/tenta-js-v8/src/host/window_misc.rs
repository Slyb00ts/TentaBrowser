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
//! - **`performance`.** `mark`, `measure`, `getEntries*` and the `clear*`
//!   methods keep plain entry objects (not `PerformanceEntry` instances) in
//!   the realm. The legacy `timing` object reports the time origin for every
//!   milestone: the page does not record them individually.

use tenta_js::bindings::{
    JsError, string_from_v8, throw_js_error, throw_type_error, v8_begin_stored_value_lease,
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
guarded_callback!(guarded_mark => mark);
guarded_callback!(guarded_measure => measure);
guarded_callback!(guarded_get_entries => get_entries);
guarded_callback!(guarded_get_entries_by_name => get_entries_by_name);
guarded_callback!(guarded_get_entries_by_type => get_entries_by_type);
guarded_callback!(guarded_clear_marks => clear_marks);
guarded_callback!(guarded_clear_measures => clear_measures);

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

/// The private property of `performance` that holds its entries.
const ENTRIES: &str = "tenta:Performance#entries";

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
    let entries = v8::Array::new(scope, 0);
    let entries_name = key(scope, ENTRIES);
    let entries_key = v8::Private::for_api(scope, Some(entries_name));
    performance.set_private(scope, entries_key, entries.into());
    define(scope, performance, "mark", guarded_mark);
    define(scope, performance, "measure", guarded_measure);
    define(scope, performance, "getEntries", guarded_get_entries);
    define(
        scope,
        performance,
        "getEntriesByName",
        guarded_get_entries_by_name,
    );
    define(
        scope,
        performance,
        "getEntriesByType",
        guarded_get_entries_by_type,
    );
    define(scope, performance, "clearMarks", guarded_clear_marks);
    define(scope, performance, "clearMeasures", guarded_clear_measures);

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

/// The entry list of the realm's `performance` object.
fn entries<'s>(scope: &mut v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Array>> {
    let global = scope.get_current_context().global(scope);
    let performance_key = key(scope, "performance");
    let performance = global.get(scope, performance_key.into())?;
    let performance = v8::Local::<v8::Object>::try_from(performance).ok()?;
    let entries_name = key(scope, ENTRIES);
    let entries_key = v8::Private::for_api(scope, Some(entries_name));
    let entries = performance.get_private(scope, entries_key)?;
    v8::Local::<v8::Array>::try_from(entries).ok()
}

fn now(scope: &mut v8::PinScope) -> f64 {
    host_state(scope).map_or(0.0, |state| state.scheduler.now().as_millis_f64())
}

fn entry<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: v8::Local<'s, v8::Value>,
    kind: &str,
    start: f64,
    duration: f64,
) -> v8::Local<'s, v8::Object> {
    let entry = v8::Object::new(scope);
    let name_key = key(scope, "name");
    entry.set(scope, name_key.into(), name);
    let type_key = key(scope, "entryType");
    let kind = key(scope, kind);
    entry.set(scope, type_key.into(), kind.into());
    set_number(scope, entry, "startTime", start);
    set_number(scope, entry, "duration", duration);
    entry
}

fn required_name<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    method: &str,
) -> Option<v8::Local<'s, v8::String>> {
    if args.length() == 0 {
        throw_type_error(
            scope,
            &format!(
                "Failed to execute '{method}' on 'Performance': 1 argument required, but only 0 present."
            ),
        );
        return None;
    }
    let name = args.get(0).to_string(scope);
    if name.is_none() {
        throw_js_error(
            scope,
            &JsError::type_error("the name cannot be converted to a string"),
        );
    }
    name
}

fn mark<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(name) = required_name(scope, &args, "mark") else {
        return;
    };
    let start = now(scope);
    let created = entry(scope, name.into(), "mark", start, 0.0);
    if let Some(list) = entries(scope) {
        let length = list.length();
        list.set_index(scope, length, created.into());
    }
    rv.set(created.into());
}

/// The start time of the newest mark named `name`.
fn mark_time<'s>(scope: &mut v8::PinScope<'s, '_>, name: v8::Local<'s, v8::Value>) -> Option<f64> {
    let list = entries(scope)?;
    let name_key = key(scope, "name");
    let type_key = key(scope, "entryType");
    let start_key = key(scope, "startTime");
    let mark = key(scope, "mark");
    for index in (0..list.length()).rev() {
        let Some(candidate) = list
            .get_index(scope, index)
            .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
        else {
            continue;
        };
        let is_mark = candidate
            .get(scope, type_key.into())
            .is_some_and(|kind| kind.strict_equals(mark.into()));
        let same_name = candidate
            .get(scope, name_key.into())
            .is_some_and(|candidate_name| candidate_name.strict_equals(name));
        if is_mark && same_name {
            return candidate.get(scope, start_key.into())?.number_value(scope);
        }
    }
    None
}

/// `measure(name, startMark?, endMark?)`: from a mark (or the time origin) to
/// a mark (or now). A name that is no mark is a `SyntaxError`. The options
/// object form is read as having neither mark.
fn measure<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(name) = required_name(scope, &args, "measure") else {
        return;
    };
    let bound = |scope: &mut v8::PinScope<'s, '_>, index: i32, default: f64| -> Option<f64> {
        let mark = args.get(index);
        if !mark.is_string() {
            return Some(default);
        }
        // User Timing lets a measure name a milestone of `performance.timing`
        // instead of a mark; every one of them is the time origin here.
        let text = mark.to_rust_string_lossy(scope);
        let found = mark_time(scope, mark)
            .or_else(|| TIMING_MILESTONES.contains(&text.as_str()).then_some(0.0));
        if found.is_none() {
            throw_js_error(
                scope,
                &JsError::dom_exception(
                    "SyntaxError",
                    &format!("The mark '{text}' does not exist."),
                ),
            );
        }
        found
    };
    let Some(start) = bound(scope, 1, 0.0) else {
        return;
    };
    let current = now(scope);
    let Some(end) = bound(scope, 2, current) else {
        return;
    };
    let created = entry(scope, name.into(), "measure", start, end - start);
    if let Some(list) = entries(scope) {
        let length = list.length();
        list.set_index(scope, length, created.into());
    }
    rv.set(created.into());
}

/// The entries whose `field` is strictly `value` (all of them without a
/// field), as a new array.
fn select<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    filters: &[(&str, v8::Local<'s, v8::Value>)],
) -> v8::Local<'s, v8::Array> {
    let selected = v8::Array::new(scope, 0);
    let Some(list) = entries(scope) else {
        return selected;
    };
    for index in 0..list.length() {
        let Some(candidate) = list
            .get_index(scope, index)
            .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
        else {
            continue;
        };
        let mut keep = true;
        for (field, value) in filters {
            let field = key(scope, field);
            keep &= candidate
                .get(scope, field.into())
                .is_some_and(|found| found.strict_equals(*value));
        }
        if keep {
            let length = selected.length();
            selected.set_index(scope, length, candidate.into());
        }
    }
    selected
}

fn get_entries<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let selected = select(scope, &[]);
    rv.set(selected.into());
}

fn get_entries_by_name<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(name) = required_name(scope, &args, "getEntriesByName") else {
        return;
    };
    let mut filters: Vec<(&str, v8::Local<v8::Value>)> = vec![("name", name.into())];
    if args.get(1).is_string() {
        filters.push(("entryType", args.get(1)));
    }
    let selected = select(scope, &filters);
    rv.set(selected.into());
}

fn get_entries_by_type<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(kind) = required_name(scope, &args, "getEntriesByType") else {
        return;
    };
    let selected = select(scope, &[("entryType", kind.into())]);
    rv.set(selected.into());
}

/// Removes the entries of `kind`, all of them or those named by the first
/// argument.
fn clear<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    kind: &str,
) {
    let Some(list) = entries(scope) else {
        return;
    };
    let name = args.get(0);
    let name_key = key(scope, "name");
    let type_key = key(scope, "entryType");
    let kind = key(scope, kind);
    let mut kept: Vec<v8::Local<v8::Value>> = Vec::new();
    for index in 0..list.length() {
        let Some(value) = list.get_index(scope, index) else {
            continue;
        };
        let Ok(candidate) = v8::Local::<v8::Object>::try_from(value) else {
            continue;
        };
        let of_kind = candidate
            .get(scope, type_key.into())
            .is_some_and(|found| found.strict_equals(kind.into()));
        let named = name.is_undefined()
            || candidate
                .get(scope, name_key.into())
                .is_some_and(|found| found.strict_equals(name));
        if !(of_kind && named) {
            kept.push(value);
        }
    }
    let zero = v8::Integer::new(scope, 0);
    let length_key = key(scope, "length");
    list.set(scope, length_key.into(), zero.into());
    for (index, value) in kept.into_iter().enumerate() {
        list.set_index(scope, index as u32, value);
    }
}

fn clear_marks<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    clear(scope, &args, "mark");
}

fn clear_measures<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    clear(scope, &args, "measure");
}
