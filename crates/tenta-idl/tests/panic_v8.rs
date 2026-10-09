//! Panics must be contained before returning through V8's C++ frames.
//!
//! Every scenario runs in a child process: an unguarded callback aborts that
//! child rather than taking down the test runner. The tests use the generated
//! fixtures themselves, including V8-optimized fast calls, not a mock catcher.

#![cfg(feature = "v8")]

use std::cell::Cell;
use std::process::Command;
use std::rc::Rc;

// These allowances cover only the included generator output, whose uniform
// conversions also handle non-Copy values and wider numeric source types.
#[allow(
    dead_code,
    unused_variables,
    clippy::extra_unused_type_parameters,
    clippy::wrong_self_convention,
    clippy::unnecessary_cast,
    clippy::single_match,
    clippy::unnecessary_map_on_constructor,
    clippy::clone_on_copy
)]
mod bindings {
    include!("golden/toy/prelude.rs");
    include!("golden/toy/types.rs");
    include!("golden/dom/types.rs");
    include!("golden/toy/counter_ops.rs");
    include!("golden/toy/counter_register.rs");
    include!("golden/toy/counter_v8.rs");
    include!("golden/toy/pair_box_ops.rs");
    include!("golden/toy/pair_box_register.rs");
    include!("golden/toy/pair_box_v8.rs");
    include!("golden/toy/payload_box_ops.rs");
    include!("golden/toy/payload_box_register.rs");
    include!("golden/toy/payload_box_v8.rs");
    include!("golden/dom/node_list_ops.rs");
    include!("golden/dom/node_list_register.rs");
    include!("golden/dom/node_list_v8.rs");
    include!("golden/dom/css_ops.rs");
    include!("golden/dom/css_register.rs");
    include!("golden/dom/css_v8.rs");
    include!("golden/dom/manager_ops.rs");
    include!("golden/dom/manager_register.rs");
    include!("golden/dom/manager_v8.rs");
}
use bindings::*;

#[derive(Default)]
struct Probe {
    point: Cell<&'static str>,
    attempts: Cell<u32>,
    world_drops: Cell<u32>,
    payload_drop: Cell<bool>,
    payload_cleanup: Cell<bool>,
    payload_drops: Cell<u32>,
}
thread_local! {
    static PROBE: Probe = Probe::default();
}

struct PanickingPayload;
impl Drop for PanickingPayload {
    fn drop(&mut self) {
        PROBE.with(|probe| probe.payload_drops.set(probe.payload_drops.get() + 1));
        panic!("a panic payload must not be dropped at the native boundary");
    }
}

struct OrdinaryPayload;
impl Drop for OrdinaryPayload {
    fn drop(&mut self) {
        PROBE.with(|probe| probe.payload_drops.set(probe.payload_drops.get() + 1));
    }
}

fn trip(point: &str) {
    PROBE.with(|probe| {
        if probe.point.get() == point {
            // Represents a partially completed host mutation. It must not be
            // performed again by a fallback or a later attempt to reuse V8.
            probe.attempts.set(probe.attempts.get() + 1);
            if probe.payload_drop.get() {
                std::panic::panic_any(PanickingPayload);
            }
            if probe.payload_cleanup.get() {
                std::panic::panic_any(OrdinaryPayload);
            }
            panic!("injected panic in {point}");
        }
    });
}

// V8's internal pointer field requires alignment even for a zero-sized host.
#[repr(align(8))]
struct Host;
impl CounterConstructor for Host {
    fn construct(_: &Realm, _: CounterOptions) -> Result<Self, JsError> {
        trip("constructor");
        Ok(Self)
    }
}
impl CounterOps for Host {
    fn count(&self) -> u32 {
        trip("getter");
        7
    }
    fn set_count(&self, _: u32) {
        trip("setter");
    }
    fn is_active(&self) -> bool {
        true
    }
    fn state(&self) -> Result<CounterState, JsError> {
        trip("slow_getter");
        Ok(CounterState::Idle)
    }
    fn reset(&self) {
        trip("method");
    }
    fn advance(&self, _: Option<u32>) -> Result<(), JsError> {
        trip("slow_method");
        Ok(())
    }
    fn stop(&self) {}
    fn stopped(&self) -> bool {
        false
    }
}
impl PairBoxOps for Host {
    fn iterable_entries(&self) -> Result<IterableEntries<String, String>, JsError> {
        trip("iterator");
        Ok(vec![("key".to_owned(), "value".to_owned())])
    }
}
impl NodeListOps for Host {
    fn supported_property_indices(&self) -> Result<Vec<u32>, JsError> {
        Ok(vec![0])
    }
    fn item(&self, _: u32) -> Result<Option<Handle<Node>>, JsError> {
        trip("indexed");
        Ok(None)
    }
    fn length(&self) -> u32 {
        1
    }
}
impl CSSOps for Host {
    fn supports_2(_: &str, _: &str) -> Result<bool, JsError> {
        trip("namespace");
        Ok(true)
    }
    fn supports_1(_: &str) -> Result<bool, JsError> {
        trip("namespace");
        Ok(true)
    }
    fn escape(_: &str) -> Result<String, JsError> {
        trip("namespace");
        Ok("escaped".to_owned())
    }
}
impl ManagerOps for Host {
    fn handler1(&self) -> Result<Option<OccurrenceHandler>, JsError> {
        Ok(None)
    }
    fn set_handler1(&self, _: Option<OccurrenceHandler>) -> Result<(), JsError> {
        Ok(())
    }
    fn handler2(&self) -> Result<Option<ErrorHandler>, JsError> {
        Ok(None)
    }
    fn set_handler2(&self, _: Option<ErrorHandler>) -> Result<(), JsError> {
        Ok(())
    }
}

