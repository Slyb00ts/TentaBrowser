//! `customElements` and the HTML element constructor (HTML §4.13, "Custom
//! elements"), for autonomous custom elements.
//!
//! The registry is built the first time a page names `customElements` or
//! `CustomElementRegistry`: both are lazy data properties of the global, so a
//! realm that never asks pays for neither. Until the first `define()` the
//! document keeps no lifecycle log and no callback looks at one.
//!
//! # Where the state lives
//!
//! Definitions are script objects and are kept by script objects: the registry
//! holds a name map and a constructor map in private properties, and an
//! element's wrapper holds its definition and whether it has been told it is
//! connected. The host state keeps one root on the registry, the
//! `whenDefined()` promises still pending and the construction stack of the
//! upgrades in progress.
//!
//! # When reactions run
//!
//! HTML queues reactions and runs them when a `[CEReactions]` operation
//! returns. Here the document records what happened to elements
//! ([`tenta_dom::LifecycleRecord`]) and [`after_callback`] acts on the records
//! after every binding call, which is the same moment: before the script that
//! made the call sees its result. Elements the parser created are picked up by
//! [`flush`] when script is next entered, which is later than HTML has it (the
//! constructor may see children the parser had not reached yet) and is the one
//! ordering difference.
//!
//! `define()` itself only notes the elements it has to upgrade; they are
//! upgraded when its own call returns, by the same step. Inside `define()`
//! every wrapper the reactions made would count against the one binding
//! call's allowance; after it, each constructor's `super()` is a binding call
//! of its own, at which the world may collect.
//!
//! - A created element whose name is defined is upgraded, unless it sits in
//!   `<template>` contents. That covers `createElement()`, cloning and
//!   fragment parsing.
//! - An element that becomes connected is upgraded if it is not yet, and told
//!   `connectedCallback()`; one that stops being connected is told
//!   `disconnectedCallback()`.
//! - An attribute change on a custom element is reported through
//!   `attributeChangedCallback()` if the definition observes the name.
//!
//! An exception a constructor or a reaction throws is reported and the next
//! reaction runs. A reaction runs while the binding call that caused it is
//! still returning; if that call is itself throwing and the reaction throws
//! too, the reaction's exception replaces the call's.
//!
//! # Not implemented
//!
//! Customized built-in elements (`extends`, `is=`), form-associated custom
//! elements, `adoptedCallback()` (nodes do not change documents here),
//! `ElementInternals`, and scoped registries. Elements inside shadow trees
//! are not walked.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use tenta_core::atom::atoms;
use tenta_core::{Atom, NodeRef};
use tenta_dom::{Document, LifecycleRecord, NodeKind};
use tenta_js::bindings::host::{HostContext, SharedContext, TAG_NODE, decode, encode_node};
use tenta_js::bindings::{
    Element, Handle, JsError, Node, handle_from_v8, handle_to_v8, throw_js_error, throw_type_error,
    v8_begin_stored_value_lease, v8_end_stored_value_lease, v8_registered_slow_callback_boundary,
};

use super::observers::{get, get_private, key, set, set_private};
use super::{HostState, host_state};

guarded_callback!(guarded_illegal_constructor => illegal_constructor);
guarded_callback!(guarded_define => define);
guarded_callback!(guarded_get => get_definition);
guarded_callback!(guarded_get_name => get_name);
guarded_callback!(guarded_when_defined => when_defined);
guarded_callback!(guarded_upgrade => upgrade_tree);

/// The registry's name → definition map; its presence is the registry's brand.
const DEFINITIONS: &str = "tenta:CustomElementRegistry#definitions";
/// The registry's constructor → definition map.
const CONSTRUCTORS: &str = "tenta:CustomElementRegistry#constructors";
/// The `CustomElementRegistry` interface object, kept on the registry.
const INTERFACE: &str = "tenta:CustomElementRegistry#interface";
/// An element's definition: set once its constructor has returned it.
const DEFINITION: &str = "tenta:CustomElement#definition";
/// Set on an element whose upgrade failed, so it is not tried again.
const FAILED: &str = "tenta:CustomElement#failed";
/// Whether a custom element has been told it is connected.
const CONNECTED: &str = "tenta:CustomElement#connected";

/// The reactions a definition may have, by the name of the prototype method.
const CALLBACKS: [&str; 4] = [
    "connectedCallback",
    "disconnectedCallback",
    "adoptedCallback",
    "attributeChangedCallback",
];

/// Names HTML reserves: they contain a hyphen and are not custom elements.
const RESERVED: [&str; 8] = [
    "annotation-xml",
    "color-profile",
    "font-face",
    "font-face-src",
    "font-face-uri",
    "font-face-format",
    "font-face-name",
    "missing-glyph",
];

