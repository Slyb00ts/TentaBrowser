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

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use tenta_js::bindings::{Event, EventTarget, Handle, handle_to_v8, value_to_v8};
use tenta_js::dom::events::{CallbackRef, HostValue, PromiseOutcome, PromiseRef, ScriptHost};

use crate::host::HostState;
use crate::promise;
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
    /// The bits of the one interned `undefined`, filled on the first request.
    ///
    /// `undefined` is a primitive, so [`WorldState::store_value`] does not intern
    /// it and a fresh call would occupy a fresh slot every time — which would
    /// make an unset `AbortSignal.reason` read back as a different value on every
    /// access. Interning it once here gives every reader the same bits, and
    /// therefore the same value.
    undefined: Cell<Option<u64>>,
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
            undefined: Cell::new(None),
        }
    }

    /// The world's value slab, for the promise seam, which stores the value a
    /// settlement carries and gives the promise's references back through it.
    pub(crate) fn world(&self) -> Rc<RefCell<WorldState>> {
        self.state.clone()
    }

    /// Enters this host's context on its isolate and runs `f` inside it.
    ///
    /// This is the bootstrap [`ScriptHost::invoke_listener`] documents: a
    /// [`v8::CallbackScope`] spun up from the isolate we are already inside,
    /// which is only legal because every caller is reached from a live DOM
    /// binding call on this isolate's thread.
    ///
    /// It must not be nested: a second `CallbackScope` inside the first is not
    /// legal, so a callee that already has a scope ([`crate::promise`]'s
    /// settlement) takes its own path rather than calling back in here.
    pub(crate) fn with_context<R>(&self, f: impl FnOnce(&mut v8::PinScope<'_, '_>) -> R) -> R {
        // SAFETY: the isolate address was captured by value at install time, so
        // it is stable and non-null for the engine's lifetime, and this runs on
        // that isolate's own thread inside a binding call — the sole valid use of
        // `CallbackScope` (see the module docs).
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
        f(scope)
    }

    /// Builds a [`HostValue`] in a context the caller has already entered, and
    /// returns the bits that carry one stored reference.
    ///
    /// The body of [`ScriptHost::build_value`], which is this wrapped in a
    /// context entry. It is separate because the promise settlement
    /// ([`crate::promise`]) already runs inside a scope and may not enter a
    /// second one.
    pub(crate) fn build_value_in(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        value: HostValue<'_>,
    ) -> Option<u64> {
        match value {
            HostValue::Undefined => {
                if let Some(bits) = self.undefined.get() {
                    return Some(bits);
                }
                let undefined = v8::undefined(scope);
                let bits = self.state.borrow_mut().store_value(scope, undefined.into());
                self.undefined.set(Some(bits));
                Some(bits)
            }
            HostValue::DomException { name, message } => {
                let object = dom_exception_value(scope, name, message)?;
                Some(self.state.borrow_mut().store_value(scope, object))
            }
            HostValue::String(text) => {
                let text = v8::String::new(scope, &text)?;
                Some(self.state.borrow_mut().store_value(scope, text.into()))
            }
            HostValue::TypeError { message } => {
                let message = v8::String::new(scope, message)?;
                let object = construct_global(scope, "TypeError", &[message.into()])?;
                Some(self.state.borrow_mut().store_value(scope, object))
            }
        }
    }
}

/// Builds `new DOMException(message, name)` from the context's global and
/// returns it as a value, or `None` when the page's DOM bootstrap is absent.
///
/// This is the construction [`tenta_js::bindings::throw_js_error`] performs to
/// raise a `DOMException`; an abort reason and a thrown abort reason have to be
/// the *same* object, so the object is built here once and the caller both
/// stores and re-throws it rather than building a second one.
pub(crate) fn dom_exception_value<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
    message: &str,
) -> Option<v8::Local<'s, v8::Value>> {
    let name = v8::String::new(scope, name)?;
    let message = v8::String::new(scope, message)?;
    construct_global(scope, "DOMException", &[message.into(), name.into()])
}

