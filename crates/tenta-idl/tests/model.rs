// Parser and model tests for both fixtures.

mod common;

use tenta_idl::model::{
    BufferType, DefaultValue, Exposure, FastApi, Member, MemberOrigin, NotFastBecause, PropertyKey,
    Special, Stringifier, Type,
};
use tenta_idl::{Interface, Model};

fn interface<'a>(model: &'a Model, name: &str) -> &'a Interface {
    model
        .interface(name)
        .unwrap_or_else(|| panic!("the model defines `{name}`"))
}

fn attribute<'a>(
    model: &'a Model,
    interface_name: &str,
    name: &str,
) -> &'a tenta_idl::model::Attribute {
    match interface(model, interface_name).member(name) {
        Some(Member::Attribute(attribute)) => attribute,
        _ => panic!("`{interface_name}.{name}` is an attribute"),
    }
}

fn operation<'a>(
    model: &'a Model,
    interface_name: &str,
    name: &str,
) -> &'a tenta_idl::model::OperationGroup {
    match interface(model, interface_name).member(name) {
        Some(Member::Operation(group)) => group,
        _ => panic!("`{interface_name}.{name}` is an operation"),
    }
}

#[test]
fn toy_model_has_the_expected_shape() {
    let model = common::toy();

    assert_eq!(model.interfaces.len(), 5);
    assert_eq!(model.dictionaries.len(), 1);
    assert_eq!(model.enums.len(), 1);
    assert!(model.namespaces.is_empty());
    // Both come from `PayloadBox.webidl`, the only fixture with a union.
    assert_eq!(model.unions.len(), 1);
    assert_eq!(model.unions[0].name, "BufferSourceOrUsvString");
    assert_eq!(model.typedefs.len(), 1);
    assert_eq!(model.typedefs[0].name, "PayloadSource");
    assert!(model.opaque_interfaces.is_empty());

    let counter = interface(&model, "Counter");
    assert_eq!(counter.parent, None);
    assert_eq!(counter.constants.len(), 2);
    assert_eq!(counter.members.len(), 7);
    assert_eq!(counter.constructors.len(), 1);
    assert_eq!(counter.included_mixins, ["Stoppable"]);
    assert_eq!(counter.partial_blocks, 0);
    assert_eq!(
        common::member_names(counter),
        [
            "count", "isActive", "state", "reset", "advance", "stop", "stopped"
        ]
    );

    let timed = interface(&model, "TimedCounter");
    assert_eq!(timed.parent.as_deref(), Some("Counter"));
    assert_eq!(timed.members.len(), 3);
    assert!(timed.constructors.is_empty());

    let pair_box = interface(&model, "PairBox");
    assert_eq!(pair_box.parent, None);
    assert!(pair_box.members.is_empty());
    assert!(pair_box.constructors.is_empty());
    assert_eq!(
        pair_box.indexed_getter, None,
        "the pair form must exercise the host hook, not an array-like surface"
    );
    let iterable = pair_box
        .iterable
        .as_ref()
        .expect("iterable<DOMString, DOMString>");
    assert_eq!(iterable.key, Some(Type::DomString));
    assert_eq!(iterable.value, Type::DomString);
    assert!(!iterable.is_async);
}

/// The buffer-source names are types of their own, not interfaces.
///
/// Leaving them opaque is what produced a `Handle<Uint8Array>` that compiled
/// and then failed at runtime: `opaque_interfaces` and `Type::Buffer` are the
/// two halves of that, so both are asserted here.
#[test]
fn the_buffer_source_names_lower_to_the_buffer_types() {
    let model = common::toy();
    let byte_box = interface(&model, "ByteBox");

    assert!(
        model.opaque_interfaces.is_empty(),
        "a buffer source is a native V8 type, not an undefined interface"
    );
    let mut lowered: Vec<Type> = byte_box
        .members
        .iter()
        .filter_map(|member| match member {
            Member::Attribute(attribute) => Some(attribute.ty.clone()),
            _ => None,
        })
        .collect();
    let mut expected: Vec<Type> = BufferType::ALL
        .iter()
        .map(|kind| Type::Buffer(*kind))
        .collect();
    lowered.sort_by_key(Type::idl_name);
    expected.sort_by_key(Type::idl_name);
    assert_eq!(
        lowered, expected,
        "every buffer type the fixture declares lowers to its own variant"
    );

    let decode = operation(&model, "ByteBox", "decode");
    assert_eq!(
        decode.overloads[0].arguments[0].ty,
        Type::Buffer(BufferType::BufferSource),
        "the `BufferSource` typedef reached the model as something else"
    );
}