/// One upgrade in progress: the definition being constructed and the element
/// its `super()` call returns, or `None` once it has returned it.
struct Construction {
    definition: v8::Global<v8::Object>,
    element: Option<v8::Global<v8::Object>>,
}

/// The realm's custom element state outside the script heap.
pub(crate) struct State {
    registry: RefCell<Option<v8::Global<v8::Object>>>,
    /// HTML's "element definition is running" flag.
    defining: Cell<bool>,
    construction: RefCell<Vec<Construction>>,
    /// The `whenDefined()` promises of names not defined yet.
    waiting: RefCell<HashMap<String, v8::Global<v8::PromiseResolver>>>,
    /// The elements a `define()` found in the document, with its definition:
    /// upgraded when that call returns.
    pending: RefCell<Vec<(NodeRef, v8::Global<v8::Object>)>>,
    context: RefCell<Weak<HostContext>>,
}

impl State {
    pub(crate) fn new() -> Self {
        State {
            registry: RefCell::new(None),
            defining: Cell::new(false),
            construction: RefCell::new(Vec::new()),
            waiting: RefCell::new(HashMap::new()),
            pending: RefCell::new(Vec::new()),
            context: RefCell::new(Weak::new()),
        }
    }

    /// Points the registry at the page's DOM context.
    pub(crate) fn set_context(&self, context: &SharedContext) {
        *self.context.borrow_mut() = Rc::downgrade(context);
    }

    /// Drops every root, before the isolate is disposed.
    pub(crate) fn clear(&self) {
        self.registry.borrow_mut().take();
        self.construction.borrow_mut().clear();
        self.waiting.borrow_mut().clear();
        self.pending.borrow_mut().clear();
        *self.context.borrow_mut() = Weak::new();
    }

    fn context(&self) -> Option<SharedContext> {
        self.context.borrow().upgrade()
    }

    /// Whether a `define()` left elements to upgrade.
    pub(crate) fn has_pending_upgrades(&self) -> bool {
        !self.pending.borrow().is_empty()
    }
}

/// Defines `customElements` and `CustomElementRegistry` on `global`, each
/// built on first use.
pub(super) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    let undefined = v8::undefined(scope);
    let registry = key(scope, "customElements");
    global.set_lazy_data_property_with_data(
        scope,
        registry.into(),
        lazy_custom_elements,
        undefined.into(),
        v8::PropertyAttribute::DONT_ENUM,
        v8::SideEffectType::HasNoSideEffect,
        v8::SideEffectType::HasSideEffect,
    );
    let interface = key(scope, "CustomElementRegistry");
    global.set_lazy_data_property_with_data(
        scope,
        interface.into(),
        lazy_interface,
        undefined.into(),
        v8::PropertyAttribute::DONT_ENUM,
        v8::SideEffectType::HasNoSideEffect,
        v8::SideEffectType::HasSideEffect,
    );
}

fn lazy_custom_elements<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _name: v8::Local<'s, v8::Name>,
    _args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'_, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        if let Some(registry) = registry(scope) {
            rv.set(registry.into());
        }
    };
    // SAFETY: a registered V8 property callback, where allocation is allowed.
    unsafe { v8_registered_slow_callback_boundary(scope, "lazy_custom_elements", (), call) };
}

fn lazy_interface<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _name: v8::Local<'s, v8::Name>,
    _args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'_, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        if let Some(registry) = registry(scope)
            && let Some(interface) = get_private(scope, registry, INTERFACE)
        {
            rv.set(interface);
        }
    };
    // SAFETY: a registered V8 property callback, where allocation is allowed.
    unsafe {
        v8_registered_slow_callback_boundary(scope, "lazy_custom_element_registry", (), call)
    };
}

