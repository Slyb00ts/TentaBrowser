// Compiles the generated traits, dictionaries, enumerations and unions of both
// fixtures, then exercises them from Rust.
//
// The golden files are included the same way a feature crate includes the
// build-script output: every file of one binding module goes into one module,
// prelude first.

// The generated engine glue needs `v8` and `rquickjs`, which `tenta-idl`
// deliberately does not depend on, so the include test only runs with the
// engine features off.
#![cfg(not(any(feature = "v8", feature = "quickjs")))]

#[allow(dead_code)]
mod toy_bindings {
    include!("golden/toy/prelude.rs");
    include!("golden/toy/types.rs");
    include!("golden/toy/counter_ops.rs");
    include!("golden/toy/counter_register.rs");
    include!("golden/toy/pair_box_ops.rs");
    include!("golden/toy/pair_box_register.rs");
    include!("golden/toy/timed_counter_ops.rs");
    include!("golden/toy/timed_counter_register.rs");
    include!("golden/toy/byte_box_ops.rs");
    include!("golden/toy/byte_box_register.rs");
}

#[allow(dead_code)]
mod dom_bindings {
    include!("golden/dom/prelude.rs");
    include!("golden/dom/types.rs");
    include!("golden/dom/event_target_ops.rs");
    include!("golden/dom/event_target_register.rs");
    include!("golden/dom/event_ops.rs");
    include!("golden/dom/event_register.rs");
    include!("golden/dom/abort_signal_ops.rs");
    include!("golden/dom/abort_signal_register.rs");
    include!("golden/dom/node_ops.rs");
    include!("golden/dom/node_register.rs");
    include!("golden/dom/node_list_ops.rs");
    include!("golden/dom/node_list_register.rs");
    include!("golden/dom/css_ops.rs");
    include!("golden/dom/css_register.rs");
}

use std::cell::Cell;

use toy_bindings::{
    CounterOps, CounterOptions, CounterState, InterfaceDescriptor, InterfaceRegistry, JsError,
};

/// A host object that implements the generated trait, which is the whole point
/// of the `*Ops` layer.
#[derive(Default)]
struct Counter {
    count: Cell<u32>,
    stopped: Cell<bool>,
}

impl CounterOps for Counter {
    fn count(&self) -> u32 {
        self.count.get()
    }

    fn set_count(&self, value: u32) {
        self.count.set(value);
    }

    fn is_active(&self) -> bool {
        !self.stopped.get()
    }

    fn state(&self) -> Result<CounterState, JsError> {
        Ok(if self.stopped.get() {
            CounterState::Paused
        } else if self.count.get() == 0 {
            CounterState::Idle
        } else {
            CounterState::Running
        })
    }

    fn reset(&self) {
        self.count.set(0);
    }

    fn advance(&self, by: Option<u32>) -> Result<(), JsError> {
        let step = by.unwrap_or(1);
        let next = self
            .count
            .get()
            .checked_add(step)
            .ok_or_else(|| JsError::range_error("Counter.advance: the count would overflow"))?;
        self.count.set(next);
        Ok(())
    }

    fn stop(&self) {
        self.stopped.set(true);
    }

    fn stopped(&self) -> bool {
        self.stopped.get()
    }
}

/// Each binding module has its own prelude, so a registry implements the
/// trait of the module it collects.
#[derive(Default)]
struct Registry {
    descriptors: Vec<InterfaceDescriptor>,
}

impl InterfaceRegistry for Registry {
    fn register(&mut self, descriptor: InterfaceDescriptor) {
        self.descriptors.push(descriptor);
    }
}

#[derive(Default)]
struct DomRegistry {
    descriptors: Vec<dom_bindings::InterfaceDescriptor>,
}

impl dom_bindings::InterfaceRegistry for DomRegistry {
    fn register(&mut self, descriptor: dom_bindings::InterfaceDescriptor) {
        self.descriptors.push(descriptor);
    }
}

