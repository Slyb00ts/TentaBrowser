//! Window members that have no generated interface: `matchMedia` and its
//! `MediaQueryList`, the scrolling methods, and `requestIdleCallback`.
//!
//! - **`matchMedia`.** The query is parsed and evaluated by Stylo
//!   (`tenta_css::cascade::match_media`) against the page's viewport, and
//!   `matches` is evaluated again on every read. A `MediaQueryList` is a real
//!   `EventTarget` (it is made by the realm's `EventTarget` constructor and
//!   given the `MediaQueryList` prototype), so listeners can be added, but no
//!   `change` event is fired: nothing tells the realm that the viewport
//!   changed.
//! - **Scrolling.** `scrollTo`, `scroll` and `scrollBy` do nothing: the
//!   viewport does not scroll (`scrollX` and `scrollY` stay 0).
//! - **`requestIdleCallback`.** The callback runs as a timer task with no
//!   delay and is told it has time left; ids share the timers' id space, as
//!   `cancelIdleCallback` cancels through it.

use tenta_js::bindings::{
    JsError, string_from_v8, throw_js_error, throw_type_error, v8_begin_stored_value_lease,
    v8_end_stored_value_lease, v8_registered_slow_callback_boundary,
};

use super::{define, host_state, window_host};

guarded_callback!(guarded_mql_constructor => mql_constructor);
guarded_callback!(guarded_match_media => match_media);
guarded_callback!(guarded_mql_media => mql_media);
guarded_callback!(guarded_mql_matches => mql_matches);
guarded_callback!(guarded_mql_onchange => mql_onchange);
guarded_callback!(guarded_mql_set_onchange => mql_set_onchange);
guarded_callback!(guarded_mql_add_listener => mql_add_listener);
guarded_callback!(guarded_mql_remove_listener => mql_remove_listener);
guarded_callback!(guarded_no_scroll => no_scroll);
guarded_callback!(guarded_request_idle_callback => request_idle_callback);
guarded_callback!(guarded_run_idle_callback => run_idle_callback);
guarded_callback!(guarded_time_remaining => time_remaining);

/// The private property of a `MediaQueryList` that holds its serialised
/// query; its presence is the brand the members check.
const MEDIA: &str = "tenta:MediaQueryList#media";
/// The private property that holds the `onchange` handler.
const ONCHANGE: &str = "tenta:MediaQueryList#onchange";
/// The private property of the global that holds what `matchMedia` needs:
/// `[MediaQueryList.prototype, EventTarget, addEventListener, removeEventListener]`.
const REALM: &str = "tenta:MediaQueryList#realm";

fn key<'s>(scope: &mut v8::PinScope<'s, '_>, name: &str) -> v8::Local<'s, v8::String> {
    v8::String::new(scope, name).expect("a member name fits V8")
}

fn private<'s>(scope: &mut v8::PinScope<'s, '_>, name: &str) -> v8::Local<'s, v8::Private> {
    let name = key(scope, name);
    v8::Private::for_api(scope, Some(name))
}

/// Installs the members of this module on `global`. The interface objects
/// (`EventTarget` among them) must already be defined.
pub(super) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    define(scope, global, "matchMedia", guarded_match_media);
    for name in ["scrollTo", "scroll", "scrollBy"] {
        define(scope, global, name, guarded_no_scroll);
    }
    define(
        scope,
        global,
        "requestIdleCallback",
        guarded_request_idle_callback,
    );
    // Idle callbacks are timers here, so cancelling one is cancelling a timer.
    let clear = key(scope, "clearTimeout");
    if let Some(clear) = global.get(scope, clear.into()) {
        let cancel = key(scope, "cancelIdleCallback");
        global.set(scope, cancel.into(), clear);
    }
    install_media_query_list(scope, global);
}