/// Builds `new <name>(arguments…)` from the context's global object and returns
/// it as a value, or `None` when the global has no such constructor or the call
/// throws.
///
/// This is how every error object the host has to *settle* a promise with is
/// built — a rejection carries a value, so `JsError::type_error` (which can only
/// throw) is not an option.
fn construct_global<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
    arguments: &[v8::Local<'s, v8::Value>],
) -> Option<v8::Local<'s, v8::Value>> {
    let context = scope.get_current_context();
    let global = context.global(scope);
    let key = v8::String::new(scope, name)?;
    let constructor = global.get(scope, key.into())?;
    let constructor = v8::Local::<v8::Function>::try_from(constructor).ok()?;
    constructor
        .new_instance(scope, arguments)
        .map(|object| object.into())
}

impl ScriptHost for V8ScriptHost {
    fn invoke_listener(&self, callback: CallbackRef, current_target: u64, event: u64) {
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

        // Whether this dispatch originated outside running script has to be read
        // *before* the listener is entered, because entering it makes the depth
        // non-zero either way.
        let native = self.host.script_depth.get() == 0;

        v8::tc_scope!(let tc, scope);
        // A callable listener is invoked directly; a non-callable object has its
        // `handleEvent` method invoked with `this` set to the object. Any thrown
        // exception is caught and swallowed here (the DOM standard reports it and
        // continues to the next listener) when the `TryCatch` drops.
        //
        // The call is bracketed by a script activation: a listener is script, so
        // it can queue a promise reaction, and `enter_script` records that debt.
        // Today every dispatch originates inside running script, whose own entry
        // already recorded it — but a real-clock page parks on `next_wakeup`, and
        // the moment the native-dispatch branch below is implemented an unpaid
        // checkpoint would strand the reaction until an unrelated timer fired.
        self.host.enter_script();
        if let Ok(function) = v8::Local::<v8::Function>::try_from(callee) {
            let _ = function.call(tc, this, &[event_value]);
        } else if let Ok(object) = v8::Local::<v8::Object>::try_from(callee)
            && let Some(key) = v8::String::new(tc, "handleEvent")
            && let Some(method) = object.get(tc, key.into())
            && let Ok(function) = v8::Local::<v8::Function>::try_from(method)
        {
            let _ = function.call(tc, object.into(), &[event_value]);
        }
        self.host.exit_script();

        // A microtask checkpoint runs only for a dispatch that did not originate
        // in running script (a native `element.click()`). Every dispatch in this
        // phase comes from `el.dispatchEvent(...)` inside script, so the script's
        // own checkpoint drains the queue; the native-dispatch checkpoint lands
        // with that path in a later sub-phase.
        if native {
            // Deferred: native-initiated dispatch checkpoint.
        }
    }

    fn release_value(&self, bits: u64) {
        self.state.borrow_mut().release_value(bits);
    }

    fn build_value(&self, value: HostValue<'_>) -> Option<u64> {
        self.with_context(|scope| self.build_value_in(scope, value))
    }

    fn new_promise(&self) -> Option<PromiseRef> {
        promise::new_promise(self)
    }

    fn settle_promise(&self, resolver: u64, outcome: PromiseOutcome<'_>) -> bool {
        promise::settle_promise(self, resolver, outcome)
    }

    fn throw_value(&self, bits: u64) {
        self.with_context(|scope| {
            // A slot that has already been freed cannot be thrown; silently
            // skipping is the same shape as `invoke_listener`'s stale-handle
            // check, and the alternative — throwing `undefined` — would turn a
            // reclaimed reason into a different exception rather than no
            // exception.
            let Some(value) = self.state.borrow().load_value(scope, bits) else {
                return;
            };
            scope.throw_exception(value);
        });
    }
}
