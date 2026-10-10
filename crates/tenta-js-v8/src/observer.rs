//! The mutation-observer microtask (DOM Standard §4.3, *queue a mutation
//! observer microtask*).
//!
//! A mutation leaves the agent's notify demand `Requested`; nothing here runs
//! script. [`queue_notification`] turns that demand into one microtask in V8's
//! queue, right after the binding call that mutated returns, so the microtask
//! is ordered with the promise reactions and `queueMicrotask` callbacks the
//! page queues after the mutation. The microtask calls the notify routine the
//! engine installed in [`HostState::mutation_notify`].

use tenta_js::bindings::host::{
    SharedContext, mark_mutation_microtask_enqueued, mutation_observers_awaiting_microtask,
};
use tenta_js::bindings::v8_registered_slow_callback_boundary;

use crate::host::host_state;

/// Enqueues the notification microtask when a mutation asked for one and none
/// is queued yet. Cheap when the world never constructed a `MutationObserver`.
pub(crate) fn queue_notification(scope: &mut v8::PinScope<'_, '_>, ctx: &SharedContext) {
    if !mutation_observers_awaiting_microtask(ctx) {
        return;
    }
    let Some(function) = v8::Function::new(scope, guarded_notify) else {
        return;
    };
    scope.enqueue_microtask(function);
    mark_mutation_microtask_enqueued(ctx);
}

fn guarded_notify<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    _retval: v8::ReturnValue<'s, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        let Some(state) = host_state(scope) else {
            return;
        };
        let notify = state.mutation_notify.borrow().clone();
        if let Some(notify) = notify {
            notify();
        }
    };
    // SAFETY: This is a registered V8 Function callback queued as a microtask,
    // not a raw MicrotaskCallback or a GC hook, so slow API allocation is
    // allowed.
    unsafe { v8_registered_slow_callback_boundary(scope, "mutation_observer_notify", (), call) };
}
