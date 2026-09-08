//! The V8 wrapper world: the per-context map between DOM host objects and their
//! JavaScript wrappers (`docs/spec/05-javascript.md` §3).
//!
//! This is the first real [`WrapperWorld`] behind the generated glue. Its
//! lifecycle mirrors the critic-cleared spike in `tests/wrapper_gc.rs`, whose
//! rules are load-bearing:
//!
//!  - The host `Box`es are owned by [`WorldState::hosts`], keyed by handle
//!    bits; the wrapper's internal field only borrows the pointer. A finalizer
//!    reclaims a host by removing it from that map — it never reads the
//!    collected object and never drops a `v8::Weak`.
//!  - Wrapper identity lives in [`WorldState::wrappers`]. A live wrapper is
//!    revived with `Weak::to_local`; a dead `Weak` is a tombstone swept only
//!    outside any finalizer (overwritten on reuse, or dropped at teardown).
//!  - Teardown drains both maps while the isolate is still alive, which
//!    reclaims every host exactly once and disarms every finalizer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;

use tenta_core::NodeId;
use tenta_dom::NodeKind;
use tenta_js::bindings::host::{
    HostObject, SharedContext, TAG_HTML_COLLECTION, TAG_NODE_LIST, decode,
};
use tenta_js::bindings::{
    InterfaceDescriptor, InterfaceRegistry, WRAPPER_HOST_FIELD, WRAPPER_TAG, WrapperWorld,
    cached_template, unwrap_this,
};

/// The split state of one wrapper world.
///
/// `hosts` is the sole owner of the host allocations and thus the single
/// reclaim point; `wrappers` holds weak handles for identity; `values` is a
/// slab that backs `any`/`object`/`symbol`/`Promise`/callback bits.
#[derive(Default)]
pub struct WorldState {
    hosts: HashMap<u64, Box<HostObject>>,
    wrappers: HashMap<u64, v8::Weak<v8::Object>>,
    values: Vec<v8::Global<v8::Value>>,
    /// Nodes whose wrapper a finalizer has just collected, awaiting a
    /// `set_has_wrapper(false)` on the document. A finalizer must not borrow
    /// the document (it runs during GC, where a running op may hold a document
    /// borrow), so it only records the id here; the ids are drained at a safe
    /// point outside any finalizer (see `V8World::wrapper` and the engine's
    /// teardown).
    dead: Vec<NodeId>,
    /// Snapshot wrapper handles (tagged with TAG_STATIC_NODE_LIST or TAG_STATIC_HTML_COLLECTION)
    /// that a finalizer has collected, awaiting cleanup of their slab entries.
    dead_snapshots: Vec<u64>,
}

impl WorldState {
    /// Reclaims every host and disarms every finalizer, and releases every
    /// stored value. Call this while the isolate is still alive: dropping the
    /// still-armed `Weak`s here — outside any finalizer — is the only safe
    /// place, and it makes the drain the sole reclaimer at teardown.
    pub fn clear(&mut self) {
        // Drop the host boxes first (the reclaim point), then the weak handles
        // (which disarms their finalizers), then the value slab.
        self.hosts.clear();
        self.wrappers.clear();
        self.values.clear();
        self.dead.clear();
        self.dead_snapshots.clear();
    }

    /// Takes the ids of wrappers collected since the last call, so the caller
    /// can apply `set_has_wrapper(false)` where no document borrow is held.
    ///
    /// A node and its child `NodeList` are distinct wrapper handles that anchor
    /// to the same node id, so `HAS_WRAPPER` is a single bit shared by both. An
    /// id is surfaced only once no live host still anchors to it, so collecting
    /// one of a node's wrappers never clears the bit while the other is alive.
    pub fn take_dead(&mut self) -> Vec<NodeId> {
        let dead = std::mem::take(&mut self.dead);
        dead.into_iter()
            .filter(|id| {
                !self.hosts.keys().any(|bits| {
                    decode(*bits)
                        .node_anchor()
                        .map(|nid| nid == *id)
                        .unwrap_or(false)
                })
            })
            .collect()
    }