/// A union that contains a buffer source, with the buffer arm declared first.
///
/// The order is the whole point of the fixture: the generated `from_v8` tries
/// the arms in declaration order and the string arm is the fallback, so a
/// buffer source declared after it would never reach its own arm.
#[test]
fn a_union_keeps_its_buffer_arm_in_declaration_order() {
    let model = common::toy();
    let union = model
        .unions
        .iter()
        .find(|union| union.name == "BufferSourceOrUsvString")
        .expect("the fixture declares exactly this union");
    assert_eq!(
        union
            .members
            .iter()
            .map(|member| member.ty.clone())
            .collect::<Vec<_>>(),
        [Type::Buffer(BufferType::BufferSource), Type::UsvString]
    );

    let send = operation(&model, "PayloadBox", "send");
    assert_eq!(
        send.overloads[0].arguments[0].ty,
        Type::Union("BufferSourceOrUsvString".to_string())
    );

    let bytes = operation(&model, "PayloadBox", "bytes");
    assert_eq!(
        bytes.overloads[0].return_type,
        Type::Sequence(Box::new(Type::Octet))
    );
}

#[test]
fn mixin_members_keep_their_origin() {
    let model = common::toy();
    let counter = interface(&model, "Counter");

    assert_eq!(
        counter.member("count").unwrap().origin(),
        &MemberOrigin::Interface
    );
    assert_eq!(
        counter.member("stop").unwrap().origin(),
        &MemberOrigin::Mixin("Stoppable".to_string())
    );
    assert_eq!(
        counter.member("stopped").unwrap().origin(),
        &MemberOrigin::Mixin("Stoppable".to_string())
    );
}

#[test]
fn toy_enumeration_order_is_constants_then_members_then_to_string_tag() {
    let model = common::toy();
    let counter = interface(&model, "Counter");

    assert_eq!(
        common::enumeration_order(counter),
        [
            "MAX_COUNT",
            "MIN_COUNT",
            "count",
            "isActive",
            "state",
            "reset",
            "advance",
            "stop",
            "stopped",
            "Symbol.toStringTag",
        ]
    );
    assert_eq!(
        counter.property_enumeration_order().first(),
        Some(&PropertyKey::Constant("MAX_COUNT".to_string()))
    );
    assert_eq!(
        counter.property_enumeration_order().last(),
        Some(&PropertyKey::ToStringTag)
    );
}

#[test]
fn prototype_chains_walk_to_object() {
    let toy = common::toy();
    assert_eq!(toy.prototype_chain("Counter"), ["Counter", "Object"]);
    assert_eq!(
        toy.prototype_chain("TimedCounter"),
        ["TimedCounter", "Counter", "Object"]
    );

    let dom = common::dom();
    assert_eq!(
        dom.prototype_chain("Node"),
        ["Node", "EventTarget", "Object"]
    );
    assert_eq!(
        dom.prototype_chain("AbortSignal"),
        ["AbortSignal", "EventTarget", "Object"]
    );
    assert_eq!(
        dom.prototype_chain("EventTarget"),
        ["EventTarget", "Object"]
    );
}

