//! Messaging inside one realm: `MessageChannel`, `MessagePort`,
//! `MessageEvent` and `window.postMessage()`.
//!
//! - **`MessageChannel`** makes two entangled ports. A message posted on one
//!   is cloned at once with `structuredClone` and delivered to the other as a
//!   `message` event from a task of its own, in the order posted. A port
//!   delivers nothing until it is started, by `start()` or by assigning
//!   `onmessage`, and keeps what arrives until then. `close()` ends the
//!   channel for both ports.
//! - **`MessagePort`** is a real `EventTarget` (made by the realm's
//!   `EventTarget` constructor and given the `MessagePort` prototype), so
//!   listeners can be added.
//! - **`MessageEvent`** is a real `Event` with the message members in front.
//! - **`window.postMessage()`** delivers to the window itself, with the
//!   document's origin and the window as source. There is no other browsing
//!   context to send to, and the target origin is not compared.
//!
//! Nothing is transferred: a transfer list is accepted and ignored, so a port
//! or a buffer named in it stays with its sender and `MessageEvent.ports` is
//! empty.

use tenta_js::bindings::{
    throw_type_error, v8_begin_stored_value_lease, v8_end_stored_value_lease,
    v8_registered_slow_callback_boundary,
};

use super::{define, queue_task};

guarded_callback!(guarded_channel_constructor => channel_constructor);
guarded_callback!(guarded_channel_port1 => channel_port1);
guarded_callback!(guarded_channel_port2 => channel_port2);
guarded_callback!(guarded_port_constructor => port_constructor);
guarded_callback!(guarded_port_post_message => port_post_message);
guarded_callback!(guarded_port_start => port_start);
guarded_callback!(guarded_port_close => port_close);
guarded_callback!(guarded_port_onmessage => port_onmessage);
guarded_callback!(guarded_port_set_onmessage => port_set_onmessage);
guarded_callback!(guarded_port_onmessageerror => port_onmessageerror);
guarded_callback!(guarded_port_set_onmessageerror => port_set_onmessageerror);
guarded_callback!(guarded_port_handler => port_handler);
guarded_callback!(guarded_deliver => deliver);
guarded_callback!(guarded_event_constructor => event_constructor);
guarded_callback!(guarded_event_member => event_member);
guarded_callback!(guarded_window_post_message => window_post_message);

/// The private property of the global that holds what this module needs from
/// the realm; see the `REALM_*` indexes.
const REALM: &str = "tenta:MessageChannel#realm";
const REALM_PORT_PROTOTYPE: u32 = 0;
const REALM_EVENT_TARGET: u32 = 1;
const REALM_EVENT_PROTOTYPE: u32 = 2;
const REALM_EVENT: u32 = 3;
const REALM_CLONE: u32 = 4;
const REALM_DISPATCH: u32 = 5;
const REALM_ADD_LISTENER: u32 = 6;
const REALM_PORT_HANDLER: u32 = 7;

/// A channel's two ports.
const PORTS: &str = "tenta:MessageChannel#ports";
/// A port's state, `[peer or null, started, closed, queue, onmessage,
/// onmessageerror, listening]`; its presence is the brand the members check.
const PORT: &str = "tenta:MessagePort#state";
const PORT_PEER: u32 = 0;
const PORT_STARTED: u32 = 1;
const PORT_CLOSED: u32 = 2;
const PORT_QUEUE: u32 = 3;
const PORT_ONMESSAGE: u32 = 4;
const PORT_ONMESSAGEERROR: u32 = 5;
const PORT_LISTENING: u32 = 6;
/// A message event's members, `[data, origin, lastEventId, source, ports]`.
const MESSAGE: &str = "tenta:MessageEvent#members";

fn key<'s>(scope: &mut v8::PinScope<'s, '_>, name: &str) -> v8::Local<'s, v8::String> {
    v8::String::new(scope, name).expect("a member name fits V8")
}

fn private<'s>(scope: &mut v8::PinScope<'s, '_>, name: &str) -> v8::Local<'s, v8::Private> {
    let name = key(scope, name);
    v8::Private::for_api(scope, Some(name))
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

fn realm_function<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    index: u32,
) -> Option<v8::Local<'s, v8::Function>> {
    realm_value(scope, index).and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
}

