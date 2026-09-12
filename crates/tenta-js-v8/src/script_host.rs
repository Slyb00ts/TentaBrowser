//! The V8 implementation of the engine-independent [`ScriptHost`] boundary
//! (`docs/spec/05-javascript.md` §3).
//!
//! The DOM event dispatcher ([`tenta_js::dom::events`]) is engine-independent:
//! it invokes a listener only through [`ScriptHost::invoke_listener`], passing
//! opaque handle bits. [`V8ScriptHost`] turns those bits back into V8 objects
//! and calls the stored function.
//!
//! # Re-entering the isolate
//!
//! `invoke_listener` is only ever reached from inside a live DOM binding call
//! (`EventTarget.dispatchEvent`, later `HTMLElement.click`), i.e. from a V8
//! callback running on this isolate's thread. That is the one sanctioned use of
//! [`v8::CallbackScope`]: it bootstraps a `HandleScope` from the isolate we are
//! already inside. The wrapper world stays installed in the isolate slot across
//! calls (`set_wrapper_world` sets it once; `handle_to_v8` removes and restores
//! it around each use), so `handle_to_v8`/`value_to_v8` resolve here.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_js::bindings::{Event, EventTarget, Handle, handle_to_v8, value_to_v8};
use tenta_js::dom::events::{CallbackRef, ScriptHost};

use crate::host::HostState;
use crate::world::WorldState;

/// The V8 side of the script-callback boundary.
pub struct V8ScriptHost {
    /// The address of the isolate this world runs on, captured by value so it
    /// survives the `Engine` being moved after construction (a raw
    /// `*mut v8::Isolate` would dangle, pointing into the moved isolate handle).
    /// Valid for as long as the owning `Engine` (and thus this host, held
    /// through the `HostContext`) is alive.
    isolate: v8::UnsafeRawIsolatePtr,
    /// The context listeners run in.
    context: v8::Global<v8::Context>,
    /// The world's value slab, for releasing stored callbacks.
    state: Rc<RefCell<WorldState>>,
    /// The host state, for the script-activation depth.
    host: Rc<HostState>,
}

impl V8ScriptHost {
    /// Builds a script host over the engine's isolate, context and world state.
    pub fn new(
        isolate: v8::UnsafeRawIsolatePtr,
        context: v8::Global<v8::Context>,
        state: Rc<RefCell<WorldState>>,
        host: Rc<HostState>,
    ) -> Self {
        Self {
            isolate,
            context,
            state,
            host,
        }
    }
}

impl ScriptHost for V8ScriptHost {
    fn invoke_listener(&self, callback: CallbackRef, current_target: u64, event: u64) {
        // Bootstrap a scope from the isolate we are re-entering. SAFETY: this is
        // only reached from inside a DOM binding call on this isolate's thread,
        // the sole valid use of `CallbackScope` (see the module docs). The
        // isolate address was captured by value at install time, so it is stable
        // and non-null for the engine's lifetime.
        let mut raw = self.isolate;
        let isolate: &mut v8::Isolate =
            unsafe { v8::Isolate::ref_from_raw_isolate_ptr_mut(&mut raw) };
        let mut storage = unsafe { v8::CallbackScope::new(isolate) };
        let mut callback_scope = {
            let pinned = unsafe { std::pin::Pin::new_unchecked(&mut storage) };
            pinned.init()
        };
        let callback_scope = &mut callback_scope;
        let context = v8::Local::new(callback_scope, &self.context);
        let mut context_scope = v8::ContextScope::new(callback_scope, context);
        v8::scope!(let scope, &mut context_scope);

        // Resolve `this`, the event argument and the callable. A stale handle or
        // a released callback simply skips the invocation.
        let Ok(this) = handle_to_v8::<EventTarget>(scope, Handle::from_bits(current_target)) else {
            return;
        };
        let Ok(event_value) = handle_to_v8::<Event>(scope, Handle::from_bits(event)) else {
            return;
        };
        let Ok(callee) = value_to_v8(scope, callback.0) else {
            return;
        };

        // A listener is script, so it can queue a promise reaction. Today every
        // dispatch originates inside running script, whose own entry already
        // recorded the debt — but a real-clock page parks on `next_wakeup`, and
        // the moment the native-dispatch branch below is implemented an unpaid
        // checkpoint would strand the reaction until an unrelated timer fired.
        self.host.microtasks_pending.set(true);

        v8::tc_scope!(let tc, scope);
        // A callable listener is invoked directly; a non-callable object has its
        // `handleEvent` method invoked with `this` set to the object. Any thrown
        // exception is caught and swallowed here (the DOM standard reports it and
        // continues to the next listener) when the `TryCatch` drops.
        if let Ok(function) = v8::Local::<v8::Function>::try_from(callee) {
            let _ = function.call(tc, this, &[event_value]);
        } else if let Ok(object) = v8::Local::<v8::Object>::try_from(callee)
            && let Some(key) = v8::String::new(tc, "handleEvent")
            && let Some(method) = object.get(tc, key.into())
            && let Ok(function) = v8::Local::<v8::Function>::try_from(method)
        {
            let _ = function.call(tc, object.into(), &[event_value]);
        }

        // A microtask checkpoint runs only for a dispatch that did not originate
        // in running script (a native `element.click()`). Every dispatch in this
        // phase comes from `el.dispatchEvent(...)` inside script, so the script's
        // own checkpoint drains the queue; the native-dispatch checkpoint lands
        // with that path in a later sub-phase.
        if self.host.script_depth.get() == 0 {
            // Deferred: native-initiated dispatch checkpoint.
        }
    }

    fn release_value(&self, bits: u64) {
        self.state.borrow_mut().release_value(bits);
    }
}