    /// Every node id that still carries a `HAS_WRAPPER` bit: the ids of
    /// wrappers a finalizer has already collected (`dead`) plus the anchor of
    /// every live host. Teardown clears the bit for all of these before
    /// dropping the hosts, so a document that outlives the engine does not keep
    /// nodes pinned. A wrapper still live at teardown is dropped without ever
    /// running its finalizer, so its bit would otherwise never be cleared. Ids
    /// may repeat (a node and its `NodeList`, or a dead id still anchored);
    /// `set_has_wrapper(false)` is idempotent, so the caller need not dedup.
    pub fn wrapped_node_ids(&self) -> Vec<NodeId> {
        self.dead
            .iter()
            .copied()
            .chain(
                self.hosts
                    .keys()
                    .filter_map(|bits| decode(*bits).node_anchor()),
            )
            .collect()
    }

    /// Takes the snapshot handle bits of wrappers collected since the last call,
    /// so the caller can reclaim snapshot slab entries. The slab is part of the
    /// context and outlives the engine; this drains dead snapshot tombstones and
    /// the stale `Weak` references.
    pub fn take_dead_snapshots(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.dead_snapshots)
    }
}

/// The V8 wrapper world. Holds shared handles to the world state and the context
/// (document + snapshots); the owning engine keeps its own clones so it can drain
/// the state at teardown after this world has been moved into the isolate slot.
pub struct V8World {
    state: Rc<RefCell<WorldState>>,
    ctx: SharedContext,
}

impl V8World {
    /// Builds a world over `state` and `ctx`.
    pub fn new(state: Rc<RefCell<WorldState>>, ctx: SharedContext) -> Self {
        Self { state, ctx }
    }
}