/// Makes `interface.prototype` inherit from `parent.prototype` and
/// `interface` from `parent`, and returns the interface's prototype.
fn inherit<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    interface: v8::Local<'s, v8::Function>,
    parent: v8::Local<'s, v8::Function>,
) -> Option<v8::Local<'s, v8::Object>> {
    let prototype_key = key(scope, "prototype");
    let parent_prototype = parent.get(scope, prototype_key.into())?;
    let own = interface.get(scope, prototype_key.into())?;
    let own = v8::Local::<v8::Object>::try_from(own).ok()?;
    own.set_prototype(scope, parent_prototype);
    let interface_object: v8::Local<v8::Object> = interface.into();
    interface_object.set_prototype(scope, parent.into());
    Some(own)
}

type Accessor = (
    &'static str,
    v8::FunctionCallback,
    Option<v8::FunctionCallback>,
);

/// Builds an interface object with the given prototype members and defines
/// it on `global`.
fn interface<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    global: v8::Local<'s, v8::Object>,
    name: &str,
    constructor: v8::FunctionCallback,
    length: i32,
    accessors: &[Accessor],
    methods: &[(&'static str, v8::FunctionCallback, i32)],
) -> Option<v8::Local<'s, v8::Function>> {
    let template = v8::FunctionTemplate::builder_raw(constructor)
        .length(length)
        .build(scope);
    let class_name = key(scope, name);
    template.set_class_name(class_name);
    let prototype = template.prototype_template(scope);
    for &(member, getter, setter) in accessors {
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
    for &(member, callback, length) in methods {
        let method = v8::FunctionTemplate::builder_raw(callback)
            .length(length)
            .build(scope);
        let member = key(scope, member);
        prototype.set(member.into(), method.into());
    }
    let tag = v8::Symbol::get_to_string_tag(scope);
    prototype.set_with_attr(
        tag.into(),
        class_name.into(),
        v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_ENUM,
    );
    let function = template.get_function(scope)?;
    global.define_own_property(
        scope,
        class_name.into(),
        function.into(),
        v8::PropertyAttribute::DONT_ENUM,
    );
    Some(function)
}

/// The function `name` names on `global`.
fn global_function<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    global: v8::Local<'s, v8::Object>,
    name: &str,
) -> Option<v8::Local<'s, v8::Function>> {
    let name = key(scope, name);
    let value = global.get(scope, name.into())?;
    v8::Local::<v8::Function>::try_from(value).ok()
}

/// Installs the members of this module on `global`. `EventTarget`, `Event`,
/// `structuredClone` and the window's event target operations must already
/// be defined.
pub(super) fn install<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    global: v8::Local<'s, v8::Object>,
) -> Option<()> {
    let event_target = global_function(scope, global, "EventTarget")?;
    let event = global_function(scope, global, "Event")?;
    let clone = global_function(scope, global, "structuredClone")?;

    let port = interface(
        scope,
        global,
        "MessagePort",
        v8::MapFnTo::map_fn_to(guarded_port_constructor),
        0,
        &[
            (
                "onmessage",
                v8::MapFnTo::map_fn_to(guarded_port_onmessage),
                Some(v8::MapFnTo::map_fn_to(guarded_port_set_onmessage)),
            ),
            (
                "onmessageerror",
                v8::MapFnTo::map_fn_to(guarded_port_onmessageerror),
                Some(v8::MapFnTo::map_fn_to(guarded_port_set_onmessageerror)),
            ),
        ],
        &[
            (
                "postMessage",
                v8::MapFnTo::map_fn_to(guarded_port_post_message),
                1,
            ),
            ("start", v8::MapFnTo::map_fn_to(guarded_port_start), 0),
            ("close", v8::MapFnTo::map_fn_to(guarded_port_close), 0),
        ],
    )?;
    let port_prototype = inherit(scope, port, event_target)?;

    let message_event = {
        let template =
            v8::FunctionTemplate::builder_raw(v8::MapFnTo::map_fn_to(guarded_event_constructor))
                .length(1)
                .build(scope);
        let class_name = key(scope, "MessageEvent");
        template.set_class_name(class_name);
        let prototype = template.prototype_template(scope);
        for (index, name) in ["data", "origin", "lastEventId", "source", "ports"]
            .into_iter()
            .enumerate()
        {
            let slot = v8::Integer::new(scope, index as i32);
            let getter =
                v8::FunctionTemplate::builder_raw(v8::MapFnTo::map_fn_to(guarded_event_member))
                    .data(slot.into())
                    .length(0)
                    .build(scope);
            let name = key(scope, name);
            prototype.set_accessor_property(
                name.into(),
                Some(getter),
                None,
                v8::PropertyAttribute::NONE,
            );
        }
        let tag = v8::Symbol::get_to_string_tag(scope);
        prototype.set_with_attr(
            tag.into(),
            class_name.into(),
            v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_ENUM,
        );
        let function = template.get_function(scope)?;
        global.define_own_property(
            scope,
            class_name.into(),
            function.into(),
            v8::PropertyAttribute::DONT_ENUM,
        );
        function
    };
    let event_prototype = inherit(scope, message_event, event)?;

    interface(
        scope,
        global,
        "MessageChannel",
        v8::MapFnTo::map_fn_to(guarded_channel_constructor),
        0,
        &[
            ("port1", v8::MapFnTo::map_fn_to(guarded_channel_port1), None),
            ("port2", v8::MapFnTo::map_fn_to(guarded_channel_port2), None),
        ],
        &[],
    )?;

    let prototype_key = key(scope, "prototype");
    let target_prototype = event_target.get(scope, prototype_key.into())?;
    let target_prototype = v8::Local::<v8::Object>::try_from(target_prototype).ok()?;
    let dispatch = key(scope, "dispatchEvent");
    let dispatch = target_prototype.get(scope, dispatch.into())?;
    let add = key(scope, "addEventListener");
    let add = target_prototype.get(scope, add.into())?;
    let handler = v8::Function::new(scope, guarded_port_handler)?;
    let realm = v8::Array::new_with_elements(
        scope,
        &[
            port_prototype.into(),
            event_target.into(),
            event_prototype.into(),
            event.into(),
            clone.into(),
            dispatch,
            add,
            handler.into(),
        ],
    );
    let realm_key = private(scope, REALM);
    global.set_private(scope, realm_key, realm.into());

    define(scope, global, "postMessage", guarded_window_post_message);
    Some(())
}

