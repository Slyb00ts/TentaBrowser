//! A temporary ephemeron graph for node wrappers, listeners, and Event.target.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use rquickjs::function::{Constructor, This};
use rquickjs::{Ctx, Function, Object, Persistent, Value};
use tenta_js::dom::events::cycles::Plan;

use super::World;
use tenta_js::bindings::host::{TAG_EVENT, cssom::graph as css_graph, decode};

pub(super) struct Intrinsics {
    constructor: Persistent<Constructor<'static>>,
    set: Persistent<Function<'static>>,
}

struct CallbackCandidate {
    bits: u64,
    weak: Persistent<Object<'static>>,
    restored: Option<Persistent<Value<'static>>>,
}

struct WrapperCandidate {
    bits: u64,
    generation: u64,
    weak: Persistent<Object<'static>>,
    restored: Option<Persistent<Object<'static>>>,
}

pub(super) struct Collection {
    plan: Plan,
    // This root must outlive GC and restoration, including failure. The values
    // are reachable only through ephemerons, never through a second strong list.
    _graph: Persistent<Object<'static>>,
    callbacks: Vec<CallbackCandidate>,
    wrappers: Vec<WrapperCandidate>,
}

// The interrupt handler exempts only this closed native transaction from the
// page deadline/watchdog. Its first condition still honors native failure.
struct CollectingGuard<'a>(&'a Cell<bool>);
impl Drop for CollectingGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl World {
    pub(super) fn initialize_cycles(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        let constructor: Constructor = ctx.globals().get("WeakMap")?;
        let prototype: Object = constructor.get("prototype")?;
        let set: Function = prototype.get("set")?;
        *self.cycle_intrinsics.borrow_mut() = Some(Intrinsics {
            constructor: Persistent::save(ctx, constructor),
            set: Persistent::save(ctx, set),
        });
        Ok(())
    }

