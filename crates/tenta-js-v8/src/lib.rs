//! V8 page execution with native WebIDL bindings and the shared HTML event loop.
//!
//! A page engine owns an isolate and wrapper world for its document. Page scripts,
//! timers, fetch/XHR completions and lifecycle events enter through the shared
//! engine contract, with watchdogs and sticky native-panic containment.
//! Isolate pooling, custom platform task routing, startup snapshots and worker
//! isolates remain open P2 packages (`docs/spec/05-javascript.md` §4).

mod host;
mod inspector;
mod modules;
mod observer;
mod page_engine;
mod promise;
mod script_host;
mod structured_clone;
mod termination;
mod world;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Once};
use std::time::Duration;

use tenta_js::bindings::WrapperWorld;
use tenta_js::bindings::host::{
    HostContext, HostObject, SharedContext, SharedDocument, deliver_image_events,
    deliver_xhr_completion, encode_host, encode_node, insert_fetched_response,
};
use tenta_js::bindings::{
    Handle, clear_v8_callback_admission, handle_from_v8, handle_to_v8,
    install_native_callback_state_for_isolate, install_v8_callback_admission,
    v8_isolate_callback_boundary,
};
use tenta_js::dom::events::{
    DispatchTarget, EventKind, EventRecord, ScriptHost, TargetKey, dispatch_at,
};
use tenta_js::engine::{LifecycleEvent, ScriptErrorKind};
use tenta_js::event_loop::{Clock, EventLoop, FiringRelease, MonoTime};
use tenta_js::fetch::{FetchCompletion, FetchOutcome};
use tenta_js::image::{ImageEvent, ImageHost};
use tenta_js::storage::PageStorage;
use tenta_js::xhr::XhrCompletion;
use tenta_net::CookieJar;

use host::{HostState, ReadyHandler};
use script_host::{V8ScriptHost, construct_global};

pub use page_engine::V8EngineFactory;

static PLATFORM_INIT: Once = Once::new();

/// Initialises the V8 platform exactly once for the process.
///
/// V8's platform and the global flags are process-global and may be
/// initialised only once; `Once` makes concurrent first calls from several
/// page threads safe. The dedicated engine-root-thread model of
/// `docs/spec/05-javascript.md` §4 (so descendant threads inherit the
/// thread-isolated JIT pkey) is layered on top of this in a later WP1 step.
pub fn ensure_platform() {
    PLATFORM_INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

/// A script error surfaced from V8, carrying the message V8 produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsError {
    pub message: String,
}

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for JsError {}

/// A script that did not run to completion, before [`Engine::run_inner`]
/// attaches the wording only it can know (which limit stopped it). The kind
/// records where the failure happened, which the message alone cannot say.
struct ScriptFailure {
    kind: ScriptErrorKind,
    message: String,
}

/// One V8 isolate with a single long-lived context. This is the seed of the
/// per-thread pooled isolate of `docs/spec/05-javascript.md` §4: it owns the
/// isolate, is neither `Send` nor `Sync`, and is created and used on one
/// thread. Reusing one [`Engine`] across evaluations keeps global state, which
/// a page needs between script runs.
pub struct Engine {
    // The isolate's near-heap-limit callback holds a raw pointer into
    // `heap_guard`; `Engine`'s `Drop` removes that callback before either field
    // is dropped, so this is sound regardless of field order.
    isolate: v8::OwnedIsolate,
    termination: Arc<termination::TerminationGate>,
    context: v8::Global<v8::Context>,
    heap_guard: Option<Box<HeapGuard>>,
    // The isolate's GC epilogue callback holds a raw pointer into `gc_counter`
    // for the same reason, and `Drop` removes it under the same discipline. A
    // `Box`'s pointee does not move when the `Engine` is moved by value out of
    // `build`, which is what keeps that pointer valid; a pointer into the
    // `Engine` itself would dangle the moment `build` returned.
    gc_counter: Box<GcCounter>,
    // The engine-independent event loop this isolate drives, plus the state the
    // host callbacks reach through the context slot. Both share the same
    // `Scheduler` and ready queue, so a `setTimeout` from script schedules into
    // the very loop `run_event_loop` drives.
    event_loop: EventLoop,
    host: Rc<HostState>,
    /// Shared with the page and any Send clone wire beyond this isolate.
    native_memory: tenta_core::NativeMemoryBudget,
    // The deadline every subsequent run is given, when the page set one. `None`
    // leaves the watchdog off, which is what one-shot evaluation wants.
    script_timeout: Option<std::time::Duration>,
    // Present when the engine was built with a DOM document. The engine keeps
    // its own clones of the wrapper world's state (and the document) so its
    // `Drop` can drain every host and disarm every finalizer while the isolate
    // is still alive, after the `V8World` itself has been moved into a slot.
    dom: Option<DomWorld>,
}

/// The DOM state an [`Engine::with_document`] engine keeps alongside the isolate
/// so it can tear the wrapper world down before the isolate is disposed.
struct DomWorld {
    state: Rc<RefCell<world::WorldState>>,
    ctx: SharedContext,
    // The promise seam's state, which lives in a context slot as well as here
    // so `Drop` can drop its `v8::Global` handles while the isolate is alive
    // (a context slot is not dropped until the isolate is disposed).
    promise: Rc<promise::PromiseState>,
}

/// Callback state for the near-heap-limit hook: a thread-safe handle to
/// terminate the offending script and a flag the owning [`Engine`] reads after
/// a run to turn that termination into a heap-limit [`JsError`].
struct HeapGuard {
    handle: v8::IsolateHandle,
    // Stable for the isolate's lifetime. The callback receives only `data`, so
    // it uses this address to record a panic in the isolate's shared marker.
    isolate: v8::UnsafeRawIsolatePtr,
    tripped: std::sync::atomic::AtomicBool,
}

/// Near-heap-limit callback: terminates the running script and raises the limit
/// enough for V8 to unwind instead of aborting the process
/// (`docs/spec/05-javascript.md` §11, `docs/spec/14-memory.md`).
unsafe extern "C" fn on_near_heap_limit(
    data: *mut std::os::raw::c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    // SAFETY: `data` is the pointer to the `HeapGuard` boxed in the owning
    // `Engine`, which outlives the isolate that holds this callback.
    let guard = unsafe { &*(data as *const HeapGuard) };
    // SAFETY: the guard's raw address belongs to the live isolate invoking
    // this callback. The shared reference is used only during this invocation.
    let isolate = unsafe { v8::Isolate::ref_from_raw_isolate_ptr(&guard.isolate) };
    let headroom = current_heap_limit.saturating_add(current_heap_limit / 2);
    v8_isolate_callback_boundary(isolate, "on_near_heap_limit", headroom, || {
        guard
            .tripped
            .store(true, std::sync::atomic::Ordering::Release);
        guard.handle.terminate_execution();
        // Headroom lets the interrupted allocation finish and unwind.
        headroom
    })
}

/// How many collections this isolate has completed, bumped by a GC epilogue
/// callback — the "after engine GC callbacks" trigger a detached-node sweep
/// reads (`docs/spec/04-dom-and-css.md` §1.1).
///
/// Boxed by its owning `Engine` so its address is stable across the move out of
/// [`Engine::build`]; see the `gc_counter` field.
struct GcCounter {
    epoch: std::sync::atomic::AtomicU64,
}

/// GC epilogue callback: records that a collection completed.
///
/// It may only bump the counter. It runs *during* collection, where touching
/// the document, the wrapper world or any `Rc<RefCell<_>>` would either panic or
/// hand out a recycled slot, so it dereferences one atomic and nothing else.
/// `kGCTypeAll` deliberately over-counts: a collection that freed nothing still
/// advances the epoch, and a spurious advance costs the sweep one pointless
/// check, while a missed one would leave a detached node pinned.
unsafe extern "C" fn on_gc_epilogue(
    isolate: v8::UnsafeRawIsolatePtr,
    _gc_type: v8::GCType,
    _flags: v8::GCCallbackFlags,
    data: *mut std::os::raw::c_void,
) {
    // SAFETY: V8 supplies its live isolate during this GC callback. No handle
    // is created and the reference does not escape the callback.
    let isolate = unsafe { v8::Isolate::ref_from_raw_isolate_ptr(&isolate) };
    v8_isolate_callback_boundary(isolate, "on_gc_epilogue", (), || {
        // SAFETY: `data` is the pointer to the `GcCounter` boxed in the owning
        // `Engine`, which outlives the isolate that holds this callback.
        let counter = unsafe { &*(data as *const GcCounter) };
        counter
            .epoch
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    });
}

// The isolates this thread has entered through an `IsolateEntry`, innermost
// last — the same stack V8 keeps, mirrored here only because V8 exposes no
// accessor for it.
//
// It exists for one purpose: letting `Engine`'s `Drop` turn "an engine was
// dropped while its own isolate was entered" into a message instead of a
// silent use-after-free. Debug-only, and read only by a `debug_assert!`, so a
// release build pays nothing for it.
#[cfg(debug_assertions)]
thread_local! {
    static ENTERED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// The address V8's own entry stack is keyed by: the `RealIsolate` an
/// [`v8::UnsafeRawIsolatePtr`] points at.
///
/// Both types are `repr(transparent)` over that one pointer, so
/// [`Isolate::ref_from_raw_isolate_ptr`](v8::Isolate::ref_from_raw_isolate_ptr)
/// hands back a view *of the pointer value*: the returned reference's own
/// address is the stack slot holding the pointer, and its contents are the
/// isolate's address. This reads the contents, which is the identity wanted;
/// the slot address would differ per call site.
#[cfg(debug_assertions)]
fn isolate_address(isolate: &v8::UnsafeRawIsolatePtr) -> usize {
    // SAFETY: `isolate` came from a live `Engine`, and the reference above is a
    // transparent view of one pointer, so reading one pointer-sized integer
    // through it is an in-bounds read of that value.
    let view: &v8::Isolate = unsafe { v8::Isolate::ref_from_raw_isolate_ptr(isolate) };
    let contents: *const usize = (view as *const v8::Isolate).cast();
    unsafe { *contents }
}

/// An RAII window during which this engine's isolate is the thread's current
/// one.
///
/// V8 ties a `HandleScope` and a `ContextScope` to the isolate that was current
/// when they were built: `ContextScope::new` compares the scope's isolate
/// against `Isolate::GetCurrent()` and **panics** when the two differ
/// (`scope.rs`). `Isolate::new` enters the isolate it creates and
/// `OwnedIsolate::drop` exits it, so a page thread hosting two engines has two
/// isolates competing for that one per-thread slot and the second engine's
/// entries panic. Every engine entry that touches V8 therefore opens this
/// window first, which makes the engine's own isolate current for exactly the
/// body of the entry.
///
/// # Why RAII and not a bare `enter()`/`exit()` pair
///
/// Every native boundary is wrapped in `catch_unwind`
/// (`docs/spec/02-architecture.md` §8) and `AGENTS.md` pins `panic = "unwind"`,
/// so an entry can unwind. A pair would leave the *previous* isolate current on
/// that path, and every later scope on the thread would be built on the wrong
/// isolate; the guard's `Drop` closes the window during unwind instead. Two
/// properties fall out of that, and both are requirements:
///
/// - the exit is **infallible**: it is one V8 call with no failure mode — a pop
///   of V8's per-thread entry stack — so the destructor cannot panic. A panic
///   there is a panic in a destructor during cleanup, which aborts the process,
///   and that is exactly the `SIGABRT` this guard exists to remove.
/// - the exit is **unconditional**: nothing disarms the guard on the happy path.
///
/// # Why the isolate is held as a raw pointer
///
/// The guard is created from `&self`, but the entries it covers mostly take
/// `&mut self` — `run_inner` and friends borrow `self.isolate` to open their
/// scopes — and [`Engine`]'s `Drop` must still run V8 teardown on `self.isolate`
/// before the field drops. Holding `&mut self.isolate` would make both
/// impossible, so the guard holds the isolate's *address* by value instead:
/// [`v8::UnsafeRawIsolatePtr`] is `Copy`, and the address of the `RealIsolate`
/// V8 allocates never changes, not even when the `Engine` is moved out of
/// `build` (which is the same fact `Engine::install_script_host` relies on).
///
/// # Invariant
///
/// **An [`Engine`] is never dropped while its own isolate is entered.** An
/// entry only ever pushes a frame onto V8's per-thread entry stack, so opening
/// one is sound at depth zero, where its matching exit restores the thread to
/// "nothing entered". At depth ≥ 1 it is not: dropping an engine inside its own
/// entry would leave the caller's frame pointing at a freed isolate once the
/// exit restored it. `Engine::drop` asserts this in debug builds; nothing does
/// it today — creation happens outside `with_runtime_for`, disposal in
/// `Page::close`'s task and in `ensure_runtime`'s rebuild, both between
/// entries.
///
/// The guard is neither `Send` nor `Sync`, because a raw isolate pointer is not.
struct IsolateEntry {
    isolate: v8::UnsafeRawIsolatePtr,
}

impl IsolateEntry {
    /// Enters `isolate` on this thread and returns the guard that undoes it.
    fn enter(isolate: v8::UnsafeRawIsolatePtr) -> Self {
        // SAFETY: `isolate` is the address of a live engine's isolate on this
        // thread (see `Engine::entered`, the only caller); entering it pushes a
        // frame onto V8's per-thread entry stack and touches nothing else.
        unsafe { v8::Isolate::ref_from_raw_isolate_ptr(&isolate).enter() };
        #[cfg(debug_assertions)]
        ENTERED.with(|stack| stack.borrow_mut().push(isolate_address(&isolate)));
        Self { isolate }
    }

    /// Whether `isolate` is entered on this thread.
    #[cfg(debug_assertions)]
    fn is_entered(isolate: v8::UnsafeRawIsolatePtr) -> bool {
        let address = isolate_address(&isolate);
        ENTERED.with(|stack| stack.borrow().contains(&address))
    }
}

impl Drop for IsolateEntry {
    fn drop(&mut self) {
        // SAFETY: the pointer is this engine's own, captured by
        // `IsolateEntry::enter`; an `Engine` is never dropped while a guard over
        // it is live (the invariant `Engine::drop` asserts), and `enter`/`exit`
        // is a per-thread stack, so this pops exactly the frame that pushed it.
        // The guard is neither `Send` nor `Sync`.
        unsafe { v8::Isolate::ref_from_raw_isolate_ptr(&self.isolate).exit() };
        #[cfg(debug_assertions)]
        ENTERED.with(|stack| {
            let popped = stack.borrow_mut().pop();
            debug_assert_eq!(
                popped,
                Some(isolate_address(&self.isolate)),
                "isolate entries were dropped out of the order they were entered"
            );
        });
    }
}

/// How one call of a queued handler ended, which is what a pump needs to know
/// before it may run the next one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CallOutcome {
    /// The timer was cancelled between its firing and its call, so no script
    /// ran. HTML aborts such a timer and script relies on it: the debounce
    /// idiom cancels a pending callback from another callback that came due in
    /// the same sweep.
    Cancelled,
    /// The callback ran and returned.
    Completed,
    /// A termination stopped it, so this pump's deadline has already fired and
    /// will not fire again for this entry.
    Terminated,
}

impl Engine {
    /// Builds an isolate with one fresh context and no heap cap.
    pub fn new() -> Self {
        Self::build(
            None,
            None,
            Clock::virtual_at(MonoTime::ZERO),
            CookieJar::new(),
            PageStorage::isolated(),
            tenta_core::NativeMemoryBudget::for_page(
                tenta_core::Budgets::platform_default().page_memory_mb,
            ),
        )
    }

    /// Builds an isolate whose heap is capped at `max_heap_bytes`; a script that
    /// exceeds it is terminated and reported as a [`JsError`] rather than
    /// crashing the process (`docs/spec/05-javascript.md` §11).
    pub fn with_heap_limit(max_heap_bytes: usize) -> Self {
        Self::build(
            Some(max_heap_bytes),
            None,
            Clock::virtual_at(MonoTime::ZERO),
            CookieJar::new(),
            PageStorage::isolated(),
            tenta_core::NativeMemoryBudget::for_page(
                tenta_core::Budgets::platform_default().page_memory_mb,
            ),
        )
    }

    /// Builds an isolate whose global object exposes the read-only DOM of
    /// `doc`: the `Node`, `Element` and `Document` interfaces and a `document`
    /// global bound to the document node (`docs/spec/05-javascript.md` §3).
    /// This is the Phase 0c vertical slice that proves the generated V8 glue
    /// end to end; it grows into the full binding surface in phase 1.
    ///
    /// The engine gets a jar of its own, so `document.cookie` works without a
    /// browsing context; a page supplies its context's jar through
    /// [`Self::with_document_on_clock`].
    pub fn with_document(doc: SharedDocument) -> Self {
        Self::build(
            None,
            Some(doc),
            Clock::virtual_at(MonoTime::ZERO),
            CookieJar::new(),
            PageStorage::isolated(),
            tenta_core::NativeMemoryBudget::for_page(
                tenta_core::Budgets::platform_default().page_memory_mb,
            ),
        )
    }