// ---- MessageChannel -------------------------------------------------------

/// A new port, entangled with nothing yet.
fn new_port<'s>(scope: &mut v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Object>> {
    let prototype = realm_value(scope, REALM_PORT_PROTOTYPE)?;
    let event_target = realm_function(scope, REALM_EVENT_TARGET)?;
    // A real `EventTarget`, with the `MessagePort` prototype in front.
    let port = event_target.new_instance(scope, &[])?;
    port.set_prototype(scope, prototype);
    let null: v8::Local<v8::Value> = v8::null(scope).into();
    let no: v8::Local<v8::Value> = v8::Boolean::new(scope, false).into();
    let queue: v8::Local<v8::Value> = v8::Array::new(scope, 0).into();
    let state = v8::Array::new_with_elements(scope, &[null, no, no, queue, null, null, no]);
    let state_key = private(scope, PORT);
    port.set_private(scope, state_key, state.into());
    Some(port)
}

/// The state of a port, or `None` for any other value.
fn port_state<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    port: v8::Local<'s, v8::Object>,
) -> Option<v8::Local<'s, v8::Object>> {
    let state_key = private(scope, PORT);
    let state = port.get_private(scope, state_key)?;
    if !state.is_array() {
        return None;
    }
    v8::Local::<v8::Object>::try_from(state).ok()
}

/// The receiver as a port and its state, or a thrown `TypeError`.
fn port_of<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
) -> Option<(v8::Local<'s, v8::Object>, v8::Local<'s, v8::Object>)> {
    let this = args.this();
    match port_state(scope, this) {
        Some(state) => Some((this, state)),
        None => {
            throw_type_error(scope, "Illegal invocation");
            None
        }
    }
}

fn flag(scope: &mut v8::PinScope, state: v8::Local<v8::Object>, index: u32) -> bool {
    state
        .get_index(scope, index)
        .is_some_and(|value| value.is_true())
}

fn set_flag(scope: &mut v8::PinScope, state: v8::Local<v8::Object>, index: u32, value: bool) {
    let value = v8::Boolean::new(scope, value);
    state.set_index(scope, index, value.into());
}