    /// Collects only while all native conversion frames are closed. The caller
    /// already owns the QuickJS runtime lock, including native callback entry.
    pub(crate) fn collect_checkpoint(&self, ctx: &Ctx<'_>, sweep: bool) -> rquickjs::Result<()> {
        use tenta_js::bindings::{QuickJsWrapperWorld, quickjs_check_native_failure};

        quickjs_check_native_failure(ctx)?;
        if self.collecting.replace(true) {
            self.record_native_failure("recursive wrapper collection");
            return quickjs_check_native_failure(ctx);
        }
        let _collecting = CollectingGuard(&self.collecting);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !self.callback_roots.borrow().is_empty() {
                return Err(rquickjs::Error::Unknown);
            }
            // An embedder can hold either kind of SharedDocument borrow while
            // evaluating pure JS or requesting trim. Housekeeping may mutate
            // wrapper flags even without an arena sweep, so defer the entire
            // transaction until every document is exclusively available.
            // Do not reset admission: deferred materializations stay bounded.
            if self
                .host
                .documents
                .all()
                .iter()
                .any(|context| context.doc.try_borrow_mut().is_err())
            {
                return Ok(());
            }
            self.reclaim();
            quickjs_check_native_failure(ctx)?;
            loop {
                self.prepare_cycles(ctx)?;
                // Preparation has returned: only the transaction and external
                // roots survive. Calling Runtime::run_gc here would try to
                // acquire the runtime lock already held by this context.
                // SAFETY: the active context belongs to this thread's locked
                // runtime, and no native borrow or transient raw handle exists.
                unsafe {
                    rquickjs::qjs::JS_RunGC(rquickjs::qjs::JS_GetRuntime(ctx.as_raw().as_ptr()));
                }
                quickjs_check_native_failure(ctx)?;
                self.restore_cycles(ctx)?;
                let pins_released = self.reclaim();
                quickjs_check_native_failure(ctx)?;
                self.gc_epoch.set(self.gc_epoch.get().wrapping_add(1));
                // A repeat requires removal of an actual snapshot/Attr/Range
                // record, so the number of passes is bounded by the records
                // present at entry. No JS or host allocation runs in this loop.
                if !pins_released {
                    break;
                }
            }
            if sweep {
                for context in self.host.documents.all() {
                    context.doc.borrow_mut().sweep_detached();
                }
                self.reclaim();
                quickjs_check_native_failure(ctx)?;
            }
            self.new_wrapper_materializations.set(0);
            Ok(())
        }));
        match result {
            Ok(Ok(())) => Ok(()),
            failed => {
                // Preparation/recovery failure keeps the transaction owned and
                // forbids reuse of callbacks or wrappers by subsequent script.
                self.record_native_failure("listener cycle collection");
                if let Err(payload) = failed
                    && let Err(nested) =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)))
                {
                    std::mem::forget(nested);
                }
                quickjs_check_native_failure(ctx)
            }
        }
    }

    /// Returns after dropping every temporary callback, wrapper and group value.
    /// The caller must close this preparation scope before entering GC.
    pub(crate) fn prepare_cycles(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        if self.cycle.borrow().is_some() {
            return Err(rquickjs::Error::Unknown);
        }
        let (rooted_handles, plan_handles): (Vec<u64>, Vec<u64>) = {
            let cache = self.wrappers.borrow();
            let mut rooted = Vec::new();
            let mut plan = Vec::new();
            for (&bits, wrapper) in cache.iter() {
                if wrapper.root.is_some() {
                    rooted.push(bits);
                    plan.push(bits);
                } else if decode(bits).tag == TAG_EVENT {
                    // Event.target may expose a node group without an Event root.
                    plan.push(bits);
                }
            }
            (rooted, plan)
        };
        // A provisional root either joins a native group or is an edge-free
        // stale candidate. Never retain a stale CSS record indefinitely.
        let mut orphaned: HashSet<u64> = rooted_handles.iter().copied().collect();
        let plan = Plan::with_wrappers(&self.host, &plan_handles);
        let css = css_graph::snapshot_wrapper_graph(&self.host, &rooted_handles)
            .ok_or(rquickjs::Error::Unknown)?;
        let potentially_eligible: HashSet<u64> = {
            let values = self.values.borrow();
            plan.references
                .iter()
                .filter_map(|(&bits, &refs)| {
                    values
                        .get(&bits)
                        .filter(|slot| slot.refs == refs)
                        .map(|_| bits)
                })
                .collect()
        };
        for group in &plan.groups {
            for bits in &group.wrappers {
                orphaned.remove(bits);
            }
        }
        for group in &css.groups {
            for bits in &group.wrappers {
                orphaned.remove(bits);
            }
        }
        let wrapper_count: usize = orphaned.len()
            + plan
                .groups
                .iter()
                .filter(|group| !group.native_root)
                .map(|group| group.wrappers.len())
                .sum::<usize>()
            + css
                .groups
                .iter()
                .map(|group| group.wrappers.len())
                .sum::<usize>();
        if potentially_eligible.is_empty() && wrapper_count == 0 {
            return Ok(());
        }
        let (constructor, set) = {
            let intrinsics = self.cycle_intrinsics.borrow();
            let intrinsics = intrinsics.as_ref().ok_or(rquickjs::Error::Unknown)?;
            (
                intrinsics.constructor.clone().restore(ctx)?,
                intrinsics.set.clone().restore(ctx)?,
            )
        };
        let graph: Object = constructor.construct(())?;
        let weak_constructor = self
            .weak_constructor
            .borrow()
            .as_ref()
            .ok_or(rquickjs::Error::Unknown)?
            .clone()
            .restore(ctx)?;
        let mut callbacks = Vec::with_capacity(potentially_eligible.len());
        let mut eligible = HashSet::new();
        for &bits in &potentially_eligible {
            let value = self
                .values
                .borrow()
                .get(&bits)
                .and_then(|slot| slot.value.clone())
                .ok_or(rquickjs::Error::Unknown)?
                .restore(ctx)?;
            // Primitive CustomEvent.detail values cannot make JS object cycles.
            // Keep their ordinary value-table root until Event reclamation.
            if !value.is_object() {
                continue;
            }
            let weak: Object = weak_constructor.construct((value,))?;
            eligible.insert(bits);
            callbacks.push(CallbackCandidate {
                bits,
                weak: Persistent::save(ctx, weak),
                restored: None,
            });
        }
        let mut incoming_events: HashMap<usize, Vec<u64>> = HashMap::new();
        let mut outgoing_events = HashSet::new();
        for edge in &plan.event_target_edges {
            if !plan.groups[edge.target_group].native_root {
                incoming_events
                    .entry(edge.target_group)
                    .or_default()
                    .push(edge.event);
                outgoing_events.insert(edge.event);
            }
        }
        for edge in &plan.event_value_edges {
            if eligible.contains(&edge.value) {
                outgoing_events.insert(edge.event);
            }
        }
        let mut event_payloads = HashMap::with_capacity(outgoing_events.len());
        for event in outgoing_events {
            let payload = Object::new_proto(ctx.clone(), None)?;
            tenta_js::bindings::quickjs_check_native_failure(ctx)?;
            event_payloads.insert(event, payload);
        }
        let mut wrappers = Vec::with_capacity(wrapper_count);
        for (group_index, group) in plan.groups.iter().enumerate() {
            if group.native_root {
                continue;
            }
            // A null prototype prevents page-defined numeric setters from
            // executing while the collector constructs its private graph.
            let values = Object::new_proto(ctx.clone(), None)?;
            let mut index = 0;
            for &bits in &group.wrappers {
                let (generation, weak, root) = {
                    let cache = self.wrappers.borrow();
                    let wrapper = cache.get(&bits).ok_or(rquickjs::Error::Unknown)?;
                    (
                        wrapper.generation,
                        wrapper.weak.clone(),
                        wrapper.root.clone().ok_or(rquickjs::Error::Unknown)?,
                    )
                };
                let wrapper = root.restore(ctx)?;
                values.set(index.to_string(), wrapper.clone())?;
                index += 1;
                set.call::<_, Value>((This(graph.clone()), wrapper, values.clone()))?;
                wrappers.push(WrapperCandidate {
                    bits,
                    generation,
                    weak,
                    restored: None,
                });
            }
            for &bits in &group.callbacks {
                if !eligible.contains(&bits) {
                    continue;
                }
                let value = self
                    .values
                    .borrow()
                    .get(&bits)
                    .and_then(|slot| slot.value.clone())
                    .ok_or(rquickjs::Error::Unknown)?
                    .restore(ctx)?;
                values.set(index.to_string(), value)?;
                index += 1;
            }
            // Event.target is one outgoing edge. Publish one combined
            // Event-keyed payload only after related/detail edges are added.
            for event in incoming_events.remove(&group_index).unwrap_or_default() {
                let payload = event_payloads.get(&event).ok_or(rquickjs::Error::Unknown)?;
                payload.set("target", values.clone())?;
                tenta_js::bindings::quickjs_check_native_failure(ctx)?;
            }
        }
        for edge in &plan.event_value_edges {
            if !eligible.contains(&edge.value) {
                continue;
            }
            let value = self
                .values
                .borrow()
                .get(&edge.value)
                .and_then(|slot| slot.value.clone())
                .ok_or(rquickjs::Error::Unknown)?
                .restore(ctx)?;
            let payload = event_payloads
                .get(&edge.event)
                .ok_or(rquickjs::Error::Unknown)?;
            payload.set("owned", value)?;
            tenta_js::bindings::quickjs_check_native_failure(ctx)?;
        }
        // Each Event key gets exactly one directed ephemeron payload.
        for (bits, payload) in event_payloads {
            if let Some(event) = self.existing(ctx, bits)? {
                set.call::<_, Value>((This(graph.clone()), event, payload))?;
                tenta_js::bindings::quickjs_check_native_failure(ctx)?;
            }
        }
        for group in &css.groups {
            // One scoped value holds the group's existing wrappers and one
            // representative from each directed target group. The latter
            // activates that target's ephemeron without reversing the edge.
            let values = Object::new_proto(ctx.clone(), None)?;
            tenta_js::bindings::quickjs_check_native_failure(ctx)?;
            let mut index = 0;
            for &bits in &group.wrappers {
                let root = {
                    let cache = self.wrappers.borrow();
                    cache
                        .get(&bits)
                        .and_then(|wrapper| wrapper.root.clone())
                        .ok_or(rquickjs::Error::Unknown)?
                };
                let wrapper = root.restore(ctx)?;
                tenta_js::bindings::quickjs_check_native_failure(ctx)?;
                values.set(index.to_string(), wrapper)?;
                tenta_js::bindings::quickjs_check_native_failure(ctx)?;
                index += 1;
            }
            for &target in &group.targets {
                let bits = *css
                    .groups
                    .get(target)
                    .and_then(|group| group.wrappers.first())
                    .ok_or(rquickjs::Error::Unknown)?;
                let root = {
                    let cache = self.wrappers.borrow();
                    cache
                        .get(&bits)
                        .and_then(|wrapper| wrapper.root.clone())
                        .ok_or(rquickjs::Error::Unknown)?
                };
                let wrapper = root.restore(ctx)?;
                tenta_js::bindings::quickjs_check_native_failure(ctx)?;
                values.set(index.to_string(), wrapper)?;
                tenta_js::bindings::quickjs_check_native_failure(ctx)?;
                index += 1;
            }
            for &bits in &group.wrappers {
                let (generation, weak, root) = {
                    let cache = self.wrappers.borrow();
                    let wrapper = cache.get(&bits).ok_or(rquickjs::Error::Unknown)?;
                    (
                        wrapper.generation,
                        wrapper.weak.clone(),
                        wrapper.root.clone().ok_or(rquickjs::Error::Unknown)?,
                    )
                };
                let wrapper = root.restore(ctx)?;
                tenta_js::bindings::quickjs_check_native_failure(ctx)?;
                set.call::<_, Value>((This(graph.clone()), wrapper, values.clone()))?;
                tenta_js::bindings::quickjs_check_native_failure(ctx)?;
                wrappers.push(WrapperCandidate {
                    bits,
                    generation,
                    weak,
                    restored: None,
                });
            }
        }
        // A native slot can become stale independently of JS (for example an
        // embedder replacing the shared Document or reclaiming a CSS record).
        // It has no valid edges but cannot retain its provisional root forever.
        for bits in orphaned {
            let cache = self.wrappers.borrow();
            let wrapper = cache.get(&bits).ok_or(rquickjs::Error::Unknown)?;
            wrappers.push(WrapperCandidate {
                bits,
                generation: wrapper.generation,
                weak: wrapper.weak.clone(),
                restored: None,
            });
        }
        // Publish recovery ownership before weakening anything. All native
        // wrapper identities and callback slots remain reserved until restore.
        let wrapper_ids: Vec<_> = wrappers
            .iter()
            .map(|wrapper| (wrapper.bits, wrapper.generation))
            .collect();
        *self.cycle.borrow_mut() = Some(Collection {
            plan,
            _graph: Persistent::save(ctx, graph),
            callbacks,
            wrappers,
        });
        for bits in eligible {
            let root = self
                .values
                .borrow_mut()
                .get_mut(&bits)
                .ok_or(rquickjs::Error::Unknown)?
                .value
                .take();
            drop(root);
            tenta_js::bindings::quickjs_check_native_failure(ctx)?;
        }
        for (bits, generation) in wrapper_ids {
            let root = {
                let mut cache = self.wrappers.borrow_mut();
                let wrapper = cache.get_mut(&bits).ok_or(rquickjs::Error::Unknown)?;
                if wrapper.generation != generation {
                    return Err(rquickjs::Error::Unknown);
                }
                wrapper.root.take()
            };
            drop(root);
            tenta_js::bindings::quickjs_check_native_failure(ctx)?;
        }
        tenta_js::bindings::quickjs_check_native_failure(ctx)
    }

    pub(crate) fn restore_cycles(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        let mut cycle = self.cycle.borrow_mut();
        let Some(collection) = cycle.as_mut() else {
            return Ok(());
        };
        let deref = self
            .weak_deref
            .borrow()
            .as_ref()
            .ok_or(rquickjs::Error::Unknown)?
            .clone()
            .restore(ctx)?;
        let mut dead = HashSet::with_capacity(collection.callbacks.len());
        for candidate in &mut collection.callbacks {
            let weak = candidate.weak.clone().restore(ctx)?;
            let value: Value = deref.call((This(weak),))?;
            if value.is_undefined() {
                dead.insert(candidate.bits);
            } else {
                candidate.restored = Some(Persistent::save(ctx, value));
            }
        }
        for candidate in &mut collection.wrappers {
            let weak = candidate.weak.clone().restore(ctx)?;
            let value: Option<Object> = deref.call((This(weak),))?;
            if let Some(value) = value {
                candidate.restored = Some(Persistent::save(ctx, value));
            }
        }
        // Keep every restored survivor rooted in the transaction until all
        // dead registrations have been detached from their exact native owner.
        let released = collection.plan.drain_collected(&dead);
        for candidate in &mut collection.wrappers {
            if candidate.restored.is_some() {
                let mut cache = self.wrappers.borrow_mut();
                let wrapper = cache
                    .get_mut(&candidate.bits)
                    .ok_or(rquickjs::Error::Unknown)?;
                if wrapper.generation != candidate.generation || wrapper.root.is_some() {
                    return Err(rquickjs::Error::Unknown);
                }
                wrapper.root = candidate.restored.take();
            }
        }
        for candidate in &mut collection.callbacks {
            if let Some(value) = candidate.restored.take() {
                self.values
                    .borrow_mut()
                    .get_mut(&candidate.bits)
                    .ok_or(rquickjs::Error::Unknown)?
                    .value = Some(value);
            }
        }
        for bits in released {
            self.release(bits);
            tenta_js::bindings::quickjs_check_native_failure(ctx)?;
        }
        let complete = cycle.take();
        drop(cycle);
        drop(complete);
        tenta_js::bindings::quickjs_check_native_failure(ctx)
    }
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use tenta_js::bindings::host::SharedContext;

    fn new_cycle_engine() -> Engine {
        Engine::new().unwrap()
    }
    fn cycle_context(engine: &Engine) -> SharedContext {
        engine.state.world.host.clone()
    }
    fn retains_failed_cycle(engine: &Engine) -> bool {
        engine.state.world.cycle.borrow().is_some()
    }
    fn expected_cycle_boundary() -> &'static str {
        "listener cycle collection"
    }

    #[test]
    fn an_external_mutable_document_borrow_defers_housekeeping() {
        let mut engine = new_cycle_engine();
        engine
            .evaluate("globalThis.kept=document.createElement('div');kept.note='live'")
            .unwrap();
        let context = cycle_context(&engine);
        let before = engine.state.world.gc_epoch.get();
        let held = context.doc.borrow_mut();
        assert_eq!(engine.evaluate("42").unwrap(), "42");
        engine.trim_memory();
        assert_eq!(engine.native_failure(), None);
        assert_eq!(engine.state.world.gc_epoch.get(), before);
        drop(held);
        engine.trim_memory();
        assert!(engine.state.world.gc_epoch.get() > before);
        assert_eq!(engine.evaluate("kept.note").unwrap(), "live");
    }

    #[test]
    fn an_external_immutable_document_borrow_defers_wrapper_reclamation() {
        let mut engine = new_cycle_engine();
        engine
            .evaluate("globalThis.kept=document.createElement('div');kept.note='live'")
            .unwrap();
        let context = cycle_context(&engine);
        let before = engine.state.world.gc_epoch.get();
        let held = context.doc.borrow();
        engine.trim_memory();
        assert_eq!(engine.evaluate("kept=null;'released'").unwrap(), "released");
        assert_eq!(engine.native_failure(), None);
        assert_eq!(engine.state.world.gc_epoch.get(), before);
        assert_eq!(held.len(), 2);
        drop(held);
        engine.trim_memory();
        assert_eq!(context.doc.borrow_mut().sweep_detached().nodes_freed, 1);
        assert_eq!(engine.native_failure(), None);
    }

    #[test]
    fn deferred_document_borrows_do_not_reset_wrapper_admission() {
        let mut engine = new_cycle_engine();
        assert_eq!(
            engine
                .evaluate("globalThis.root=document.createElement('div');root.innerHTML='<b></b>'.repeat(5000);root.childNodes.length")
                .unwrap(),
            "5000"
        );
        let context = cycle_context(&engine);
        // Defer the world's checkpoint through another document. The source
        // document remains writable so publishing its first-time wrappers can
        // update has_wrapper before admission reaches the pressure threshold.
        let other = context
            .new_child(std::rc::Rc::new(std::cell::RefCell::new(
                tenta_dom::Document::new(),
            )))
            .unwrap();
        let node_count = context.doc.borrow().len();
        let before = engine.state.world.gc_epoch.get();
        let held = other.doc.borrow();
        let observed = engine
            .evaluate(
                "let visited=0,refused=false;try{for(let node=root.firstChild;node;node=node.nextSibling)visited++;}\
                 catch(error){if(error.name!=='RangeError')throw error;refused=true;}[visited,refused].join('|')",
            )
            .unwrap();
        let (visited, refused) = observed.split_once('|').unwrap();
        assert!(visited.parse::<usize>().unwrap() < 5000);
        assert_eq!(refused, "true");
        assert_eq!(engine.native_failure(), None);
        assert_eq!(engine.state.world.gc_epoch.get(), before);
        assert_eq!(held.len(), 1);
        assert_eq!(context.doc.borrow().len(), node_count);
        drop(held);
        engine.trim_memory();
        assert_eq!(engine.evaluate("root.lastChild.nodeName").unwrap(), "B");
        assert_eq!(engine.native_failure(), None);
    }

    include!("../../../tenta-js/tests/support/collector_failure.rs");
}