#[test]
fn fast_api_classification_follows_the_spec_rules() {
    let model = common::toy();

    let count = attribute(&model, "Counter", "count");
    assert_eq!(count.fast_getter, FastApi::Eligible);
    assert_eq!(count.fast_setter, Some(FastApi::Eligible));

    let state = attribute(&model, "Counter", "state");
    assert_eq!(
        state.fast_getter,
        FastApi::Ineligible(NotFastBecause::ReturnType)
    );
    assert_eq!(state.fast_setter, None);

    assert_eq!(
        operation(&model, "Counter", "reset").fast,
        FastApi::Eligible
    );
    assert_eq!(
        operation(&model, "Counter", "advance").fast,
        FastApi::Ineligible(NotFastBecause::VariableArity),
        "an optional argument gives the member two argument counts"
    );
    assert_eq!(
        operation(&model, "TimedCounter", "describe").fast,
        FastApi::Ineligible(NotFastBecause::ReturnType)
    );

    let dom = common::dom();
    assert_eq!(
        operation(&dom, "AbortSignal", "abort").fast,
        FastApi::Ineligible(NotFastBecause::Static)
    );
    assert_eq!(
        operation(&dom, "EventTarget", "dispatchEvent").fast,
        FastApi::Ineligible(NotFastBecause::ArgumentType),
        "an interface argument cannot travel over a fast call"
    );
    assert_eq!(
        operation(&dom, "Node", "compareDocumentPosition").fast,
        FastApi::Ineligible(NotFastBecause::ArgumentType)
    );
    assert_eq!(
        operation(&dom, "Node", "normalize").fast,
        FastApi::Eligible,
        "`undefined normalize()` is the simplest eligible shape"
    );
    assert_eq!(
        operation(&dom, "Node", "setOpacity").fast,
        FastApi::Eligible,
        "`[Clamp] octet` narrows without a possible TypeError"
    );
    assert_eq!(
        operation(&dom, "Node", "isDefaultNamespace").fast,
        FastApi::Ineligible(NotFastBecause::ArgumentType),
        "a nullable string is not a one-byte string"
    );
    assert_eq!(
        operation(&dom, "Node", "scroll").fast,
        FastApi::Ineligible(NotFastBecause::Overloaded)
    );
    assert_eq!(
        attribute(&dom, "Node", "nodeType").fast_getter,
        FastApi::Eligible
    );
    assert_eq!(
        attribute(&dom, "Node", "nodeName").fast_getter,
        FastApi::Ineligible(NotFastBecause::ReturnType)
    );
    assert_eq!(
        operation(&dom, "NodeList", "item").fast,
        FastApi::Ineligible(NotFastBecause::Special)
    );
}

#[test]
fn dom_model_counts_every_definition() {
    let model = common::dom();

    assert_eq!(model.interfaces.len(), 6);
    assert_eq!(model.namespaces.len(), 1);
    assert_eq!(model.dictionaries.len(), 5);
    assert_eq!(model.enums.len(), 1);
    // `EventHandlerNonNull` (HTML) plus the WebIDL standard's §3.4.8 pair.
    assert_eq!(model.callbacks.len(), 3);
    assert_eq!(model.callback_interfaces.len(), 1);
    assert_eq!(model.typedefs.len(), 4);
    assert_eq!(model.unions.len(), 3);
    assert_eq!(
        model.opaque_interfaces,
        [
            "DOMTokenList",
            "Document",
            "Element",
            "GetRootNodeOptions",
            "HTMLCollection"
        ]
    );

    let counts: Vec<(&str, usize, usize, usize)> = model
        .interfaces
        .iter()
        .map(|i| {
            (
                i.name.as_str(),
                i.constants.len(),
                i.members.len(),
                i.property_enumeration_order().len(),
            )
        })
        .collect();
    assert_eq!(
        counts,
        [
            ("EventTarget", 0, 3, 4),
            ("Event", 4, 18, 23),
            // The three statics are not own properties of the prototype, so
            // they do not take part in the prototype's enumeration order.
            ("AbortSignal", 0, 7, 5),
            ("Node", 18, 46, 65),
            ("NodeList", 0, 2, 3),
            ("Manager", 0, 2, 3),
        ]
    );

    let css = model.namespace("CSS").expect("the CSS namespace");
    assert_eq!(css.members.len(), 2);
    assert_eq!(css.partial_blocks, 1);
    assert_eq!(
        common::enumeration_order_of_namespace(css),
        ["supports", "escape", "Symbol.toStringTag"]
    );
}