fn channel_constructor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    if args.new_target().is_undefined() {
        throw_type_error(
            scope,
            "Failed to construct 'MessageChannel': Please use the 'new' operator, this DOM object constructor cannot be called as a function.",
        );
        return;
    }
    let (Some(first), Some(second)) = (new_port(scope), new_port(scope)) else {
        return;
    };
    for (port, peer) in [(first, second), (second, first)] {
        if let Some(state) = port_state(scope, port) {
            state.set_index(scope, PORT_PEER, peer.into());
        }
    }
    let ports = v8::Array::new_with_elements(scope, &[first.into(), second.into()]);
    let ports_key = private(scope, PORTS);
    args.this().set_private(scope, ports_key, ports.into());
}

fn channel_port<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    index: u32,
    rv: &mut v8::ReturnValue<'s, v8::Value>,
) {
    let ports_key = private(scope, PORTS);
    let port = args
        .this()
        .get_private(scope, ports_key)
        .filter(|ports| ports.is_array())
        .and_then(|ports| v8::Local::<v8::Object>::try_from(ports).ok())
        .and_then(|ports| ports.get_index(scope, index));
    match port {
        Some(port) => rv.set(port),
        None => throw_type_error(scope, "Illegal invocation"),
    }
}

fn channel_port1<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    channel_port(scope, &args, 0, &mut rv);
}

fn channel_port2<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    channel_port(scope, &args, 1, &mut rv);
}

// ---- MessagePort ----------------------------------------------------------

/// `new MessagePort()`: the interface has no constructor.
fn port_constructor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    throw_type_error(scope, "Illegal constructor");
}

/// A structured clone of `message`, or `None` with the exception pending.
fn clone_message<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    message: v8::Local<'s, v8::Value>,
) -> Option<v8::Local<'s, v8::Value>> {
    let clone = realm_function(scope, REALM_CLONE)?;
    let undefined = v8::undefined(scope);
    clone.call(scope, undefined.into(), &[message])
}

/// Queues the task that delivers `event` members to `target`, which is a
/// port or the window.
fn schedule<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    target: v8::Local<'s, v8::Value>,
    members: v8::Local<'s, v8::Array>,
) {
    let pair = v8::Array::new_with_elements(scope, &[target, members.into()]);
    let Some(task) = v8::Function::builder(guarded_deliver)
        .data(pair.into())
        .build(scope)
    else {
        return;
    };
    queue_task(scope, task);
}

/// The members of a message event that carries `data`.
fn members<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    data: v8::Local<'s, v8::Value>,
    origin: v8::Local<'s, v8::Value>,
    source: v8::Local<'s, v8::Value>,
) -> v8::Local<'s, v8::Array> {
    let empty: v8::Local<v8::Value> = v8::String::empty(scope).into();
    let ports: v8::Local<v8::Value> = v8::Array::new(scope, 0).into();
    v8::Array::new_with_elements(scope, &[data, origin, empty, source, ports])
}

fn port_post_message<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some((_, state)) = port_of(scope, &args) else {
        return;
    };
    if args.length() == 0 {
        throw_type_error(
            scope,
            "Failed to execute 'postMessage' on 'MessagePort': 1 argument required, but only 0 present.",
        );
        return;
    }
    // Cloned before anything else, so a value that cannot be cloned throws
    // here and nothing is sent.
    let Some(data) = clone_message(scope, args.get(0)) else {
        return;
    };
    let Some(peer) = state
        .get_index(scope, PORT_PEER)
        .and_then(|peer| v8::Local::<v8::Object>::try_from(peer).ok())
    else {
        return;
    };
    let Some(peer_state) = port_state(scope, peer) else {
        return;
    };
    let empty: v8::Local<v8::Value> = v8::String::empty(scope).into();
    let null: v8::Local<v8::Value> = v8::null(scope).into();
    let members = members(scope, data, empty, null);
    if flag(scope, peer_state, PORT_STARTED) {
        schedule(scope, peer.into(), members);
    } else if let Some(queue) = peer_state
        .get_index(scope, PORT_QUEUE)
        .and_then(|queue| v8::Local::<v8::Array>::try_from(queue).ok())
    {
        let at = queue.length();
        queue.set_index(scope, at, members.into());
    }
}

