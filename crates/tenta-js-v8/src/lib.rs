//! V8 backend (rusty_v8): isolates, snapshots, fast API, native Wasm, worker isolates.
//!
//! P2 package (a) foundation (`docs/spec/05-javascript.md` §4). This first
//! slice initialises the V8 platform once per process and evaluates a script
//! on a freshly created isolate. The per-thread isolate pool, the dedicated
//! engine root thread that owns `V8::initialize`, the custom `PlatformImpl`
//! task routing and the startup snapshot land in the following WP1 steps; the
//! `evaluate` entry point and its error mapping are kept stable so they grow
//! into the pooled path rather than being replaced.

#[allow(unused_imports)]
use tenta_core as _;

mod host;
mod page_engine;
mod promise;
mod script_host;
mod world;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;

use tenta_js::bindings::WrapperWorld;
use tenta_js::bindings::host::{
    HostContext, HostObject, SharedContext, SharedDocument, encode_host, insert_fetched_response,
};
use tenta_js::bindings::{Handle, handle_to_v8};
use tenta_js::dom::events::ScriptHost;
use tenta_js::engine::ScriptErrorKind;
use tenta_js::event_loop::{Clock, EventLoop, FiringRelease, MonoTime};
use tenta_js::fetch::{FetchCompletion, FetchOutcome};
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
    guard
        .tripped
        .store(true, std::sync::atomic::Ordering::Release);
    guard.handle.terminate_execution();
    // Headroom so the interrupted allocation can complete and unwind.
    current_heap_limit + current_heap_limit / 2
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
    _isolate: v8::UnsafeRawIsolatePtr,
    _gc_type: v8::GCType,
    _flags: v8::GCCallbackFlags,
    data: *mut std::os::raw::c_void,
) {
    // SAFETY: `data` is the pointer to the `GcCounter` boxed in the owning
    // `Engine`, which outlives the isolate that holds this callback.
    let counter = unsafe { &*(data as *const GcCounter) };
    counter
        .epoch
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        Self::build(max_heap_bytes, Some(doc), clock, cookies)
    }

    fn build(
        max_heap_bytes: Option<usize>,
        doc: Option<SharedDocument>,
        clock: Clock,
        cookies: CookieJar,
    ) -> Self {
        ensure_platform();
        let mut params = v8::CreateParams::default();
        if let Some(max) = max_heap_bytes {
            params = params.heap_limits(0, max);
        }
        let mut isolate = v8::Isolate::new(params);

        // The event loop owns "when a task runs"; V8 must not drain microtasks on
        // its own, so the engine can run them at the HTML checkpoint instead.
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);

        let heap_guard = max_heap_bytes.map(|_| {
            let mut guard = Box::new(HeapGuard {
                handle: isolate.thread_safe_handle(),
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

        let event_loop = EventLoop::new(clock);
        let host = HostState::new(event_loop.scheduler(), cookies);

        let (context, dom) = {
            v8::scope!(let handle_scope, &mut isolate);
            match doc {
                None => {
                    let local = v8::Context::new(handle_scope, Default::default());
                    {
                        let mut context_scope = v8::ContextScope::new(handle_scope, local);
                        v8::scope!(let scope, &mut context_scope);
                        host::install(scope, local, host.scheduler.clock().time_origin_ms());
                    }
                    // The host callbacks reach this state through the context slot.
                    local.set_slot::<HostState>(host.clone());
                    (v8::Global::new(handle_scope, local), None)
                }
                Some(doc) => Self::install_dom(handle_scope, &host, doc),
            }
        };
        let mut engine = Engine {
            isolate,
            context,
            heap_guard,
            gc_counter,
            event_loop,
            host,
            script_timeout: None,
            dom,
        };
        engine.install_script_host();
        engine
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
        // SAFETY: `as_raw_isolate_ptr` reads the live isolate's address, only
        // ever reconstructed by `invoke_listener`, which runs synchronously
        // inside a DOM binding call on this engine's thread while the engine
        // (and thus the isolate) is alive. `Drop` clears the stored host before
        // the isolate is disposed.
        let isolate = unsafe { self.isolate.as_raw_isolate_ptr() };
        let script: Rc<dyn ScriptHost> = Rc::new(V8ScriptHost::new(
            isolate,
            self.context.clone(),
            dom.state.clone(),
            self.host.clone(),
        ));
        *dom.ctx.script.borrow_mut() = Some(script);
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
    ) -> (v8::Global<v8::Context>, Option<DomWorld>) {
        let state = Rc::new(RefCell::new(world::WorldState::default()));
        // Events timestamp against the loop clock, so `evt.timeStamp` and
        // `performance.now()` are readings of the same clock.
        let ctx = Rc::new(HostContext::new(
            doc.clone(),
            host.scheduler.clock(),
            host.cookies.clone(),
        ));
        let document_ref = {
            let doc = doc.borrow();
            doc.node_ref(doc.document_node())
        };

        // The generated installers require a context-carrying scope, but the
        // global template must exist before the context built from it. A
        // throwaway bootstrap context supplies that scope; the templates it
        // builds are isolate-global, so the real context inherits every
        // interface.
        let bootstrap = v8::Context::new(handle_scope, Default::default());
        let mut bootstrap_scope = v8::ContextScope::new(handle_scope, bootstrap);
        v8::scope!(let scope, &mut bootstrap_scope);

        let global_template = v8::ObjectTemplate::new(scope);
        // This one template outlives the block: the window's prototype chain is
        // chained to `EventTarget.prototype` once the context exists
        // (`host::install_window_prototype`).
        let event_target;
        {
            let mut registry = world::SkeletonRegistry::default();
            // Install interfaces parent-first: EventTarget is the root (Node
            // inherits it), so it must precede Node.
            event_target = tenta_js::bindings::install_event_target::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the EventTarget interface");
            tenta_js::bindings::install_node::<HostObject>(scope, global_template, &mut registry)
                .expect("install the Node interface");
            tenta_js::bindings::install_character_data::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the CharacterData interface");
            tenta_js::bindings::install_text::<HostObject>(scope, global_template, &mut registry)
                .expect("install the Text interface");
            tenta_js::bindings::install_comment::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the Comment interface");
            tenta_js::bindings::install_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the Element interface");
            // SVGElement : Element — installs after Element; SVG-namespace
            // elements get this prototype (see world.rs interface selection).
            tenta_js::bindings::install_svg_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the SVGElement interface");
            // HTMLElement's prototype chain roots at Element, so it installs
            // after Element and before Document (HTML-namespace elements get
            // the HTMLElement prototype; see world.rs interface selection).
            tenta_js::bindings::install_html_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLElement interface");
            // HTMLIFrameElement : HTMLElement — installs after HTMLElement;
            // `<iframe>` elements get this prototype. (The snake name fuses the
            // `HTMLI` capital run per the generator's word splitter.)
            tenta_js::bindings::install_htmli_frame_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLIFrameElement interface");
            // HTMLTemplateElement : HTMLElement — its `content` returns a
            // DocumentFragment, so DocumentFragment must be installed too.
            tenta_js::bindings::install_html_template_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLTemplateElement interface");
            // Phase 4d per-tag interfaces, all `: HTMLElement`, so they install
            // after it. Each gives its tag's wrappers a distinct prototype and a
            // global constructor for `instanceof` (see world.rs tag selection).
            tenta_js::bindings::install_html_div_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLDivElement interface");
            tenta_js::bindings::install_html_span_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLSpanElement interface");
            tenta_js::bindings::install_html_paragraph_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLParagraphElement interface");
            tenta_js::bindings::install_html_anchor_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLAnchorElement interface");
            tenta_js::bindings::install_html_image_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLImageElement interface");
            tenta_js::bindings::install_html_label_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLLabelElement interface");
            tenta_js::bindings::install_html_form_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLFormElement interface");
            tenta_js::bindings::install_html_input_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLInputElement interface");
            tenta_js::bindings::install_html_button_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLButtonElement interface");
            tenta_js::bindings::install_html_select_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLSelectElement interface");
            tenta_js::bindings::install_html_text_area_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLTextAreaElement interface");
            tenta_js::bindings::install_html_option_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLOptionElement interface");
            tenta_js::bindings::install_html_script_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLScriptElement interface");
            tenta_js::bindings::install_document_fragment::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the DocumentFragment interface");
            tenta_js::bindings::install_document::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the Document interface");
            tenta_js::bindings::install_node_list::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the NodeList interface");
            tenta_js::bindings::install_html_collection::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLCollection interface");
            tenta_js::bindings::install_dom_token_list::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the DOMTokenList interface");
            tenta_js::bindings::install_css_style_declaration::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the CSSStyleDeclaration interface");
            // Event has no DOM parent (its prototype chain roots at Object), so
            // its install order relative to the node types is free.
            tenta_js::bindings::install_event::<HostObject>(scope, global_template, &mut registry)
                .expect("install the Event interface");
            // CustomEvent's prototype chain roots at Event, so it installs after.
            tenta_js::bindings::install_custom_event::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the CustomEvent interface");
            // UIEvent : Event, then MouseEvent : UIEvent (parent-first).
            tenta_js::bindings::install_ui_event::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the UIEvent interface");
            tenta_js::bindings::install_mouse_event::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the MouseEvent interface");
            // ProgressEvent's prototype chain also roots at Event.
            tenta_js::bindings::install_progress_event::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the ProgressEvent interface");
            // `URL` and `URLSearchParams` carry no parent chain of their own;
            // their order among themselves does not matter, but
            // `URLSearchParams` must precede no one either — `URL.searchParams`
            // resolves the template through the isolate's cache at call time.
            // `Headers` roots at `Object` too, and names its constructor from
            // the interface name alone.
            tenta_js::bindings::install_url_search_params::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the URLSearchParams interface");
            tenta_js::bindings::install_url::<HostObject>(scope, global_template, &mut registry)
                .expect("install the URL interface");
            tenta_js::bindings::install_headers::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the Headers interface");
            // `AbortSignal : EventTarget`, so `EventTarget` must already be in
            // the template cache; `AbortController` roots at `Object`. Installing
            // the signal first keeps the two in dependency order, and both are
            // independent of the URL block above.
            tenta_js::bindings::install_abort_signal::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the AbortSignal interface");
            tenta_js::bindings::install_abort_controller::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the AbortController interface");
            // The encoding pair roots at `Object` and names its constructors
            // from the interface name alone, so its order among the JS-owned
            // hosts does not matter. `TextDecoder`'s `decode` is the second
            // consumer of the buffer-source conversion after the toy fixture.
            tenta_js::bindings::install_text_encoder::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the TextEncoder interface");
            tenta_js::bindings::install_text_decoder::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the TextDecoder interface");
            // `Response` roots at `Object` too. It is installed last of the
            // JS-owned hosts because it is the only one that *owns* another
            // (`Response.headers`), and a `Response` constructor fills that
            // `Headers` through the same record it hands out here.
            tenta_js::bindings::install_response::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the Response interface");
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
            host::install(inner, local, host.scheduler.clock().time_origin_ms());
            // The window is an `EventTarget`, so its prototype chain has to
            // reach that interface's prototype — the one fact about the window
            // the generated glue cannot express, since the window is the global
            // object rather than an instance of any template.
            host::install_window_prototype(inner, local, &event_target);
            install_dom_exception(inner);

            // Materialise the `document` wrapper, expose it as a global, then
            // hand the world to the runtime so the generated getters can create
            // further wrappers (e.g. `documentElement`) on demand.
            let world = world::V8World::new(state.clone(), ctx.clone());
            let document_object = world
                .wrapper(
                    inner,
                    "Document",
                    tenta_js::bindings::host::encode_node(document_ref),
                )
                .expect("the document wrapper");
            let key = v8::String::new(inner, "document").expect("`document` fits the string limit");
            local
                .global(inner)
                .set(inner, key.into(), document_object.into());
            tenta_js::bindings::set_wrapper_world(inner, Box::new(world));
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
        self.isolate.low_memory_notification();
    }

    /// Drains event wrappers a finalizer has collected, releasing their slab
    /// records and any interned `CustomEvent` detail (test-only; the same sweep
    /// runs lazily at the head of `wrapper`). Call after [`Engine::low_memory_gc`]
    /// to make reclamation observable without waiting for the next wrapper.
    pub fn reclaim_dead_events(&mut self) {
        if let Some(dom) = &self.dom {
            let dead = dom.state.borrow_mut().take_dead_events();
            let mut state = dom.state.borrow_mut();
            world::reclaim_dead_events(&mut state, &dom.ctx.events, dead);
        }
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
        if let Some(dom) = &self.dom {
            let dead = dom.state.borrow_mut().take_dead_hosts();
            let mut state = dom.state.borrow_mut();
            world::reclaim_dead_hosts(&mut state, &dom.ctx, dead);
        }
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
        self.run_inner(source, timeout).map(|_| ())
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
        self.isolate.cancel_terminate_execution();
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
        // One of the three ways script is entered (see
        // `discard_stray_termination`): clear before the watchdog is even
        // spawned, so a stray termination from an earlier entry cannot abort
        // this script and be blamed on this run's own deadline.
        self.discard_stray_termination();

        let watchdog = timeout.map(|timeout| {
            let handle = self.isolate.thread_safe_handle();
            let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
            let join = std::thread::spawn(move || {
                // Terminate only on a genuine deadline; an early cancel (the run
                // finished) or a dropped sender (the caller panicked) is a no-op.
                if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    cancel_rx.recv_timeout(timeout)
                {
                    handle.terminate_execution();
                }
            });
            (cancel_tx, join)
        });

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
            let outcome = compile_and_run(tc, source);
            (outcome, tc.has_terminated())
        };
        self.host.exit_script();

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
        if terminated || timeout.is_some() {
            self.isolate.cancel_terminate_execution();
        }

        if heap_tripped {
            Err(ScriptFailure {
                kind: ScriptErrorKind::Terminated,
                message: "script exceeded the isolate heap limit".to_owned(),
            })
        } else if terminated && timeout.is_some() {
            Err(ScriptFailure {
                kind: ScriptErrorKind::Terminated,
                message: format!(
                    "script execution exceeded the {:?} deadline",
                    timeout.expect("timeout is set when a deadline watchdog ran")
                ),
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
        // Microtasks queued by the preceding top-level script run first.
        self.checkpoint();

        let mut run = 0;
        while run < max_tasks {
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
            // handler that had not run yet. (A panic *inside* a binding does not
            // reach this frame at all: it crosses an `extern "C"` boundary and
            // aborts the process.)
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
        self.discard_stray_termination();
        self.isolate.perform_microtask_checkpoint();
        self.host.microtasks_pending.set(false);
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
        if completions.is_empty() {
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
            self.host.microtasks_pending.set(true);
        }
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
        while taken < budget {
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
        // Handlers a bounded pump held back are due work the timer heap no
        // longer knows about: it handed them over when they fired. A microtask
        // checkpoint owed since the last script entry is work no queue here
        // knows about at all — V8's microtask queue is not introspectable.
        // Parking on the timer heap alone would leave either unrun until some
        // unrelated timer happened to come due.
        if !self.host.ready.borrow().is_empty()
            || !self.host.ready_frames.borrow().is_empty()
            || self.host.microtasks_pending.get()
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
        self.checkpoint();

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
        loop {
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
    /// The nesting depth and provenance are reinstalled here, around the JS
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
        let nesting = ready.nesting;
        let handler = &ready.handler;
        scheduler.with_nesting(nesting, || {
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
        self.discard_stray_termination();
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
            match timestamp {
                Some(ms) => {
                    let arg: v8::Local<v8::Value> = v8::Number::new(tc, ms).into();
                    let _ = function.call(tc, recv, &[arg]);
                }
                None => {
                    let _ = function.call(tc, recv, &[]);
                }
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

        // Drain the wrapper world while the isolate is still alive: this
        // reclaims every host box and drops every still-armed `Weak`, which
        // disarms its finalizer so no later collection can run against a
        // reclaimed host. The world is the sole owner of the host boxes, so
        // this drain is the no-leak guarantee (`tests/wrapper_gc.rs`).
        if let Some(dom) = &self.dom {
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
            // later `wrapper` call swept: `HAS_WRAPPER` for the node wrappers it
            // collected, and the snapshot and event slab entries of the others.
            // It must run before the `clear` below drops those lists, and it is
            // the same function the lazy sweep and `trim_memory` use, so the
            // three cannot drift.
            world::reclaim_dead_bookkeeping(&dom.state, &dom.ctx);

            // Clear `HAS_WRAPPER` for every node still wrapped — the anchor of
            // every live host (the ids a finalizer recorded were cleared just
            // above) — while the document is still borrowable and the isolate
            // still alive, before dropping the hosts. Wrappers still live at
            // teardown are dropped here without ever running a finalizer, so
            // their bits must be cleared explicitly; otherwise a document that
            // outlives the engine keeps those nodes pinned forever.
            let ids = dom.state.borrow().wrapped_node_ids();
            if !ids.is_empty() {
                let mut doc = dom.ctx.doc.borrow_mut();
                for id in ids {
                    doc.set_has_wrapper(id, false);
                }
            }
            dom.state.borrow_mut().clear();
        }
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

/// Compiles `source`, runs it, and coerces the result to a string, mapping any
/// caught exception to [`JsError`].
/// JavaScript that installs `globalThis.DOMException` with the legacy code
/// table (<https://webidl.spec.whatwg.org/#idl-DOMException>). The binding throw
/// path (`throw_js_error`) constructs instances with `new DOMException(message,
/// name)`, so `e.name`, `e.message`, `e.code` and `e instanceof DOMException`
/// all behave per the standard.
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
                Handle::from_bits(encode_host(key)),
            )
            .ok()?;
            Some((dom.state.borrow_mut().store_value(scope, wrapper), false))
        }
        FetchOutcome::Failed(message) => {
            let message = v8::String::new(scope, &message)?;
            let error = construct_global(scope, "TypeError", &[message.into()])?;
            Some((dom.state.borrow_mut().store_value(scope, error), true))
        }
    }
}

fn compile_and_run(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    source: &str,
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
        return Err(ScriptFailure {
            kind,
            message: exception_message(tc),
        });
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
        return Err(ScriptFailure {
            kind: ScriptErrorKind::Uncaught,
            message: exception_message(tc),
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
}