/// The realm's registry, built on first use.
fn registry<'s>(scope: &mut v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Object>> {
    let host = host_state(scope)?;
    if let Some(existing) = host.custom_elements.registry.borrow().as_ref() {
        return Some(v8::Local::new(scope, existing));
    }
    let constructor: v8::FunctionCallback = v8::MapFnTo::map_fn_to(guarded_illegal_constructor);
    let template = v8::FunctionTemplate::builder_raw(constructor)
        .length(0)
        .build(scope);
    let class_name = key(scope, "CustomElementRegistry");
    template.set_class_name(class_name);
    let prototype = template.prototype_template(scope);
    let methods: [(&str, v8::FunctionCallback, i32); 5] = [
        ("define", v8::MapFnTo::map_fn_to(guarded_define), 2),
        ("get", v8::MapFnTo::map_fn_to(guarded_get), 1),
        ("getName", v8::MapFnTo::map_fn_to(guarded_get_name), 1),
        (
            "whenDefined",
            v8::MapFnTo::map_fn_to(guarded_when_defined),
            1,
        ),
        ("upgrade", v8::MapFnTo::map_fn_to(guarded_upgrade), 1),
    ];
    for (member, callback, length) in methods {
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
    let interface = template.get_function(scope)?;
    let prototype = get(scope, interface.into(), "prototype")?;
    // The interface has no constructor, so its one instance is made here.
    let registry = v8::Object::new(scope);
    registry.set_prototype(scope, prototype)?;
    for name in [DEFINITIONS, CONSTRUCTORS] {
        let map = v8::Map::new(scope);
        set_private(scope, registry, name, map.into());
    }
    set_private(scope, registry, INTERFACE, interface.into());
    *host.custom_elements.registry.borrow_mut() = Some(v8::Global::new(scope, registry));
    Some(registry)
}

fn illegal_constructor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    throw_type_error(scope, "Illegal constructor");
}

/// The registry `this` is, or `None` after throwing.
fn this_registry<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
) -> Option<v8::Local<'s, v8::Object>> {
    let this = args.this();
    if get_private(scope, this, DEFINITIONS).is_some() {
        return Some(this);
    }
    throw_type_error(scope, "Illegal invocation");
    None
}

fn map<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    registry: v8::Local<v8::Object>,
    name: &str,
) -> Option<v8::Local<'s, v8::Map>> {
    let value = get_private(scope, registry, name)?;
    v8::Local::<v8::Map>::try_from(value).ok()
}

/// The definition `key` names in one of the registry's maps.
fn lookup<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    registry: v8::Local<v8::Object>,
    map_name: &str,
    key: v8::Local<v8::Value>,
) -> Option<v8::Local<'s, v8::Object>> {
    let map = map(scope, registry, map_name)?;
    let value = map.get(scope, key)?;
    v8::Local::<v8::Object>::try_from(value).ok()
}

fn definition_named<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    registry: v8::Local<v8::Object>,
    name: &str,
) -> Option<v8::Local<'s, v8::Object>> {
    let name = key(scope, name);
    lookup(scope, registry, DEFINITIONS, name.into())
}

/// Whether `name` is a valid custom element name (HTML's
/// `PotentialCustomElementName`, less the reserved names).
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    if !chars.next().is_some_and(|first| first.is_ascii_lowercase()) {
        return false;
    }
    let allowed = |c: char| {
        matches!(c,
            '-' | '.' | '_' | '0'..='9' | 'a'..='z' | '\u{B7}'
            | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{37D}'
            | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{203F}'..='\u{2040}'
            | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
    };
    name.contains('-') && chars.all(allowed) && !RESERVED.contains(&name)
}

fn throw_dom(scope: &mut v8::PinScope, name: &str, message: &str) {
    throw_js_error(scope, &JsError::dom_exception(name, message));
}