/// Starts `port`: what arrived before is delivered, in order.
fn start<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    port: v8::Local<'s, v8::Object>,
    state: v8::Local<'s, v8::Object>,
) {
    if flag(scope, state, PORT_STARTED) {
        return;
    }
    set_flag(scope, state, PORT_STARTED, true);
    let Some(queue) = state
        .get_index(scope, PORT_QUEUE)
        .and_then(|queue| v8::Local::<v8::Array>::try_from(queue).ok())
    else {
        return;
    };
    for index in 0..queue.length() {
        if let Some(members) = queue
            .get_index(scope, index)
            .and_then(|members| v8::Local::<v8::Array>::try_from(members).ok())
        {
            schedule(scope, port.into(), members);
        }
    }
    let emptied = v8::Array::new(scope, 0);
    state.set_index(scope, PORT_QUEUE, emptied.into());
}

fn port_start<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    if let Some((port, state)) = port_of(scope, &args) {
        start(scope, port, state);
    }
}

fn port_close<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some((_, state)) = port_of(scope, &args) else {
        return;
    };
    set_flag(scope, state, PORT_CLOSED, true);
    let null: v8::Local<v8::Value> = v8::null(scope).into();
    // Disentangled: neither port sends to the other any more.
    if let Some(peer_state) = state
        .get_index(scope, PORT_PEER)
        .and_then(|peer| v8::Local::<v8::Object>::try_from(peer).ok())
        .and_then(|peer| port_state(scope, peer))
    {
        peer_state.set_index(scope, PORT_PEER, null);
    }
    state.set_index(scope, PORT_PEER, null);
}

fn port_handler_value<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    index: u32,
    rv: &mut v8::ReturnValue<'s, v8::Value>,
) {
    if let Some((_, state)) = port_of(scope, args)
        && let Some(handler) = state.get_index(scope, index)
    {
        rv.set(handler);
    }
}

fn port_onmessage<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    port_handler_value(scope, &args, PORT_ONMESSAGE, &mut rv);
}

fn port_onmessageerror<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    port_handler_value(scope, &args, PORT_ONMESSAGEERROR, &mut rv);
}

/// An event handler attribute holds a function or null.
fn handler_value<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> v8::Local<'s, v8::Value> {
    if value.is_function() {
        value
    } else {
        v8::null(scope).into()
    }
}

fn port_set_onmessage<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some((port, state)) = port_of(scope, &args) else {
        return;
    };
    let handler = handler_value(scope, args.get(0));
    state.set_index(scope, PORT_ONMESSAGE, handler);
    // The handler takes its place among the listeners the first time it is
    // set, as an event handler attribute does.
    if !flag(scope, state, PORT_LISTENING) {
        set_flag(scope, state, PORT_LISTENING, true);
        if let (Some(add), Some(listener)) = (
            realm_function(scope, REALM_ADD_LISTENER),
            realm_value(scope, REALM_PORT_HANDLER),
        ) {
            let message = key(scope, "message");
            let _ = add.call(scope, port.into(), &[message.into(), listener]);
        }
    }
    // Assigning `onmessage` starts the port.
    start(scope, port, state);
}

fn port_set_onmessageerror<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    if let Some((_, state)) = port_of(scope, &args) {
        let handler = handler_value(scope, args.get(0));
        state.set_index(scope, PORT_ONMESSAGEERROR, handler);
    }
}

/// The listener behind a port's `onmessage`: calls the handler the port has
/// now.
fn port_handler<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let port = args.this();
    let Some(state) = port_state(scope, port) else {
        return;
    };
    if let Some(handler) = state
        .get_index(scope, PORT_ONMESSAGE)
        .and_then(|handler| v8::Local::<v8::Function>::try_from(handler).ok())
    {
        let _ = handler.call(scope, port.into(), &[args.get(0)]);
    }
}

/// The task one message is delivered by.
fn deliver<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Ok(pair) = v8::Local::<v8::Object>::try_from(args.data()) else {
        return;
    };
    let (Some(target), Some(members)) = (pair.get_index(scope, 0), pair.get_index(scope, 1)) else {
        return;
    };
    let Ok(target) = v8::Local::<v8::Object>::try_from(target) else {
        return;
    };
    // A port closed since the message was posted delivers nothing.
    if let Some(state) = port_state(scope, target)
        && flag(scope, state, PORT_CLOSED)
    {
        return;
    }
    let message = key(scope, "message");
    let Some(event) = new_event(scope, message.into(), None, members) else {
        return;
    };
    let global = scope.get_current_context().global(scope);
    let dispatch = if target.strict_equals(global.into()) {
        // The window's own operation: the global is not made from the
        // `EventTarget` template.
        let name = key(scope, "dispatchEvent");
        global
            .get(scope, name.into())
            .and_then(|function| v8::Local::<v8::Function>::try_from(function).ok())
    } else {
        realm_function(scope, REALM_DISPATCH)
    };
    if let Some(dispatch) = dispatch {
        let _ = dispatch.call(scope, target.into(), &[event.into()]);
    }
}