#[test]
fn partial_blocks_and_mixins_merge_into_the_interface() {
    let model = common::dom();
    let node = interface(&model, "Node");

    assert_eq!(node.partial_blocks, 1);
    assert_eq!(node.included_mixins, ["ParentNode"]);
    assert_eq!(
        node.member("prepend").unwrap().origin(),
        &MemberOrigin::Mixin("ParentNode".to_string())
    );
    assert_eq!(
        node.member("scrollTop").unwrap().origin(),
        &MemberOrigin::Partial
    );
    assert_eq!(
        node.member("appendChild").unwrap().origin(),
        &MemberOrigin::Interface
    );

    // Merge order: the interface's own members in declaration order, then the
    // members of each `partial interface` block, then the included mixins.
    let names = common::member_names(node);
    let own_end = names.iter().position(|n| *n == "removeChild").unwrap();
    let partial_start = names.iter().position(|n| *n == "scroll").unwrap();
    let mixin_start = names.iter().position(|n| *n == "children").unwrap();
    assert!(
        own_end < partial_start && partial_start < mixin_start,
        "unexpected merge order: {names:?}"
    );
    // Attributes and operations keep their relative declaration order.
    let owner_document = names.iter().position(|n| *n == "ownerDocument").unwrap();
    let get_root_node = names.iter().position(|n| *n == "getRootNode").unwrap();
    let parent_node = names.iter().position(|n| *n == "parentNode").unwrap();
    assert!(owner_document < get_root_node && get_root_node < parent_node);

    assert_eq!(
        node.stringifier,
        Some(Stringifier::Operation("toString".into()))
    );
}

#[test]
fn overloads_are_grouped_and_numbered() {
    let model = common::dom();
    let scroll = operation(&model, "Node", "scroll");

    assert_eq!(scroll.overloads.len(), 2);
    assert_eq!(scroll.arity_classes(), [0, 1, 2]);
    assert_eq!(scroll.overloads[0].rust_name, "scroll_1");
    assert_eq!(scroll.overloads[1].rust_name, "scroll_2");
    assert_eq!(scroll.overloads[0].required_argument_count(), 0);
    assert_eq!(scroll.overloads[1].required_argument_count(), 2);

    let supports = model
        .namespace("CSS")
        .and_then(|ns| {
            ns.members.iter().find_map(|m| match m {
                Member::Operation(group) if group.name == "supports" => Some(group),
                _ => None,
            })
        })
        .expect("CSS.supports");
    assert_eq!(supports.overloads.len(), 2);
    assert!(supports.is_static);
    assert_eq!(supports.arity_classes(), [1, 2]);

    let append_child = operation(&model, "Node", "appendChild");
    assert_eq!(append_child.overloads.len(), 1);
    assert_eq!(append_child.overloads[0].rust_name, "append_child");
}

#[test]
fn typedefs_are_resolved_at_every_use_site() {
    let model = common::dom();

    assert_eq!(
        attribute(&model, "Event", "timeStamp").ty,
        Type::Double {
            unrestricted: false
        },
        "DOMHighResTimeStamp resolves to double"
    );

    let escape = model
        .namespace("CSS")
        .and_then(|ns| {
            ns.members.iter().find_map(|m| match m {
                Member::Operation(group) if group.name == "escape" => Some(group),
                _ => None,
            })
        })
        .expect("CSS.escape");
    assert_eq!(escape.overloads[0].return_type, Type::DomString);
    assert_eq!(escape.overloads[0].arguments[0].ty, Type::DomString);

    let onabort = attribute(&model, "AbortSignal", "onabort");
    assert_eq!(
        onabort.ty,
        Type::Nullable(Box::new(Type::Callback("EventHandlerNonNull".into()))),
        "EventHandler resolves to EventHandlerNonNull?"
    );

    let names: Vec<&str> = model.typedefs.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "DOMHighResTimeStamp",
            "CSSOMString",
            "NodeOrString",
            "EventHandler"
        ]
    );
}