/// `customElements.define(name, constructor, options)`.
fn define<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    const PREFIX: &str = "Failed to execute 'define' on 'CustomElementRegistry'";
    let Some(registry) = this_registry(scope, &args) else {
        return;
    };
    let Some(host) = host_state(scope) else {
        return;
    };
    let Some(context) = host.custom_elements.context() else {
        return;
    };
    if args.length() < 2 {
        throw_type_error(
            scope,
            &format!(
                "{PREFIX}: 2 arguments required, but only {} present.",
                args.length()
            ),
        );
        return;
    }
    let Some(name) = args.get(0).to_string(scope) else {
        return;
    };
    let name_text = name.to_rust_string_lossy(scope);
    let constructor = match v8::Local::<v8::Function>::try_from(args.get(1)) {
        Ok(constructor) if constructor.is_constructor() => constructor,
        _ => {
            throw_type_error(
                scope,
                &format!("{PREFIX}: parameter 2 is not a constructor."),
            );
            return;
        }
    };
    if !valid_name(&name_text) {
        throw_dom(
            scope,
            "SyntaxError",
            &format!("{PREFIX}: \"{name_text}\" is not a valid custom element name"),
        );
        return;
    }
    if lookup(scope, registry, DEFINITIONS, name.into()).is_some() {
        throw_dom(
            scope,
            "NotSupportedError",
            &format!("{PREFIX}: the name \"{name_text}\" has already been used with this registry"),
        );
        return;
    }
    if lookup(scope, registry, CONSTRUCTORS, constructor.into()).is_some() {
        throw_dom(
            scope,
            "NotSupportedError",
            &format!("{PREFIX}: this constructor has already been used with this registry"),
        );
        return;
    }
    if let Ok(options) = v8::Local::<v8::Object>::try_from(args.get(2)) {
        let Some(extends) = get(scope, options, "extends") else {
            return;
        };
        if !extends.is_null_or_undefined() {
            throw_dom(
                scope,
                "NotSupportedError",
                &format!("{PREFIX}: customized built-in elements are not supported"),
            );
            return;
        }
    }
    if host.custom_elements.defining.replace(true) {
        throw_dom(
            scope,
            "NotSupportedError",
            &format!("{PREFIX}: an element definition is already running"),
        );
        return;
    }
    let definition = read_definition(scope, name, constructor);
    host.custom_elements.defining.set(false);
    let Some(definition) = definition else {
        return;
    };
    let (Some(definitions), Some(constructors)) = (
        map(scope, registry, DEFINITIONS),
        map(scope, registry, CONSTRUCTORS),
    ) else {
        return;
    };
    definitions.set(scope, name.into(), definition.into());
    constructors.set(scope, constructor.into(), definition.into());

    // From here on the document reports what happens to its elements.
    context.doc.borrow_mut().enable_lifecycle_log();
    context.documents.set_custom_elements_used();

    // The elements of this name that are already in the document are
    // upgraded when this call returns (`after_callback`).
    let local = Atom::from(name_text.as_str());
    let candidates: Vec<NodeRef> = {
        let doc = context.doc.borrow();
        let root = doc.document_node();
        let mut found = Vec::new();
        let mut node = root;
        while node.is_some() {
            if doc.kind(node) == NodeKind::Element
                && doc.namespace(node) == Some(atoms::NS_HTML)
                && doc.tag(node) == Some(local)
            {
                found.push(doc.node_ref(node));
            }
            node = doc.next_in_subtree(node, root);
        }
        found
    };
    if !candidates.is_empty() {
        let definition = v8::Global::new(scope, definition);
        host.custom_elements.pending.borrow_mut().extend(
            candidates
                .into_iter()
                .map(|candidate| (candidate, definition.clone())),
        );
    }

    // Settle `whenDefined(name)`.
    let waiting = host.custom_elements.waiting.borrow_mut().remove(&name_text);
    if let Some(resolver) = waiting {
        let resolver = v8::Local::new(scope, &resolver);
        resolver.resolve(scope, constructor.into());
    }
}

/// Reads what a definition needs off `constructor`, as HTML's `define()`
/// steps do while the "element definition is running" flag is set. `None`
/// after throwing.
fn read_definition<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: v8::Local<'s, v8::String>,
    constructor: v8::Local<'s, v8::Function>,
) -> Option<v8::Local<'s, v8::Object>> {
    const PREFIX: &str = "Failed to execute 'define' on 'CustomElementRegistry'";
    let prototype = get(scope, constructor.into(), "prototype")?;
    let Ok(prototype) = v8::Local::<v8::Object>::try_from(prototype) else {
        throw_type_error(
            scope,
            &format!("{PREFIX}: the constructor's prototype is not an object."),
        );
        return None;
    };
    let definition = v8::Object::new(scope);
    set(scope, definition, "name", name.into());
    set(scope, definition, "constructor", constructor.into());
    let mut observes = false;
    for callback in CALLBACKS {
        let value = get(scope, prototype, callback)?;
        if !value.is_undefined() && !value.is_function() {
            throw_type_error(
                scope,
                &format!("{PREFIX}: the '{callback}' member is not a function."),
            );
            return None;
        }
        observes |= callback == "attributeChangedCallback" && value.is_function();
        set(scope, definition, callback, value);
    }
    let observed = v8::Map::new(scope);
    if observes {
        let list = get(scope, constructor.into(), "observedAttributes")?;
        if !list.is_undefined() {
            let Some(names) = sequence(scope, list) else {
                if !scope.is_execution_terminating() {
                    throw_type_error(
                        scope,
                        &format!("{PREFIX}: 'observedAttributes' is not a sequence."),
                    );
                }
                return None;
            };
            let yes = v8::Boolean::new(scope, true);
            for index in 0..names.length() {
                let attribute = names.get_index(scope, index)?.to_string(scope)?;
                observed.set(scope, attribute.into(), yes.into());
            }
        }
    }
    set(scope, definition, "observed", observed.into());
    Some(definition)
}