struct World;
impl Drop for World {
    fn drop(&mut self) {
        PROBE.with(|probe| probe.world_drops.set(probe.world_drops.get() + 1));
    }
}
impl WrapperWorld for World {
    fn wrapper<'s>(
        &self,
        _: &mut v8::PinScope<'s, '_>,
        _: &'static str,
        _: u64,
    ) -> Result<Option<v8::Local<'s, v8::Object>>, JsError> {
        Ok(None)
    }
    fn handle(
        &self,
        _: &mut v8::PinScope<'_, '_>,
        _: &'static str,
        _: v8::Local<v8::Object>,
    ) -> Option<u64> {
        None
    }
    fn store_value(&self, _: &mut v8::PinScope<'_, '_>, _: v8::Local<v8::Value>) -> u64 {
        trip("conversion");
        1
    }
    fn load_value<'s>(
        &self,
        _: &mut v8::PinScope<'s, '_>,
        _: u64,
    ) -> Option<v8::Local<'s, v8::Value>> {
        None
    }
    fn realm(&self) -> Realm {
        Rc::new(())
    }
    fn adopt(
        &self,
        _: &mut v8::PinScope<'_, '_>,
        _: &'static str,
        _: v8::Local<v8::Object>,
        _: Box<dyn std::any::Any>,
    ) -> Result<(), JsError> {
        trip("adopt");
        Ok(())
    }
}
struct Registry;
impl InterfaceRegistry for Registry {
    fn register(&mut self, _: InterfaceDescriptor) {}
}

fn eval(scope: &mut v8::PinScope<'_, '_>, source: &str) -> String {
    let source = v8::String::new(scope, source).expect("source");
    v8::Script::compile(scope, source, None)
        .expect("compile")
        .run(scope)
        .expect("evaluate")
        .to_rust_string_lossy(scope)
}

fn expect_termination(scope: &mut v8::PinScope<'_, '_>, source: &str) {
    v8::tc_scope!(let tc, scope);
    // TerminateExecution requests an interrupt, which a tiny script can return
    // before consuming. A bounded back-edge makes V8 observe it in this run;
    // real embedders additionally inspect native_callback_failure on return.
    let source = format!("{source}; for (let i = 0; i < 100_000; ++i) {{ globalThis.tick = i; }}");
    let source = v8::String::new(tc, &source).expect("source");
    let script = v8::Script::compile(tc, source, None).expect("compile");
    assert!(script.run(tc).is_none(), "panic cannot return a JS value");
    assert!(tc.has_terminated(), "panic cannot be caught by JavaScript");
}