    /// Builds a DOM engine over `doc` driven by `clock`. A live page passes
    /// [`Clock::real`] so timers fire in wall time and `performance.now()`
    /// advances without anyone stepping it; headless work keeps the virtual
    /// clock the other constructors install, which makes timer ordering
    /// reproducible (`docs/spec/05-javascript.md` §5).
    ///
    /// A real-clock engine is driven by [`Self::next_wakeup`] and
    /// [`Self::pump_due_timers`] rather than [`Self::run_event_loop`], which
    /// drives the clock itself and so only moves a virtual one.
    ///
    /// `max_heap_bytes` caps the isolate's heap as [`Self::with_heap_limit`]
    /// does. It is a parameter rather than an afterthought because this is the
    /// constructor that runs *untrusted* page script: without a cap, script
    /// that allocates without bound aborts the process instead of producing a
    /// contained [`JsError`].
    ///
    /// `cookies` is the browsing context's jar, which `document.cookie` reads
    /// and writes. It is passed to the constructor rather than installed later
    /// because the context is built during `build`: the member must not depend
    /// on install order, and the jar a page supplies never changes identity. A
    /// caller with no context of its own passes [`CookieJar::new`].
    pub fn with_document_on_clock(
        doc: SharedDocument,
        clock: Clock,
        max_heap_bytes: Option<usize>,
        cookies: CookieJar,
    ) -> Self {
        Self::with_document_on_clock_and_storage(
            doc,
            clock,
            max_heap_bytes,
            cookies,
            PageStorage::isolated(),
        )
    }

    /// Builds a DOM engine with the page's storage namespace.
    pub fn with_document_on_clock_and_storage(
        doc: SharedDocument,
        clock: Clock,
        max_heap_bytes: Option<usize>,
        cookies: CookieJar,
        storage: PageStorage,
    ) -> Self {
        Self::with_document_on_clock_storage_and_memory(
            doc,
            clock,
            max_heap_bytes,
            cookies,
            storage,
            tenta_core::NativeMemoryBudget::for_page(
                tenta_core::Budgets::platform_default().page_memory_mb,
            ),
        )
    }

    /// Builds a DOM engine with the page's shared native memory ledger.
    pub fn with_document_on_clock_storage_and_memory(
        doc: SharedDocument,
        clock: Clock,
        max_heap_bytes: Option<usize>,
        cookies: CookieJar,
        storage: PageStorage,
        native_memory: tenta_core::NativeMemoryBudget,
    ) -> Self {
        Self::build(
            max_heap_bytes,
            Some(doc),
            clock,
            cookies,
            storage,
            native_memory,
        )
    }

    fn build(
        max_heap_bytes: Option<usize>,
        doc: Option<SharedDocument>,
        clock: Clock,
        cookies: CookieJar,
        storage: PageStorage,
        native_memory: tenta_core::NativeMemoryBudget,
    ) -> Self {
        ensure_platform();
        let mut params = v8::CreateParams::default();
        if let Some(max) = max_heap_bytes {
            params = params.heap_limits(0, max);
        }
        let mut isolate = v8::Isolate::new(params);

        // This slot must exist before any native V8 hook can fire, including
        // on a DOM-less engine that installs no generated WebIDL templates.
        install_native_callback_state_for_isolate(&mut isolate);

        // The event loop owns "when a task runs"; V8 must not drain microtasks on
        // its own, so the engine can run them at the HTML checkpoint instead.
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
        // Module scripts: `import()` leaves a request for the page to load, and
        // `import.meta` names the module's URL (`crate::modules`).
        isolate.set_host_import_module_dynamically_callback(modules::dynamic_import_callback);
        isolate.set_host_initialize_import_meta_object_callback(modules::import_meta_callback);

        let heap_guard = max_heap_bytes.map(|_| {
            let mut guard = Box::new(HeapGuard {
                handle: isolate.thread_safe_handle(),
                // SAFETY: the isolate is alive here and its RealIsolate address
                // stays fixed until Engine removes the callback during Drop.
                isolate: unsafe { isolate.as_raw_isolate_ptr() },
                tripped: std::sync::atomic::AtomicBool::new(false),
            });
            let data = (&mut *guard as *mut HeapGuard).cast::<std::os::raw::c_void>();
            isolate.add_near_heap_limit_callback(on_near_heap_limit, data);
            guard
        });

        let gc_counter = {
            let mut counter = Box::new(GcCounter {
                epoch: std::sync::atomic::AtomicU64::new(0),
            });
            let data = (&mut *counter as *mut GcCounter).cast::<std::os::raw::c_void>();
            isolate.add_gc_epilogue_callback(on_gc_epilogue, data, v8::GCType::kGCTypeAll);
            counter
        };

        let termination = termination::TerminationGate::new(isolate.thread_safe_handle());
        let event_loop = EventLoop::new(clock);
        let host = HostState::new(
            event_loop.scheduler(),
            cookies,
            native_memory.clone(),
            Arc::clone(&termination),
        );
        let engine_native_memory = native_memory.clone();

        let (context, dom) = {
            v8::scope!(let handle_scope, &mut isolate);
            match doc {
                None => {
                    let local = v8::Context::new(handle_scope, Default::default());
                    {
                        let mut context_scope = v8::ContextScope::new(handle_scope, local);
                        v8::scope!(let scope, &mut context_scope);
                        host::install(scope, local, host.scheduler.clock().time_origin_ms());
                        install_dom_exception(scope);
                    }
                    // The host callbacks reach this state through the context slot.
                    local.set_slot::<HostState>(host.clone());
                    (v8::Global::new(handle_scope, local), None)
                }
                Some(doc) => Self::install_dom(handle_scope, &host, doc, storage, native_memory),
            }
        };
        let mut engine = Engine {
            isolate,
            termination,
            context,
            heap_guard,
            gc_counter,
            event_loop,
            host,
            native_memory: engine_native_memory,
            script_timeout: None,
            dom,
        };
        if let Some(dom) = &engine.dom {
            let admission: Rc<dyn tenta_js::bindings::V8CallbackAdmission> =
                Rc::new(world::cycles::Admission::new(
                    Rc::clone(&dom.state),
                    dom.ctx.clone(),
                    Arc::clone(&engine.termination),
                ));
            install_v8_callback_admission(&mut engine.isolate, admission);
        }
        engine.install_script_host();
        // `Isolate::new` leaves the isolate it built **entered**, and that entry
        // has to be given back before `build` returns. A settled `Engine` is
        // then not the thread's current isolate, so building a second engine
        // pushes 0 -> 1 -> 0 instead of leaving a stack behind, and every entry
        // below opens a window of its own ([`IsolateEntry`]).
        //
        // SAFETY: `engine.isolate` is the isolate `Isolate::new` entered above,
        // and nothing has exited it since — every scope in this function nests
        // inside that one entry — so this pops exactly that frame.
        unsafe { engine.isolate.exit() };
        engine
    }

    /// The stable address of this engine's V8 isolate, captured by value.
    ///
    /// The `RealIsolate` V8 allocates never moves, unlike the `OwnedIsolate`
    /// field that points at it, so the address survives the `Engine` being
    /// moved out of [`Self::build`] or boxed by the engine factory.
    fn raw_isolate(&self) -> v8::UnsafeRawIsolatePtr {
        // SAFETY: `self.isolate` is alive for `&self`'s lifetime, so this reads
        // a live isolate's address and dereferences nothing.
        unsafe { self.isolate.as_raw_isolate_ptr() }
    }

    /// Enters this engine's isolate for as long as the returned guard lives.
    ///
    /// Every entry that touches V8 opens one of these, so the scopes it builds
    /// belong to *this* engine's isolate rather than to whichever engine
    /// happens to be the thread's current one. See [`IsolateEntry`].
    fn entered(&self) -> IsolateEntry {
        IsolateEntry::enter(self.raw_isolate())
    }

    /// Wires the DOM event dispatcher's callback boundary to this engine's
    /// isolate. The dispatcher ([`tenta_js::dom::events`]) invokes listeners only
    /// through [`ScriptHost`]; a [`V8ScriptHost`] over this isolate turns handle
    /// bits back into V8 objects and calls the stored functions. Stored in the
    /// `HostContext` the bindings already share, and dropped in `Drop` before the
    /// wrapper world is cleared. A no-op for a DOM-less engine.
    fn install_script_host(&mut self) {
        let Some(dom) = &self.dom else {
            return;
        };
        // The stable address of the underlying V8 isolate, captured by value.
        // A raw `*mut v8::Isolate` would point into `self.isolate`'s
        // `NonNull<RealIsolate>` field, which moves with the `Engine` when
        // `build` returns it by value, leaving `V8ScriptHost` with a dangling
        // pointer; the raw isolate address itself never moves.
        //
        // The stored host is only ever reconstructed inside `invoke_listener`,
        // which runs synchronously inside a DOM binding call on this engine's
        // thread while the engine (and thus the isolate) is alive; `Drop`
        // clears the stored host before the isolate is disposed.
        let isolate = self.raw_isolate();
        let script: Rc<dyn ScriptHost> = Rc::new(V8ScriptHost::new(
            isolate,
            self.context.clone(),
            dom.state.clone(),
            self.host.clone(),
        ));
        *dom.ctx.script.borrow_mut() = Some(script);
        self.host.custom_elements.set_context(&dom.ctx);
        let context = Rc::downgrade(&dom.ctx);
        *self.host.mutation_notify.borrow_mut() = Some(Rc::new(move || {
            if let Some(context) = context.upgrade() {
                tenta_js::bindings::host::notify_mutation_observers(&context);
            }
        }));
    }