/// `value` as an array: itself if it is one, or what `Array.from` makes of
/// an iterable. `None` if it is neither or the iteration threw.
fn sequence<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Option<v8::Local<'s, v8::Array>> {
    if let Ok(array) = v8::Local::<v8::Array>::try_from(value) {
        return Some(array);
    }
    if !value.is_object() {
        return None;
    }
    let global = scope.get_current_context().global(scope);
    let array = get(scope, global, "Array")?;
    let array = v8::Local::<v8::Object>::try_from(array).ok()?;
    let from = get(scope, array, "from")?;
    let from = v8::Local::<v8::Function>::try_from(from).ok()?;
    v8::tc_scope!(let tc, scope);
    let made = from.call(tc, array.into(), &[value]);
    if made.is_none() {
        tc.reset();
        return None;
    }
    made.and_then(|made| v8::Local::<v8::Array>::try_from(made).ok())
}

/// `customElements.get(name)`.
fn get_definition<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(registry) = this_registry(scope, &args) else {
        return;
    };
    let Some(name) = args.get(0).to_string(scope) else {
        return;
    };
    if let Some(definition) = lookup(scope, registry, DEFINITIONS, name.into())
        && let Some(constructor) = get(scope, definition, "constructor")
    {
        rv.set(constructor);
    }
}

/// `customElements.getName(constructor)`.
fn get_name<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(registry) = this_registry(scope, &args) else {
        return;
    };
    rv.set_null();
    if let Some(definition) = lookup(scope, registry, CONSTRUCTORS, args.get(0))
        && let Some(name) = get(scope, definition, "name")
    {
        rv.set(name);
    }
}

/// `customElements.whenDefined(name)`.
fn when_defined<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(registry) = this_registry(scope, &args) else {
        return;
    };
    let Some(name) = args.get(0).to_string(scope) else {
        return;
    };
    let name_text = name.to_rust_string_lossy(scope);
    let Some(host) = host_state(scope) else {
        return;
    };
    let existing = host
        .custom_elements
        .waiting
        .borrow()
        .get(&name_text)
        .map(|resolver| v8::Local::new(scope, resolver));
    if let Some(resolver) = existing {
        rv.set(resolver.get_promise(scope).into());
        return;
    }
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        return;
    };
    rv.set(resolver.get_promise(scope).into());
    if !valid_name(&name_text) {
        // The rejection reason is the exception `define()` would throw.
        v8::tc_scope!(let tc, scope);
        throw_dom(
            tc,
            "SyntaxError",
            &format!(
                "Failed to execute 'whenDefined' on 'CustomElementRegistry': \"{name_text}\" is not a valid custom element name"
            ),
        );
        if let Some(reason) = tc.exception() {
            tc.reset();
            resolver.reject(tc, reason);
        }
        return;
    }
    if let Some(definition) = lookup(scope, registry, DEFINITIONS, name.into()) {
        if let Some(constructor) = get(scope, definition, "constructor") {
            resolver.resolve(scope, constructor);
        }
        return;
    }
    host.custom_elements
        .waiting
        .borrow_mut()
        .insert(name_text, v8::Global::new(scope, resolver));
}

/// `customElements.upgrade(root)`: upgrades `root` and its descendants whose
/// names are defined, connected or not.
fn upgrade_tree<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(registry) = this_registry(scope, &args) else {
        return;
    };
    let Some(host) = host_state(scope) else {
        return;
    };
    let Some(context) = host.custom_elements.context() else {
        return;
    };
    let root = match handle_from_v8::<Node>(scope, args.get(0)) {
        Ok(root) => decode(root.bits()),
        Err(_) => {
            throw_type_error(
                scope,
                "Failed to execute 'upgrade' on 'CustomElementRegistry': parameter 1 is not of type 'Node'.",
            );
            return;
        }
    };
    if root.tag != TAG_NODE || root.slot != context.slot {
        return;
    }
    let candidates = {
        let doc = context.doc.borrow();
        if !doc.is_live(root.node_id) {
            return;
        }
        candidates_under(&doc, doc.node_ref(root.node_id))
    };
    for (candidate, name) in candidates {
        if let Some(definition) = definition_named(scope, registry, name.as_str()) {
            upgrade(scope, &host, &context, candidate, definition);
        }
    }
}

/// The HTML elements at and under `root` whose names contain a hyphen, in
/// tree order, with their names. Empty when `root` is no longer the node the
/// reference was made for.
fn candidates_under(doc: &Document, root: NodeRef) -> Vec<(NodeRef, Atom)> {
    let mut found = Vec::new();
    if !doc.is_live(root.id) || doc.node_ref(root.id) != root {
        return found;
    }
    let mut node = root.id;
    while node.is_some() {
        if doc.kind(node) == NodeKind::Element
            && doc.namespace(node) == Some(atoms::NS_HTML)
            && let Some(tag) = doc.tag(node)
            && tag.as_str().contains('-')
        {
            found.push((doc.node_ref(node), tag));
        }
        node = doc.next_in_subtree(node, root.id);
    }
    found
}