fn install_media_query_list(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) -> Option<()> {
    let interface = v8::FunctionTemplate::new(scope, guarded_mql_constructor);
    let name = key(scope, "MediaQueryList");
    interface.set_class_name(name);
    let prototype = interface.prototype_template(scope);
    let accessors: [(&str, v8::FunctionCallback, Option<v8::FunctionCallback>); 3] = [
        ("media", v8::MapFnTo::map_fn_to(guarded_mql_media), None),
        ("matches", v8::MapFnTo::map_fn_to(guarded_mql_matches), None),
        (
            "onchange",
            v8::MapFnTo::map_fn_to(guarded_mql_onchange),
            Some(v8::MapFnTo::map_fn_to(guarded_mql_set_onchange)),
        ),
    ];
    for (member, getter, setter) in accessors {
        let getter = v8::FunctionTemplate::builder_raw(getter)
            .length(0)
            .build(scope);
        let setter = setter.map(|setter| {
            v8::FunctionTemplate::builder_raw(setter)
                .length(1)
                .build(scope)
        });
        let member = key(scope, member);
        prototype.set_accessor_property(
            member.into(),
            Some(getter),
            setter,
            v8::PropertyAttribute::NONE,
        );
    }
    let methods: [(&str, v8::FunctionCallback); 2] = [
        (
            "addListener",
            v8::MapFnTo::map_fn_to(guarded_mql_add_listener),
        ),
        (
            "removeListener",
            v8::MapFnTo::map_fn_to(guarded_mql_remove_listener),
        ),
    ];
    for (member, callback) in methods {
        let method = v8::FunctionTemplate::builder_raw(callback)
            .length(1)
            .build(scope);
        let member = key(scope, member);
        prototype.set(member.into(), method.into());
    }
    let tag = v8::Symbol::get_to_string_tag(scope);
    prototype.set_with_attr(
        tag.into(),
        name.into(),
        v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_ENUM,
    );

    let constructor = interface.get_function(scope)?;
    global.define_own_property(
        scope,
        name.into(),
        constructor.into(),
        v8::PropertyAttribute::DONT_ENUM,
    );

    // `MediaQueryList : EventTarget`, for the prototype and the constructor.
    let event_target = key(scope, "EventTarget");
    let event_target = global.get(scope, event_target.into())?;
    let event_target = v8::Local::<v8::Function>::try_from(event_target).ok()?;
    let prototype_key = key(scope, "prototype");
    let target_prototype = event_target.get(scope, prototype_key.into())?;
    let target_prototype = v8::Local::<v8::Object>::try_from(target_prototype).ok()?;
    let own_prototype = constructor.get(scope, prototype_key.into())?;
    let own_prototype = v8::Local::<v8::Object>::try_from(own_prototype).ok()?;
    own_prototype.set_prototype(scope, target_prototype.into());
    let constructor_object: v8::Local<v8::Object> = constructor.into();
    constructor_object.set_prototype(scope, event_target.into());

    let add = key(scope, "addEventListener");
    let add = target_prototype.get(scope, add.into())?;
    let remove = key(scope, "removeEventListener");
    let remove = target_prototype.get(scope, remove.into())?;
    let realm = v8::Array::new_with_elements(
        scope,
        &[own_prototype.into(), event_target.into(), add, remove],
    );
    let realm_key = private(scope, REALM);
    global.set_private(scope, realm_key, realm.into());
    Some(())
}

/// One of the realm's stashed values (see [`REALM`]).
fn realm_value<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    index: u32,
) -> Option<v8::Local<'s, v8::Value>> {
    let realm_key = private(scope, REALM);
    let global = scope.get_current_context().global(scope);
    let realm = global.get_private(scope, realm_key)?;
    v8::Local::<v8::Object>::try_from(realm)
        .ok()?
        .get_index(scope, index)
}

/// `new MediaQueryList()`: the interface has no constructor.
fn mql_constructor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    throw_type_error(scope, "Illegal constructor");
}

/// The query's serialisation and whether it matches the viewport now.
fn evaluate(scope: &mut v8::PinScope, query: &str) -> Option<(String, bool)> {
    let host = match window_host(scope) {
        Ok(host) => host,
        Err(error) => {
            throw_js_error(scope, &error);
            return None;
        }
    };
    let fallback = host_state(scope)?.viewport.get();
    Some(host.match_media(query, &fallback))
}

fn match_media<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    if args.length() == 0 {
        throw_type_error(
            scope,
            "Failed to execute 'matchMedia' on 'Window': 1 argument required, but only 0 present.",
        );
        return;
    }
    let query = match string_from_v8(scope, args.get(0)) {
        Ok(query) => query,
        Err(error) => return throw_js_error(scope, &error),
    };
    let Some((media, _)) = evaluate(scope, &query) else {
        return;
    };
    let (Some(prototype), Some(event_target)) = (realm_value(scope, 0), realm_value(scope, 1))
    else {
        return;
    };
    let Ok(event_target) = v8::Local::<v8::Function>::try_from(event_target) else {
        return;
    };
    // A real `EventTarget`, so `addEventListener` and `dispatchEvent` work on
    // it, with the `MediaQueryList` prototype in front of `EventTarget`'s.
    let Some(list) = event_target.new_instance(scope, &[]) else {
        return;
    };
    list.set_prototype(scope, prototype);
    let Some(media) = v8::String::new(scope, &media) else {
        return throw_js_error(scope, &JsError::range_error("Invalid string length"));
    };
    let media_key = private(scope, MEDIA);
    list.set_private(scope, media_key, media.into());
    rv.set(list.into());
}

