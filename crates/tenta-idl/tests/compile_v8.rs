// Type-checks the generated V8 engine glue against the pinned rusty_v8 version.
//
// The plain `compile` test builds the engine-independent output (ops, types,
// register) with the engine features off. This test turns the `v8` feature on,
// which pulls the `v8` crate and includes each binding module's `*_v8.rs` glue
// alongside its prelude, so any drift between the generator and the rusty_v8 API
// surfaces here as a build error rather than only when a feature crate first
// wires up a real `WrapperWorld`.
//
// It is a compile-only guard: the generic `install_*`/getter/setter/method
// functions are checked at definition, which is where an API mismatch on a
// `v8::` type shows up. No isolate is created and nothing is run.

#![cfg(feature = "v8")]

#[allow(dead_code)]
mod toy_bindings {
    include!("golden/toy/prelude.rs");
    include!("golden/toy/types.rs");
    include!("golden/toy/counter_ops.rs");
    include!("golden/toy/counter_register.rs");
    include!("golden/toy/counter_v8.rs");
    include!("golden/toy/pair_box_ops.rs");
    include!("golden/toy/pair_box_register.rs");
    include!("golden/toy/pair_box_v8.rs");
    include!("golden/toy/timed_counter_ops.rs");
    include!("golden/toy/timed_counter_register.rs");
    include!("golden/toy/timed_counter_v8.rs");
    include!("golden/toy/byte_box_ops.rs");
    include!("golden/toy/byte_box_register.rs");
    include!("golden/toy/byte_box_v8.rs");
}

#[allow(dead_code)]
mod dom_bindings {
    include!("golden/dom/prelude.rs");
    include!("golden/dom/types.rs");
    include!("golden/dom/event_target_ops.rs");
    include!("golden/dom/event_target_register.rs");
    include!("golden/dom/event_target_v8.rs");
    include!("golden/dom/event_ops.rs");
    include!("golden/dom/event_register.rs");
    include!("golden/dom/event_v8.rs");
    include!("golden/dom/abort_signal_ops.rs");
    include!("golden/dom/abort_signal_register.rs");
    include!("golden/dom/abort_signal_v8.rs");
    include!("golden/dom/node_ops.rs");
    include!("golden/dom/node_register.rs");
    include!("golden/dom/node_v8.rs");
    include!("golden/dom/node_list_ops.rs");
    include!("golden/dom/node_list_register.rs");
    include!("golden/dom/node_list_v8.rs");
    include!("golden/dom/css_ops.rs");
    include!("golden/dom/css_register.rs");
    include!("golden/dom/css_v8.rs");
}

// A trivial reference so the modules are considered used even if a future
// refactor drops the other `use`s; the real assertion is that this file
// compiles at all with the V8 glue included.
#[test]
fn the_generated_v8_glue_compiles() {
    let _ = std::any::type_name::<toy_bindings::CounterOptions>();
    let _ = std::any::type_name::<dom_bindings::NodeOrDomString>();
}
