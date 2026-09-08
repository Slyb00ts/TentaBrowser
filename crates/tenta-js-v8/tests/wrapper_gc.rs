//! Phase 0b de-risking spike: the wrapper garbage-collection lifecycle.
//!
//! Before wiring the generated DOM glue into a real `WrapperWorld`
//! (`docs/spec/05-javascript.md` §3), this test proves the load-bearing
//! mechanics against the pinned rusty_v8 (`v8` 152.2.0) in isolation:
//!
//!  1. A wrapper `Object` carries its Rust host behind an aligned pointer in
//!     internal field 0 — the exact convention the generator's prelude uses
//!     (`WRAPPER_HOST_FIELD = 0`, `WRAPPER_TAG = 0`), so `unwrap_this` can reach
//!     the host synchronously from a JS callback.
//!  2. Wrapper identity: a live node maps to one wrapper. Asking the world for
//!     an id whose wrapper is still alive returns the same object and creates no
//!     second host, so a live entry is never overwritten.
//!  3. When a wrapper becomes unreachable and V8 collects it, a
//!     `Weak::with_finalizer` callback runs and reclaims the host, so nothing
//!     leaks over the lifetime of a page.
//!  4. Teardown of a world that still holds live wrappers reclaims every host
//!     and disarms every finalizer (by dropping the `Weak`s) before the isolate
//!     is disposed — exactly once each, with no double free.
//!
//! Three soundness facts from rusty_v8 shape the design:
//!  - `with_finalizer`'s callback runs *after* the object is collected, so it
//!    must NOT read the object's internal field. The host `Box` is therefore
//!    owned by the world, keyed by id; the internal field only borrows the
//!    pointer, and the finalizer captures the *id* (never the object).
//!  - **A finalizer must never drop a `v8::Weak`.** rusty_v8's
//!    `second_pass_callback` holds a `&WeakData` across the finalizer call
//!    (handle.rs); dropping the corresponding `Weak` inside the finalizer
//!    deallocates that `WeakData` and leaves rusty_v8's reference dangling. So
//!    the finalizer touches only the host map; the now-dead `Weak` is left as a
//!    tombstone and swept OUTSIDE any finalizer (on reuse, or at teardown).
//!  - Dropping a `Weak` cancels its pending finalizer (rusty_v8 "the finalizer
//!    for a dropped Weak is never called"), so the teardown drain disarms
//!    finalizers and cannot race the GC path.
//!
//! No-leak does NOT depend on best-effort finalizers ever firing: the teardown
//! drain (the world is the sole `Box` owner) is the guarantee; the finalizer is
//! only eager reclamation during a long-lived page. Note the real generator
//! `attach_host` instead `Box::into_raw`s the host into the field; that model is
//! equivalent here — its finalizer would reclaim via a captured raw pointer
//! (never by reading the collected field, and never dropping a `Weak`).
//!
//! This runs in its own test binary with `--expose_gc` set before the platform
//! is initialised. It deliberately does not touch `tenta_js_v8::ensure_platform`
//! (which must never expose `gc()` to web content); a browser build forces
//! collection with `low_memory_notification`, not this flag.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::Once;

/// Initialises the V8 platform once for this test process with `--expose_gc`,
/// which `request_garbage_collection_for_testing` requires. The flag is read
/// during `V8::initialize`, so it must be set first.
fn init_v8_with_gc() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        v8::V8::set_flags_from_string("--expose_gc");
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

/// The generator's host-attach convention (`prelude.rs`): host pointer lives in
/// internal field 0 under tag 0.
const WRAPPER_HOST_FIELD: i32 = 0;
const WRAPPER_TAG: u16 = 0;

/// Stores a borrowed host pointer in the wrapper's internal field, mirroring
/// `attach_host`. The pointer is owned by the world; this is the fast-path copy
/// `unwrap_this` reads.
fn attach_host_ptr(wrapper: &v8::Local<v8::Object>, host: *const c_void) {
    wrapper.set_aligned_pointer_in_internal_field(WRAPPER_HOST_FIELD, host, WRAPPER_TAG);
}

/// Reads the borrowed host pointer back, mirroring `unwrap_this`.
///
/// # Safety
/// The wrapper must still be alive and its host still owned by the world;
/// callers must not read a wrapper whose host has been reclaimed.
unsafe fn read_host_ptr(wrapper: &v8::Local<v8::Object>) -> *const c_void {
    unsafe { wrapper.get_aligned_pointer_from_internal_field(WRAPPER_HOST_FIELD, WRAPPER_TAG) }
}