// ---- MessageEvent ---------------------------------------------------------

/// A `MessageEvent`: a real `Event` with the message prototype in front and
/// `members` as its message members.
fn new_event<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    event_type: v8::Local<'s, v8::Value>,
    init: Option<v8::Local<'s, v8::Value>>,
    members: v8::Local<'s, v8::Value>,
) -> Option<v8::Local<'s, v8::Object>> {
    let prototype = realm_value(scope, REALM_EVENT_PROTOTYPE)?;
    let event = realm_function(scope, REALM_EVENT)?;
    let arguments = match init {
        Some(init) => vec![event_type, init],
        None => vec![event_type],
    };
    let event = event.new_instance(scope, &arguments)?;
    event.set_prototype(scope, prototype);
    let members_key = private(scope, MESSAGE);
    event.set_private(scope, members_key, members);
    Some(event)
}

fn event_constructor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    if args.new_target().is_undefined() {
        throw_type_error(
            scope,
            "Failed to construct 'MessageEvent': Please use the 'new' operator, this DOM object constructor cannot be called as a function.",
        );
        return;
    }
    if args.length() == 0 {
        throw_type_error(
            scope,
            "Failed to construct 'MessageEvent': 1 argument required, but only 0 present.",
        );
        return;
    }
    let init = args.get(1);
    let init_object = v8::Local::<v8::Object>::try_from(init).ok();
    let member = |scope: &mut v8::PinScope<'s, '_>,
                  name: &str,
                  default: v8::Local<'s, v8::Value>|
     -> Option<v8::Local<'s, v8::Value>> {
        let Some(init) = init_object else {
            return Some(default);
        };
        let name = key(scope, name);
        // `None` is an exception thrown by a getter of the dictionary.
        let value = init.get(scope, name.into())?;
        Some(if value.is_undefined() { default } else { value })
    };
    let null: v8::Local<v8::Value> = v8::null(scope).into();
    let empty: v8::Local<v8::Value> = v8::String::empty(scope).into();
    let no_ports: v8::Local<v8::Value> = v8::Array::new(scope, 0).into();
    let (Some(data), Some(origin), Some(last_event_id), Some(source), Some(ports)) = (
        member(scope, "data", null),
        member(scope, "origin", empty),
        member(scope, "lastEventId", empty),
        member(scope, "source", null),
        member(scope, "ports", no_ports),
    ) else {
        return;
    };
    let (Some(origin), Some(last_event_id)) =
        (origin.to_string(scope), last_event_id.to_string(scope))
    else {
        return;
    };
    let members = v8::Array::new_with_elements(
        scope,
        &[data, origin.into(), last_event_id.into(), source, ports],
    );
    let init = (!init.is_undefined()).then_some(init);
    if let Some(event) = new_event(scope, args.get(0), init, members.into()) {
        rv.set(event.into());
    }
}

/// One member of a message event; which is the function's data.
fn event_member<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let members_key = private(scope, MESSAGE);
    let members = args
        .this()
        .get_private(scope, members_key)
        .filter(|members| members.is_array())
        .and_then(|members| v8::Local::<v8::Object>::try_from(members).ok());
    let Some(members) = members else {
        throw_type_error(scope, "Illegal invocation");
        return;
    };
    let index = args.data().uint32_value(scope).unwrap_or(0);
    if let Some(value) = members.get_index(scope, index) {
        rv.set(value);
    }
}

// ---- window.postMessage ---------------------------------------------------

fn window_post_message<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    if args.length() == 0 {
        throw_type_error(
            scope,
            "Failed to execute 'postMessage' on 'Window': 1 argument required, but only 0 present.",
        );
        return;
    }
    let Some(data) = clone_message(scope, args.get(0)) else {
        return;
    };
    let global = scope.get_current_context().global(scope);
    let origin_key = key(scope, "origin");
    let origin = global
        .get(scope, origin_key.into())
        .filter(|origin| origin.is_string())
        .unwrap_or_else(|| v8::String::empty(scope).into());
    let members = members(scope, data, origin, global.into());
    schedule(scope, global.into(), members);
}
