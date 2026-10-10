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
//! `invoke_listener` is normally reached from inside a live DOM binding call
//! (`EventTarget.dispatchEvent`, `HTMLElement.click`), i.e. from a V8 callback
//! running on this isolate's thread. That is the one sanctioned use of
//! [`v8::CallbackScope`]: it bootstraps a `HandleScope` from the isolate we are
//! already inside. The one exception is a dispatch the *engine* initiates — an
//! XHR completion drained by the page's turn loop — which has no callback under
//! it at all; the entry that drains one marks itself through
//! [`HostState::enter_native_dispatch`], and [`V8ScriptHost::with_context`] then
//! opens a plain `HandleScope`, which is legal in both places. The wrapper world
//! stays installed in the isolate slot across calls (`set_wrapper_world` sets it
//! once; `handle_to_v8` removes and restores it around each use), so
//! `handle_to_v8`/`value_to_v8` resolve here.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use tenta_js::bindings::host::encode_window;
use tenta_js::bindings::{
    Event, EventTarget, Handle, JsError, MutationObserver, MutationRecord, handle_to_v8,
    value_to_v8,
};
use tenta_js::dom::events::{
    CallbackRef, HostValue, MutationDelivery, PromiseOutcome, PromiseRef, ScriptHost,
};
use tenta_js::fetch::FetchStartError;
use tenta_js::xhr::{XhrHandle, XhrRequest};

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
    /// The one caller that is *not* inside a binding call — the engine's native
    /// drain of an XHR completion — marks itself through
    /// [`HostState::enter_native_dispatch`], and this opens a plain
    /// `HandleScope` instead. A plain scope is legal whether or not a callback
    /// is on the stack, whereas a `CallbackScope` is not, so the flag is what
    /// keeps the one native entry from being undefined behaviour.
    ///
    /// The callback path must not be nested: a second `CallbackScope` inside the
    /// first is not legal, so a callee that already has a scope
    /// ([`crate::promise`]'s settlement) takes its own path rather than calling
    /// back in here.
    pub(crate) fn with_context<R>(&self, f: impl FnOnce(&mut v8::PinScope<'_, '_>) -> R) -> R {
        // SAFETY: the isolate address was captured by value at install time, so
        // it is stable and non-null for the engine's lifetime, and this runs on
        // that isolate's own thread.
        let mut raw = self.isolate;
        let isolate: &mut v8::Isolate =
            unsafe { v8::Isolate::ref_from_raw_isolate_ptr_mut(&mut raw) };

        // A native entry has no V8 callback under it, so the callback bootstrap
        // below would be undefined behaviour; a plain scope is legal either way.
        if self.host.native_dispatch.get() {
            v8::scope!(let handle_scope, &mut *isolate);
            let context = v8::Local::new(handle_scope, &self.context);
            let mut context_scope = v8::ContextScope::new(handle_scope, context);
            v8::scope!(let scope, &mut context_scope);
            return f(scope);
        }

        // SAFETY: same isolate and thread as above; this branch is the call path
        // the binding call above reaches, the sole valid use of `CallbackScope`
        // (see the module docs).
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
/// throw) is not an option. `pub(crate)` because the engine's fetch drain builds
/// its `TypeError` the same way, in a scope of its own rather than through
/// [`V8ScriptHost::build_value_in`] (`crate::Engine::settle_fetches`).
pub(crate) fn construct_global<'s>(
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
    fn array_buffer_view_info(
        &self,
        view: tenta_js::bindings::ArrayBufferViewRef,
    ) -> Result<tenta_js::bindings::BufferMetadata, JsError> {
        self.with_context(|scope| tenta_js::bindings::v8_array_buffer_view_info(scope, view))
    }

    fn write_array_buffer_view(
        &self,
        view: tenta_js::bindings::ArrayBufferViewRef,
        bytes: &[u8],
    ) -> Result<tenta_js::bindings::BufferMetadata, JsError> {
        self.with_context(|scope| {
            tenta_js::bindings::v8_write_array_buffer_view(scope, view, bytes)
        })
    }

    fn check_native_buffer_work(&self) -> Result<(), JsError> {
        self.with_context(|scope| tenta_js::bindings::v8_check_native_buffer_work(scope))
    }

    fn native_buffer_memory_failure(&self) -> JsError {
        // The installed callback's admission cell carries this sticky failure
        // to the registered slow-callback termination tail.
        self.with_context(|scope| {
            let _ = tenta_js::bindings::v8_check_native_buffer_work(scope);
        });
        tenta_js::bindings::pending_js_exception_error()
    }

    fn invoke_listener(&self, callback: CallbackRef, current_target: u64, event: u64) -> bool {
        // Whether this dispatch originated outside running script has to be read
        // *before* the listener is entered, because entering it makes the depth
        // non-zero either way.
        let native = self.host.script_depth.get() == 0;

        // The context entry is `with_context`'s, not a second copy of it: the
        // engine's native drain reaches this same method with no V8 callback on
        // the stack, and only that one bootstrap knows which kind of scope is
        // legal there.
        let cancelled = self.with_context(|scope| {
            // Resolve `this`, the event argument and the callable. A stale handle
            // or a released callback simply skips the invocation, which is not a
            // `false` return.
            let Ok(this) = handle_to_v8::<EventTarget>(scope, Handle::from_bits(current_target))
            else {
                return false;
            };
            let Ok(event_value) = handle_to_v8::<Event>(scope, Handle::from_bits(event)) else {
                return false;
            };
            let Ok(callee) = value_to_v8(scope, callback.0) else {
                return false;
            };

            v8::tc_scope!(let tc, scope);
            // A callable listener is invoked directly; a non-callable object has
            // its `handleEvent` method invoked with `this` set to the object. Any
            // thrown exception is caught and swallowed here (the DOM standard
            // reports it and continues to the next listener) when the `TryCatch`
            // drops.
            //
            // The call is bracketed by a script activation: a listener is script,
            // so it can queue a promise reaction, and `enter_script` records that
            // debt. Today every dispatch originates inside running script, whose
            // own entry already recorded it — but a real-clock page parks on
            // `next_wakeup`, and the moment the native-dispatch branch below is
            // implemented an unpaid checkpoint would strand the reaction until an
            // unrelated timer fired.
            self.host.enter_script();
            let result = if let Ok(function) = v8::Local::<v8::Function>::try_from(callee) {
                function.call(tc, this, &[event_value])
            } else if let Ok(object) = v8::Local::<v8::Object>::try_from(callee)
                && let Some(key) = v8::String::new(tc, "handleEvent")
                && let Some(method) = object.get(tc, key.into())
                && let Ok(function) = v8::Local::<v8::Function>::try_from(method)
            {
                function.call(tc, object.into(), &[event_value])
            } else {
                None
            };
            if result.is_none() {
                crate::inspector::report_caught(tc, &self.host);
            }
            self.host.exit_script();

            // Only an event handler acts on its return value, and only on exactly
            // `false` (HTML §8.1.8.1): `is_false` is the strict comparison the
            // `any` return type needs, so `0`, `""`, `null` and `undefined` do not
            // read as a cancel. A thrown exception leaves no return value and so
            // never cancels either.
            result.is_some_and(|value| value.is_false())
        });

        // A microtask checkpoint runs only for a dispatch that did not originate
        // in running script (the engine's XHR drain, a native `element.click()`).
        // A script's own entry drains the queue for a dispatch it made itself,
        // so the native-dispatch checkpoint lands with that path in a later
        // sub-phase — and when it does, the flag must be computed per dispatch,
        // not per listener: a dispatch from native code can call a listener that
        // itself dispatches, and the inner one is script-initiated.
        if native {
            // Deferred: native-initiated dispatch checkpoint.
        }

        cancelled
    }

    fn invoke_mutation_callback(
        &self,
        callback: CallbackRef,
        observer: u64,
        records: &[u64],
    ) -> MutationDelivery {
        self.with_context(|scope| {
            let Ok(this) = handle_to_v8::<MutationObserver>(scope, Handle::from_bits(observer))
            else {
                return MutationDelivery::Unreachable;
            };
            let Ok(callee) = value_to_v8(scope, callback.0) else {
                return MutationDelivery::Unreachable;
            };
            let Ok(function) = v8::Local::<v8::Function>::try_from(callee) else {
                return MutationDelivery::Unreachable;
            };
            let mut wrappers: Vec<v8::Local<v8::Value>> = Vec::with_capacity(records.len());
            for bits in records {
                let Ok(record) = handle_to_v8::<MutationRecord>(scope, Handle::from_bits(*bits))
                else {
                    return MutationDelivery::Unreachable;
                };
                wrappers.push(record);
            }
            let array = v8::Array::new_with_elements(scope, &wrappers);
            v8::tc_scope!(let tc, scope);
            // The call is script, so it is bracketed like a listener's: it can
            // queue a promise reaction and `enter_script` records the debt.
            self.host.enter_script();
            let result = function.call(tc, this, &[array.into(), this]);
            if result.is_none() {
                crate::inspector::report_caught(tc, &self.host);
            }
            self.host.exit_script();
            MutationDelivery::Called
        })
    }

    fn computed_style_value(&self, node: tenta_core::NodeId, property: &str) -> Option<String> {
        let host = self.host.geometry_host.borrow().clone()?;
        host.computed_value(node, None, property)
    }

    fn release_value(&self, bits: u64) {
        self.state.borrow_mut().release_value(bits);
    }

    fn retain_existing_target(&self, handle: u64) -> Result<Option<u64>, JsError> {
        self.with_context(|scope| {
            if tenta_js::bindings::native_callback_failure(scope).is_some() {
                return Err(tenta_js::bindings::v8_pending_exception_error());
            }
            if handle == encode_window() {
                return Ok(None);
            }
            let Some(wrapper) = ({ self.state.borrow().existing_wrapper(scope, handle) }) else {
                // Conversion still owns the source Local. A missing exact weak
                // wrapper is native cache corruption, not a catchable page error.
                tenta_js::bindings::poison_native_callback(
                    scope,
                    "relatedTarget wrapper retention",
                );
                return Err(tenta_js::bindings::v8_pending_exception_error());
            };
            if tenta_js::bindings::native_callback_failure(scope).is_some() {
                return Err(tenta_js::bindings::v8_pending_exception_error());
            }
            let bits = self.state.borrow_mut().store_value(scope, wrapper.into());
            if tenta_js::bindings::native_callback_failure(scope).is_some() {
                // The newly stored root is retained by the poisoned world until
                // teardown; publishing an EventRecord now would be unsafe.
                return Err(tenta_js::bindings::v8_pending_exception_error());
            }
            Ok(Some(bits))
        })
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

    /// Adds the engine's half of the request and hands it to the driver.
    ///
    /// The split is the point: the host layer built the carrier out of the page's
    /// own pieces (the method, the URL, the author request headers and the
    /// initiator), and the one field it cannot name — the calling task's
    /// [`Origin`](tenta_core::Origin) — comes from the scheduler here, exactly as
    /// the `fetch` global's does. The driver slot is the backend's too.
    ///
    /// No driver is a state a page can be in rather than a panic — `fetch`'s
    /// precedent, "fetch is not available" — but XHR has no promise to reject, so
    /// its translation is this `Err`: the caller reports the standard's *request
    /// error steps* synchronously.
    fn start_xhr(&self, request: XhrRequest, handle: XhrHandle) -> Result<(), FetchStartError> {
        let request = request.into_fetch_request(self.host.scheduler.current_origin());
        let Some(driver) = self.host.xhr_driver() else {
            return Err(FetchStartError::new("XMLHttpRequest is not available"));
        };
        driver.start(request, handle)
    }

    /// Hands the request to the driver to end, through the same slot
    /// [`Self::start_xhr`] took it from.
    ///
    /// No driver means nothing was started, hence nothing to terminate: the
    /// answer is `false`, which is exactly what a driver that does not hold the
    /// handle says. The two are indistinguishable to the caller on purpose —
    /// `abort()` is a void method whose steps raise nothing, so "there was
    /// nothing to end" is not a failure whatever the reason.
    fn abort_xhr(&self, handle: XhrHandle) -> bool {
        let Some(driver) = self.host.xhr_driver() else {
            return false;
        };
        driver.abort(handle)
    }

    /// Arms the timer on the engine's own loop, as page activity, with the
    /// plain-scope entry the callback needs.
    ///
    /// The wrapper is what makes the callback runnable: a timer's task runs from
    /// the engine's pump with no V8 callback under it, so `with_context` — which
    /// the listener dispatch inside `task` reaches — would otherwise bootstrap a
    /// `v8::CallbackScope`, which is undefined behaviour off a callback. The
    /// guard is the same one [`crate::Engine::settle_xhr`] takes for the
    /// completion drain, and it is held for the whole of `task` because the whole
    /// of it is one native entry.
    fn schedule_background_task(&self, delay_ms: u64, task: Box<dyn FnOnce()>) {
        let host = Rc::clone(&self.host);
        self.host.scheduler.set_background_timeout(
            delay_ms,
            Box::new(move || {
                let _native = host.enter_native_dispatch();
                task();
            }),
        );
    }
}
