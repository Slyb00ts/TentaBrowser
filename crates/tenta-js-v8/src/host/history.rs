//! `window.history` and the `History` interface
//! (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#the-history-interface>).
//!
//! What exists is the session history of one document: the entry the document
//! was loaded into and the entries script adds with `pushState` and
//! `replaceState`. `go`, `back` and `forward` move between those entries,
//! restore the entry's URL and fire `popstate`; they do not load another
//! document, so a step that would leave this document does nothing, and
//! `go(0)` (a reload) does nothing either. A new document starts a new
//! history: the entries live in the realm, in a store object only these
//! callbacks hold.
//!
//! Known gaps: `popstate` is a plain `Event` carrying `state` as an own
//! property (there is no `PopStateEvent` interface yet), `hashchange` is not
//! fired, and an entry keeps the one clone made when it was pushed instead of
//! deserializing a new one on each traversal.

use tenta_js::bindings::{
    JsError, string_from_v8, throw_js_error, throw_type_error, v8_begin_stored_value_lease,
    v8_end_stored_value_lease, v8_registered_slow_callback_boundary,
};

use super::window_host;

// Slots of the store array.
/// The entries: an array of `[url, state]` pairs.
const ENTRIES: u32 = 0;
/// The index of the current entry.
const INDEX: u32 = 1;
/// `scrollRestoration`.
const SCROLL: u32 = 2;
/// The realm's own `structuredClone`, captured before page script could
/// replace it.
const CLONE: u32 = 3;
/// The window's own `dispatchEvent`, captured the same way.
const DISPATCH: u32 = 4;

guarded_callback!(guarded_constructor => constructor);
guarded_callback!(guarded_length => length);
guarded_callback!(guarded_state => state);
guarded_callback!(guarded_scroll_restoration => scroll_restoration);
guarded_callback!(guarded_set_scroll_restoration => set_scroll_restoration);
guarded_callback!(guarded_push_state => push_state);
guarded_callback!(guarded_replace_state => replace_state);
guarded_callback!(guarded_go => go);
guarded_callback!(guarded_back => back);
guarded_callback!(guarded_forward => forward);
guarded_callback!(guarded_traverse => traverse);
guarded_callback!(guarded_window_history => window_history);
guarded_callback!(guarded_hash_changed => hash_changed);

/// The name of the private property of the global that holds the store, for
/// the callers that have no callback data to carry it: a fragment navigation
/// through `location` or an activated link adds a history entry too.
const STORE_KEY: &str = "tenta:History#store";

fn store_of<'s>(scope: &mut v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Value>> {
    let name = key(scope, STORE_KEY);
    let private = v8::Private::for_api(scope, Some(name));
    let global = scope.get_current_context().global(scope);
    global.get_private(scope, private)
}

/// Records a navigation that stayed in the document because only the
/// fragment changed: a new entry without state (or the current entry's URL,
/// when `replace`), and a `hashchange` event queued for the window. The
/// document's URL is already `new_url`.
pub(crate) fn fragment_navigated(
    scope: &mut v8::PinScope,
    old_url: &str,
    new_url: &str,
    replace: bool,
) {
    if old_url == new_url {
        return;
    }
    let Some(data) = store_of(scope) else {
        return;
    };
    // The document's URL has changed already, so the entry the document was
    // loaded into, if this is the first use of the history, is at `old_url`.
    let Some(mut store) = Store::open_at(scope, data, Some(old_url)) else {
        return;
    };
    let null = v8::null(scope).into();
    let Some(added) = entry(scope, new_url, null) else {
        return;
    };
    if replace {
        store.entries.set_index(scope, store.index, added.into());
    } else {
        let next = store.index + 1;
        let length = v8::Integer::new_from_unsigned(scope, next);
        let length_key = key(scope, "length");
        store.entries.set(scope, length_key.into(), length.into());
        store.entries.set_index(scope, next, added.into());
        store.set_index(scope, next);
    }
    let (Some(old), Some(new)) = (
        v8::String::new(scope, old_url),
        v8::String::new(scope, new_url),
    ) else {
        return;
    };
    let urls = v8::Array::new_with_elements(scope, &[data, old.into(), new.into()]);
    if let Some(task) = v8::Function::builder(guarded_hash_changed)
        .data(urls.into())
        .build(scope)
    {
        super::queue_task(scope, task);
    }
}