/// A stand-in for a DOM host object. Bumps a shared counter when dropped so the
/// test can prove exactly-once reclamation.
struct HostProbe {
    id: u64,
    drops: Rc<Cell<u32>>,
}

impl Drop for HostProbe {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

/// The world's state, split so the finalizer never has to drop a `Weak`:
/// `hosts` owns the host `Box`es (the sole reclaim point), and `wrappers` holds
/// the weak handles for identity. A finalizer removes only from `hosts`.
#[derive(Default)]
struct WorldState {
    hosts: HashMap<u64, Box<HostProbe>>,
    wrappers: HashMap<u64, v8::Weak<v8::Object>>,
}

type World = Rc<RefCell<WorldState>>;

/// Returns the wrapper for `id`, reusing the live one if it exists.
///
/// This is the identity invariant the real `WrapperWorld` enforces by checking
/// the map first: while a wrapper is alive its entry is never overwritten, so a
/// host is created at most once per live id.
fn wrapper_for<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    world: &World,
    id: u64,
    drops: &Rc<Cell<u32>>,
    finalizer_runs: &Rc<Cell<u32>>,
) -> v8::Local<'s, v8::Object> {
    // Fast path: a live wrapper for this id is revived and reused. The RefCell
    // borrow is released with the block; only the revived `Local` escapes.
    if let Some(existing) = {
        let state = world.borrow();
        state
            .wrappers
            .get(&id)
            .and_then(|weak| weak.to_local(scope))
    } {
        return existing;
    }

    let template = v8::ObjectTemplate::new(scope);
    assert!(
        template.set_internal_field_count(1),
        "template must accept one internal field"
    );
    let wrapper = template
        .new_instance(scope)
        .expect("instance from a one-field template");

    // Box the host so its address is stable: moving the Box into the map, and
    // any later HashMap table growth, moves only the pointer, never the
    // pointee. The internal field therefore keeps pointing at a valid host.
    let host = Box::new(HostProbe {
        id,
        drops: drops.clone(),
    });
    let raw = host.as_ref() as *const HostProbe as *const c_void;
    attach_host_ptr(&wrapper, raw);

    let world_fin = world.clone();
    let runs = finalizer_runs.clone();
    let weak = v8::Weak::with_finalizer(
        scope,
        wrapper,
        Box::new(move |_isolate| {
            // Eager reclamation: drop the host Box (the sole owner). We touch
            // ONLY `hosts` here — never `wrappers` — because dropping this
            // entry's `Weak` inside its own finalizer would dangle rusty_v8's
            // `second_pass` reference. The dead `Weak` is swept elsewhere. If a
            // teardown drain already removed the host, this is a no-op.
            world_fin.borrow_mut().hosts.remove(&id);
            runs.set(runs.get() + 1);
        }),
    );
    let mut state = world.borrow_mut();
    state.hosts.insert(id, host);
    // Overwrites any dead tombstone `Weak` for a reused id. Safe: this runs in
    // `wrapper_for`, not in a finalizer, so dropping the old `Weak` is fine.
    state.wrappers.insert(id, weak);
    wrapper
}

/// True while the wrapper for `id` is still uncollected.
fn wrapper_is_live(world: &World, id: u64) -> bool {
    world
        .borrow()
        .wrappers
        .get(&id)
        .map(|weak| !weak.is_empty())
        .unwrap_or(false)
}

#[test]
fn a_repeated_request_for_a_live_id_returns_the_same_object_and_one_host() {
    init_v8_with_gc();

    let isolate = &mut v8::Isolate::new(Default::default());
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let mut context_scope = v8::ContextScope::new(scope, context);
    // Re-open as a context-carrying `PinScope` so `new_instance` and GC calls
    // see `HandleScope<Context>` (the repo idiom, mirroring `Engine::run`).
    v8::scope!(let scope, &mut context_scope);

    let drops = Rc::new(Cell::new(0u32));
    let finalizer_runs = Rc::new(Cell::new(0u32));
    let world: World = Rc::new(RefCell::new(WorldState::default()));
    let id = 3u64;

    {
        v8::scope!(let scope, scope);
        let first = wrapper_for(scope, &world, id, &drops, &finalizer_runs);
        let second = wrapper_for(scope, &world, id, &drops, &finalizer_runs);

        // Identity: the second request revives the same JS object, and no
        // second host was created — the entry was never overwritten.
        assert!(
            first.strict_equals(second.into()),
            "a live id yields the same wrapper object"
        );
        assert_eq!(world.borrow().hosts.len(), 1, "exactly one host for the id");
        assert_eq!(drops.get(), 0, "only one host, nothing reclaimed yet");
    }

    assert!(wrapper_is_live(&world, id), "still alive before GC");

    scope.request_garbage_collection_for_testing(v8::GarbageCollectionType::Full);

    assert_eq!(
        finalizer_runs.get(),
        1,
        "the single host's finalizer ran once"
    );
    assert_eq!(drops.get(), 1, "the single host was reclaimed exactly once");
    assert!(world.borrow().hosts.is_empty(), "the host was removed");
    // The finalizer leaves the dead `Weak` as a tombstone (it must not drop it):
    // the real world sweeps these outside any finalizer.
    assert!(
        !wrapper_is_live(&world, id),
        "the wrapper is a dead tombstone"
    );
}