/// The wrapper of the element `node` names, if it is still that element.
fn wrapper_of<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    context: &SharedContext,
    node: NodeRef,
) -> Option<v8::Local<'s, v8::Object>> {
    {
        let doc = context.doc.borrow();
        if !doc.is_live(node.id) || doc.node_ref(node.id) != node {
            return None;
        }
    }
    let handle = Handle::<Element>::from_bits(encode_node(context.slot, node));
    let wrapper = handle_to_v8(scope, handle).ok()?;
    v8::Local::<v8::Object>::try_from(wrapper).ok()
}

/// Calls the reaction `name` of `definition` on `element`, reporting what it
/// throws.
fn react(
    scope: &mut v8::PinScope,
    host: &HostState,
    definition: v8::Local<v8::Object>,
    name: &str,
    element: v8::Local<v8::Object>,
    arguments: &[v8::Local<v8::Value>],
) {
    let Some(callback) = get(scope, definition, name) else {
        return;
    };
    let Ok(callback) = v8::Local::<v8::Function>::try_from(callback) else {
        return;
    };
    v8::tc_scope!(let tc, scope);
    host.enter_script();
    if callback.call(tc, element.into(), arguments).is_none() {
        crate::inspector::report_caught(tc, host);
        tc.reset();
    }
    host.exit_script();
}

/// Upgrades the element `node` names with `definition` (HTML "upgrade an
/// element"), unless it is already custom, failed before, is gone or sits in
/// `<template>` contents.
fn upgrade(
    scope: &mut v8::PinScope,
    host: &Rc<HostState>,
    context: &SharedContext,
    node: NodeRef,
    definition: v8::Local<v8::Object>,
) {
    if context.doc.borrow().in_template_contents(node.id) {
        return;
    }
    v8::scope!(let scope, scope);
    let Some(element) = wrapper_of(scope, context, node) else {
        return;
    };
    if get_private(scope, element, DEFINITION).is_some()
        || get_private(scope, element, FAILED).is_some()
    {
        return;
    }
    let Some(constructor) = get(scope, definition, "constructor")
        .and_then(|constructor| v8::Local::<v8::Function>::try_from(constructor).ok())
    else {
        return;
    };
    // The attributes the element has now are reported after construction.
    let attributes: Vec<(Atom, String)> = context
        .doc
        .borrow()
        .attributes(node.id)
        .filter(|(name, _)| name.ns == atoms::EMPTY)
        .map(|(name, value)| (name.local, value.to_owned()))
        .collect();

    host.custom_elements
        .construction
        .borrow_mut()
        .push(Construction {
            definition: v8::Global::new(scope, definition),
            element: Some(v8::Global::new(scope, element)),
        });
    let constructed = {
        v8::tc_scope!(let tc, scope);
        host.enter_script();
        let result = constructor.new_instance(tc, &[]);
        let constructed = match result {
            Some(result) if result.strict_equals(element.into()) => true,
            Some(_) => {
                throw_type_error(
                    tc,
                    "The custom element constructor did not produce the element being upgraded.",
                );
                crate::inspector::report_caught(tc, host);
                tc.reset();
                false
            }
            None => {
                crate::inspector::report_caught(tc, host);
                tc.reset();
                false
            }
        };
        host.exit_script();
        constructed
    };
    host.custom_elements.construction.borrow_mut().pop();
    let yes = v8::Boolean::new(scope, true);
    if !constructed {
        set_private(scope, element, FAILED, yes.into());
        return;
    }
    set_private(scope, element, DEFINITION, definition.into());

    let observed = get(scope, definition, "observed")
        .and_then(|observed| v8::Local::<v8::Map>::try_from(observed).ok());
    if let Some(observed) = observed {
        for (name, value) in attributes {
            let name = key(scope, name.as_str());
            if !observed.has(scope, name.into()).unwrap_or(false) {
                continue;
            }
            let Some(value) = v8::String::new(scope, &value) else {
                continue;
            };
            let null = v8::null(scope);
            react(
                scope,
                host,
                definition,
                "attributeChangedCallback",
                element,
                &[name.into(), null.into(), value.into(), null.into()],
            );
        }
    }
    let connected = {
        let doc = context.doc.borrow();
        doc.is_live(node.id) && doc.node_ref(node.id) == node && doc.is_connected(node.id)
    };
    if connected {
        set_private(scope, element, CONNECTED, yes.into());
        react(scope, host, definition, "connectedCallback", element, &[]);
    }
}