/// The queued half of a fragment navigation: `hashchange` at the window, as a
/// plain `Event` carrying `oldURL` and `newURL` (there is no `HashChangeEvent`
/// interface yet).
fn hash_changed<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Ok(data) = v8::Local::<v8::Object>::try_from(args.data()) else {
        return;
    };
    let (Some(store), Some(old), Some(new)) = (
        data.get_index(scope, 0),
        data.get_index(scope, 1),
        data.get_index(scope, 2),
    ) else {
        return;
    };
    let Ok(store) = v8::Local::<v8::Object>::try_from(store) else {
        return;
    };
    let global = scope.get_current_context().global(scope);
    let Some(event) = super::event_interfaces::construct(
        scope,
        "HashChangeEvent",
        "hashchange",
        &[("oldURL", old), ("newURL", new)],
    ) else {
        return;
    };
    let Some(dispatch) = store
        .get_index(scope, DISPATCH)
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
    else {
        return;
    };
    let _ = dispatch.call(scope, global.into(), &[event.into()]);
}

fn key<'s>(scope: &mut v8::PinScope<'s, '_>, name: &str) -> v8::Local<'s, v8::String> {
    v8::String::new(scope, name).expect("a member name fits V8")
}

/// Installs `window.history` and the `History` interface object. Call after
/// `structuredClone` and the window's `dispatchEvent` exist on `global`.
pub(super) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    let store = v8::Array::new(scope, 5);
    let entries = v8::Array::new(scope, 0);
    store.set_index(scope, ENTRIES, entries.into());
    let zero = v8::Integer::new(scope, 0);
    store.set_index(scope, INDEX, zero.into());
    let auto = key(scope, "auto");
    store.set_index(scope, SCROLL, auto.into());
    for (slot, name) in [(CLONE, "structuredClone"), (DISPATCH, "dispatchEvent")] {
        let name = key(scope, name);
        if let Some(function) = global.get(scope, name.into()) {
            store.set_index(scope, slot, function);
        }
    }

    let store_name = key(scope, STORE_KEY);
    let private = v8::Private::for_api(scope, Some(store_name));
    global.set_private(scope, private, store.into());

    let interface = v8::FunctionTemplate::new(scope, guarded_constructor);
    let name = key(scope, "History");
    interface.set_class_name(name);
    // A receiver that is not a `History` gets V8's own "Illegal invocation".
    let signature = v8::Signature::new(scope, interface);
    let prototype = interface.prototype_template(scope);

    let accessors: [(&str, v8::FunctionCallback, Option<v8::FunctionCallback>); 3] = [
        ("length", v8::MapFnTo::map_fn_to(guarded_length), None),
        (
            "scrollRestoration",
            v8::MapFnTo::map_fn_to(guarded_scroll_restoration),
            Some(v8::MapFnTo::map_fn_to(guarded_set_scroll_restoration)),
        ),
        ("state", v8::MapFnTo::map_fn_to(guarded_state), None),
    ];
    for (name, getter, setter) in accessors {
        let getter = v8::FunctionTemplate::builder_raw(getter)
            .data(store.into())
            .signature(signature)
            .length(0)
            .build(scope);
        let setter = setter.map(|setter| {
            v8::FunctionTemplate::builder_raw(setter)
                .data(store.into())
                .signature(signature)
                .length(1)
                .build(scope)
        });
        let name_key = key(scope, name);
        prototype.set_accessor_property(
            name_key.into(),
            Some(getter),
            setter,
            v8::PropertyAttribute::NONE,
        );
    }

    let methods: [(&str, v8::FunctionCallback, i32); 5] = [
        ("go", v8::MapFnTo::map_fn_to(guarded_go), 0),
        ("back", v8::MapFnTo::map_fn_to(guarded_back), 0),
        ("forward", v8::MapFnTo::map_fn_to(guarded_forward), 0),
        ("pushState", v8::MapFnTo::map_fn_to(guarded_push_state), 2),
        (
            "replaceState",
            v8::MapFnTo::map_fn_to(guarded_replace_state),
            2,
        ),
    ];
    for (name, callback, arity) in methods {
        let method = v8::FunctionTemplate::builder_raw(callback)
            .data(store.into())
            .signature(signature)
            .length(arity)
            .build(scope);
        let name_key = key(scope, name);
        prototype.set(name_key.into(), method.into());
    }
    let tag = v8::Symbol::get_to_string_tag(scope);
    prototype.set_with_attr(
        tag.into(),
        name.into(),
        v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_ENUM,
    );

    let constructor = interface
        .get_function(scope)
        .expect("a template has a function instance in an entered context");
    global.define_own_property(
        scope,
        name.into(),
        constructor.into(),
        v8::PropertyAttribute::DONT_ENUM,
    );
    let history = interface
        .instance_template(scope)
        .new_instance(scope)
        .expect("the History template instantiates in an entered context");
    // The getter carries the object, so the realm needs no other slot for it.
    let getter = v8::Function::builder(guarded_window_history)
        .data(history.into())
        .length(0)
        .build(scope)
        .expect("function creation while a context is entered");
    let setter = v8::undefined(scope);
    let mut descriptor = v8::PropertyDescriptor::new_from_get_set(getter.into(), setter.into());
    descriptor.set_enumerable(true);
    descriptor.set_configurable(true);
    let name_key = key(scope, "history");
    global.define_property(scope, name_key.into(), &descriptor);
}