#[test]
fn unions_are_interned_under_generated_names() {
    let model = common::dom();

    let names: Vec<&str> = model.unions.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "AddEventListenerOptionsOrBoolean",
            "EventListenerOptionsOrBoolean",
            "NodeOrDomString"
        ]
    );

    let node_or_string = model
        .union("NodeOrDomString")
        .expect("the NodeOrString union");
    assert_eq!(
        node_or_string
            .members
            .iter()
            .map(|m| m.rust_name.as_str())
            .collect::<Vec<_>>(),
        ["Node", "DomString"]
    );
    assert_eq!(
        node_or_string.members[0].ty,
        Type::Interface("Node".to_string())
    );
    assert_eq!(node_or_string.members[1].ty, Type::DomString);

    let typedef = model
        .typedefs
        .iter()
        .find(|t| t.name == "NodeOrString")
        .expect("the NodeOrString typedef");
    assert_eq!(typedef.ty, Type::Union("NodeOrDomString".to_string()));

    // The same union written twice is interned once.
    let prepend = operation(&model, "Node", "prepend");
    assert_eq!(
        prepend.overloads[0].arguments[0].ty,
        Type::Union("NodeOrDomString".to_string())
    );
    let append = operation(&model, "Node", "append");
    assert_eq!(
        append.overloads[0].arguments[0].ty,
        prepend.overloads[0].arguments[0].ty
    );
}

#[test]
fn argument_modifiers_are_recorded() {
    let model = common::dom();

    let prepend = &operation(&model, "Node", "prepend").overloads[0];
    assert!(prepend.arguments[0].variadic);
    assert!(!prepend.arguments[0].optional);

    let set_opacity = &operation(&model, "Node", "setOpacity").overloads[0];
    assert!(set_opacity.arguments[0].clamp());
    assert!(!set_opacity.arguments[0].enforce_range());
    assert_eq!(set_opacity.arguments[0].ty, Type::Octet);

    let timeout = &operation(&model, "AbortSignal", "timeout").overloads[0];
    assert!(timeout.arguments[0].enforce_range());
    assert_eq!(timeout.arguments[0].ty, Type::UnsignedLongLong);

    let init_event = &operation(&model, "Event", "initEvent").overloads[0];
    assert!(init_event.arguments[1].optional);
    assert_eq!(
        init_event.arguments[1].default,
        Some(DefaultValue::Boolean(false))
    );

    let get_root_node = &operation(&model, "Node", "getRootNode").overloads[0];
    assert_eq!(
        get_root_node.arguments[0].default,
        Some(DefaultValue::EmptyDictionary)
    );
    assert_eq!(
        get_root_node.arguments[0].ty,
        Type::Interface("GetRootNodeOptions".to_string()),
        "a name this compilation unit does not define stays an opaque handle"
    );
}

#[test]
fn extended_attributes_survive_lowering() {
    let model = common::dom();

    assert!(attribute(&model, "Event", "isTrusted").legacy_unforgeable());

    let class_list = attribute(&model, "Node", "classList");
    assert!(class_list.same_object());
    assert_eq!(class_list.put_forwards(), Some("value"));

    assert!(attribute(&model, "Node", "depth").replaceable());
    assert!(operation(&model, "Node", "prepend").unscopable());
    assert!(!operation(&model, "Node", "appendChild").unscopable());
    assert!(
        operation(&model, "Node", "cloneNode").overloads[0]
            .attrs
            .has("CEReactions")
    );
    assert!(
        operation(&model, "Node", "cloneNode").overloads[0]
            .attrs
            .has("NewObject")
    );
}