/// The HTML element constructor: what `new` on `HTMLElement` and on a class
/// that extends it returns. Returns whether the call was handled, by setting
/// the return value or by throwing.
pub(crate) fn html_constructor<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    rv: &mut v8::ReturnValue<v8::Value>,
) -> bool {
    let Some(host) = host_state(scope) else {
        return false;
    };
    let registry = host
        .custom_elements
        .registry
        .borrow()
        .as_ref()
        .map(|registry| v8::Local::new(scope, registry));
    let Some(registry) = registry else {
        return false;
    };
    if !args.is_construct_call() {
        throw_type_error(
            scope,
            "Failed to construct 'HTMLElement': Please use the 'new' operator, this DOM object constructor cannot be called as a function.",
        );
        return true;
    }
    let new_target = args.new_target();
    let Some(definition) = lookup(scope, registry, CONSTRUCTORS, new_target) else {
        return false;
    };
    // The prototype the element gets is the one `new.target` has now.
    let Ok(new_target) = v8::Local::<v8::Object>::try_from(new_target) else {
        return false;
    };
    let Some(prototype) = get(scope, new_target, "prototype") else {
        return true;
    };
    if !prototype.is_object() {
        throw_type_error(
            scope,
            "Failed to construct 'HTMLElement': the constructor's prototype is not an object.",
        );
        return true;
    }

    // An upgrade in progress for this definition hands over its element.
    let pending = {
        let construction = host.custom_elements.construction.borrow();
        construction.iter().rposition(|entry| {
            v8::Local::new(scope, &entry.definition).strict_equals(definition.into())
        })
    };
    if let Some(index) = pending {
        let element = host.custom_elements.construction.borrow_mut()[index]
            .element
            .take();
        let Some(element) = element else {
            throw_type_error(
                scope,
                "Failed to construct 'HTMLElement': this instance is already constructed.",
            );
            return true;
        };
        let element = v8::Local::new(scope, &element);
        if element.set_prototype(scope, prototype).is_none() {
            return true;
        }
        rv.set(element.into());
        return true;
    }

    // `new` on the class itself: a new element of the definition's name.
    let Some(context) = host.custom_elements.context() else {
        return false;
    };
    let Some(name) = get(scope, definition, "name") else {
        return true;
    };
    let name = name.to_rust_string_lossy(scope);
    let node = {
        let mut doc = context.doc.borrow_mut();
        let node = doc.create_html_element(Atom::from(name.as_str()));
        doc.node_ref(node)
    };
    let Some(element) = wrapper_of(scope, &context, node) else {
        throw_type_error(scope, "Failed to construct 'HTMLElement': no wrapper.");
        return true;
    };
    if element.set_prototype(scope, prototype).is_none() {
        return true;
    }
    set_private(scope, element, DEFINITION, definition.into());
    rv.set(element.into());
    true
}

/// Acts on what a `define()` left to upgrade and on the lifecycle records the
/// document has made, if no document borrow is outstanding. Called after
/// every binding call once the page has defined a custom element.
pub(crate) fn after_callback(scope: &mut v8::PinScope, context: &SharedContext) {
    let Ok(records) = context
        .doc
        .try_borrow_mut()
        .map(|doc| doc.has_lifecycle_records())
    else {
        return;
    };
    if records || host_state(scope).is_some_and(|host| host.custom_elements.has_pending_upgrades())
    {
        flush(scope);
    }
}

/// Acts on what a `define()` left to upgrade and on the lifecycle records the
/// document has made. Called after a binding call, and where script is
/// entered for the elements the parser created since the last entry.
pub(crate) fn flush(scope: &mut v8::PinScope) {
    let Some(host) = host_state(scope) else {
        return;
    };
    let registry = host
        .custom_elements
        .registry
        .borrow()
        .as_ref()
        .map(|registry| v8::Local::new(scope, registry));
    let (Some(registry), Some(context)) = (registry, host.custom_elements.context()) else {
        return;
    };
    // A reaction can make more records; they are taken by the next round, or
    // by the call that made them when it returns.
    loop {
        let records = match context.doc.try_borrow_mut() {
            Ok(mut doc) => doc.take_lifecycle_log(),
            Err(_) => return,
        };
        let upgrades = std::mem::take(&mut *host.custom_elements.pending.borrow_mut());
        if records.is_empty() && upgrades.is_empty() {
            return;
        }
        for (node, definition) in upgrades {
            if scope.is_execution_terminating() {
                return;
            }
            v8::scope!(let scope, scope);
            let definition = v8::Local::new(scope, &definition);
            upgrade(scope, &host, &context, node, definition);
        }
        for record in records {
            if scope.is_execution_terminating() {
                return;
            }
            v8::scope!(let scope, scope);
            apply(scope, &host, &context, registry, record);
        }
    }
}