/// `new History()` and `History()`: the interface has no constructor.
fn constructor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    throw_type_error(scope, "Illegal constructor");
}

fn window_history<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let global = scope.get_current_context().global(scope);
    if !args.this().strict_equals(global.into()) {
        throw_js_error(scope, &JsError::type_error("Illegal invocation"));
        return;
    }
    rv.set(args.data());
}

/// The store these callbacks share, with the entry the document was loaded
/// into added on first use: its URL is not known when the realm is built.
struct Store<'s> {
    store: v8::Local<'s, v8::Object>,
    entries: v8::Local<'s, v8::Array>,
    index: u32,
}

impl<'s> Store<'s> {
    fn open(scope: &mut v8::PinScope<'s, '_>, data: v8::Local<'s, v8::Value>) -> Option<Self> {
        Self::open_at(scope, data, None)
    }

    /// [`Self::open`] for a caller that knows the URL of the first entry
    /// better than the document does now.
    fn open_at(
        scope: &mut v8::PinScope<'s, '_>,
        data: v8::Local<'s, v8::Value>,
        first_url: Option<&str>,
    ) -> Option<Self> {
        let store = v8::Local::<v8::Object>::try_from(data).ok()?;
        let entries = store.get_index(scope, ENTRIES)?;
        let entries = v8::Local::<v8::Array>::try_from(entries).ok()?;
        if entries.length() == 0 {
            let url = match (first_url, window_host(scope)) {
                (Some(url), _) => url.to_owned(),
                (None, Ok(host)) => host.rewrite_document_url(None).ok()?,
                (None, Err(_)) => "about:blank".to_owned(),
            };
            let entry = entry(scope, &url, v8::null(scope).into())?;
            entries.set_index(scope, 0, entry.into());
        }
        let index = store
            .get_index(scope, INDEX)?
            .uint32_value(scope)
            .unwrap_or(0)
            .min(entries.length().saturating_sub(1));
        Some(Self {
            store,
            entries,
            index,
        })
    }

    /// `[url, state]` of the entry at `index`.
    fn entry(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        index: u32,
    ) -> Option<(v8::Local<'s, v8::Value>, v8::Local<'s, v8::Value>)> {
        let pair = self.entries.get_index(scope, index)?;
        let pair = v8::Local::<v8::Object>::try_from(pair).ok()?;
        Some((pair.get_index(scope, 0)?, pair.get_index(scope, 1)?))
    }

    fn set_index(&mut self, scope: &mut v8::PinScope<'s, '_>, index: u32) {
        self.index = index;
        let value = v8::Integer::new_from_unsigned(scope, index);
        self.store.set_index(scope, INDEX, value.into());
    }
}

fn entry<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    url: &str,
    state: v8::Local<'s, v8::Value>,
) -> Option<v8::Local<'s, v8::Array>> {
    let url = v8::String::new(scope, url)?;
    Some(v8::Array::new_with_elements(scope, &[url.into(), state]))
}

fn length<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    if let Some(store) = Store::open(scope, args.data()) {
        rv.set_uint32(store.entries.length());
    }
}

fn state<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(store) = Store::open(scope, args.data()) else {
        return;
    };
    if let Some((_, state)) = store.entry(scope, store.index) {
        rv.set(state);
    }
}

fn scroll_restoration<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Ok(store) = v8::Local::<v8::Object>::try_from(args.data()) else {
        return;
    };
    if let Some(value) = store.get_index(scope, SCROLL) {
        rv.set(value);
    }
}

/// An enumeration attribute: a value that is not one of its strings is
/// ignored, as WebIDL has it. Nothing scrolls, so the mode has no effect yet.
fn set_scroll_restoration<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Ok(store) = v8::Local::<v8::Object>::try_from(args.data()) else {
        return;
    };
    let mode = match string_from_v8(scope, args.get(0)) {
        Ok(mode) => mode,
        Err(error) => return throw_js_error(scope, &error),
    };
    if mode == "auto" || mode == "manual" {
        let value = key(scope, &mode);
        store.set_index(scope, SCROLL, value.into());
    }
}

