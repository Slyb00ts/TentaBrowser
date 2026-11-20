//! The native half of `tenta_js::performance_timeline`: the names of the
//! timeline's interfaces and of the `performance` members that keep and read
//! entries, defined when the engine is built, and the script behind them,
//! run when a page first uses one.
//!
//! The script asks two things of the host: the clock `performance.now()`
//! reads, and when the document's lifecycle events were fired
//! (`HostState::lifecycle_times`), which is what the navigation entry
//! reports.

use tenta_js::bindings::{
    v8_begin_stored_value_lease, v8_end_stored_value_lease, v8_registered_slow_callback_boundary,
};

use super::observers::key;
use super::{host_state, lazy};

/// The private property of the global that holds the script's exports, or
/// `false` once the script has failed.
const EXPORTS: &str = "tenta.performanceTimeline";

/// The interface objects the script exports.
const INTERFACES: [&str; 7] = [
    "PerformanceEntry",
    "PerformanceMark",
    "PerformanceMeasure",
    "PerformanceResourceTiming",
    "PerformanceNavigationTiming",
    "PerformanceObserver",
    "PerformanceObserverEntryList",
];

/// The members of `performance` the script exports under `methods`.
const METHODS: [&str; 9] = [
    "mark",
    "measure",
    "getEntries",
    "getEntriesByType",
    "getEntriesByName",
    "clearMarks",
    "clearMeasures",
    "clearResourceTimings",
    "setResourceTimingBufferSize",
];

guarded_callback!(guarded_lifecycle => lifecycle);

/// Defines the interface names on the global.
pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    for name in INTERFACES {
        lazy::define_interface(scope, global, name, lazy_interface);
    }
}

/// Defines the timeline members of the `performance` object.
pub(super) fn install_methods(scope: &mut v8::PinScope, performance: v8::Local<v8::Object>) {
    for name in METHODS {
        lazy::define_interface(scope, performance, name, lazy_method);
    }
}

/// `[dclStart, dclEnd, loadStart, loadEnd]` in the clock's milliseconds, 0
/// for an event that has not been fired.
fn lifecycle<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let times = host_state(scope).map_or([0.0; 4], |state| state.lifecycle_times.get());
    let values: Vec<v8::Local<v8::Value>> = times
        .iter()
        .map(|&time| v8::Number::new(scope, time).into())
        .collect();
    rv.set(v8::Array::new_with_elements(scope, &values).into());
}

fn exports<'s>(scope: &mut v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Object>> {
    lazy::exports(
        scope,
        EXPORTS,
        tenta_js::performance_timeline::PERFORMANCE_TIMELINE_PRELUDE_SOURCE,
        |scope| {
            // The clock is the engine's own function, not whatever the page
            // may have put at `performance.now` since.
            let now = v8::Function::new(scope, super::guarded_performance_now)?;
            let lifecycle = v8::Function::new(scope, guarded_lifecycle)?;
            Some(vec![now.into(), lifecycle.into()])
        },
    )
}

fn lazy_interface<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: v8::Local<'s, v8::Name>,
    _args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'_, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        if let Some(exports) = exports(scope)
            && let Some(interface) = exports.get(scope, name.into())
        {
            rv.set(interface);
        }
    };
    // SAFETY: a registered V8 property callback, where allocation is allowed.
    unsafe { v8_registered_slow_callback_boundary(scope, "lazy_timeline_interface", (), call) };
}

fn lazy_method<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: v8::Local<'s, v8::Name>,
    _args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'_, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        let methods = key(scope, "methods");
        if let Some(exports) = exports(scope)
            && let Some(methods) = exports.get(scope, methods.into())
            && let Ok(methods) = v8::Local::<v8::Object>::try_from(methods)
            && let Some(method) = methods.get(scope, name.into())
        {
            rv.set(method);
        }
    };
    // SAFETY: a registered V8 property callback, where allocation is allowed.
    unsafe { v8_registered_slow_callback_boundary(scope, "lazy_timeline_method", (), call) };
}