fn apply(
    scope: &mut v8::PinScope,
    host: &Rc<HostState>,
    context: &SharedContext,
    registry: v8::Local<v8::Object>,
    record: LifecycleRecord,
) {
    match record {
        LifecycleRecord::Created(node) => {
            let name = {
                let doc = context.doc.borrow();
                if !doc.is_live(node.id) || doc.node_ref(node.id) != node {
                    return;
                }
                doc.tag(node.id)
            };
            if let Some(name) = name
                && let Some(definition) = definition_named(scope, registry, name.as_str())
            {
                upgrade(scope, host, context, node, definition);
            }
        }
        LifecycleRecord::Connected(root) => {
            let candidates = candidates_under(&context.doc.borrow(), root);
            for (node, name) in candidates {
                let Some(definition) = definition_named(scope, registry, name.as_str()) else {
                    continue;
                };
                if scope.is_execution_terminating() {
                    return;
                }
                v8::scope!(let scope, scope);
                let connected = {
                    let doc = context.doc.borrow();
                    doc.is_live(node.id)
                        && doc.node_ref(node.id) == node
                        && doc.is_connected(node.id)
                };
                if !connected {
                    continue;
                }
                let Some(element) = wrapper_of(scope, context, node) else {
                    continue;
                };
                let Some(own) = definition_of(scope, element) else {
                    // Not custom yet: upgrading tells it that it is connected.
                    upgrade(scope, host, context, node, definition);
                    continue;
                };
                if get_private(scope, element, CONNECTED).is_some_and(|flag| flag.is_true()) {
                    continue;
                }
                let yes = v8::Boolean::new(scope, true);
                set_private(scope, element, CONNECTED, yes.into());
                react(scope, host, own, "connectedCallback", element, &[]);
            }
        }
        LifecycleRecord::Disconnected(root) => {
            let candidates = candidates_under(&context.doc.borrow(), root);
            for (node, name) in candidates {
                if definition_named(scope, registry, name.as_str()).is_none() {
                    continue;
                }
                if scope.is_execution_terminating() {
                    return;
                }
                v8::scope!(let scope, scope);
                let Some(element) = wrapper_of(scope, context, node) else {
                    continue;
                };
                let Some(own) = definition_of(scope, element) else {
                    continue;
                };
                if !get_private(scope, element, CONNECTED).is_some_and(|flag| flag.is_true()) {
                    continue;
                }
                let no = v8::Boolean::new(scope, false);
                set_private(scope, element, CONNECTED, no.into());
                react(scope, host, own, "disconnectedCallback", element, &[]);
            }
        }
        LifecycleRecord::Attribute {
            element: node,
            name,
            namespace,
            old_value,
        } => {
            let tag = {
                let doc = context.doc.borrow();
                if !doc.is_live(node.id) || doc.node_ref(node.id) != node {
                    return;
                }
                doc.tag(node.id)
            };
            if !tag.is_some_and(|tag| definition_named(scope, registry, tag.as_str()).is_some()) {
                return;
            }
            let Some(element) = wrapper_of(scope, context, node) else {
                return;
            };
            let Some(own) = definition_of(scope, element) else {
                return;
            };
            let attribute = key(scope, name.as_str());
            let observes = get(scope, own, "observed")
                .and_then(|observed| v8::Local::<v8::Map>::try_from(observed).ok())
                .and_then(|observed| observed.has(scope, attribute.into()))
                .unwrap_or(false);
            if !observes {
                return;
            }
            let new_value = context
                .doc
                .borrow()
                .attributes(node.id)
                .find(|(candidate, _)| candidate.local == name && candidate.ns == namespace)
                .map(|(_, value)| value.to_owned());
            let old_value = optional_string(scope, old_value.as_deref());
            let new_value = optional_string(scope, new_value.as_deref());
            let namespace = optional_string(
                scope,
                (namespace != atoms::EMPTY).then(|| namespace.as_str()),
            );
            react(
                scope,
                host,
                own,
                "attributeChangedCallback",
                element,
                &[attribute.into(), old_value, new_value, namespace],
            );
        }
    }
}

/// The definition an element was constructed with, if it is custom.
fn definition_of<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    element: v8::Local<v8::Object>,
) -> Option<v8::Local<'s, v8::Object>> {
    let definition = get_private(scope, element, DEFINITION)?;
    v8::Local::<v8::Object>::try_from(definition).ok()
}

/// `text` as a string, or `null`.
fn optional_string<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    text: Option<&str>,
) -> v8::Local<'s, v8::Value> {
    match text.and_then(|text| v8::String::new(scope, text)) {
        Some(text) => text.into(),
        None => v8::null(scope).into(),
    }
}