/// `[LegacyTreatNonObjectAsNull]` decides the callback conversion the generator
/// emits, so it has to survive the front end for the generator to see it at
/// all. The HTML Standard carries it on `EventHandlerNonNull`; the WebIDL
/// standard's §3.4.8 pair contrasts it against an unannotated callback.
#[test]
fn the_legacy_treat_non_object_as_null_annotation_survives_lowering() {
    let model = common::dom();

    assert!(
        model
            .callback("EventHandlerNonNull")
            .expect("the callback")
            .attrs
            .has("LegacyTreatNonObjectAsNull"),
        "the HTML Standard's annotation was dropped"
    );
    assert!(
        model
            .callback("ErrorHandler")
            .expect("the callback")
            .attrs
            .has("LegacyTreatNonObjectAsNull")
    );
    assert!(
        !model
            .callback("OccurrenceHandler")
            .expect("the callback")
            .attrs
            .has("LegacyTreatNonObjectAsNull"),
        "the unannotated half of the contrastive pair gained the annotation"
    );

    // The annotated callback is reached through a typedef on one attribute and
    // directly under `?` on the other; both lower to the same shape.
    assert_eq!(
        attribute(&model, "AbortSignal", "onabort").ty,
        Type::Nullable(Box::new(Type::Callback("EventHandlerNonNull".to_string())))
    );
    assert_eq!(
        attribute(&model, "Manager", "handler2").ty,
        Type::Nullable(Box::new(Type::Callback("ErrorHandler".to_string())))
    );
    assert_eq!(
        attribute(&model, "Manager", "handler1").ty,
        Type::Nullable(Box::new(Type::Callback("OccurrenceHandler".to_string())))
    );
}

#[test]
fn exposure_is_recorded_per_definition() {
    let model = common::dom();

    assert_eq!(interface(&model, "EventTarget").exposure, Exposure::All);
    assert_eq!(
        interface(&model, "AbortSignal").exposure,
        Exposure::Globals(vec!["Window".to_string(), "Worker".to_string()])
    );
    assert_eq!(
        interface(&model, "Node").exposure,
        Exposure::Globals(vec!["Window".to_string()])
    );
    assert_eq!(interface(&model, "EventTarget").exposure.names(), ["*"]);

    let toy = common::toy();
    assert_eq!(
        interface(&toy, "Counter").exposure,
        Exposure::Globals(vec!["Window".to_string(), "Worker".to_string()])
    );
}

#[test]
fn dictionaries_flatten_inherited_members_parent_first() {
    let model = common::dom();

    let options = model
        .dictionary("AddEventListenerOptions")
        .expect("AddEventListenerOptions");
    assert_eq!(options.parent.as_deref(), Some("EventListenerOptions"));
    assert_eq!(
        model
            .dictionary_members("AddEventListenerOptions")
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["capture", "passive", "once", "signal"]
    );

    let toy = common::toy();
    let counter_options = toy.dictionary("CounterOptions").expect("CounterOptions");
    assert_eq!(counter_options.members[0].name, "initialValue");
    assert!(counter_options.members[0].required);
    assert_eq!(counter_options.members[0].default, None);
    assert!(!counter_options.members[1].required);
    assert_eq!(
        counter_options.members[2].default,
        Some(DefaultValue::Boolean(false))
    );
}