#[test]
fn the_generated_trait_describes_a_usable_host_object() {
    let counter = Counter::default();
    assert_eq!(counter.state().unwrap(), CounterState::Idle);

    counter.advance(None).unwrap();
    counter.advance(Some(4)).unwrap();
    assert_eq!(counter.count(), 5);
    assert_eq!(counter.state().unwrap(), CounterState::Running);

    counter.set_count(u32::MAX);
    assert!(counter.advance(Some(1)).is_err());

    counter.reset();
    assert_eq!(counter.count(), 0);
    assert!(counter.is_active());
    counter.stop();
    assert!(counter.stopped());
    assert_eq!(counter.state().unwrap(), CounterState::Paused);

    assert_eq!(<Counter as CounterOps>::MAX_COUNT, 100_000);
    assert_eq!(<Counter as CounterOps>::MIN_COUNT, 0);
}

#[test]
fn generated_enumerations_round_trip_their_idl_strings() {
    assert_eq!(
        CounterState::VALUES
            .iter()
            .map(CounterState::as_str)
            .collect::<Vec<_>>(),
        ["idle", "running", "paused"]
    );
    assert_eq!(CounterState::Running.as_str(), "running");
    assert_eq!(CounterState::parse("paused"), Some(CounterState::Paused));
    assert_eq!(CounterState::parse("Paused"), None);

    use dom_bindings::ScrollBehavior;
    assert_eq!(
        ScrollBehavior::parse("smooth"),
        Some(ScrollBehavior::Smooth)
    );
    assert_eq!(ScrollBehavior::VALUES.len(), 3);
    assert_eq!(ScrollBehavior::parse("nope"), None);
}

#[test]
fn generated_dictionaries_default_their_optional_members() {
    let options = CounterOptions::default();
    assert_eq!(options.initial_value, 0);
    assert_eq!(options.label, None);
    assert!(!options.auto_start);

    let options = CounterOptions {
        initial_value: 7,
        label: Some("ticks".to_string()),
        ..CounterOptions::default()
    };
    assert_eq!(options.initial_value, 7);
    assert_eq!(options.label.as_deref(), Some("ticks"));

    use dom_bindings::{AddEventListenerOptions, ScrollToOptions};
    // Inherited members come first and keep their own defaults.
    let listener = AddEventListenerOptions::default();
    assert!(!listener.capture);
    assert_eq!(listener.passive, None);
    assert!(!listener.once);
    assert!(listener.signal.is_none());

    let scroll = ScrollToOptions::default();
    assert_eq!(scroll.behavior, dom_bindings::ScrollBehavior::Auto);
    assert_eq!(scroll.left, None);
}

#[test]
fn generated_unions_carry_one_variant_per_idl_member() {
    use dom_bindings::{Handle, Node, NodeOrDomString};

    let text = NodeOrDomString::DomString("hello".to_string());
    match &text {
        NodeOrDomString::DomString(value) => assert_eq!(value, "hello"),
        NodeOrDomString::Node(_) => panic!("the wrong variant"),
    }

    let node: NodeOrDomString = NodeOrDomString::Node(Handle::<Node>::from_bits(1));
    assert!(matches!(node, NodeOrDomString::Node(_)));
}

#[test]
fn the_registry_receives_the_interface_description() {
    let mut registry = Registry::default();
    toy_bindings::register_counter(&mut registry);
    toy_bindings::register_timed_counter(&mut registry);

    assert_eq!(registry.descriptors.len(), 2);
    let counter = &registry.descriptors[0];
    assert_eq!(counter.name, "Counter");
    assert_eq!(counter.parent, None);
    assert_eq!(counter.constants.len(), 2);
    assert_eq!(
        counter.enumeration_order,
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

    let timed = &registry.descriptors[1];
    assert_eq!(timed.name, "TimedCounter");
    assert_eq!(timed.parent, Some("Counter"));

    let mut registry = DomRegistry::default();
    dom_bindings::register_node(&mut registry);
    let node = &registry.descriptors[0];
    assert_eq!(node.parent, Some("EventTarget"));
    assert_eq!(node.constants.len(), 18);
    assert_eq!(node.enumeration_order.len(), 65);
    assert_eq!(node.exposure, ["Window"]);
}