    /// Creates the DOM-enabled context: installs `Node`, `Element` and
    /// `Document` on a fresh global template (parent-first, so each finds its
    /// parent constructor in the isolate's template cache), builds the context
    /// from it, binds the `document` global and hands the wrapper world to the
    /// runtime. Returns the context and the state the engine drains at teardown.
    fn install_dom(
        handle_scope: &mut v8::PinScope<'_, '_, ()>,
        host: &Rc<HostState>,
        doc: SharedDocument,
        storage: PageStorage,
        native_memory: tenta_core::NativeMemoryBudget,
    ) -> (v8::Global<v8::Context>, Option<DomWorld>) {
        let state = Rc::new(RefCell::new(world::WorldState::default()));
        // Events timestamp against the loop clock, so `evt.timeStamp` and
        // `performance.now()` are readings of the same clock.
        let (ctx, document_ref) = HostContext::new_with_installed_observer(
            doc.clone(),
            host.scheduler.clock(),
            host.cookies.clone(),
            storage,
            native_memory,
        );

        // The generated installers require a context-carrying scope, but the
        // global template must exist before the context built from it. A
        // throwaway bootstrap context supplies that scope; the templates it
        // builds are isolate-global, so the real context inherits every
        // interface.
        let bootstrap = v8::Context::new(handle_scope, Default::default());
        let mut bootstrap_scope = v8::ContextScope::new(handle_scope, bootstrap);
        v8::scope!(let scope, &mut bootstrap_scope);

        let global_template = v8::ObjectTemplate::new(scope);
        // The window's `on*` handler attributes are accessors, so they have to
        // be on the template the global object is built from rather than added
        // to the object afterwards (`host::install_window_event_handlers`).
        host::install_window_event_handlers(scope, global_template);
        host::install_window_storage_accessors(scope, global_template);
        host::install_window_crypto_accessor(scope, global_template);
        host::install_window_geometry_accessors(scope, global_template);
        // This one template outlives the block: the window's prototype chain is
        // chained to `EventTarget.prototype` once the context exists
        // (`host::install_window_prototype`).
        let event_target;
        // The interface objects are not put on the template the global object
        // is built from. V8 instantiates every function template it finds
        // there while it bootstraps the context, with each prototype in fast
        // mode, where every added member copies the prototype's descriptor
        // array: the cost grew with the square of an interface's member count
        // and came to more than half of building an engine. The generated
        // installers still want a template to write to, so they get one that
        // is never instantiated, and `install_constructors` defines the
        // interface objects on the global once the context exists.
        let interface_sink = v8::ObjectTemplate::new(scope);
        let mut registry = world::SkeletonRegistry::default();
        {
            // Install interfaces parent-first: EventTarget is the root (Node
            // inherits it), so it must precede Node.
            event_target = tenta_js::bindings::install_event_target::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the EventTarget interface");
            tenta_js::bindings::install_node::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Node interface");
            tenta_js::bindings::install_character_data::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CharacterData interface");
            tenta_js::bindings::install_text::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Text interface");
            // CDATASection : Text, so it installs after Text. A `CData` node's
            // wrapper is picked by kind in `world.rs`.
            tenta_js::bindings::install_cdata_section::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CDATASection interface");
            tenta_js::bindings::install_comment::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Comment interface");
            tenta_js::bindings::install_element::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Element interface");
            // SVGElement : Element — installs after Element; SVG-namespace
            // elements get this prototype (see world.rs interface selection).
            tenta_js::bindings::install_svg_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the SVGElement interface");
            // HTMLElement's prototype chain roots at Element, so it installs
            // after Element and before Document (HTML-namespace elements get
            // the HTMLElement prototype; see world.rs interface selection).
            tenta_js::bindings::install_html_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLElement interface");
            // HTMLIFrameElement : HTMLElement — installs after HTMLElement;
            // `<iframe>` elements get this prototype. (The snake name fuses the
            // `HTMLI` capital run per the generator's word splitter.)
            tenta_js::bindings::install_htmli_frame_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLIFrameElement interface");
            // HTMLTemplateElement : HTMLElement — its `content` returns a
            // DocumentFragment, so DocumentFragment must be installed too.
            tenta_js::bindings::install_html_template_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLTemplateElement interface");
            // Phase 4d per-tag interfaces, all `: HTMLElement`, so they install
            // after it. Each gives its tag's wrappers a distinct prototype and a
            // global constructor for `instanceof` (see world.rs tag selection).
            tenta_js::bindings::install_html_div_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLDivElement interface");
            tenta_js::bindings::install_html_span_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLSpanElement interface");
            tenta_js::bindings::install_html_paragraph_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLParagraphElement interface");
            // EL1: the element interfaces declared without members.
            tenta_js::bindings::install_html_html_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLHtmlElement interface");
            tenta_js::bindings::install_html_head_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLHeadElement interface");
            tenta_js::bindings::install_html_title_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLTitleElement interface");
            tenta_js::bindings::install_html_base_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLBaseElement interface");
            tenta_js::bindings::install_html_body_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLBodyElement interface");
            tenta_js::bindings::install_html_heading_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLHeadingElement interface");
            tenta_js::bindings::install_htmlhr_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLHRElement interface");
            tenta_js::bindings::install_html_pre_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLPreElement interface");
            tenta_js::bindings::install_html_quote_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLQuoteElement interface");
            tenta_js::bindings::install_htmlo_list_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLOListElement interface");
            tenta_js::bindings::install_htmlu_list_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLUListElement interface");
            tenta_js::bindings::install_html_menu_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLMenuElement interface");
            tenta_js::bindings::install_htmlli_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLLIElement interface");
            tenta_js::bindings::install_htmld_list_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLDListElement interface");
            tenta_js::bindings::install_html_data_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLDataElement interface");
            tenta_js::bindings::install_html_time_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLTimeElement interface");
            tenta_js::bindings::install_htmlbr_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLBRElement interface");
            tenta_js::bindings::install_html_mod_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLModElement interface");
            tenta_js::bindings::install_html_picture_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLPictureElement interface");
            tenta_js::bindings::install_html_embed_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLEmbedElement interface");
            tenta_js::bindings::install_html_object_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLObjectElement interface");
            tenta_js::bindings::install_html_track_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLTrackElement interface");
            tenta_js::bindings::install_html_map_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLMapElement interface");
            tenta_js::bindings::install_html_area_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLAreaElement interface");
            tenta_js::bindings::install_html_table_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLTableElement interface");
            tenta_js::bindings::install_html_table_caption_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLTableCaptionElement interface");
            tenta_js::bindings::install_html_table_col_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLTableColElement interface");
            tenta_js::bindings::install_html_table_section_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLTableSectionElement interface");
            tenta_js::bindings::install_html_table_row_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLTableRowElement interface");
            tenta_js::bindings::install_html_table_cell_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLTableCellElement interface");
            tenta_js::bindings::install_html_field_set_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLFieldSetElement interface");
            tenta_js::bindings::install_html_legend_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLLegendElement interface");
            tenta_js::bindings::install_html_opt_group_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLOptGroupElement interface");
            tenta_js::bindings::install_html_data_list_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLDataListElement interface");
            tenta_js::bindings::install_html_output_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLOutputElement interface");
            tenta_js::bindings::install_html_progress_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLProgressElement interface");
            tenta_js::bindings::install_html_meter_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLMeterElement interface");
            tenta_js::bindings::install_html_details_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLDetailsElement interface");
            tenta_js::bindings::install_html_param_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLParamElement interface");
            tenta_js::bindings::install_html_font_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLFontElement interface");
            tenta_js::bindings::install_html_directory_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLDirectoryElement interface");
            tenta_js::bindings::install_html_frame_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLFrameElement interface");
            tenta_js::bindings::install_html_frame_set_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLFrameSetElement interface");
            tenta_js::bindings::install_html_marquee_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLMarqueeElement interface");
            tenta_js::bindings::install_html_unknown_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLUnknownElement interface");
            tenta_js::bindings::install_html_anchor_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLAnchorElement interface");
            tenta_js::bindings::install_html_image_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLImageElement interface");
            tenta_js::bindings::install_html_label_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLLabelElement interface");
            tenta_js::bindings::install_html_form_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLFormElement interface");
            tenta_js::bindings::install_html_input_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLInputElement interface");
            tenta_js::bindings::install_html_button_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLButtonElement interface");
            tenta_js::bindings::install_html_select_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLSelectElement interface");
            tenta_js::bindings::install_html_text_area_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLTextAreaElement interface");
            tenta_js::bindings::install_html_option_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLOptionElement interface");
            tenta_js::bindings::install_html_script_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLScriptElement interface");
            tenta_js::bindings::install_html_link_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLLinkElement interface");
            tenta_js::bindings::install_html_style_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLStyleElement interface");
            // IF1: media, canvas and the SVG interfaces, each after its parent.
            tenta_js::bindings::install_html_media_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLMediaElement interface");
            tenta_js::bindings::install_html_video_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLVideoElement interface");
            tenta_js::bindings::install_html_audio_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLAudioElement interface");
            tenta_js::bindings::install_html_canvas_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLCanvasElement interface");
            tenta_js::bindings::install_html_slot_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLSlotElement interface");
            tenta_js::bindings::install_html_meta_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLMetaElement interface");
            tenta_js::bindings::install_html_source_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLSourceElement interface");
            tenta_js::bindings::install_html_dialog_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLDialogElement interface");
            tenta_js::bindings::install_svg_graphics_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the SVGGraphicsElement interface");
            tenta_js::bindings::install_svgsvg_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the SVGSVGElement interface");
            tenta_js::bindings::install_svga_element::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the SVGAElement interface");
            // `DOMRect` extends `DOMRectReadOnly`, so the base installs first.
            tenta_js::bindings::install_dom_rect_read_only::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the DOMRectReadOnly interface");
            tenta_js::bindings::install_dom_rect::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the DOMRect interface");
            tenta_js::bindings::install_dom_rect_list::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the DOMRectList interface");
            tenta_js::bindings::install_document_fragment::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the DocumentFragment interface");
            tenta_js::bindings::install_shadow_root::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the ShadowRoot interface");
            tenta_js::bindings::install_document::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the Document interface");
            // DocumentType : Node and ProcessingInstruction : CharacterData —
            // both roots are installed above, so both install here. A doctype
            // and a processing instruction each get a prototype of their own
            // rather than the `Node` shell (see world.rs interface selection).
            tenta_js::bindings::install_document_type::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the DocumentType interface");
            tenta_js::bindings::install_processing_instruction::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the ProcessingInstruction interface");
            // DOMImplementation has no DOM parent, so its install order relative
            // to the node types is free. It is installed so `Document.implementation`
            // has a prototype and a global constructor.
            tenta_js::bindings::install_dom_implementation::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the DOMImplementation interface");
            tenta_js::bindings::install_dom_parser::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the DOMParser interface");
            tenta_js::bindings::install_xml_serializer::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the XMLSerializer interface");
            // AbstractRange roots at Object; StaticRange and Range both derive
            // from it, so it is installed first and the two concrete interfaces
            // after it. Neither is a node: a range spends no handle tag, it is a
            // host record under TAG_HOST (see `HostRecord::Range`).
            tenta_js::bindings::install_abstract_range::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the AbstractRange interface");
            tenta_js::bindings::install_static_range::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the StaticRange interface");
            tenta_js::bindings::install_range::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Range interface");
            // Neither is a node: both are host records under TAG_HOST. A record
            // is installed with its observer, which hands the callback its
            // `MutationRecord`s.
            tenta_js::bindings::install_mutation_record::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the MutationRecord interface");
            tenta_js::bindings::install_mutation_observer::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the MutationObserver interface");
            tenta_js::bindings::install_node_list::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the NodeList interface");
            tenta_js::bindings::install_html_collection::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the HTMLCollection interface");
            tenta_js::bindings::install_dom_token_list::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the DOMTokenList interface");
            tenta_js::bindings::install_dom_string_map::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the DOMStringMap interface");
            // `Attr : Node` requires Node's template to be installed first.
            // `NamedNodeMap` is independent and resolves Attr wrappers through
            // the isolate's template cache when its members run.
            tenta_js::bindings::install_attr::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Attr interface");
            tenta_js::bindings::install_named_node_map::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the NamedNodeMap interface");
            tenta_js::bindings::install_style_sheet::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the StyleSheet interface");
            tenta_js::bindings::install_css_style_sheet::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSStyleSheet interface");
            tenta_js::bindings::install_css_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSRule interface");
            tenta_js::bindings::install_css_grouping_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSGroupingRule interface");
            tenta_js::bindings::install_css_style_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSStyleRule interface");
            tenta_js::bindings::install_css_condition_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSConditionRule interface");
            tenta_js::bindings::install_css_media_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSMediaRule interface");
            tenta_js::bindings::install_media_list::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the MediaList interface");
            tenta_js::bindings::install_css_supports_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSSupportsRule interface");
            tenta_js::bindings::install_css_layer_block_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSLayerBlockRule interface");
            tenta_js::bindings::install_css_layer_statement_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSLayerStatementRule interface");
            tenta_js::bindings::install_css_scope_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSScopeRule interface");
            tenta_js::bindings::install_css_namespace_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSNamespaceRule interface");
            tenta_js::bindings::install_css_property_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSPropertyRule interface");
            tenta_js::bindings::install_css_keyframes_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSKeyframesRule interface");
            tenta_js::bindings::install_css_keyframe_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSKeyframeRule interface");
            tenta_js::bindings::install_css_font_face_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSFontFaceRule interface");
            tenta_js::bindings::install_css_font_feature_values_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSFontFeatureValuesRule interface");
            tenta_js::bindings::install_css_nested_declarations::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSNestedDeclarations interface");
            tenta_js::bindings::install_css_import_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSImportRule interface");
            tenta_js::bindings::install_css_page_rule::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSPageRule interface");
            tenta_js::bindings::install_css_rule_list::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSRuleList interface");
            tenta_js::bindings::install_style_sheet_list::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the StyleSheetList interface");
            tenta_js::bindings::install_css_style_declaration::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSStyleDeclaration interface");
            tenta_js::bindings::install_css_style_properties::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CSSStyleProperties interface");
            // A namespace is a plain object, not an interface object: it has no
            // function template to instantiate later, and stays on the template.
            tenta_js::bindings::install_css::<HostObject>(scope, global_template, &mut registry)
                .expect("install the CSS namespace");
            tenta_js::bindings::install_storage::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Storage interface");
            tenta_js::bindings::install_crypto::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Crypto interface");
            // Event has no DOM parent (its prototype chain roots at Object), so
            // its install order relative to the node types is free.
            tenta_js::bindings::install_event::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Event interface");
            // CustomEvent's prototype chain roots at Event, so it installs after.
            tenta_js::bindings::install_custom_event::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CustomEvent interface");
            // UIEvent : Event, then the UI event subclasses (parent-first).
            tenta_js::bindings::install_ui_event::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the UIEvent interface");
            tenta_js::bindings::install_composition_event::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the CompositionEvent interface");
            tenta_js::bindings::install_focus_event::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the FocusEvent interface");
            tenta_js::bindings::install_keyboard_event::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the KeyboardEvent interface");
            tenta_js::bindings::install_mouse_event::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the MouseEvent interface");
            tenta_js::bindings::install_wheel_event::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the WheelEvent interface");
            // ProgressEvent's prototype chain also roots at Event.
            tenta_js::bindings::install_progress_event::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the ProgressEvent interface");
            // So does ErrorEvent's.
            tenta_js::bindings::install_error_event::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the ErrorEvent interface");
            // `URL` and `URLSearchParams` carry no parent chain of their own;
            // their order among themselves does not matter, but
            // `URLSearchParams` must precede no one either — `URL.searchParams`
            // resolves the template through the isolate's cache at call time.
            // `Headers` roots at `Object` too, and names its constructor from
            // the interface name alone.
            tenta_js::bindings::install_url_search_params::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the URLSearchParams interface");
            tenta_js::bindings::install_url::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the URL interface");
            tenta_js::bindings::install_headers::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Headers interface");
            // `AbortSignal : EventTarget`, so `EventTarget` must already be in
            // the template cache; `AbortController` roots at `Object`. Installing
            // the signal first keeps the two in dependency order, and both are
            // independent of the URL block above.
            tenta_js::bindings::install_abort_signal::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the AbortSignal interface");
            tenta_js::bindings::install_abort_controller::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the AbortController interface");
            // The encoding pair roots at `Object` and names its constructors
            // from the interface name alone, so its order among the JS-owned
            // hosts does not matter. `TextDecoder`'s `decode` is the second
            // consumer of the buffer-source conversion after the toy fixture.
            tenta_js::bindings::install_text_encoder::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the TextEncoder interface");
            tenta_js::bindings::install_text_decoder::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the TextDecoder interface");
            // `Response` roots at `Object` too. It is installed last of the
            // JS-owned hosts because it is the only one that *owns* another
            // (`Response.headers`), and a `Response` constructor fills that
            // `Headers` through the same record it hands out here.
            tenta_js::bindings::install_response::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the Response interface");
            // `Request` owns a `Headers` the way `Response` does, and its
            // `signal` answers with an `AbortSignal`, so both are installed
            // before it.
            tenta_js::bindings::install_request::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Request interface");
            // `XMLHttpRequest` is an `EventTarget` twice over — its own two-level
            // chain is `XMLHttpRequest -> XMLHttpRequestEventTarget ->
            // EventTarget` — so the event target interface installs first, and
            // `EventTarget` before it, which the block above already did.
            tenta_js::bindings::install_xml_http_request_event_target::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the XMLHttpRequestEventTarget interface");
            tenta_js::bindings::install_xml_http_request::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the XMLHttpRequest interface");
            // `File` is a `Blob` — its chain is `File -> Blob -> Object` — and
            // `constructor.inherit` reads the parent's template out of the
            // isolate's template cache, so the parent installs first. The
            // generated `install_file` also demands `BlobOps` and
            // `BlobConstructor` of its type parameter, which is the same fact
            // stated in the type system.
            tenta_js::bindings::install_blob::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the Blob interface");
            tenta_js::bindings::install_file::<HostObject>(scope, interface_sink, &mut registry)
                .expect("install the File interface");
            // `FormData` roots at `Object` too, and it is installed after `File`
            // for a reason that is not a prototype chain: one of its operations
            // *mints* a `File` (`host.rs`'s `entry_file`), and the wrapper it
            // hands the page is created from the template this call caches.
            tenta_js::bindings::install_form_data::<HostObject>(
                scope,
                interface_sink,
                &mut registry,
            )
            .expect("install the FormData interface");
        }

        let local = v8::Context::new(
            scope,
            v8::ContextOptions {
                global_template: Some(global_template),
                ..Default::default()
            },
        );
        {
            let mut context_scope = v8::ContextScope::new(scope, local);
            v8::scope!(let inner, &mut context_scope);
            registry
                .install_constructors(inner, local)
                .expect("define the interface objects on the global");
            host::install(inner, local, host.scheduler.clock().time_origin_ms());
            registry
                .link_constructor_prototypes(inner)
                .expect("link interface constructors in the real context");
            // The window is an `EventTarget`, so its prototype chain has to
            // reach that interface's prototype — the one fact about the window
            // the generated glue cannot express, since the window is the global
            // object rather than an instance of any template.
            host::install_window_prototype(inner, local, &event_target);
            install_dom_exception(inner);
            world::cycles::initialize(inner, &mut state.borrow_mut())
                .expect("capture native listener-collection intrinsics");
            state
                .borrow_mut()
                .capture_buffer_intrinsic(inner)
                .expect("capture native ArrayBuffer resizable getter");

            // Materialise the `document` wrapper, expose it as a global, then
            // hand the world to the runtime so the generated getters can create
            // further wrappers (e.g. `documentElement`) on demand.
            let world = world::V8World::new(state.clone(), ctx.clone());
            let document_object = world
                .wrapper(
                    inner,
                    "Document",
                    tenta_js::bindings::host::encode_node(ctx.slot, document_ref),
                )
                .expect("document wrapper admission")
                .expect("the document wrapper");
            let key = v8::String::new(inner, "document").expect("`document` fits the string limit");
            local
                .global(inner)
                .set(inner, key.into(), document_object.into());
            tenta_js::bindings::set_wrapper_world(inner, Box::new(world))
                .expect("capture legacy property intrinsics and install the wrapper world");
        }
        // The host callbacks reach this state through the context slot.
        local.set_slot::<HostState>(host.clone());
        // The promise seam's job table. It belongs to the context rather than to
        // either side of the seam: a settlement runs inside a binding call and
        // the microtask that applies it runs from V8's queue, and the context
        // slot is the only thing both can reach (`crate::promise`). The engine
        // keeps a clone so `Drop` can clear it while the isolate is alive.
        let promise = promise::PromiseState::new(state.clone());
        local.set_slot::<promise::PromiseState>(promise.clone());
        (
            v8::Global::new(scope, local),
            Some(DomWorld {
                state,
                ctx,
                promise,
            }),
        )
    }

    /// Sets the deadline every subsequent [`Self::run_script`] is given, or
    /// clears it with `None`. The page carries this on its engine configuration
    /// ([`tenta_js::engine::PageEngineConfig::script_timeout`]) and the engine
    /// boundary's `run_script` takes no timeout argument, so the engine has to
    /// remember it. One-shot evaluation ([`Self::evaluate`]) is unaffected: it
    /// is the no-deadline path.
    pub fn set_script_timeout(&mut self, timeout: Option<std::time::Duration>) {
        self.script_timeout = timeout;
    }

    /// Compiles and runs `source` in this engine's context. Global state set by
    /// an earlier call is still visible. A compile-time or runtime exception is
    /// returned as [`JsError`] rather than unwinding across the V8 boundary
    /// (`docs/spec/02-architecture.md` §8).
    pub fn evaluate(&mut self, source: &str) -> Result<String, JsError> {
        self.run(source, None)
    }

    /// Forces a full garbage collection (test/diagnostic only). The low-memory
    /// notification runs a full GC synchronously and is safe for a browser
    /// build, unlike exposing `gc()` to web content.
    pub fn low_memory_gc(&mut self) {
        let _entry = self.entered();
        self.isolate.low_memory_notification();
    }