/// The receiver as a `MediaQueryList` and its query, or a thrown `TypeError`.
fn list_of<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
) -> Option<(v8::Local<'s, v8::Object>, String)> {
    let this = args.this();
    let media_key = private(scope, MEDIA);
    let media = this
        .get_private(scope, media_key)
        .filter(|media| media.is_string());
    match media {
        Some(media) => Some((this, media.to_rust_string_lossy(scope))),
        None => {
            throw_type_error(scope, "Illegal invocation");
            None
        }
    }
}

fn mql_media<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    if let Some((_, media)) = list_of(scope, &args)
        && let Some(media) = v8::String::new(scope, &media)
    {
        rv.set(media.into());
    }
}

fn mql_matches<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    if let Some((_, media)) = list_of(scope, &args)
        && let Some((_, matches)) = evaluate(scope, &media)
    {
        rv.set_bool(matches);
    }
}

/// Calls the realm's own `addEventListener` (index 2) or
/// `removeEventListener` (index 3) for `change` on `list`.
fn listen<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    list: v8::Local<'s, v8::Object>,
    method: u32,
    callback: v8::Local<'s, v8::Value>,
) {
    if callback.is_null_or_undefined() {
        return;
    }
    let Some(function) = realm_value(scope, method)
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
    else {
        return;
    };
    let change = key(scope, "change");
    let _ = function.call(scope, list.into(), &[change.into(), callback]);
}

fn mql_add_listener<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    if let Some((list, _)) = list_of(scope, &args) {
        listen(scope, list, 2, args.get(0));
    }
}

fn mql_remove_listener<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    if let Some((list, _)) = list_of(scope, &args) {
        listen(scope, list, 3, args.get(0));
    }
}

fn mql_onchange<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some((list, _)) = list_of(scope, &args) else {
        return;
    };
    let handler_key = private(scope, ONCHANGE);
    match list.get_private(scope, handler_key) {
        Some(handler) if handler.is_object() => rv.set(handler),
        _ => rv.set_null(),
    }
}

/// The handler is a `change` listener like any other; assigning replaces the
/// one assigned before, and a value that is not an object removes it.
fn mql_set_onchange<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some((list, _)) = list_of(scope, &args) else {
        return;
    };
    let handler_key = private(scope, ONCHANGE);
    if let Some(previous) = list.get_private(scope, handler_key)
        && previous.is_object()
    {
        listen(scope, list, 3, previous);
    }
    let value = args.get(0);
    if value.is_object() {
        list.set_private(scope, handler_key, value);
        listen(scope, list, 2, value);
    } else {
        let null = v8::null(scope);
        list.set_private(scope, handler_key, null.into());
    }
}

/// `scrollTo`, `scroll` and `scrollBy`: the viewport does not scroll.
fn no_scroll<'s>(
    _scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
}

fn request_idle_callback<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let callback = args.get(0);
    if !callback.is_function() {
        throw_type_error(
            scope,
            "Failed to execute 'requestIdleCallback' on 'Window': parameter 1 is not of type 'Function'.",
        );
        return;
    }
    let Some(task) = v8::Function::builder(guarded_run_idle_callback)
        .data(callback)
        .build(scope)
    else {
        return;
    };
    if let Some(id) = super::queue_background_task(scope, task) {
        rv.set_double(id as f64);
    }
}

/// The task of an idle callback: it is handed a deadline that has not timed
/// out and says there is time left.
fn run_idle_callback<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Ok(callback) = v8::Local::<v8::Function>::try_from(args.data()) else {
        return;
    };
    let deadline = v8::Object::new(scope);
    let did_timeout = key(scope, "didTimeout");
    let no = v8::Boolean::new(scope, false);
    deadline.set(scope, did_timeout.into(), no.into());
    define(scope, deadline, "timeRemaining", guarded_time_remaining);
    let undefined = v8::undefined(scope);
    // An exception the callback throws is reported by the timer that runs it.
    let _ = callback.call(scope, undefined.into(), &[deadline.into()]);
}

/// `IdleDeadline.timeRemaining()`: the page is idle whenever this runs, and
/// 50 ms is the longest an idle period is said to last.
fn time_remaining<'s>(
    _scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    rv.set_double(50.0);
}