#[test]
fn gc_of_an_unreachable_wrapper_runs_the_finalizer_and_drops_the_host() {
    init_v8_with_gc();

    let isolate = &mut v8::Isolate::new(Default::default());
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let mut context_scope = v8::ContextScope::new(scope, context);
    v8::scope!(let scope, &mut context_scope);

    let drops = Rc::new(Cell::new(0u32));
    let finalizer_runs = Rc::new(Cell::new(0u32));
    let world: World = Rc::new(RefCell::new(WorldState::default()));
    let id = 1u64;

    // Create the wrapper in an inner scope so no strong handle to it survives:
    // after this block the only reference V8 knows of is the map's weak one,
    // which does not keep the object alive.
    {
        v8::scope!(let scope, scope);
        let wrapper = wrapper_for(scope, &world, id, &drops, &finalizer_runs);

        // The fast path round-trips: the field yields the exact host we stored.
        let raw = unsafe { read_host_ptr(&wrapper) };
        let host_ref = unsafe { &*(raw as *const HostProbe) };
        assert_eq!(host_ref.id, id, "unwrap_this reaches the right host");
    }

    assert!(
        wrapper_is_live(&world, id),
        "the wrapper is alive before GC"
    );
    assert_eq!(drops.get(), 0, "nothing reclaimed before GC");

    scope.request_garbage_collection_for_testing(v8::GarbageCollectionType::Full);

    assert_eq!(finalizer_runs.get(), 1, "the finalizer ran during GC");
    assert_eq!(drops.get(), 1, "the host was reclaimed exactly once");
    assert!(world.borrow().hosts.is_empty(), "the host was removed");

    // Sweep the dead tombstone `Weak` here, outside any finalizer — the only
    // safe place to drop it. This drop runs while the isolate is still alive.
    world.borrow_mut().wrappers.clear();
}

#[test]
fn teardown_drains_live_wrappers_without_leaking_or_double_freeing() {
    init_v8_with_gc();

    let isolate = &mut v8::Isolate::new(Default::default());
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let mut context_scope = v8::ContextScope::new(scope, context);
    v8::scope!(let scope, &mut context_scope);

    let drops = Rc::new(Cell::new(0u32));
    let finalizer_runs = Rc::new(Cell::new(0u32));
    let world: World = Rc::new(RefCell::new(WorldState::default()));
    let id = 2u64;

    // A wrapper that stays reachable via a strong Global, exactly as a live DOM
    // node's wrapper would while script still holds it.
    let global = {
        v8::scope!(let scope, scope);
        let wrapper = wrapper_for(scope, &world, id, &drops, &finalizer_runs);
        v8::Global::new(scope.as_ref(), wrapper)
    };

    assert_eq!(world.borrow().hosts.len(), 1);
    assert_eq!(drops.get(), 0);

    // Teardown, before the isolate is disposed: reclaim every live host and drop
    // every `Weak`. Dropping the (still-armed) `Weak`s here — outside any
    // finalizer — is safe and disarms them, so no later collection can run a
    // finalizer against the drained host. After this the wrapper's internal
    // field dangles, but JS cannot run during teardown, so nothing unwraps it,
    // and V8 never dereferences the opaque field itself.
    {
        let mut state = world.borrow_mut();
        state.hosts.clear();
        state.wrappers.clear();
    }
    assert_eq!(drops.get(), 1, "the live host was reclaimed by the drain");

    // Release the wrapper and force collection. Because the drain dropped its
    // `Weak`, the finalizer is disarmed and cannot run — proving the drain is
    // the sole reclaimer at teardown and there is no double free.
    drop(global);
    scope.request_garbage_collection_for_testing(v8::GarbageCollectionType::Full);

    assert_eq!(
        finalizer_runs.get(),
        0,
        "the drain disarmed the finalizer; it must not run afterwards"
    );
    assert_eq!(drops.get(), 1, "no double free after the drain");
}