fn run_case(case: &str) {
    v8::V8::set_flags_from_string("--allow-natives-syntax --turbo-fast-api-calls");
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();
    let mut isolate = v8::Isolate::new(Default::default());
    {
        v8::scope!(let handle_scope, &mut isolate);
        let bootstrap = v8::Context::new(handle_scope, Default::default());
        let mut bootstrap_scope = v8::ContextScope::new(handle_scope, bootstrap);
        v8::scope!(let scope, &mut bootstrap_scope);
        let global = v8::ObjectTemplate::new(scope);
        let mut registry = Registry;
        // This case deliberately installs no interface templates: a namespace
        // must preallocate its own failure marker as well.
        if case.starts_with("namespace") {
            install_css::<Host>(scope, global, &mut registry).expect("CSS");
        } else {
            install_counter::<Host>(scope, global, &mut registry).expect("Counter");
            install_pair_box::<Host>(scope, global, &mut registry).expect("PairBox");
            install_node_list::<Host>(scope, global, &mut registry).expect("NodeList");
            install_manager::<Host>(scope, global, &mut registry).expect("Manager");
        }
        set_wrapper_world(scope, Box::new(World)).expect("install the wrapper world");
        let context = v8::Context::new(
            scope,
            v8::ContextOptions {
                global_template: Some(global),
                ..Default::default()
            },
        );
        let mut context_scope = v8::ContextScope::new(scope, context);
        v8::scope!(let scope, &mut context_scope);
        let global = context.global(scope);
        let mut instances = Vec::new();
        if !case.starts_with("namespace") {
            for (interface, variable) in [
                ("Counter", "counter"),
                ("PairBox", "pairs"),
                ("NodeList", "list"),
                ("Manager", "manager"),
            ] {
                let template = cached_template(scope, interface).expect("template");
                let object = template
                    .instance_template(scope)
                    .new_instance(scope)
                    .expect("instance");
                attach_host(&object, Host);
                let key = v8::String::new(scope, variable).expect("key");
                global.set(scope, key.into(), object.into());
                instances.push(object);
            }
        }
        let (point, expression, expected) = match case {
            "constructor" => (
                "constructor",
                "new Counter({initialValue: 0})",
                "construct_counter",
            ),
            "adopt" => (
                "adopt",
                "new Counter({initialValue: 0})",
                "construct_counter",
            ),
            "conversion" => (
                "conversion",
                "manager.handler1 = () => {}",
                "set_manager_handler1",
            ),
            "getter" => ("getter", "counter.count", "get_counter_count"),
            "setter" => ("setter", "counter.count = 3", "set_counter_count"),
            "slow_getter" => ("slow_getter", "counter.state", "get_counter_state"),
            "method" => ("method", "counter.reset()", "call_counter_reset"),
            "slow_method" | "payload_drop" | "payload_cleanup" => {
                ("slow_method", "counter.advance()", "call_counter_advance")
            }
            "indexed" => ("indexed", "list[0]", "indexed_get_node_list"),
            "entries" => ("iterator", "pairs.entries()", "call_pair_box_entries"),
            "keys" => ("iterator", "pairs.keys()", "call_pair_box_keys"),
            "values" => ("iterator", "pairs.values()", "call_pair_box_values"),
            "for_each" => (
                "iterator",
                "pairs.forEach(() => {})",
                "call_pair_box_for_each",
            ),
            "namespace" => ("namespace", "CSS.escape('x')", "call_css_escape"),
            "namespace_overload" => ("namespace", "CSS.supports('x', 'y')", "call_css_supports_2"),
            "fast_getter" => ("getter", "counter.count", "fast_get_counter_count"),
            "fast_setter" => ("setter", "counter.count = 3", "fast_set_counter_count"),
            "fast_method" => ("method", "counter.reset()", "fast_call_counter_reset"),
            other => panic!("unknown case {other}"),
        };
        if case.starts_with("fast_") {
            eval(
                scope,
                &format!(
                    "function run() {{ return {expression}; }}\n\
                 %PrepareFunctionForOptimization(run); run(); run();\n\
                 %OptimizeFunctionOnNextCall(run); run();"
                ),
            );
        }
        PROBE.with(|probe| {
            probe.point.set(point);
            probe.payload_drop.set(case == "payload_drop");
            probe.payload_cleanup.set(case == "payload_cleanup");
        });
        let expression = if case.starts_with("fast_") {
            "run()"
        } else {
            expression
        };
        expect_termination(
            scope,
            &format!("try {{ {expression}; }} catch (error) {{ 'caught'; }}"),
        );
        assert_eq!(native_callback_failure(scope), Some(expected));
        PROBE.with(|probe| {
            assert_eq!(probe.attempts.get(), 1, "partial mutation ran once");
            assert_eq!(
                probe.world_drops.get(),
                0,
                "world ownership survived unwind"
            );
            assert_eq!(
                probe.payload_drops.get(),
                0,
                "payload retained until teardown"
            );
        });
        // Embedders must discard this isolate, but even an erroneous explicit
        // cancellation cannot re-enter the poisoned generated host callbacks.
        scope.cancel_terminate_execution();
        expect_termination(scope, expression);
        PROBE.with(|probe| assert_eq!(probe.attempts.get(), 1, "no retry after panic"));
        for instance in instances {
            // SAFETY: each live instance was attached to Host above; no GC
            // finalizer owns these fixture hosts and they are detached once.
            drop(unsafe { detach_host::<Host>(&instance) });
        }
        drop(scope.remove_slot::<TemplateCache>());
    }
    drop(isolate);
    PROBE.with(|probe| {
        assert_eq!(probe.world_drops.get(), 1);
        assert_eq!(
            probe.payload_drops.get(),
            u32::from(case == "payload_cleanup" || case == "payload_drop"),
            "ordinary payloads are freed, and a panicking destructor is contained"
        );
    });
}

#[test]
fn panic_child() {
    if let Ok(case) = std::env::var("TENTA_IDL_PANIC_CASE") {
        run_case(&case);
    }
}

#[test]
fn generated_callback_panics_terminate_the_isolate_without_aborting_the_process() {
    for case in [
        "constructor",
        "adopt",
        "conversion",
        "getter",
        "setter",
        "slow_getter",
        "method",
        "slow_method",
        "payload_drop",
        "payload_cleanup",
        "indexed",
        "entries",
        "keys",
        "values",
        "for_each",
        "namespace",
        "namespace_overload",
        "fast_getter",
        "fast_setter",
        "fast_method",
    ] {
        let output = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "panic_child", "--nocapture"])
            .env("TENTA_IDL_PANIC_CASE", case)
            .output()
            .expect("child starts");
        assert!(
            output.status.success(),
            "{case} child failed: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