    /// Applies pending finalizers for diagnostics after [`Engine::low_memory_gc`].
    /// All kinds share the same poison-aware reclamation path used by the
    /// wrapper collector, including event details and host-owned listeners.
    fn reclaim_finalized_bookkeeping(&mut self) {
        let _entry = self.entered();
        if self.native_failure().is_some() {
            return;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.dom.as_ref().map_or(Ok(false), |dom| {
                world::reclaim_dead_bookkeeping(&self.isolate, &dom.state, &dom.ctx.documents)
            })
        }));
        match result {
            Ok(Ok(_)) => {}
            Ok(Err(())) => {
                tenta_js::bindings::poison_native_callback(
                    &self.isolate,
                    "finalized wrapper reclamation",
                );
            }
            Err(payload) => {
                tenta_js::bindings::poison_native_callback(
                    &self.isolate,
                    "finalized wrapper reclamation",
                );
                if let Err(nested) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)))
                {
                    std::mem::forget(nested);
                }
            }
        }
    }

    /// Drains event wrappers a finalizer has collected, releasing their slab
    /// records and any interned `CustomEvent` detail (test-only; the same sweep
    /// runs lazily at the head of `wrapper`). Call after [`Engine::low_memory_gc`]
    /// to make reclamation observable without waiting for the next wrapper.
    pub fn reclaim_dead_events(&mut self) {
        self.reclaim_finalized_bookkeeping();
    }

    /// The number of live stored values in the wrapper world (test-only): a
    /// leaked `CustomEvent` detail keeps its slot occupied after the event is
    /// reclaimed.
    pub fn stored_value_count(&self) -> usize {
        self.dom
            .as_ref()
            .map(|dom| dom.state.borrow().value_count())
            .unwrap_or(0)
    }

    /// Drains host-object wrappers a finalizer has collected, freeing their
    /// slab records — along with whatever those records owned, such as a freed
    /// signal's listeners and abort reason (test-only; the same sweep runs
    /// lazily at the head of `wrapper`). Call after [`Engine::low_memory_gc`] to
    /// make reclamation observable without waiting for the next wrapper.
    pub fn reclaim_dead_hosts(&mut self) {
        self.reclaim_finalized_bookkeeping();
    }

    /// The number of live `URL`, `URLSearchParams`, `Headers`,
    /// `AbortController` and `AbortSignal` records in this engine's world
    /// (test-only): a record whose wrapper has been collected is still counted
    /// until [`Engine::reclaim_dead_hosts`] drains it, which is what makes the
    /// finalizer path observable.
    pub fn host_record_count(&self) -> usize {
        self.dom
            .as_ref()
            .map(|dom| dom.ctx.host_records.borrow().len())
            .unwrap_or(0)
    }

    /// The number of listeners registered in this engine's world (test-only).
    /// A listener registered on a node the document later swept stays in the
    /// table, and keeps the stored callback it owns alive, until the next
    /// `PageEngine::trim_memory` drains the swept node's entries.
    pub fn listener_count(&self) -> usize {
        self.dom
            .as_ref()
            .map(|dom| dom.ctx.listeners.borrow().len())
            .unwrap_or(0)
    }

    /// The handle bits of the interface object the global expression
    /// `expression` evaluates to (test-only).
    ///
    /// A driver learns its target's bits from the object itself, and no page
    /// script can hand them to Rust: they are the wrapper world's private
    /// identity for the object, not a value any surface exposes. So a test that
    /// has to deliver a *native* completion at an object it created from script
    /// — an `XMLHttpRequest` its own completion drives — has no way to name
    /// that object, and this is that way. Nothing but a test reads it.
    ///
    /// The expression is compiled and run rather than looked up on the global
    /// object, because a `v8::Object::get` on a context's global proxy does not
    /// reach the global object's own properties (the same measured fact the
    /// wrapper world's own receiver path is built around).
    ///
    /// `T` types the returned handle only: the world's conversion asks the
    /// object for its bits and does not check the interface it was named with,
    /// so any marker answers for any object. `None` when the expression throws,
    /// or when it names neither an interface object nor the window.
    pub fn global_handle_bits<T: tenta_js::bindings::InterfaceMarker>(
        &mut self,
        expression: &str,
    ) -> Option<Handle<T>> {
        if self.native_failure().is_some() {
            return None;
        }
        // Running the expression is entering script, so this entry clears a
        // termination a watchdog left pending, like every other way in.
        let _entry = self.entered();
        self.discard_stray_termination();
        v8::scope!(let handle_scope, &mut self.isolate);
        let context = v8::Local::new(handle_scope, &self.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        // A `TryCatch` so an expression that throws leaves no pending exception
        // behind on the isolate.
        v8::tc_scope!(let tc, scope);
        let source = v8::String::new(tc, expression)?;
        let script = v8::Script::compile(tc, source, None)?;
        let value = script.run(tc)?;
        handle_from_v8::<T>(tc, value).ok()
    }

    /// Like [`Engine::evaluate`], but a watchdog thread requests termination if
    /// the script runs longer than `timeout` (`docs/spec/05-javascript.md` §11).
    pub fn evaluate_with_timeout(
        &mut self,
        source: &str,
        timeout: std::time::Duration,
    ) -> Result<String, JsError> {
        self.run(source, Some(timeout))
    }

    /// The shared run path: an optional deadline watchdog, one context-scoped
    /// evaluation, then translation of a termination into the right [`JsError`]
    /// (heap-limit or deadline) and a reset so the isolate stays usable.
    ///
    /// A thin mapper over [`Self::run_inner`]: the message is already final by
    /// the time a failure leaves there, so this only restates it as a
    /// [`JsError`].
    fn run(
        &mut self,
        source: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<String, JsError> {
        self.run_inner(source, timeout).map_err(|failure| JsError {
            message: failure.message,
        })
    }

    /// Runs `source` and reports only *where* it failed, for a caller at the
    /// engine boundary that has no use for the evaluated value.
    ///
    /// A thin mapper over [`Self::run_inner`], as [`Self::run`] is: the body —
    /// the watchdog, the termination reset and the heap/deadline wording — is
    /// shared, never duplicated.
    fn run_classified(
        &mut self,
        source: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<(), ScriptFailure> {
        // The script is page script, so its exception is an uncaught one the
        // inspector reports; `run_inner` is the host's own evaluation, whose
        // exception goes back to its caller instead.
        let host = Rc::clone(&self.host);
        self.run_with(timeout, |tc| compile_and_run(tc, source, Some(&host)))
            .map(|_| ())
    }

    /// Whether the module `key` has been compiled into this realm's module
    /// map.
    pub fn has_module(&self, key: &str) -> bool {
        self.host.modules.has(key)
    }

    /// Adds a document's import map to the realm's. `base` is the document's
    /// base URL, which the map's own URLs resolve against.
    pub fn add_import_map(&mut self, json: &str, base: &url::Url) -> Result<(), JsError> {
        let map = tenta_js::modules::ImportMap::parse(json, base)
            .map_err(|message| JsError { message })?;
        self.host.modules.merge_import_map(map);
        Ok(())
    }

    /// Compiles `source` as the module `key`, whose specifiers resolve against
    /// `base`, and returns the URLs it imports statically. Nothing runs: the
    /// caller loads those modules too and then runs the root
    /// ([`Self::run_module`]).
    fn compile_module_classified(
        &mut self,
        key: &str,
        source: &str,
        base: &url::Url,
    ) -> Result<Vec<url::Url>, ScriptFailure> {
        let host = Rc::clone(&self.host);
        let timeout = self.script_timeout;
        self.run_with(timeout, |tc| modules::compile(tc, &host, key, source, base))
    }

    /// Links and evaluates the module `key`, every import of which has been
    /// compiled.
    fn run_module_classified(&mut self, key: &str) -> Result<(), ScriptFailure> {
        let host = Rc::clone(&self.host);
        let timeout = self.script_timeout;
        self.run_with(timeout, |tc| modules::run(tc, &host, key))
    }

    /// [`Self::compile_module_classified`] for a caller outside the page
    /// boundary.
    pub fn compile_module(
        &mut self,
        key: &str,
        source: &str,
        base: &url::Url,
    ) -> Result<Vec<url::Url>, JsError> {
        self.compile_module_classified(key, source, base)
            .map_err(|failure| JsError {
                message: failure.message,
            })
    }

    /// [`Self::run_module_classified`] for a caller outside the page boundary.
    pub fn run_module(&mut self, key: &str) -> Result<(), JsError> {
        self.run_module_classified(key).map_err(|failure| JsError {
            message: failure.message,
        })
    }

    /// The script elements script has inserted since the last call that the
    /// page has to load and run (`tenta_js::scripts`). An inline classic
    /// script is not among them: it ran when it was inserted.
    pub fn take_inserted_scripts(&mut self) -> Vec<tenta_js::scripts::InsertedScript> {
        // Asked after every entry, and almost always there is nothing.
        if !self.host.scripts.has_requests() {
            return Vec::new();
        }
        self.host.scripts.take()
    }

    /// The `import()` calls script has made since the last call, each with the
    /// URL to load. One whose specifier does not resolve is rejected here.
    pub fn take_dynamic_imports(&mut self) -> Vec<tenta_js::modules::DynamicImport> {
        // Asked after every entry; almost always there is nothing, and that
        // answer must not enter the isolate.
        if !self.host.modules.has_imports() {
            return Vec::new();
        }
        let _entry = self.entered();
        if self.native_failure().is_some() {
            return Vec::new();
        }
        let base = self.dom.as_ref().and_then(|dom| {
            let document = dom.ctx.doc.borrow();
            document.base_url().or(document.url()).cloned()
        });
        let host = Rc::clone(&self.host);
        v8::scope!(let handle_scope, &mut self.isolate);
        let context = v8::Local::new(handle_scope, &self.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let imports = host.modules.take_imports(scope, base.as_ref());
        // A rejection made here has reactions to run.
        host.owe_checkpoint();
        imports
    }

    /// Settles the promise of the `import()` named `id`: `Ok` is the key of
    /// the module, whose whole graph has been compiled; `Err` is why it could
    /// not be loaded.
    pub fn finish_dynamic_import(&mut self, id: u64, outcome: Result<&str, &str>) {
        let host = Rc::clone(&self.host);
        let timeout = self.script_timeout;
        let _ = self.run_with(timeout, |tc| {
            modules::finish_import(tc, &host, id, outcome);
            Ok(())
        });
    }

    /// Discards a termination left pending on the isolate by a watchdog that
    /// fired while nothing was running to consume it.
    ///
    /// V8 termination is a *pending request*, not a one-shot signal delivered
    /// to whoever asked. `terminate_execution()` sets the isolate's
    /// `TERMINATE_EXECUTION` interrupt flag and nothing else; the flag becomes a
    /// thrown termination exception only when running code next handles an
    /// interrupt. A terminate that lands while an entry runs no interruptible
    /// script — or in the window just after one returned — therefore stays set,
    /// and aborts the *next* entry's script, which is innocent code.
    /// [`tenta_js::engine::EngineWatchdog::terminate`] describes the same fact
    /// from the caller's side: "best effort and idempotent: the engine may be
    /// between scripts, or already stopped".
    ///
    /// The clear must be **unconditional**. `Isolate::is_execution_terminating()`
    /// — and `TryCatch::has_terminated()`, which is the same predicate — answer
    /// "is a termination exception *active*", which is exactly false for the
    /// state this clears: a request no running code has handled yet. V8 only
    /// reports the active exception (`isolate-inl.h`: the current thread's
    /// exception compared against the termination exception), while a pending
    /// request is a stack-guard interrupt bit. `cancel_terminate_execution()`
    /// clears both — the api drops the interrupt flag *and* the internal
    /// exception — and clearing an unset flag is a no-op, so gating the call on
    /// a predicate that is false here would leave the poison in place. That is
    /// not theoretical: a terminate issued from another thread while the engine
    /// is idle makes `is_execution_terminating()` read false and the following
    /// script get aborted anyway
    /// (`tests/page_engine.rs::a_stray_termination_does_not_abort_the_next_script`).
    ///
    /// The *start* of an entry is the safe place to clear, because the isolate
    /// is thread-affine and the page thread is the only thread that ever enters
    /// it: a termination pending when an entry starts cannot belong to a script
    /// this entry has not yet run, since there is no other script in flight. The
    /// clear cannot blunt a real kill either — a terminate issued *during* the
    /// entry lands after it and still aborts that entry's script. It is one
    /// stack-guard interrupt flag behind a short lock, once per script or
    /// callback, which is nothing next to running one.
    fn discard_stray_termination(&mut self) {
        if self.native_failure().is_some() {
            self.isolate.terminate_execution();
        } else {
            self.termination.reset();
        }
    }

    /// Whether the page document holds lifecycle records its custom element
    /// definitions have not acted on: elements the parser created or changed
    /// since script was last entered.
    fn custom_elements_pending(&self) -> bool {
        self.dom.as_ref().is_some_and(|dom| {
            dom.ctx.documents.custom_elements_used()
                && (self.host.custom_elements.has_pending_upgrades()
                    || dom
                        .ctx
                        .doc
                        .try_borrow()
                        .is_ok_and(|doc| doc.has_lifecycle_records()))
        })
    }

    /// Runs the custom element reactions owed for what the parser did since
    /// script was last entered, at an entry that is about to run script.
    fn flush_custom_elements(&mut self) {
        if !self.custom_elements_pending() {
            return;
        }
        v8::scope!(let handle_scope, &mut self.isolate);
        let context = v8::Local::new(handle_scope, &self.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        host::custom_elements::flush(scope);
    }

    /// Runs the closed wrapper graph transaction at an outer task safe point.
    /// Its temporary handle scopes close before GC and before restored roots
    /// are published. The caller has already entered this isolate and holds no
    /// native document, listener, or wrapper-world borrow.
    fn graph_checkpoint(&mut self) -> bool {
        if self.native_failure().is_some() {
            return false;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(dom) = &self.dom {
                v8::scope!(let handle_scope, &mut self.isolate);
                let context = v8::Local::new(handle_scope, &self.context);
                let mut context_scope = v8::ContextScope::new(handle_scope, context);
                v8::scope!(let scope, &mut context_scope);
                world::cycles::checkpoint(scope, &dom.state, &dom.ctx, &self.termination, false)
            } else {
                self.isolate.low_memory_notification();
                Ok(true)
            }
        }));
        match result {
            Ok(Ok(completed)) => {
                // A checkpoint that could not run is owed at the next task end.
                if completed {
                    self.host
                        .last_graph_collection
                        .set(Some(std::time::Instant::now()));
                }
                completed
            }
            Ok(Err(())) => {
                tenta_js::bindings::poison_native_callback(
                    &self.isolate,
                    "wrapper graph collection",
                );
                false
            }
            Err(payload) => {
                tenta_js::bindings::poison_native_callback(
                    &self.isolate,
                    "wrapper graph collection",
                );
                if let Err(nested) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)))
                {
                    std::mem::forget(nested);
                }
                false
            }
        }
    }

    /// The native boundary that failed, if this engine is poisoned.
    ///
    /// The engine cannot be resumed after this failure: a host operation may
    /// have only partly completed. Page hosts use this sticky marker to report
    /// a crash; direct callers receive an error from every later evaluation.
    pub fn native_failure(&self) -> Option<&'static str> {
        tenta_js::bindings::native_callback_failure(&self.isolate)
    }

    fn native_failure_error(&self) -> Option<ScriptFailure> {
        self.native_failure().map(|callback| ScriptFailure {
            kind: ScriptErrorKind::Terminated,
            message: format!("native boundary `{callback}` failed; this engine must be discarded"),
        })
    }

    fn native_memory_failure_error(&self) -> Option<ScriptFailure> {
        self.native_memory
            .first_failure()
            .map(|failure| ScriptFailure {
                kind: ScriptErrorKind::Terminated,
                message: format!(
                    "{} budget exceeded in {}",
                    failure.kind,
                    failure.component.name()
                ),
            })
    }

    /// The whole run: an optional deadline watchdog, one context-scoped
    /// compilation and evaluation, then a failure classified by *where* it
    /// happened and worded by what stopped it.
    ///
    /// The heap-limit and deadline branches below are the only places that know
    /// which limit stopped a script, so a termination carries the wording they
    /// produce; [`Engine::run`] and [`Engine::run_classified`] both just hand it
    /// on.
    fn run_inner(
        &mut self,
        source: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<String, ScriptFailure> {
        self.run_with(timeout, |tc| compile_and_run(tc, source, None))
    }

    /// [`Self::run_inner`] for a body other than compile-and-run: the same
    /// entry, deadline, termination and budget handling around whatever `body`
    /// does inside the context's `TryCatch`.
    fn run_with<T>(
        &mut self,
        timeout: Option<std::time::Duration>,
        body: impl FnOnce(
            &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
        ) -> Result<T, ScriptFailure>,
    ) -> Result<T, ScriptFailure> {
        // One of the three ways script is entered (see
        // `discard_stray_termination`): clear before the watchdog is even
        // spawned, so a stray termination from an earlier entry cannot abort
        // this script and be blamed on this run's own deadline.
        let _entry = self.entered();
        if let Some(failure) = self.native_failure_error() {
            return Err(failure);
        }
        if let Some(failure) = self.native_memory_failure_error() {
            return Err(failure);
        }
        self.discard_stray_termination();

        let watchdog = timeout.map(|timeout| {
            let termination = Arc::clone(&self.termination);
            let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
            let join = std::thread::spawn(move || {
                // Terminate only on a genuine deadline; an early cancel (the run
                // finished) or a dropped sender (the caller panicked) is a no-op.
                if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    cancel_rx.recv_timeout(timeout)
                {
                    termination.request();
                }
            });
            (cancel_tx, join)
        });

        let custom_elements_pending = self.custom_elements_pending();
        // Mark a script activation so a DOM event dispatched from within this
        // run does not run a microtask checkpoint between listeners.
        self.host.enter_script();
        let (outcome, terminated) = {
            v8::scope!(let handle_scope, &mut self.isolate);
            let context = v8::Local::new(handle_scope, &self.context);
            let mut context_scope = v8::ContextScope::new(handle_scope, context);
            // Re-open as a context-carrying `PinScope` so `to_string` and
            // friends see `HandleScope<Context>`.
            v8::scope!(let scope, &mut context_scope);
            v8::tc_scope!(let tc, scope);
            // Elements the parser created since script was last entered are
            // upgraded before this script can look at them, under the same
            // deadline as the script.
            if custom_elements_pending {
                host::custom_elements::flush(tc);
            }
            let outcome = body(tc);
            (outcome, tc.has_terminated())
        };
        self.host.exit_script();
        // The script's handle scopes and callback frames are closed. The run
        // collects the wrapper graph when a collection is owed, as a task end
        // does: a collection is a full garbage collection, and a page pays for
        // one per `<script>` otherwise. Keep the deadline armed while it runs:
        // its elapsed time belongs to this task, and a request during recovery
        // is handed off only after the graph has been restored.
        if self.dom.is_some() && self.native_memory.first_failure().is_none() {
            self.collect_after_task();
        }

        if let Some((cancel_tx, join)) = watchdog {
            let _ = cancel_tx.send(());
            let _ = join.join();
        }

        let heap_tripped = self
            .heap_guard
            .as_ref()
            .map(|guard| {
                guard
                    .tripped
                    .swap(false, std::sync::atomic::Ordering::AcqRel)
            })
            .unwrap_or(false);

        // A watchdog can call `terminate_execution()` just after the script
        // already finished — a stray termination that `has_terminated()` did not
        // observe. Clear any pending termination unconditionally once the
        // watchdog has joined (and whenever a run was terminated), so the next
        // evaluation on this isolate never inherits it.
        //
        // The start-of-entry clear makes this redundant for every entry that
        // runs script — each clears before its own script anyway — but it stays:
        // it keeps this function's own contract, that a run which ended in a
        // termination leaves the isolate resumable, true without depending on
        // what the caller does next, and it is one flag clear per run.
        if let Some(failure) = self.native_failure_error() {
            return Err(failure);
        }
        let requested = self.termination.take_requested_and_reset();

        if let Some(failure) = self.native_memory_failure_error() {
            return Err(failure);
        }

        if heap_tripped {
            Err(ScriptFailure {
                kind: ScriptErrorKind::Terminated,
                message: "script exceeded the isolate heap limit".to_owned(),
            })
        } else if (terminated || requested) && timeout.is_some() {
            Err(ScriptFailure {
                kind: ScriptErrorKind::Terminated,
                message: format!(
                    "script execution exceeded the {:?} deadline",
                    timeout.expect("timeout is set when a deadline watchdog ran")
                ),
            })
        } else if requested {
            Err(ScriptFailure {
                kind: ScriptErrorKind::Terminated,
                message: "script execution was terminated".to_owned(),
            })
        } else {
            // Anything else: the script's own syntax or uncaught exception, or a
            // termination no limit here accounts for (`compile_and_run` words
            // that one, and records the kind where it happened).
            outcome
        }
    }

    /// Drives the event loop to quiescence: drains any microtasks the last
    /// script left pending, then repeatedly advances the virtual clock to the
    /// next timer and runs the timers that come due, each followed by a
    /// microtask checkpoint. `max_tasks` bounds the number of timer callbacks
    /// run so a `setTimeout`-recursion cannot spin forever; it returns the
    /// number actually run (`docs/spec/05-javascript.md` §5, §11).
    pub fn run_event_loop(&mut self, max_tasks: usize) -> usize {
        let _entry = self.entered();
        // Microtasks queued by the preceding top-level script run first.
        self.checkpoint();

        let mut run = 0;
        while run < max_tasks && self.native_failure().is_none() {
            let progressed = self.event_loop.advance_to_next_timer();
            if self.host.ready.borrow().is_empty() {
                if progressed {
                    // The clock moved but every due timer was cancelled; keep
                    // draining later deadlines.
                    continue;
                }
                break;
            }
            // The ready queue stays authoritative: a handler is taken from it
            // one at a time rather than moved into a local batch. A panic raised
            // on the Rust side of the drain unwinds through here, and a local
            // batch would be dropped on the way out, silently losing every
            // handler that had not run yet. A generated binding catches its
            // own panic before returning through V8, poisons the engine and
            // terminates script; the page observes that marker after return.
            run += self.invoke_ready(max_tasks - run);
        }
        run
    }

    /// Whether a rendering opportunity is owed, so a woken driver knows to call
    /// [`Self::run_animation_frame`]. See [`Self::next_wakeup`]'s contract.
    pub fn frame_due(&self) -> bool {
        !self.host.ready_frames.borrow().is_empty() || self.event_loop.frame_due()
    }

    /// Runs a microtask checkpoint: every pending promise reaction and
    /// `queueMicrotask` callback, including those the checkpoint itself queues,
    /// until the queue is empty.
    ///
    /// The drive methods run one after each callback. A driver that entered
    /// script by some other route — [`Self::evaluate`], a dispatched DOM event
    /// — owes one too, and until it is paid [`Self::next_wakeup`] reports zero.
    pub fn run_microtasks(&mut self) {
        let _entry = self.entered();
        self.checkpoint();
    }

    /// Drains V8's microtask queue and records that the debt is paid.
    ///
    /// Every checkpoint goes through here so the "a checkpoint is owed" flag
    /// cannot drift from reality. It is cleared *after* the drain: V8 runs
    /// microtasks queued by microtasks in the same pass, so once this returns
    /// the queue is genuinely empty, and any script entered afterwards sets the
    /// flag again on its own.
    ///
    /// The checkpoint enters script — a promise reaction or `queueMicrotask`
    /// callback — so it starts with no termination pending, like every other
    /// way in (`discard_stray_termination`). It is the only clear `run_microtasks`
    /// goes through; on the other drive paths it is the one in front of the
    /// first handler.
    fn checkpoint(&mut self) {
        if self.native_failure().is_some() {
            return;
        }
        self.discard_stray_termination();
        self.queue_observer_notification();
        // The reactions continue the tasks that queued them, so they run under
        // the provenance those tasks left behind (`HostState::microtask_origin`).
        let owed = self
            .host
            .microtask_origin
            .replace(tenta_core::Origin::Background);
        let scheduler = self.host.scheduler.clone();
        if owed == tenta_core::Origin::Tracked {
            scheduler.with_origin(tenta_core::Origin::Tracked, || {
                self.isolate.perform_microtask_checkpoint();
            });
        } else {
            self.isolate.perform_microtask_checkpoint();
        }
        self.host.microtasks_pending.set(false);
        self.flush_uncaught();
        self.flush_rejections();
        self.collect_after_task();
    }

    /// Fires `error` at the window for each exception nothing caught during
    /// the task that just ended, oldest first.
    ///
    /// The standard reports an exception where it is thrown, in the middle of
    /// the task. Here the report is queued (`inspector::report_caught`) and the
    /// event is fired once the task and its microtasks are over, which is the
    /// one point where the engine may dispatch with no script on the stack. A
    /// handler therefore runs after the rest of the task instead of inside it.
    ///
    /// An exception a listener of these events throws is reported to the
    /// inspector and is not an event of its own. The microtasks the listeners
    /// queued run before this returns.
    fn flush_uncaught(&mut self) {
        let pending = self.host.inspector.take_uncaught();
        if pending.is_empty() || self.native_failure().is_some() {
            return;
        }
        let Some(dom) = &self.dom else {
            return;
        };
        // Nothing listens on most pages, and then there is nothing to build.
        let listened = {
            let listeners = dom.ctx.listeners.borrow();
            [false, true].into_iter().any(|capture| {
                !listeners
                    .snapshot_at(TargetKey::Window, "error", capture)
                    .is_empty()
            })
        };
        if !listened {
            return;
        }
        let _native = self.host.enter_native_dispatch();
        let was_reporting = self.host.inspector.set_reporting_errors(true);
        for uncaught in pending {
            // The thrown value becomes a stored value the event record owns,
            // as a `CustomEvent` owns its `detail`.
            let error = {
                v8::scope!(let handle_scope, &mut self.isolate);
                let context = v8::Local::new(handle_scope, &self.context);
                let mut context_scope = v8::ContextScope::new(handle_scope, context);
                v8::scope!(let scope, &mut context_scope);
                let value = v8::Local::new(scope, &uncaught.error);
                dom.state.borrow_mut().store_value(scope, value)
            };
            let mut record = EventRecord::new(
                "error".to_owned(),
                EventKind::Error {
                    message: uncaught.message,
                    filename: uncaught.filename,
                    lineno: uncaught.lineno,
                    colno: uncaught.colno,
                    error: Some(error),
                },
                dom.ctx.time_stamp(),
            );
            record.cancelable = true;
            record.flags.initialized = true;
            let event_key = dom.ctx.events.borrow_mut().insert(record);
            let _ = dispatch_at(&dom.ctx, DispatchTarget::window(), event_key, true);
            // A record no listener made a wrapper for is removed here, with
            // the stored value it owns; one with a wrapper is the world's
            // dead-event sweep's to remove.
            let orphaned = dom
                .ctx
                .events
                .borrow()
                .get(event_key)
                .is_some_and(|record| !record.has_wrapper);
            if orphaned {
                dom.ctx.events.borrow_mut().remove(event_key);
                dom.state.borrow_mut().release_value(error);
            }
            if self.native_failure().is_some() {
                break;
            }
        }
        self.host.inspector.set_reporting_errors(was_reporting);
        drop(_native);
        if self.native_failure().is_none() {
            self.isolate.perform_microtask_checkpoint();
            self.host.microtasks_pending.set(false);
        }
    }

    /// Collects the wrapper graph at the end of a task, a script run, a
    /// settlement or a lifecycle event, when a collection is owed.
    ///
    /// A collection is a full garbage collection: about half a millisecond on
    /// the smallest heap, 2 ms on a small page's and over 10 ms on a large
    /// one's. Script runs, timer callbacks, animation frames, image events,
    /// fetch and XHR settlements each end with one; a page with fifty scripts
    /// ends fifty runs, one that makes a hundred requests settles a hundred
    /// times, and one that animates ends a task every frame, so paying for a
    /// collection each time spends most of a frame on it. One is owed once
    /// the wrappers made since the last collection reach the count at which
    /// the next binding call would collect anyway, or once the last
    /// collection is [`TASK_COLLECTION_INTERVAL`] old. What is left
    /// uncollected in between stays within the bound admission enforces inside
    /// a task, and [`PageEngine::trim_memory`] collects without waiting.
    fn collect_after_task(&mut self) {
        let Some(dom) = &self.dom else {
            return;
        };
        if self.native_failure().is_some() {
            return;
        }
        // A world that is borrowed here is one a failed callback left behind;
        // the checkpoint is what reports that.
        let debt = dom
            .state
            .try_borrow()
            .map_or(usize::MAX, |state| state.wrapper_debt());
        let waited = self
            .host
            .last_graph_collection
            .get()
            .map_or(Duration::MAX, |last| last.elapsed());
        if debt >= world::SOFT_WRAPPER_MATERIALIZATIONS || waited >= TASK_COLLECTION_INTERVAL {
            self.graph_checkpoint();
        }
    }

    /// Queues the mutation-observer microtask for a mutation made by native
    /// code, which has no binding call to queue it after. A mutation made by
    /// script was queued when its call returned and finds nothing owed here.
    fn queue_observer_notification(&mut self) {
        let Some(dom) = &self.dom else {
            return;
        };
        if !tenta_js::bindings::host::mutation_observers_awaiting_microtask(&dom.ctx) {
            return;
        }
        let ctx = dom.ctx.clone();
        v8::scope!(let handle_scope, &mut self.isolate);
        let context = v8::Local::new(handle_scope, &self.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        observer::queue_notification(scope, &ctx);
    }

    /// Whether native code left a mutation-observer notification unqueued, which
    /// is work no queue here knows about.
    fn observer_notification_owed(&self) -> bool {
        self.dom.as_ref().is_some_and(|dom| {
            tenta_js::bindings::host::mutation_observers_awaiting_microtask(&dom.ctx)
        })
    }

    /// Settles the promises a batch of fetch completions names, entering the
    /// context to build each value and queueing each settlement as a microtask.
    ///
    /// This is the one engine entry a *network* completion needs, and it is the
    /// only one that is not reachable from script: a driver pushes its answer
    /// into the page's table from whatever task it completes on, and the page
    /// drains that table into here (`tenta_page::runtime`). The entry enters the
    /// context the way [`Self::run_inner`] does — a plain `HandleScope` on the
    /// isolate — because there is no V8 callback on the stack to bootstrap a
    /// `CallbackScope` from
    /// ([`v8::CallbackScope`]'s own doc calls a callback the only valid use of
    /// it), so [`tenta_js::dom::events::ScriptHost::settle_promise`] cannot be
    /// used here.
    ///
    /// Three points are decisions rather than style:
    ///
    /// - An empty batch returns before anything else: an entry that settles
    ///   nothing must not arm the watchdog or charge the script budget.
    /// - `microtasks_pending` is set only when a settlement was actually queued.
    ///   A spurious flag spins the page loop one turn for nothing and would mask
    ///   a genuine "nothing owed".
    /// - The settlement stays a *microtask*; nothing here resolves a promise in
    ///   place. Resolving here would enqueue the reactions at call time and
    ///   break the ordering of a settlement requested from inside a microtask,
    ///   which [`promise`]'s module doc explains and `tests/dom_response.rs`
    ///   pins.
    pub fn settle_fetches(&mut self, completions: Vec<FetchCompletion>) {
        let _entry = self.entered();
        if completions.is_empty() || self.native_failure().is_some() {
            return;
        }
        // This entry can settle a promise a terminated script left behind, and
        // the microtask it queues runs script, so it starts with nothing
        // pending, like every other way in.
        self.discard_stray_termination();
        let Some(dom) = &self.dom else {
            return;
        };
        let mut queued = false;
        {
            v8::scope!(let handle_scope, &mut self.isolate);
            let context = v8::Local::new(handle_scope, &self.context);
            let mut context_scope = v8::ContextScope::new(handle_scope, context);
            v8::scope!(let scope, &mut context_scope);
            let Some(state) = promise::state_of(scope) else {
                return;
            };
            for completion in completions {
                // The request is over, so its signal no longer has anything
                // of it to abort.
                tenta_js::bindings::host::forget_fetch_signal(&dom.ctx, completion.resolver);
                // A completion whose value cannot be built is skipped rather
                // than aborting the batch: the promises after it in the batch
                // did nothing wrong.
                let Some((value, rejected)) = build_completion(dom, scope, completion.outcome)
                else {
                    continue;
                };
                queued |= promise::queue_settlement(
                    &state,
                    scope,
                    completion.resolver,
                    value,
                    // Both values are stored fresh here, so the settlement owns
                    // them. The world's interned `undefined` is the only case
                    // that is not owned, and this entry never builds it.
                    true,
                    rejected,
                );
            }
        }
        if queued {
            self.host.owe_checkpoint();
        }
        // A settlement ends a task like any other: the wrapper graph is
        // collected when a collection is owed, not once per batch.
        self.collect_after_task();
    }

    /// Delivers a batch of XHR completions: for each, the event its outcome
    /// names is dispatched at the event target the completion carries.
    ///
    /// **This is the codebase's first native-initiated DOM dispatch**, and the
    /// reason the entry is not simply [`Self::settle_fetches`] with a different
    /// payload. Every existing `dispatch_at` caller runs inside a live DOM
    /// binding call, so the listener invocation behind it bootstraps its scope
    /// with a `v8::CallbackScope` — legal only from a callback. This entry runs
    /// from the page's turn loop with **no V8 callback anywhere on the stack**,
    /// so it marks itself through `HostState::enter_native_dispatch` and
    /// [`V8ScriptHost::with_context`](script_host::V8ScriptHost) opens a plain
    /// `HandleScope` instead. The guard covers the whole entry — the record
    /// building below and every dispatch in the batch — because a half-set flag
    /// would make the two scope choices disagree.
    ///
    /// Two points are decisions rather than style:
    ///
    /// - An empty batch returns before anything else, exactly as
    ///   [`Self::settle_fetches`] does: an entry that delivers nothing must not
    ///   arm the watchdog, charge the script budget or clear a termination.
    /// - An `outcome` at an `XMLHttpRequest` is delivered by the standard's own
    ///   response state machine, which lives in `tenta-js`
    ///   ([`deliver_xhr_completion`]) and fires the events it prescribes — the
    ///   readystate changes, the two `progress` events of a body, `load` and
    ///   `loadend`, or `error` and `loadend` for a failure. A completion at any
    ///   *other* target is still mapped here, one event per outcome: a response
    ///   is a `load` and anything else an `error`, both of them
    ///   `ProgressEvent`s with the init defaults. That fallback is what keeps
    ///   this entry total for a target that is not an XHR at all, and a `true`
    ///   return from the machine is the only thing that suppresses it. One
    ///   such `true` is not a delivery at all: a completion whose request id
    ///   the object has moved past belongs to a request it abandoned, and
    ///   nothing — not even the one-event fallback — is fired for it.
    pub fn settle_xhr(&mut self, completions: Vec<XhrCompletion>) {
        let _entry = self.entered();
        if completions.is_empty() || self.native_failure().is_some() {
            return;
        }
        // A completion can arrive after a watchdog terminated the script that
        // started the request, and the dispatch below runs script, so this entry
        // starts with nothing pending, like every other way in.
        self.discard_stray_termination();
        let Some(dom) = &self.dom else {
            return;
        };
        // Held for the whole batch: `with_context` reads it on every dispatch,
        // and the guard restores whatever it found when the entry returns — or
        // unwinds.
        let _native = self.host.enter_native_dispatch();
        for completion in completions {
            // A handle that does not name an event target is skipped rather than
            // aborting the batch: the completions after it did nothing wrong.
            // A target whose record has since been reclaimed is not a skip here
            // — its key simply matches no listener, and its wrapper is never
            // built.
            let Some(target) = DispatchTarget::from_bits(completion.handle.target) else {
                continue;
            };
            if deliver_xhr_completion(
                &dom.ctx,
                target,
                completion.handle.request,
                &completion.outcome,
            ) {
                continue;
            }
            let type_ = match completion.outcome {
                FetchOutcome::Response(_) => "load",
                FetchOutcome::Failed(_) | FetchOutcome::Opaque => "error",
            };
            let mut record = EventRecord::new(
                type_.to_owned(),
                EventKind::Progress {
                    length_computable: false,
                    loaded: 0.0,
                    total: 0.0,
                },
                dom.ctx.time_stamp(),
            );
            record.flags.initialized = true;
            let event_key = dom.ctx.events.borrow_mut().insert(record);
            // `trusted` is true: every completion reaching this entry comes from
            // a driver, never from script, so the event is one the user agent
            // fired — the same call `AbortSignal::abort` makes for its `abort`.
            // The untrusted kind is a script-synthesized event, `element.click()`
            // or `dispatchEvent(...)`.
            let _ = dispatch_at(&dom.ctx, target, event_key, true);
            // Remove the transient record unless a listener made a wrapper for it
            // (in which case the world's dead-event sweep owns its removal).
            let orphaned = dom
                .ctx
                .events
                .borrow()
                .get(event_key)
                .is_some_and(|record| !record.has_wrapper);
            if orphaned {
                dom.ctx.events.borrow_mut().remove(event_key);
            }
        }
        drop(_native);
        self.collect_after_task();
    }

    /// Installs the page's linked stylesheets for the document this engine is
    /// bound to (`link.sheet`, `document.styleSheets`).
    pub fn set_style_host(&mut self, host: Rc<dyn tenta_js::stylesheets::StyleHost>) {
        if let Some(dom) = &self.dom {
            dom.ctx.set_style_host(host);
        }
    }

    /// Runs the scripts that define what WebIDL cannot declare:
    /// `DOMRect.fromRect()`, `toJSON()` and the iterator of `DOMRectList`;
    /// the traversal interfaces (`TreeWalker`, `NodeIterator`, `NodeFilter`),
    /// whose algorithms call back into the page at every step; and
    /// `document.fonts` with `FontFaceSet` and `FontFace`, which have no
    /// engine state behind them; and `AbortSignal.timeout()`,
    /// `AbortSignal.any()` and `Response.json()`, which are a few lines over
    /// their interfaces. `PromiseRejectionEvent` and the dispatch
    /// of `unhandledrejection` are installed with them, and so are
    /// `ReadableStream` and the body streams built on it, and `Selection`.
    /// A failure is not an error of the host; the members are then absent.
    pub fn install_preludes(&mut self) {
        for source in [
            tenta_js::geometry::GEOMETRY_PRELUDE_SOURCE,
            tenta_js::traversal::TRAVERSAL_PRELUDE_SOURCE,
            tenta_js::fonts::FONTS_PRELUDE_SOURCE,
            tenta_js::conveniences::CONVENIENCES_PRELUDE_SOURCE,
        ] {
            if self.native_failure().is_none() {
                let _ = self.run_inner(source, None);
            }
        }
        self.install_rejection_events();
        self.install_streams();
    }

    /// Defines `ReadableStream` and what reads one, `Response.prototype.body`
    /// and `Blob.prototype.stream()` (`tenta_js::streams`), `Selection` with
    /// the two `getSelection()` (`tenta_js::selection`), `SubtleCrypto` with
    /// `crypto.subtle` (`tenta_js::subtle`), `DOMMatrix`, `DOMPoint` and
    /// `DOMQuad` (`tenta_js::matrix`), and `SVGAnimatedString` with the SVG
    /// `className` and `href` (`tenta_js::svg_animated`). Only the names are
    /// defined here; the script behind them runs when a page first uses one.
    /// `CSSStyleSheet.prototype.replace()` is defined here too, natively.
    fn install_streams(&mut self) {
        if self.native_failure().is_some() {
            return;
        }
        let _entry = self.entered();
        v8::scope!(let handle_scope, &mut self.isolate);
        let context = v8::Local::new(handle_scope, &self.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let global = context.global(scope);
        host::streams::install(scope, global);
        host::selection::install(scope, global);
        host::subtle::install(scope, global);
        host::matrix::install(scope, global);
        host::event_interfaces::install(scope, global);
        host::svg_animated::install(scope, global);
        host::sheet_replace::install(scope, global);
    }

    /// Installs the page's image state for the document this engine is bound
    /// to (`complete`, `naturalWidth`, `naturalHeight`, `currentSrc`,
    /// `decode()`).
    ///
    /// Also defines the `Image` constructor, which only a page that loads
    /// images has: a failure to define it is not an error of the host.
    pub fn set_image_host(&mut self, host: Rc<dyn ImageHost>) {
        if let Some(dom) = &self.dom {
            dom.ctx.set_image_host(host);
        }
        if self.native_failure().is_none() {
            let _ = self.run_inner(tenta_js::image::IMAGE_CONSTRUCTOR_SOURCE, None);
        }
    }

    /// Delivers the image loads that ended: a trusted `load` or `error` at
    /// each element, and the settlement of the `decode()` promises waiting on
    /// it.
    ///
    /// The shape is [`Self::settle_xhr`]'s, for the same reason: the entry runs
    /// from the page's turn with no V8 callback on the stack, so it marks itself
    /// through [`HostState::enter_native_dispatch`] and every scope the dispatch
    /// and the promise settlement open is a plain `HandleScope`. An empty batch
    /// returns before anything is armed or charged.
    pub fn settle_images(
        &mut self,
        events: Vec<ImageEvent>,
        current: &mut dyn FnMut(usize, &ImageEvent) -> bool,
    ) {
        let _entry = self.entered();
        if events.is_empty() || self.native_failure().is_some() {
            return;
        }
        // A listener runs script, so the entry starts with nothing pending,
        // like every other way in.
        self.discard_stray_termination();
        for (index, event) in events.iter().enumerate() {
            if self.native_failure().is_some() || !current(index, event) {
                continue;
            }
            let Some(dom) = &self.dom else {
                return;
            };
            {
                let _native = self.host.enter_native_dispatch();
                deliver_image_events(&dom.ctx, std::slice::from_ref(event));
            }
            // This event's task is over: its microtasks run before the next
            // event's, but the wrapper graph is only walked once, below.
            self.isolate.perform_microtask_checkpoint();
            self.host.microtasks_pending.set(false);
            self.flush_uncaught();
            self.flush_rejections();
        }
        self.collect_after_task();
    }

    /// Fires one user-agent lifecycle event — `DOMContentLoaded` or `load` — as
    /// a real DOM event at the target HTML prescribes.
    ///
    /// The shape is [`Self::settle_xhr`]'s, because the constraint is the same
    /// one: this entry runs from the page's own code with **no V8 callback on
    /// the stack**, so it marks itself through
    /// [`HostState::enter_native_dispatch`] and
    /// [`V8ScriptHost::with_context`](script_host::V8ScriptHost) opens a plain
    /// `HandleScope`. The guard covers the record building and the dispatch, so
    /// the two scope choices cannot disagree.
    ///
    /// The two targets and the four flags are HTML's, not this method's
    /// choices, and each is stated where it is set:
    ///
    /// - `DOMContentLoaded` is fired at the `Document`, with `bubbles` true and
    ///   `cancelable` false (HTML "the end": "Fire an event named
    ///   DOMContentLoaded at the Document object, with its bubbles attribute
    ///   initialized to true").
    /// - `load` is fired at the `Window` with the **legacy target override
    ///   flag** (HTML "the load event"), both attributes left at their `Event`
    ///   constructor defaults, which are false. The flag's effect is DOM's: with
    ///   it set, `targetOverride` is the target's associated `Document`, so
    ///   `event.target` at that dispatch is the document rather than the window.
    ///
    /// A `load` handler that queues work is the reason this method is entered
    /// under the caller's [`Origin::Tracked`](tenta_core::Origin) scope rather
    /// than left at the loop's default: the event is part of loading the
    /// document, so the `setTimeout` it schedules is a deferral of that load and
    /// not background activity, exactly as it is for a script the page handed
    /// out (`crate::page_engine`).
    ///
    /// Nothing here remembers what it has fired: once-ness belongs to the
    /// caller's settle bookkeeping, and a listener may legitimately re-enter
    /// this path by calling `dispatchEvent`.
    pub fn dispatch_lifecycle(&mut self, event: LifecycleEvent) {
        let _entry = self.entered();
        if self.native_failure().is_some() {
            return;
        }
        // Runs page script, so it starts with nothing pending, like every other
        // way in.
        self.discard_stray_termination();
        self.flush_custom_elements();
        let Some(dom) = &self.dom else {
            return;
        };
        let document_ref = {
            let document = dom.ctx.doc.borrow();
            document.node_ref(document.document_node())
        };
        let (target, type_, bubbles, cancelable) = match event {
            LifecycleEvent::DomContentLoaded => (
                DispatchTarget::node(dom.ctx.slot, document_ref),
                "DOMContentLoaded",
                true,
                false,
            ),
            LifecycleEvent::Load => (
                // HTML fires `load` at the window *with the legacy target
                // override flag set*, which is what makes `event.target` the
                // window's document rather than the window
                // (`DispatchTarget::with_target_override`). The listener table
                // is unaffected: a `window` listener still fires.
                DispatchTarget::window()
                    .with_target_override(encode_node(dom.ctx.slot, document_ref)),
                "load",
                false,
                false,
            ),
            // `LifecycleEvent` is `#[non_exhaustive]`, so a variant added in
            // `tenta-js` cannot fail this match at compile time. Firing nothing
            // is the only honest answer: inventing an event the page did not
            // ask for would make a *wrong* event observable where a missing one
            // is at least visible as missing. The assertion is what turns that
            // silence into a test failure the moment such a variant is added.
            other => {
                debug_assert!(
                    false,
                    "no implementation for the lifecycle event {other:?}: the backend has to \
                     learn a target and its flags before `tenta-js` may add one"
                );
                return;
            }
        };
        let mut record = EventRecord::new(type_.to_owned(), EventKind::Plain, dom.ctx.time_stamp());
        record.bubbles = bubbles;
        record.cancelable = cancelable;
        record.flags.initialized = true;
        let event_key = dom.ctx.events.borrow_mut().insert(record);
        // Held for the whole entry, as in the XHR drain: the flag is what makes
        // `with_context` take the plain-scope path, and it must describe the
        // entry rather than one statement of it. The guard restores whatever it
        // found on the way out — or on unwind.
        let _native = self.host.enter_native_dispatch();
        // `trusted` is true: the user agent fired this, not script. It is what
        // makes `event.isTrusted` report true to the listener.
        let _ = dispatch_at(&dom.ctx, target, event_key, true);
        if matches!(event, LifecycleEvent::DomContentLoaded) {
            // The document is parsed and its deferred scripts have run: the
            // first focusable `autofocus` element takes focus.
            let _ = tenta_js::bindings::host::flush_autofocus(&dom.ctx);
        }
        // Remove the transient record unless a listener made a wrapper for it
        // (in which case the world's dead-event sweep owns its removal), exactly
        // as the XHR drain does.
        let orphaned = dom
            .ctx
            .events
            .borrow()
            .get(event_key)
            .is_some_and(|record| !record.has_wrapper);
        if orphaned {
            dom.ctx.events.borrow_mut().remove(event_key);
        }
        drop(_native);
        self.collect_after_task();
    }

    /// Invokes up to `budget` handlers from the ready queue and returns how many
    /// ran. Handlers past the budget stay queued in order for the next drive.
    ///
    /// A callback that runs to its own end is followed by a microtask checkpoint.
    /// A callback the watchdog terminated is counted as run — it did enter
    /// script, and was stopped inside it — but is not followed by a checkpoint,
    /// and ends this pump: the single arm the caller holds over this entry has
    /// already fired and will not fire again, so a later callback of the same
    /// sweep would be a runaway nothing could stop. Returning lets the caller arm
    /// a fresh deadline for the next entry; the handlers left queued are what
    /// keeps [`Self::next_wakeup`] reporting "now", so it does arm one.
    ///
    /// The checkpoint that would have followed costs nothing to skip. The
    /// `TryCatch` has just unwound a termination, but the `host.enter_script` in
    /// [`Self::invoke_handler`] already set `microtasks_pending`, so the debt
    /// stays visible through `next_wakeup` and the next pump's leading
    /// checkpoint pays it.
    fn invoke_ready(&mut self, budget: usize) -> usize {
        let mut run = 0;
        let mut taken = 0;
        while taken < budget && self.native_failure().is_none() {
            let next = self.host.ready.borrow_mut().pop_front();
            let Some(ready) = next else { break };
            // A handler cancelled in the deferral window still costs budget: a
            // flood of cancelled timers must not let one drive run unbounded.
            // It runs no script, so it is owed no checkpoint and is not counted.
            taken += 1;
            match self.invoke_timer(&ready) {
                CallOutcome::Cancelled => {}
                CallOutcome::Completed => {
                    self.checkpoint();
                    run += 1;
                }
                CallOutcome::Terminated => {
                    run += 1;
                    break;
                }
            }
        }
        run
    }

    /// How long until this engine next has work, or `None` when it has none —
    /// the interval a real-clock page thread parks for. `Duration::ZERO` means
    /// work is due already and the thread must not park at all.
    ///
    /// **Contract.** A woken driver must run what it was woken for, or it will
    /// be told "now" again and spin: [`Self::pump_due_timers`] for timers and
    /// owed microtasks, and [`Self::run_animation_frame`] when
    /// [`Self::frame_due`] says a rendering opportunity is owed — that call is
    /// the only thing that advances the frame clock.
    pub fn next_wakeup(&self) -> Option<std::time::Duration> {
        if self.native_failure().is_some() {
            return None;
        }
        // Handlers a bounded pump held back are due work the timer heap no
        // longer knows about: it handed them over when they fired. A microtask
        // checkpoint owed since the last script entry is work no queue here
        // knows about at all — V8's microtask queue is not introspectable.
        // Parking on the timer heap alone would leave either unrun until some
        // unrelated timer happened to come due.
        if !self.host.ready.borrow().is_empty()
            || !self.host.ready_frames.borrow().is_empty()
            || self.host.microtasks_pending.get()
            || self.observer_notification_owed()
        {
            return Some(std::time::Duration::ZERO);
        }
        self.event_loop.next_wakeup()
    }

    /// Runs the timers that are due *now*, without moving the clock, and
    /// returns how many callbacks ran. `max_tasks` bounds one pump so a timer
    /// that reschedules itself at zero delay cannot starve the rest of the
    /// thread; anything still due is picked up by the next pump. A budget of
    /// zero still fires the due timers — their tasks queue the handlers — but
    /// invokes none of them, which is how a caller observes the queue between
    /// the firing and the call.
    ///
    /// This is the real-clock counterpart of [`Self::run_event_loop`]: wall
    /// time supplies the advance, so the engine only ever fires what has
    /// already come due (`docs/spec/05-javascript.md` §5).
    pub fn pump_due_timers(&mut self, max_tasks: usize) -> usize {
        let _entry = self.entered();
        self.checkpoint();
        if self.native_failure().is_some() {
            return 0;
        }

        // Exactly one sweep of the timer heap per pump. A zero-delay timer a
        // handler schedules is due the moment it is scheduled, so re-reading
        // the heap here would let a `setTimeout(f, 0)` chain spin inside one
        // pump and starve everything else on the thread. It waits for the next
        // pump instead, which the caller enters immediately because
        // `next_wakeup` reports zero.
        self.event_loop.fire_due_timers();
        // Handlers past `max_tasks` stay queued, in order, for the next pump —
        // which the caller enters immediately, because `next_wakeup` reports
        // zero while the ready queue is non-empty.
        self.invoke_ready(max_tasks)
    }

    /// Runs one rendering opportunity at the current virtual time: every
    /// `requestAnimationFrame` callback registered before now runs once, each
    /// with the frame timestamp in milliseconds, followed by a microtask
    /// checkpoint. Callbacks registered during the frame wait for the next one
    /// (`docs/spec/05-javascript.md` §5).
    ///
    /// It runs script only through `invoke_handler`, which clears a stray
    /// termination before each callback; the checkpoint that follows is behind
    /// them, so this entry does not clear one itself. A callback the watchdog
    /// terminated ends the opportunity — its deadline is spent, as in
    /// [`Self::invoke_ready`] — and the callbacks after it stay queued, so the
    /// next `run_animation_frame`, entered on `next_wakeup`'s word, runs them
    /// under an arm of its own.
    pub fn run_animation_frame(&mut self) {
        let _entry = self.entered();
        if self.native_failure().is_some() {
            return;
        }
        // Moves each due callback's handle onto the ready-frame queue.
        self.event_loop.run_animation_frame();
        self.drain_ready_frames();
    }

    /// Steps the virtual clock forward one 60 Hz frame, runs the timers that
    /// come due within it, then the rendering opportunity — the building block
    /// of a deterministic headless render loop. New timers a callback schedules
    /// this frame do not fire until a later frame, so one `tick_frame` cannot
    /// spin.
    pub fn tick_frame(&mut self) {
        let _entry = self.entered();
        if self.native_failure().is_some() {
            return;
        }
        self.event_loop.tick_frame();
        // Timers that came due this frame run as tasks, a checkpoint behind each
        // that runs to its own end. Through `invoke_ready` like every other drive
        // path, so a panicking handler cannot take the rest of the queue down
        // with it.
        self.invoke_ready(usize::MAX);
        self.drain_ready_frames();
    }

    /// Invokes every animation-frame callback whose frame has come, each with
    /// its timestamp and followed by a microtask checkpoint — until one is
    /// terminated, which ends the drain for the same reason it ends
    /// [`Self::invoke_ready`]: the deadline that killed it is spent, and this
    /// entry has to return before another callback can be run under a new one.
    /// The callbacks that did not run stay queued, in order, which is what makes
    /// the deferred work visible to [`Self::next_wakeup`].
    fn drain_ready_frames(&mut self) {
        // Popped one at a time for the same reason as the timer ready queue: a
        // local batch would be dropped by an unwind, losing every callback that
        // had not run yet, and `clear_pending_callbacks` (a navigation commit)
        // cannot reach a batch that has already been detached from the queue, so
        // the discarded callbacks would keep running against the old document.
        while self.native_failure().is_none() {
            let next = self.host.ready_frames.borrow_mut().pop_front();
            let Some((handler, timestamp)) = next else {
                break;
            };
            if self.invoke_handler(&handler, Some(timestamp)) == CallOutcome::Terminated {
                break;
            }
            self.checkpoint();
        }
    }

    /// Calls one fired timer handler with the loop state it was queued under.
    ///
    /// The timer scope and provenance are reinstalled here, around the JS
    /// call itself, because the task that dequeued the timer only moved the
    /// handle onto the ready queue and has already restored them. Without this
    /// a `setTimeout(f, 0)` chain would be scheduled from depth 1 every time
    /// and never reach the clamp, and every timer-scheduled task would look
    /// like top-level work to the settle machine.
    ///
    /// The outcome is [`Self::invoke_handler`]'s, except for the timer that was
    /// cancelled in the window between its firing and this call: it ran no
    /// script, so it is [`CallOutcome::Cancelled`] rather than completed.
    fn invoke_timer(&mut self, ready: &ReadyHandler) -> CallOutcome {
        let scheduler = self.host.scheduler.clone();
        // The claim both spends the firing and hands back the count it holds.
        // The guard releases it when it drops, after the call below - and on
        // unwind, so a Rust panic inside the handler cannot leave the settle
        // machine counting work that will never finish.
        let firing = match ready.timer {
            Some(id) => scheduler.claim_fired(id),
            None => None,
        };
        if ready.timer.is_some() && firing.is_none() {
            // `clearTimeout` landed after this timer fired but before the call.
            // HTML aborts such a timer, and script relies on it: the debounce
            // idiom cancels a pending callback from another callback that came
            // due in the same sweep. The cancel already released the firing's
            // count, so nothing is owed here.
            return CallOutcome::Cancelled;
        }
        let _release = firing.map(|firing| FiringRelease::new(&scheduler, firing));
        let origin = ready.origin;
        let scope = ready.scope;
        let handler = &ready.handler;
        scheduler.with_timer_scope(scope, || {
            scheduler.with_origin(origin, || self.invoke_handler(handler, None))
        })
    }

    /// Calls one handler in this engine's context, passing `timestamp` as the
    /// sole argument when present (animation-frame callbacks) or no arguments
    /// (timers). The call runs under a `TryCatch` so an exception the handler
    /// throws is contained here — it leaves no pending exception on the isolate
    /// to disturb the following microtask checkpoint, and the next handler still
    /// runs. Surfacing the exception to the error console lands with the
    /// reporting work package.
    ///
    /// It reports how the call ended, because a caller about to run a *second*
    /// callback needs to know a termination is what stopped the first. A
    /// termination is not merely an error like the exceptions the `TryCatch`
    /// contains: it means a watchdog's deadline fired, and that deadline covered
    /// this whole entry. The pump's caller knows that, and returns so a fresh one
    /// can be armed; see [`Self::invoke_ready`].
    ///
    /// This is the third way script is entered and so the third start-of-entry
    /// clear (`discard_stray_termination`): every timer and animation-frame
    /// callback in the crate reaches V8 through here, which is what makes
    /// `pump_due_timers`, `run_animation_frame` *and* `tick_frame` covered
    /// without each clearing on its own — `run_animation_frame` runs its
    /// callbacks before its first checkpoint, so a clear there would come too
    /// late for the first one.
    fn invoke_handler(
        &mut self,
        handler: &v8::Global<v8::Function>,
        timestamp: Option<f64>,
    ) -> CallOutcome {
        if self.native_failure().is_some() {
            return CallOutcome::Terminated;
        }
        self.discard_stray_termination();
        self.flush_custom_elements();
        // A timer or animation-frame callback is a script activation too: an
        // event it dispatches must not checkpoint microtasks between listeners.
        self.host.enter_script();
        let terminated = {
            v8::scope!(let handle_scope, &mut self.isolate);
            let context = v8::Local::new(handle_scope, &self.context);
            let mut context_scope = v8::ContextScope::new(handle_scope, context);
            v8::scope!(let scope, &mut context_scope);
            v8::tc_scope!(let tc, scope);
            let function = v8::Local::new(tc, handler);
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let returned = match timestamp {
                Some(ms) => {
                    let arg: v8::Local<v8::Value> = v8::Number::new(tc, ms).into();
                    function.call(tc, recv, &[arg])
                }
                None => function.call(tc, recv, &[]),
            };
            if returned.is_none() {
                inspector::report_caught(tc, &self.host);
            }
            tc.has_terminated()
        };
        self.host.exit_script();
        if terminated {
            CallOutcome::Terminated
        } else {
            CallOutcome::Completed
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Enter for the whole of teardown, and **unpaired on purpose**:
        // `OwnedIsolate`'s own `Drop` calls `exit()`, and that is this enter's
        // pair. V8 requires an isolate to be current to dispose it, and
        // `OwnedIsolate::drop` asserts exactly that (`Isolate::GetCurrent() ==
        // self`), so without this a page thread hosting two engines aborts
        // right here.
        //
        // A local `IsolateEntry` would *not* do the job: a guard local to this
        // body is released at the end of it, before the fields drop, so the
        // `isolate` field would then be disposed while the thread's current
        // isolate was something else — the very mismatch this pairing removes.
        //
        // **Invariant.** Entering is only sound at entry depth zero: an `Engine`
        // must never be dropped while its own isolate is entered, or the exit
        // `OwnedIsolate::drop` performs would leave the caller's frame pointing
        // at a freed isolate. The check below is what turns a violation of that
        // into a message.
        //
        // `#[cfg]` rather than `debug_assert!`: `debug_assert!` type-checks its
        // argument in every profile, and the mirror this reads is
        // debug-only, so a plain `debug_assert!` would demand a release-mode
        // `is_entered` — and a release build would pay for the mirror to have
        // something to answer.
        #[cfg(debug_assertions)]
        assert!(
            !IsolateEntry::is_entered(self.raw_isolate()),
            "an Engine is being dropped while its own isolate is entered on this \
             thread: the exit the isolate's disposal performs would leave the \
             caller's frame pointing at a freed isolate"
        );
        // SAFETY: `raw_isolate` is this engine's own isolate, alive until the
        // `isolate` field drops below, and the assertion above rules out the one
        // state — already entered — in which entering here would be unsound.
        unsafe { v8::Isolate::ref_from_raw_isolate_ptr(&self.raw_isolate()).enter() };

        // Release every script-handle a pending timer or animation-frame
        // callback still holds while the isolate is alive. These callbacks own
        // `v8::Global<Function>` handles (a live `setInterval` keeps one
        // indefinitely, so this is the common case, not an edge one); dropping
        // them here — rather than through field drop order, where `isolate` goes
        // first — keeps teardown sound regardless of that order, matching how the
        // heap guard and wrapper world are handled below. `ready`/`ready_frames`
        // hold handles for callbacks already fired but not yet invoked.
        self.event_loop.clear_pending_callbacks();
        self.host.ready.borrow_mut().clear();
        self.host.ready_frames.borrow_mut().clear();
        // The inspector's dispatcher and any unreported rejections are
        // `v8::Global`s in the same position.
        self.host.inspector.clear();
        // The module map's records and the resolvers of imports in flight.
        self.host.modules.clear();
        self.host.custom_elements.clear();
        clear_v8_callback_admission(&mut self.isolate);

        // Drain the wrapper world while the isolate is still alive: this
        // reclaims every host box and drops every still-armed `Weak`, which
        // disarms its finalizer so no later collection can run against a
        // reclaimed host. The world is the sole owner of the host boxes, so
        // this drain is the no-leak guarantee (`tests/wrapper_gc.rs`).
        if let Some(dom) = &self.dom {
            // Stop native observer admission before releasing any wrapper,
            // callback, or host-record roots. Retirement needs no Document
            // borrow and leaves physical pins for its next mutable seam.
            dom.ctx.documents.retire_observers();
            // Drop the script host first: it holds a raw isolate pointer and
            // clones of the world state, so it must go before the isolate is
            // disposed and before the world is drained below.
            dom.ctx.script.borrow_mut().take();

            // The promise seam's pending promises and queued settlements hold
            // `v8::Global<PromiseResolver>` handles, and its state lives in a
            // context slot, which V8 drops only when the isolate is disposed.
            // Clearing the tables here drops those handles while they are still
            // valid, the same reasoning as the timer handles cleared above.
            dom.promise.clear();

            // Apply every piece of dead bookkeeping a finalizer recorded and no
            // later `wrapper` call swept: the claims of node wrappers it
            // collected, and the snapshot and event slab entries of the others.
            // It must run before the `clear` below drops those lists, and it is
            // the same function the lazy sweep and `trim_memory` use, so the
            // three cannot drift.
            let _ = world::reclaim_dead_bookkeeping(&self.isolate, &dom.state, &dom.ctx.documents);

            // Release this world's remaining claims before disarming live
            // finalizers. Other worlds' wrappers retain their own claims, and
            // the generation prevents old deaths from touching a reused slot.
            let nodes = dom.state.borrow().wrapped_nodes();
            for (slot, node) in nodes {
                if let Some(ctx) = dom.ctx.documents.get(slot) {
                    ctx.doc.borrow_mut().release_wrapper(node);
                }
            }
            let storage_wrappers = dom.state.borrow_mut().clear();
            drop(storage_wrappers);
            // Records can outlive wrappers until teardown, including records
            // in documents minted by DOMImplementation. Drop retained range
            // endpoints while each document is still available, then break the
            // document-set/context ownership cycle.
            for ctx in dom.ctx.documents.all() {
                ctx.script.borrow_mut().take();
                ctx.host_records.replace(Default::default());
            }
            dom.ctx.documents.clear_minted();
        }
        // The generated constructor and interface-brand templates own Globals
        // in an isolate slot. Release them while the isolate is entered/alive.
        drop(
            self.isolate
                .remove_slot::<tenta_js::bindings::TemplateCache>(),
        );
        tenta_js::bindings::clear_legacy_reflect_set(&mut self.isolate);
        // Remove the near-heap-limit callback while both fields are still alive,
        // so the isolate can never dereference the `HeapGuard` pointer during
        // teardown. This makes soundness independent of struct field drop order
        // rather than relying on `isolate` being declared before `heap_guard`.
        if self.heap_guard.is_some() {
            self.isolate
                .remove_near_heap_limit_callback(on_near_heap_limit, 0);
        }
        // Same reasoning for the GC epilogue callback and the `GcCounter` box:
        // the callback has to be gone before either the isolate or the box is
        // dropped, and the `data` pointer has to be the one it was registered
        // with — V8 matches the pair, not the function alone. The pointer is
        // recomputed from the box, which has not moved.
        self.isolate.remove_gc_epilogue_callback(
            on_gc_epilogue,
            (&*self.gc_counter as *const GcCounter)
                .cast_mut()
                .cast::<std::os::raw::c_void>(),
        );
    }
}

/// Compiles and runs `source` on a throwaway isolate. Convenience for one-shot
/// evaluation; a page uses a long-lived [`Engine`].
pub fn evaluate(source: &str) -> Result<String, JsError> {
    Engine::new().evaluate(source)
}

/// The longest the wrapper graph goes uncollected while tasks keep ending.
const TASK_COLLECTION_INTERVAL: Duration = Duration::from_millis(250);

/// Installs `globalThis.DOMException` and its legacy code table
/// (<https://webidl.spec.whatwg.org/#idl-DOMException>).
/// Native binding failures use the captured prototype and native own-property
/// definitions, without calling the page-visible constructor.
const DOM_EXCEPTION_BOOTSTRAP: &str = r#"
(function () {
  "use strict";
  const CODES = {
    IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4,
    InvalidCharacterError: 5, NoModificationAllowedError: 7, NotFoundError: 8,
    NotSupportedError: 9, InUseAttributeError: 10, InvalidStateError: 11,
    SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14,
    InvalidAccessError: 15, TypeMismatchError: 17, SecurityError: 18,
    NetworkError: 19, AbortError: 20, URLMismatchError: 21,
    QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24,
    DataCloneError: 25,
  };
  function DOMException(message = "", name = "Error") {
    const self = Object.create(DOMException.prototype);
    Object.defineProperty(self, "message", {
      value: String(message), enumerable: false, configurable: true,
    });
    Object.defineProperty(self, "name", {
      value: String(name), enumerable: false, configurable: true,
    });
    if (typeof Error.captureStackTrace === "function") {
      Error.captureStackTrace(self, DOMException);
    }
    return self;
  }
  DOMException.prototype = Object.create(Error.prototype);
  Object.defineProperty(DOMException.prototype, "constructor", {
    value: DOMException, writable: true, configurable: true,
  });
  Object.defineProperty(DOMException.prototype, "name", {
    value: "Error", writable: true, configurable: true,
  });
  Object.defineProperty(DOMException.prototype, "message", {
    value: "", writable: true, configurable: true,
  });
  Object.defineProperty(DOMException.prototype, "code", {
    get() { return CODES[this.name] || 0; },
    enumerable: false, configurable: true,
  });
  const CONSTANTS = {
    INDEX_SIZE_ERR: 1, DOMSTRING_SIZE_ERR: 2, HIERARCHY_REQUEST_ERR: 3,
    WRONG_DOCUMENT_ERR: 4, INVALID_CHARACTER_ERR: 5, NO_DATA_ALLOWED_ERR: 6,
    NO_MODIFICATION_ALLOWED_ERR: 7, NOT_FOUND_ERR: 8, NOT_SUPPORTED_ERR: 9,
    INUSE_ATTRIBUTE_ERR: 10, INVALID_STATE_ERR: 11, SYNTAX_ERR: 12,
    INVALID_MODIFICATION_ERR: 13, NAMESPACE_ERR: 14, INVALID_ACCESS_ERR: 15,
    VALIDATION_ERR: 16, TYPE_MISMATCH_ERR: 17, SECURITY_ERR: 18,
    NETWORK_ERR: 19, ABORT_ERR: 20, URL_MISMATCH_ERR: 21,
    QUOTA_EXCEEDED_ERR: 22, TIMEOUT_ERR: 23, INVALID_NODE_TYPE_ERR: 24,
    DATA_CLONE_ERR: 25,
  };
  for (const key of Object.keys(CONSTANTS)) {
    const descriptor = { value: CONSTANTS[key], enumerable: true };
    Object.defineProperty(DOMException, key, descriptor);
    Object.defineProperty(DOMException.prototype, key, descriptor);
  }
  Object.defineProperty(globalThis, "DOMException", {
    value: DOMException, writable: true, configurable: true,
  });
})();
"#;

/// Installs the `DOMException` global into the entered context.
fn install_dom_exception(scope: &mut v8::PinScope) {
    let code = v8::String::new(scope, DOM_EXCEPTION_BOOTSTRAP)
        .expect("the DOMException bootstrap fits the V8 string limit");
    let script =
        v8::Script::compile(scope, code, None).expect("the DOMException bootstrap compiles");
    script
        .run(scope)
        .expect("the DOMException bootstrap runs without throwing");
    assert!(
        tenta_js::bindings::capture_v8_dom_exception_prototype(scope),
        "capture the realm's DOMException prototype before page script"
    );
}

/// The value a fetch completion settles its promise with, stored in the world,
/// paired with whether the settlement rejects.
///
/// The `Response` arm inserts the host record and builds its wrapper — the
/// driver delivers bytes and fields, never a V8 value, which is what keeps
/// `tenta-net` out of this crate's dependency list. The failure arm builds the
/// `TypeError` the Fetch Standard rejects a network error with.
///
/// `None` when the value cannot be built, which leaves that one promise
/// unsettled rather than failing the batch: a response this engine cannot
/// represent is not a reason to drop the completions behind it.
fn build_completion(
    dom: &DomWorld,
    scope: &mut v8::PinScope<'_, '_>,
    outcome: FetchOutcome,
) -> Option<(u64, bool)> {
    match outcome {
        FetchOutcome::Response(data) => {
            let key = insert_fetched_response(&dom.ctx, data);
            let wrapper = handle_to_v8::<tenta_js::bindings::Response>(
                scope,
                Handle::from_bits(encode_host(dom.ctx.slot, key)),
            )
            .ok()?;
            Some((dom.state.borrow_mut().store_value(scope, wrapper), false))
        }
        FetchOutcome::Failed(message) => {
            let message = v8::String::new(scope, &message)?;
            let error = construct_global(scope, "TypeError", &[message.into()])?;
            Some((dom.state.borrow_mut().store_value(scope, error), true))
        }
        FetchOutcome::Opaque => {
            let key = tenta_js::bindings::host::insert_opaque_response(&dom.ctx);
            let wrapper = handle_to_v8::<tenta_js::bindings::Response>(
                scope,
                Handle::from_bits(encode_host(dom.ctx.slot, key)),
            )
            .ok()?;
            Some((dom.state.borrow_mut().store_value(scope, wrapper), false))
        }
    }
}

/// Compiles and runs `source`. With `report`, an exception the script throws or
/// a syntax error is also reported to the realm's inspector as uncaught.
fn compile_and_run(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    source: &str,
    report: Option<&HostState>,
) -> Result<String, ScriptFailure> {
    let Some(code) = v8::String::new(tc, source) else {
        return Err(ScriptFailure {
            kind: ScriptErrorKind::Syntax,
            message: "source string exceeds the V8 string length limit".to_owned(),
        });
    };

    let Some(script) = v8::Script::compile(tc, code, None) else {
        // A compile that was terminated returns `None` just as one that did not
        // parse does, and the termination flag is the only thing that tells them
        // apart: reporting a stop as a syntax error would blame the page for the
        // engine's own limit.
        let kind = if tc.has_terminated() {
            ScriptErrorKind::Terminated
        } else {
            ScriptErrorKind::Syntax
        };
        let message = exception_message(tc);
        if let Some(host) = report {
            inspector::report_caught(tc, host);
        }
        return Err(ScriptFailure { kind, message });
    };
    let Some(value) = script.run(tc) else {
        // A terminated run has no exception value; report it as termination so
        // the caller (`Engine::run_inner`) can classify it as a deadline or
        // heap-limit stop instead of a misleading "uncaught exception with no
        // value".
        if tc.has_terminated() {
            return Err(ScriptFailure {
                kind: ScriptErrorKind::Terminated,
                message: "script execution was terminated".to_owned(),
            });
        }
        let message = exception_message(tc);
        if let Some(host) = report {
            inspector::report_caught(tc, host);
        }
        return Err(ScriptFailure {
            kind: ScriptErrorKind::Uncaught,
            message,
        });
    };

    match value.to_string(tc) {
        Some(string) => Ok(string.to_rust_string_lossy(tc)),
        None => Err(ScriptFailure {
            kind: ScriptErrorKind::Uncaught,
            message: "result value could not be coerced to a string".to_owned(),
        }),
    }
}

/// Extracts the pending exception message from a caught [`v8::TryCatch`]. The
/// kind is the caller's to supply: the same wording is reachable from a compile
/// and from a run, and only the call site knows which it was.
fn exception_message(tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>) -> String {
    let Some(value) = tc.exception() else {
        return "uncaught exception with no value".to_owned();
    };
    value
        .to_string(tc)
        .map(|string| string.to_rust_string_lossy(tc))
        .unwrap_or_else(|| "uncaught exception".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tenta_dom::Document;

    include!("css_graph_pressure_tests.inc.rs");
    include!("native_materialization_tests.inc.rs");
    include!("standalone_event_target_tests.inc.rs");
    include!("slow_panic_continuation_tests.inc.rs");
    include!("buffer_reference_runtime_tests.inc.rs");
    include!("crypto_budget_tests.inc.rs");
    include!("task_collection_tests.inc.rs");

    #[test]
    fn a_panic_in_the_host_promise_microtask_is_local() {
        let document = Rc::new(RefCell::new(Document::new()));
        let mut engine = Engine::with_document(document);
        let mut neighbor = Engine::new();
        engine
            .evaluate("new Response('ok').text().then(value => globalThis.settled = value)")
            .expect("queue a host promise settlement");
        let world = engine.dom.as_ref().expect("document world").state.clone();
        {
            // The settlement job takes the world to load the value. Holding its
            // borrow here makes that actual host path panic inside the V8
            // microtask, without adding a production-only test hook.
            let _borrow = world.borrow_mut();
            engine.run_microtasks();
        }
        assert_eq!(engine.native_failure(), Some("promise_microtask"));
        let error = engine.evaluate("42").expect_err("sticky native failure");
        assert!(error.message.contains("must be discarded"));
        engine.run_microtasks();
        assert_eq!(engine.evaluate("1").unwrap_err().message, error.message);
        assert_eq!(neighbor.evaluate("6 * 7").unwrap(), "42");
        drop(engine);
        assert_eq!(neighbor.evaluate("40 + 2").unwrap(), "42");
    }

    #[test]
    fn weak_finalizer_panics_poison_only_the_owning_engine() {
        for (source, callback) in [
            (
                "globalThis.kept = new Response('ok'); 42",
                "adopted_wrapper_finalizer",
            ),
            (
                "globalThis.kept = document.createElement('div'); 42",
                "wrapper_finalizer",
            ),
        ] {
            let document = Rc::new(RefCell::new(Document::new()));
            let mut engine = Engine::with_document(document);
            let mut neighbor = Engine::new();
            engine
                .evaluate(source)
                .expect("create a rooted host wrapper");
            // Task-end collection now runs after evaluate. Remove only the JS
            // root through V8's native property API, without entering another
            // script task that would collect the wrapper before the injected
            // finalizer borrow is in place.
            {
                let _entry = engine.entered();
                v8::scope!(let handle_scope, &mut engine.isolate);
                let context = v8::Local::new(handle_scope, &engine.context);
                let mut context_scope = v8::ContextScope::new(handle_scope, context);
                v8::scope!(let scope, &mut context_scope);
                let key = v8::String::new(scope, "kept").expect("property name");
                assert_eq!(context.global(scope).delete(scope, key.into()), Some(true));
            }
            if callback == "wrapper_finalizer" {
                // Node wrappers have a provisional native root until the
                // closed graph checkpoint transfers it to an ephemeron. Do
                // that transfer before the injected GC; otherwise this test
                // would exercise a rooted object and no finalizer at all.
                let (state, context) = {
                    let dom = engine.dom.as_ref().expect("document world");
                    (Rc::clone(&dom.state), dom.ctx.clone())
                };
                let _entry = engine.entered();
                v8::scope!(let handle_scope, &mut engine.isolate);
                let local = v8::Local::new(handle_scope, &engine.context);
                let mut context_scope = v8::ContextScope::new(handle_scope, local);
                v8::scope!(let scope, &mut context_scope);
                world::cycles::prepare(scope, &state, &context)
                    .expect("publish unrooted node wrapper for GC");
            }
            let world = engine.dom.as_ref().expect("document world").state.clone();
            for _ in 0..5 {
                {
                    // The finalizer takes this borrow when V8 collects the
                    // unreachable wrapper. The conflict tests the real GC
                    // callback boundary without an injectable product API.
                    let _borrow = world.borrow_mut();
                    engine.low_memory_gc();
                }
                if engine.native_failure().is_some() {
                    break;
                }
            }
            assert_eq!(engine.native_failure(), Some(callback));
            let error = engine.evaluate("1").expect_err("sticky native failure");
            assert!(error.message.contains("must be discarded"));
            assert_eq!(neighbor.evaluate("6 * 7").unwrap(), "42");
            drop(engine);
            assert_eq!(neighbor.evaluate("40 + 2").unwrap(), "42");
        }
    }

    #[test]
    fn evaluates_arithmetic_to_a_number() {
        assert_eq!(evaluate("2 + 3").unwrap(), "5");
    }

    #[test]
    fn evaluates_string_expressions() {
        assert_eq!(evaluate("'ten' + 'ta'").unwrap(), "tenta");
    }

    #[test]
    fn a_thrown_error_is_reported_not_panicked() {
        let error = evaluate("throw new Error('boom')").unwrap_err();
        assert!(
            error.message.contains("boom"),
            "expected the thrown message, got {error:?}"
        );
    }

    #[test]
    fn global_state_survives_across_evaluations_on_one_engine() {
        let mut engine = Engine::new();
        assert_eq!(engine.evaluate("globalThis.x = 41; x + 1").unwrap(), "42");
        // A second evaluation sees the global set by the first.
        assert_eq!(engine.evaluate("x + 1").unwrap(), "42");
    }

    #[test]
    fn an_infinite_loop_is_terminated_by_the_watchdog() {
        let mut engine = Engine::new();
        let error = engine
            .evaluate_with_timeout("while (true) {}", std::time::Duration::from_millis(200))
            .unwrap_err();
        assert!(
            error.message.contains("deadline"),
            "expected a timeout error, got {error:?}"
        );
        // The isolate is reusable after a terminated run.
        assert_eq!(engine.evaluate("1 + 1").unwrap(), "2");
    }

    #[test]
    fn a_watchdog_firing_near_completion_does_not_poison_later_runs() {
        let mut engine = Engine::new();
        // A zero deadline makes the watchdog terminate around the moment the
        // trivial script finishes, exercising the stray-termination window.
        // Every run must leave the isolate clean for the next one.
        for _ in 0..50 {
            let _ = engine.evaluate_with_timeout("1 + 1", std::time::Duration::from_millis(0));
        }
        // A subsequent normal evaluation must not inherit a stray termination.
        assert_eq!(engine.evaluate("3 + 4").unwrap(), "7");
    }

    #[test]
    fn deep_recursion_throws_range_error_and_the_engine_survives() {
        let mut engine = Engine::new();
        let error = engine
            .evaluate("function f() { return f(); } f();")
            .unwrap_err();
        assert!(
            error.message.contains("RangeError") || error.message.to_lowercase().contains("stack"),
            "expected a stack-overflow RangeError, got {error:?}"
        );
        // The stack overflow was an exception, not a crash.
        assert_eq!(engine.evaluate("2 + 2").unwrap(), "4");
    }

    #[test]
    fn exceeding_the_heap_limit_is_reported_and_the_engine_survives() {
        let mut engine = Engine::with_heap_limit(24 * 1024 * 1024);
        let error = engine
            .evaluate("const a = []; for (;;) { a.push(new Array(100_000).fill(0)); }")
            .unwrap_err();
        assert!(
            error.message.to_lowercase().contains("heap"),
            "expected a heap-limit error, got {error:?}"
        );
        // The near-heap-limit callback terminated the script instead of aborting
        // the process, so the isolate is still usable.
        assert_eq!(engine.evaluate("6 * 7").unwrap(), "42");
    }

    #[test]
    fn a_syntax_error_is_reported() {
        let error = evaluate("function (").unwrap_err();
        assert!(
            error.message.to_ascii_lowercase().contains("syntax"),
            "expected a syntax error, got {error:?}"
        );
    }

    #[test]
    fn a_set_timeout_handler_runs_when_the_loop_is_driven() {
        let mut engine = Engine::new();
        engine
            .evaluate("globalThis.hit = 0; setTimeout(() => { globalThis.hit = 42; }, 10);")
            .unwrap();
        // Nothing runs until the loop is driven.
        assert_eq!(engine.evaluate("globalThis.hit").unwrap(), "0");
        let run = engine.run_event_loop(100);
        assert_eq!(run, 1);
        assert_eq!(engine.evaluate("globalThis.hit").unwrap(), "42");
    }

    #[test]
    fn clear_timeout_prevents_a_handler_from_running() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.hit = 0; \
                 const id = setTimeout(() => { globalThis.hit = 1; }, 10); \
                 clearTimeout(id);",
            )
            .unwrap();
        assert_eq!(engine.run_event_loop(100), 0);
        assert_eq!(engine.evaluate("globalThis.hit").unwrap(), "0");
    }

    #[test]
    fn timers_run_in_deadline_order_across_the_boundary() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.log = ''; \
                 setTimeout(() => { globalThis.log += 'b'; }, 20); \
                 setTimeout(() => { globalThis.log += 'a'; }, 10);",
            )
            .unwrap();
        assert_eq!(engine.run_event_loop(100), 2);
        assert_eq!(engine.evaluate("globalThis.log").unwrap(), "ab");
    }

    #[test]
    fn a_timer_can_schedule_another_timer() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.n = 0; \
                 setTimeout(() => { globalThis.n++; \
                   setTimeout(() => { globalThis.n++; }, 5); }, 5);",
            )
            .unwrap();
        assert_eq!(engine.run_event_loop(100), 2);
        assert_eq!(engine.evaluate("globalThis.n").unwrap(), "2");
    }

    #[test]
    fn queue_microtask_runs_at_the_checkpoint() {
        let mut engine = Engine::new();
        engine
            .evaluate("globalThis.m = ''; queueMicrotask(() => { globalThis.m = 'ran'; });")
            .unwrap();
        // The microtask waits for the checkpoint the driven loop performs.
        assert_eq!(engine.evaluate("globalThis.m").unwrap(), "");
        engine.run_event_loop(100);
        assert_eq!(engine.evaluate("globalThis.m").unwrap(), "ran");
    }

    #[test]
    fn a_promise_reaction_resolves_when_the_loop_drains_microtasks() {
        let mut engine = Engine::new();
        engine
            .evaluate("globalThis.p = 0; Promise.resolve().then(() => { globalThis.p = 7; });")
            .unwrap();
        assert_eq!(engine.evaluate("globalThis.p").unwrap(), "0");
        engine.run_event_loop(100);
        assert_eq!(engine.evaluate("globalThis.p").unwrap(), "7");
    }

    #[test]
    fn the_task_budget_bounds_a_set_timeout_recursion() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.c = 0; \
                 function spin() { globalThis.c++; setTimeout(spin, 0); } \
                 setTimeout(spin, 0);",
            )
            .unwrap();
        // A self-rescheduling timer would run forever; the budget caps it.
        let run = engine.run_event_loop(10);
        assert_eq!(run, 10);
        assert_eq!(engine.evaluate("globalThis.c").unwrap(), "10");
    }

    #[test]
    fn a_throwing_timer_handler_is_contained_and_the_loop_continues() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.x = 0; \
                 setTimeout(() => { throw new Error('boom'); }, 10); \
                 setTimeout(() => { globalThis.x = 1; }, 20);",
            )
            .unwrap();
        // The first handler throws; it must not abort the loop or corrupt the
        // isolate, so the second handler still runs and both count as run.
        let run = engine.run_event_loop(100);
        assert_eq!(run, 2);
        assert_eq!(engine.evaluate("globalThis.x").unwrap(), "1");
        // The isolate is still healthy for ordinary evaluation.
        assert_eq!(engine.evaluate("2 + 2").unwrap(), "4");
    }

    #[test]
    fn dropping_an_engine_with_unrun_timers_is_safe() {
        // Both a fired-but-not-invoked handler (in the ready queue) and an
        // un-fired one (held in the scheduler's timer closure) leave a
        // `v8::Global<Function>` alive at drop; rusty_v8 no-ops those resets once
        // the isolate is disposed, so this must not crash under the leak/UAF
        // checkers.
        let mut engine = Engine::new();
        engine
            .evaluate(
                "setTimeout(() => { globalThis.x = 1; }, 1_000_000); \
                 setTimeout(() => { globalThis.y = 2; }, 5);",
            )
            .unwrap();
        // Advance the clock enough to move the 5 ms timer onto the ready queue,
        // but do not invoke it, then drop with both Globals still held.
        engine.event_loop.advance_to_next_timer();
        assert!(!engine.host.ready.borrow().is_empty());
        drop(engine);
    }

    #[test]
    fn an_animation_frame_callback_runs_when_a_frame_is_driven() {
        let mut engine = Engine::new();
        engine
            .evaluate("globalThis.f = -1; requestAnimationFrame((ts) => { globalThis.f = ts; });")
            .unwrap();
        // Nothing runs until a frame is driven.
        assert_eq!(engine.evaluate("globalThis.f").unwrap(), "-1");
        engine.run_animation_frame();
        // At the loop origin the frame timestamp is zero.
        assert_eq!(engine.evaluate("globalThis.f").unwrap(), "0");
    }

    #[test]
    fn a_frame_callback_receives_the_advancing_frame_timestamp() {
        let mut engine = Engine::new();
        engine
            .evaluate("globalThis.f = -1; requestAnimationFrame((ts) => { globalThis.f = ts; });")
            .unwrap();
        // tick_frame advances the virtual clock one 60 Hz frame (~16.667 ms)
        // before running the callback.
        engine.tick_frame();
        assert_eq!(
            engine
                .evaluate("globalThis.f > 16 && globalThis.f < 17")
                .unwrap(),
            "true"
        );
    }

    #[test]
    fn cancel_animation_frame_prevents_a_callback_from_running() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.f = 0; \
                 const id = requestAnimationFrame(() => { globalThis.f = 1; }); \
                 cancelAnimationFrame(id);",
            )
            .unwrap();
        engine.run_animation_frame();
        assert_eq!(engine.evaluate("globalThis.f").unwrap(), "0");
    }

    #[test]
    fn a_frame_callback_that_reschedules_runs_on_the_following_frame() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.count = 0; \
                 function tick() { globalThis.count++; \
                   if (globalThis.count < 2) requestAnimationFrame(tick); } \
                 requestAnimationFrame(tick);",
            )
            .unwrap();
        engine.run_animation_frame();
        assert_eq!(engine.evaluate("globalThis.count").unwrap(), "1");
        // The reschedule lands on the next frame, not this one.
        engine.run_animation_frame();
        assert_eq!(engine.evaluate("globalThis.count").unwrap(), "2");
    }

    #[test]
    fn a_throwing_frame_callback_is_contained_and_the_next_frame_runs() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.f = 0; \
                 requestAnimationFrame(() => { throw new Error('boom'); }); \
                 requestAnimationFrame(() => { globalThis.f = 1; });",
            )
            .unwrap();
        // The first callback throws; it must not abort the frame or corrupt the
        // isolate, so the second callback in the same frame still runs.
        engine.run_animation_frame();
        assert_eq!(engine.evaluate("globalThis.f").unwrap(), "1");
        assert_eq!(engine.evaluate("2 + 2").unwrap(), "4");
    }

    #[test]
    fn tick_frame_runs_due_timers_then_the_animation_frame() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.log = ''; \
                 setTimeout(() => { globalThis.log += 't'; }, 5); \
                 requestAnimationFrame(() => { globalThis.log += 'r'; });",
            )
            .unwrap();
        // One 60 Hz frame is ~16.7 ms, so the 5 ms timer is due; it runs before
        // the rendering opportunity's animation-frame callback.
        engine.tick_frame();
        assert_eq!(engine.evaluate("globalThis.log").unwrap(), "tr");
    }

    #[test]
    fn ranges_keep_foreign_endpoints_and_teardown_releases_child_contexts() {
        use tenta_js::engine::PageEngine;

        let root = std::rc::Rc::new(std::cell::RefCell::new(tenta_dom::Document::new()));
        let mut engine = Engine::with_document(root);
        engine
            .evaluate(
                r#"
            (() => {
                const foreign = new Document();
                const text = foreign.createTextNode('foreign');
                globalThis.range = document.createRange();
                range.selectNodeContents(text);
                globalThis.staticRange = new StaticRange({
                    startContainer: text, startOffset: 1,
                    endContainer: document, endOffset: 0
                });
            })();
        "#,
            )
            .unwrap();
        let child = engine.dom.as_ref().unwrap().ctx.documents.get(1).unwrap();
        let document = child.doc.clone();
        let weak_context = std::rc::Rc::downgrade(&child);
        drop(child);
        engine.trim_memory();
        assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 0);
        assert_eq!(
            engine
                .evaluate("String(range) + ':' + staticRange.startContainer.data")
                .unwrap(),
            "foreign:foreign"
        );
        drop(engine);
        assert!(
            weak_context.upgrade().is_none(),
            "teardown breaks the document-set cycle"
        );
        assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 1);
    }

    #[test]
    fn collected_foreign_ranges_do_not_reclaim_the_pages_same_slab_key() {
        let document = std::rc::Rc::new(std::cell::RefCell::new(tenta_dom::Document::new()));
        let mut engine = Engine::with_document(document);
        engine
            .evaluate(
                r#"
            globalThis.kept = document.createRange();
            (() => {
                const foreign = new Document();
                const range = foreign.createRange();
                range.selectNodeContents(foreign.createTextNode('collected'));
            })();
        "#,
            )
            .unwrap();
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
        assert_eq!(engine.host_record_count(), 1);
        let child = engine.dom.as_ref().unwrap().ctx.documents.get(1).unwrap();
        assert!(child.host_records.borrow().is_empty());
        assert_eq!(
            engine.evaluate("kept.startContainer === document").unwrap(),
            "true"
        );
    }
}