impl WrapperWorld for V8World {
    fn wrapper<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        interface: &'static str,
        handle: u64,
    ) -> Option<v8::Local<'s, v8::Object>> {
        // Reflect wrappers collected since the last call into the document now,
        // where no document borrow is held. The finalizer only records ids (it
        // must not borrow the document during GC), so this is the safe point to
        // apply `set_has_wrapper(false)`.
        //
        // INVARIANT: nothing on this method's path may call back into
        // `handle_to_v8` (which removes the wrapper-world slot around this
        // call). Wrapper creation only instantiates a template and attaches a
        // host — it never runs script or a getter — so re-entry cannot happen.
        let mut state_mut = self.state.borrow_mut();
        let dead = state_mut.take_dead();
        if !dead.is_empty() {
            let mut doc = self.ctx.doc.borrow_mut();
            for id in dead {
                doc.set_has_wrapper(id, false);
            }
        }

        // Clean up dead snapshot wrappers: remove their slab entries and disarm tombstone Weaks.
        let dead_snapshot_bits = state_mut.take_dead_snapshots();
        for bits in dead_snapshot_bits {
            let snapshot_key = decode(bits).snapshot_key();
            self.ctx.snapshots.borrow_mut().remove(snapshot_key);
            // Also remove the tombstone Weak so it doesn't accumulate.
            state_mut.wrappers.remove(&bits);
        }
        drop(state_mut);

        // Fast path: a live wrapper for this handle is revived and reused, so a
        // host is created at most once per live id. The borrow is released with
        // the block; only the revived `Local` escapes.
        if let Some(existing) = {
            let state = self.state.borrow();
            state
                .wrappers
                .get(&handle)
                .and_then(|weak| weak.to_local(scope))
        } {
            return Some(existing);
        }

        // Decode the tagged, generational handle.
        let decoded = decode(handle);

        // Pick the interface name and host. For snapshots, validate the slab entry
        // still exists. For nodes, validate the handle isn't stale.
        let _ = interface;
        let interface_name: &str;
        let host: Box<HostObject>;
        let is_snapshot: bool;

        if decoded.tag == 3 {
            // TAG_STATIC_NODE_LIST
            interface_name = "NodeList";
            let key = decoded.snapshot_key();
            let _ = self.ctx.snapshots.borrow().get(key)?; // Snapshot was reclaimed or never existed
            host = Box::new(HostObject::static_node_list(self.ctx.clone(), key));
            is_snapshot = true;
        } else if decoded.tag == 4 {
            // TAG_STATIC_HTML_COLLECTION
            interface_name = "HTMLCollection";
            let key = decoded.snapshot_key();
            let _ = self.ctx.snapshots.borrow().get(key)?;
            host = Box::new(HostObject::static_html_collection(self.ctx.clone(), key));
            is_snapshot = true;
        } else {
            // Live node handle
            is_snapshot = false;
            let node_id = decoded.node_id;
            let node = {
                let doc = self.ctx.doc.borrow();
                if !doc.is_live(node_id)
                    || doc.node_ref(node_id).generation as u16 != decoded.generation
                {
                    return None;
                }
                doc.node_ref(node_id)
            };

            interface_name = if decoded.tag == TAG_NODE_LIST {
                "NodeList"
            } else if decoded.tag == TAG_HTML_COLLECTION {
                "HTMLCollection"
            } else {
                match self.ctx.doc.borrow().kind(node_id) {
                    NodeKind::Element => "Element",
                    NodeKind::Document => "Document",
                    NodeKind::Text => "Text",
                    NodeKind::Comment => "Comment",
                    _ => "Node",
                }
            };

            host = Box::new(if decoded.tag == TAG_NODE_LIST {
                HostObject::node_list(self.ctx.clone(), node)
            } else if decoded.tag == TAG_HTML_COLLECTION {
                HostObject::html_collection(self.ctx.clone(), node)
            } else {
                HostObject::node(self.ctx.clone(), node)
            });
        }

        // The template must already be installed for this interface.
        let template = cached_template(scope, interface_name)?;
        let instance = template.instance_template(scope).new_instance(scope)?;

        // Box the host so its address is stable across map growth: only the
        // pointer moves, never the pointee, so the internal field stays valid.
        let raw = host.as_ref() as *const HostObject as *const c_void;
        instance.set_aligned_pointer_in_internal_field(WRAPPER_HOST_FIELD, raw, WRAPPER_TAG);

        // For live node hosts, mark the node as having a wrapper. Snapshot hosts
        // don't touch the document.
        if !is_snapshot {
            let node_id = decoded.node_id;
            self.ctx.doc.borrow_mut().set_has_wrapper(node_id, true);
        }

        let state = self.state.clone();
        let weak = v8::Weak::with_finalizer(
            scope,
            instance,
            Box::new(move |_isolate| {
                // Eager reclamation: drop the host box (the sole owner) and
                // record the id for a later `set_has_wrapper(false)` or snapshot cleanup.
                // Touch ONLY `state` here — never the document (a running op may hold
                // a document borrow while this GC callback fires) and never a
                // `Weak` (dropping this entry's `Weak` inside its own finalizer
                // would dangle rusty_v8's `second_pass` reference). The dead
                // `Weak` is swept elsewhere; if a teardown drain already removed
                // the host, this is a no-op.
                let mut state = state.borrow_mut();
                if state.hosts.remove(&handle).is_some() {
                    if is_snapshot {
                        state.dead_snapshots.push(handle);
                    } else {
                        state.dead.push(decoded.node_id);
                    }
                }
            }),
        );

        let mut state = self.state.borrow_mut();
        state.hosts.insert(handle, host);
        // Overwrites any dead tombstone `Weak` for a reused id. Safe: this runs
        // in `wrapper`, not in a finalizer, so dropping the old `Weak` is fine.
        state.wrappers.insert(handle, weak);
        Some(instance)
    }

    fn handle(&self, _interface: &'static str, wrapper: v8::Local<v8::Object>) -> Option<u64> {
        // SAFETY: only wrappers created by `wrapper` carry a host pointer in
        // internal field 0; `unwrap_this` rejects any object without one.
        let host = unsafe { unwrap_this::<HostObject>(&wrapper) }.ok()?;
        Some(host.handle_bits())
    }

    fn store_value(&self, scope: &mut v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> u64 {
        let global = v8::Global::new(scope.as_ref(), value);
        let mut state = self.state.borrow_mut();
        let bits = state.values.len() as u64;
        state.values.push(global);
        bits
    }

    fn load_value<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        bits: u64,
    ) -> Option<v8::Local<'s, v8::Value>> {
        let state = self.state.borrow();
        let global = state.values.get(bits as usize)?;
        Some(v8::Local::new(scope, global))
    }
}

/// A minimal [`InterfaceRegistry`] for the Phase 0c slice.
///
/// `install_*` records each interface's descriptor here; the slice does not yet
/// consume them (lazy per-context prototype installation lands with binding
/// phase 1), so this just retains them.
#[derive(Default)]
pub struct SkeletonRegistry {
    descriptors: Vec<InterfaceDescriptor>,
}

impl InterfaceRegistry for SkeletonRegistry {
    fn register(&mut self, descriptor: InterfaceDescriptor) {
        self.descriptors.push(descriptor);
    }
}