#[test]
fn enums_callbacks_and_callback_interfaces_are_lowered() {
    let model = common::dom();

    let behavior = model.enumeration("ScrollBehavior").expect("ScrollBehavior");
    assert_eq!(
        behavior
            .values
            .iter()
            .map(|v| (v.value.as_str(), v.rust_name.as_str()))
            .collect::<Vec<_>>(),
        [
            ("auto", "Auto"),
            ("instant", "Instant"),
            ("smooth", "Smooth")
        ]
    );

    let handler = model.callback("EventHandlerNonNull").expect("the callback");
    assert_eq!(handler.return_type, Type::Any);
    assert_eq!(handler.arguments.len(), 1);
    assert_eq!(
        handler.arguments[0].ty,
        Type::Interface("Event".to_string())
    );

    let listener = model
        .callback_interface("EventListener")
        .expect("the callback interface");
    assert_eq!(listener.operation_name, "handleEvent");
    assert_eq!(listener.operation.return_type, Type::Undefined);
    assert_eq!(listener.operation.arguments.len(), 1);

    let toy = common::toy();
    let state = toy.enumeration("CounterState").expect("CounterState");
    assert_eq!(
        state
            .values
            .iter()
            .map(|v| v.rust_name.as_str())
            .collect::<Vec<_>>(),
        ["Idle", "Running", "Paused"]
    );
}

#[test]
fn special_operations_iterables_and_promises_are_captured() {
    let model = common::dom();

    let list = interface(&model, "NodeList");
    assert_eq!(list.indexed_getter.as_deref(), Some("item"));
    assert_eq!(list.named_getter, None);
    let iterable = list.iterable.as_ref().expect("iterable<Node>");
    assert_eq!(iterable.key, None);
    assert_eq!(iterable.value, Type::Interface("Node".to_string()));
    assert!(!iterable.is_async);
    assert_eq!(
        operation(&model, "NodeList", "item").overloads[0].special,
        Some(Special::Getter)
    );

    let request = &operation(&model, "Node", "requestFullscreen").overloads[0];
    assert_eq!(
        request.return_type,
        Type::Promise(Box::new(Type::Undefined))
    );

    let map = &operation(&model, "Node", "attributeMap").overloads[0];
    assert_eq!(
        map.return_type,
        Type::Record(Box::new(Type::DomString), Box::new(Type::DomString))
    );

    let composed = &operation(&model, "Event", "composedPath").overloads[0];
    assert_eq!(
        composed.return_type,
        Type::Sequence(Box::new(Type::Interface("EventTarget".to_string())))
    );
}

#[test]
fn static_members_are_separated_from_instance_members() {
    let model = common::dom();
    let signal = interface(&model, "AbortSignal");

    let statics: Vec<&str> = signal.static_members().map(|m| m.name()).collect();
    // WebIDL strips the leading underscore of an escaped identifier.
    assert_eq!(statics, ["abort", "timeout", "any"]);
    let instance: Vec<&str> = signal.instance_members().map(|m| m.name()).collect();
    assert_eq!(instance, ["aborted", "reason", "throwIfAborted", "onabort"]);

    let any = operation(&model, "AbortSignal", "any");
    assert_eq!(any.overloads[0].rust_name, "any");
}

#[test]
fn a_syntax_error_is_reported_rather_than_panicking() {
    let error = tenta_idl::parse_webidl("interface Broken { attribute; };")
        .expect_err("the source is not valid WebIDL");
    assert!(
        matches!(error, tenta_idl::Error::Syntax(_)),
        "expected a syntax error, got {error:?}"
    );

    let error = tenta_idl::parse_webidl("interface A : Missing {}; interface A {};")
        .expect_err("a duplicate interface is a model error");
    assert!(
        matches!(error, tenta_idl::Error::Model(_)),
        "expected a model error, got {error:?}"
    );

    let error = tenta_idl::parse_webidl("interface A { setlike<DOMString>; };")
        .expect_err("`setlike` is not generated yet");
    assert!(
        matches!(error, tenta_idl::Error::Unsupported(_)),
        "expected an unsupported-construct error, got {error:?}"
    );

    let error = tenta_idl::parse_webidl("typedef A B; typedef B A;")
        .expect_err("a typedef cycle is a model error");
    assert!(
        matches!(error, tenta_idl::Error::Model(_)),
        "expected a model error, got {error:?}"
    );
}