fn push_state<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    update(scope, args, rv, "pushState", true);
}

fn replace_state<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    update(scope, args, rv, "replaceState", false);
}

/// The shared history push/replace state steps: clone the data, resolve and
/// check the URL, rewrite the document's URL, then add or replace the entry.
fn update<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
    name: &str,
    push: bool,
) {
    if args.length() < 2 {
        throw_type_error(
            scope,
            &format!(
                "Failed to execute '{name}' on 'History': 2 arguments required, but only {} present.",
                args.length()
            ),
        );
        return;
    }
    let Some(mut store) = Store::open(scope, args.data()) else {
        return;
    };
    // StructuredSerializeForStorage, through the realm's own structuredClone:
    // a value that cannot be cloned throws its DataCloneError from here.
    let Some(clone) = store
        .store
        .get_index(scope, CLONE)
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
    else {
        return;
    };
    let undefined = v8::undefined(scope).into();
    let Some(state) = clone.call(scope, undefined, &[args.get(0)]) else {
        return;
    };
    let url = if args.get(2).is_null_or_undefined() {
        None
    } else {
        match string_from_v8(scope, args.get(2)) {
            Ok(url) => Some(url),
            Err(error) => return throw_js_error(scope, &error),
        }
    };
    let host = match window_host(scope) {
        Ok(host) => host,
        Err(error) => return throw_js_error(scope, &error),
    };
    let url = match host.rewrite_document_url(url.as_deref()) {
        Ok(url) => url,
        Err(error) => return throw_js_error(scope, &error),
    };
    let Some(entry) = entry(scope, &url, state) else {
        return throw_js_error(scope, &JsError::range_error("Invalid string length"));
    };
    if push {
        // The entries ahead of the current one are dropped.
        let next = store.index + 1;
        let length = v8::Integer::new_from_unsigned(scope, next);
        let length_key = key(scope, "length");
        store.entries.set(scope, length_key.into(), length.into());
        store.entries.set_index(scope, next, entry.into());
        store.set_index(scope, next);
    } else {
        store.entries.set_index(scope, store.index, entry.into());
    }
}

fn go<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let delta = args.get(0).int32_value(scope).unwrap_or(0);
    schedule(scope, args.data(), delta);
}

fn back<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    schedule(scope, args.data(), -1);
}

fn forward<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    schedule(scope, args.data(), 1);
}

/// Queues the traversal as a task: the caller's script, and its microtasks,
/// finish before the URL changes and `popstate` fires.
fn schedule<'s>(scope: &mut v8::PinScope<'s, '_>, store: v8::Local<'s, v8::Value>, delta: i32) {
    if delta == 0 {
        return;
    }
    let delta = v8::Integer::new(scope, delta);
    let data = v8::Array::new_with_elements(scope, &[store, delta.into()]);
    let Some(task) = v8::Function::builder(guarded_traverse)
        .data(data.into())
        .build(scope)
    else {
        return;
    };
    super::queue_task(scope, task);
}

fn traverse<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Ok(data) = v8::Local::<v8::Object>::try_from(args.data()) else {
        return;
    };
    let (Some(store), Some(delta)) = (data.get_index(scope, 0), data.get_index(scope, 1)) else {
        return;
    };
    let delta = delta.int32_value(scope).unwrap_or(0);
    let Some(mut store) = Store::open(scope, store) else {
        return;
    };
    // Judged when the task runs: entries may have changed since the call.
    let Some(target) = store
        .index
        .checked_add_signed(delta)
        .filter(|target| *target < store.entries.length())
    else {
        return;
    };
    let Some((url, state)) = store.entry(scope, target) else {
        return;
    };
    store.set_index(scope, target);
    if let Ok(host) = window_host(scope) {
        host.restore_document_url(&url.to_rust_string_lossy(scope));
    }

    let global = scope.get_current_context().global(scope);
    let Some(event) =
        super::event_interfaces::construct(scope, "PopStateEvent", "popstate", &[("state", state)])
    else {
        return;
    };
    let Some(dispatch) = store
        .store
        .get_index(scope, DISPATCH)
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
    else {
        return;
    };
    // A listener that throws reports its own exception; nothing to add here.
    let _ = dispatch.call(scope, global.into(), &[event.into()]);
}
