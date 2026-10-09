//! Temporary ephemeron ownership for node, Event and CSS wrapper graphs.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use tenta_js::bindings::host::SharedContext;
use tenta_js::dom::events::cycles::Plan;

use super::{HARD_WRAPPER_MATERIALIZATIONS, SOFT_WRAPPER_MATERIALIZATIONS, WorldState};
use crate::termination::TerminationGate;

/// Tracks the complete slow-callback frame, including outgoing conversion.
/// Only its outermost entry may collect; nested script getters run while an
/// outer native host operation or raw result may still own document borrows.
pub(crate) struct Admission {
    state: Rc<RefCell<WorldState>>,
    context: SharedContext,
    termination: std::sync::Arc<TerminationGate>,
    depth: Cell<usize>,
    stored_value_lease: RefCell<Vec<StoredValueLease>>,
    native_work_termination_pending: Cell<bool>,
    #[cfg(test)]
    buffer_poll_fault: Cell<Option<(usize, BufferPollFault)>>,
    #[cfg(test)]
    buffer_poll_count: Cell<usize>,
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum BufferPollFault {
    Watchdog,
    PageMemory,
}

struct StoredValueLease {
    depth: usize,
    bits: Vec<u64>,
    transferred: bool,
    buffer_lease_id: Option<u64>,
}

impl Admission {
    pub(crate) fn new(
        state: Rc<RefCell<WorldState>>,
        context: SharedContext,
        termination: std::sync::Arc<TerminationGate>,
    ) -> Self {
        Self {
            state,
            context,
            termination,
            depth: Cell::new(0),
            stored_value_lease: RefCell::new(Vec::new()),
            native_work_termination_pending: Cell::new(false),
            #[cfg(test)]
            buffer_poll_fault: Cell::new(None),
            #[cfg(test)]
            buffer_poll_count: Cell::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn fail_on_buffer_poll(&self, number: usize, fault: BufferPollFault) {
        assert!(number > 0);
        self.buffer_poll_count.set(0);
        self.buffer_poll_fault.set(Some((number, fault)));
    }

    #[cfg(test)]
    pub(crate) fn buffer_poll_count(&self) -> usize {
        self.buffer_poll_count.get()
    }
}

impl tenta_js::bindings::V8CallbackAdmission for Admission {
    fn enter(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
    ) -> Result<(), tenta_js::bindings::V8AdmissionError> {
        if self.depth.get() == 0 {
            if self.termination.requested() {
                scope.terminate_execution();
                return Err(tenta_js::bindings::V8AdmissionError::Terminated);
            }
            let pressure_checkpoint =
                self.state.borrow().new_wrapper_materializations >= SOFT_WRAPPER_MATERIALIZATIONS;
            if pressure_checkpoint {
                match checkpoint(scope, &self.state, &self.context, &self.termination, true) {
                    Ok(true) => {}
                    Ok(false) => {
                        scope.terminate_execution();
                        return Err(tenta_js::bindings::V8AdmissionError::Terminated);
                    }
                    Err(()) => {
                        tenta_js::bindings::poison_native_callback(
                            scope,
                            "wrapper pressure collection",
                        );
                        return Err(tenta_js::bindings::V8AdmissionError::Terminated);
                    }
                }
            }
            // A watchdog request during closed recovery is delivered by the
            // collection lease. Without a checkpoint, request() delivers V8
            // termination directly; the entry poll above is sufficient.
            if pressure_checkpoint && self.termination.requested() {
                scope.terminate_execution();
                return Err(tenta_js::bindings::V8AdmissionError::Terminated);
            }
        }
        if self.state.borrow().new_wrapper_materializations >= HARD_WRAPPER_MATERIALIZATIONS {
            return Err(tenta_js::bindings::V8AdmissionError::Throw(
                tenta_js::bindings::JsError::range_error(
                    "native wrapper allocation limit exceeded",
                ),
            ));
        }
        self.depth.set(self.depth.get() + 1);
        Ok(())
    }

    fn before_constructor_host(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
    ) -> Result<(), tenta_js::bindings::V8AdmissionError> {
        if self.termination.requested()
            || tenta_js::bindings::native_callback_failure(scope).is_some()
        {
            scope.terminate_execution();
            return Err(tenta_js::bindings::V8AdmissionError::Terminated);
        }
        if self.state.borrow().new_wrapper_materializations >= HARD_WRAPPER_MATERIALIZATIONS {
            return Err(tenta_js::bindings::V8AdmissionError::Throw(
                tenta_js::bindings::JsError::range_error(
                    "native wrapper allocation limit exceeded",
                ),
            ));
        }
        Ok(())
    }

    fn begin_stored_value_lease(&self) {
        self.stored_value_lease.borrow_mut().push(StoredValueLease {
            depth: self.depth.get(),
            bits: Vec::new(),
            transferred: false,
            buffer_lease_id: None,
        });
    }

    fn begin_checked_stored_value_lease(&self, lease_id: u64) -> bool {
        let depth = self.depth.get();
        if depth == 0
            || self
                .stored_value_lease
                .borrow()
                .last()
                .is_some_and(|lease| lease.depth == depth)
        {
            return false;
        }
        self.begin_stored_value_lease();
        if let Some(frame) = self.stored_value_lease.borrow_mut().last_mut() {
            frame.buffer_lease_id = Some(lease_id);
        }
        true
    }

    fn current_buffer_reference_lease(&self) -> Option<u64> {
        let depth = self.depth.get();
        self.stored_value_lease
            .borrow()
            .last()
            .filter(|frame| frame.depth == depth)
            .and_then(|frame| frame.buffer_lease_id)
    }

    fn buffer_reference_lease_active(&self, lease_id: u64) -> bool {
        let depth = self.depth.get();
        self.stored_value_lease
            .borrow()
            .iter()
            .any(|frame| frame.depth <= depth && frame.buffer_lease_id == Some(lease_id))
    }

    fn poll_native_buffer_work_termination(&self) -> bool {
        #[cfg(test)]
        {
            let number = self.buffer_poll_count.get() + 1;
            self.buffer_poll_count.set(number);
            if let Some((selected, fault)) = self.buffer_poll_fault.get()
                && number == selected
            {
                self.buffer_poll_fault.set(None);
                match fault {
                    BufferPollFault::Watchdog => self.termination.request(),
                    BufferPollFault::PageMemory => {
                        let mut reservation = self
                            .context
                            .native_memory
                            .begin(tenta_core::NativeMemoryComponent::StructuredClone);
                        assert!(!reservation.charge(usize::MAX));
                    }
                }
            }
        }
        let requested =
            self.termination.requested() || self.context.native_memory.first_failure().is_some();
        if requested {
            self.native_work_termination_pending.set(true);
        }
        requested
    }

    fn take_native_buffer_work_termination(&self) -> bool {
        let pending = self.native_work_termination_pending.get();
        if self.depth.get() == 0 {
            self.native_work_termination_pending.set(false);
        }
        pending
    }

    fn track_stored_value(&self, bits: u64) {
        let depth = self.depth.get();
        if let Some(frame) = self.stored_value_lease.borrow_mut().last_mut()
            && frame.depth == depth
        {
            frame.bits.push(bits);
        }
    }

    fn transfer_stored_value_lease(&self) {
        let depth = self.depth.get();
        if let Some(frame) = self.stored_value_lease.borrow_mut().last_mut()
            && frame.depth == depth
        {
            frame.transferred = true;
        }
    }

    fn end_stored_value_lease(&self, isolate: &v8::Isolate) {
        let depth = self.depth.get();
        let frame = {
            let mut frames = self.stored_value_lease.borrow_mut();
            if frames.last().is_some_and(|frame| frame.depth == depth) {
                frames.pop()
            } else {
                None
            }
        };
        let Some(frame) = frame else { return };
        if frame.transferred {
            return;
        }
        for bits in frame.bits {
            if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                // The still-live WorldState value slots retain the remaining
                // references until the poisoned engine's teardown.
                return;
            }
            let root = self.state.borrow_mut().take_released_value(bits);
            drop(root);
            if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                return;
            }
        }
    }

    fn exit(&self) {
        let depth = self.depth.get();
        // A Rust panic skipped the generated wrapper's normal end call. The
        // callback boundary makes that failure sticky; keep any converted JS
        // roots until world teardown rather than finalizing through poison.
        if let Ok(mut frames) = self.stored_value_lease.try_borrow_mut() {
            while frames.last().is_some_and(|frame| frame.depth == depth) {
                // The WorldState value slots remain rooted until teardown;
                // dropping this metadata during unwind cannot finalize them.
                frames.pop();
            }
        }
        self.depth.set(depth - 1);
    }
}

pub(super) struct Intrinsics {
    constructor: Rc<v8::Global<v8::Function>>,
    set: Rc<v8::Global<v8::Function>>,
}

struct Candidate {
    bits: u64,
    weak: v8::Weak<v8::Object>,
    restored: Option<v8::Global<v8::Value>>,
}

struct WrapperCandidate {
    bits: u64,
    generation: u64,
    weak: Rc<v8::Weak<v8::Object>>,
    restored: Option<v8::Global<v8::Object>>,
}

pub(super) struct Collection {
    plan: Plan,
    // Retain even on recovery failure. Poisoned engines cannot execute script
    // or reuse these reserved slots, and teardown releases the graph safely.
    _graph: v8::Global<v8::Object>,
    candidates: Vec<Candidate>,
    wrappers: Vec<WrapperCandidate>,
}

fn healthy(isolate: &v8::Isolate) -> Option<()> {
    tenta_js::bindings::native_callback_failure(isolate)
        .is_none()
        .then_some(())
}

/// A failed restoration keeps the graph and all recovered roots in WorldState
/// until isolate teardown. No recovery path may accidentally drop ownership.
struct Recovery<'a> {
    state: &'a RefCell<WorldState>,
    collection: Option<Collection>,
}

impl Drop for Recovery<'_> {
    fn drop(&mut self) {
        if let Some(collection) = self.collection.take() {
            self.state.borrow_mut().cycle = Some(collection);
        }
    }
}

/// Captures native intrinsics before any page code can replace them.
pub(crate) fn initialize(scope: &mut v8::PinScope<'_, '_>, state: &mut WorldState) -> Option<()> {
    let global = scope.get_current_context().global(scope);
    let key = v8::String::new(scope, "WeakMap")?;
    let constructor = v8::Local::<v8::Function>::try_from(global.get(scope, key.into())?).ok()?;
    let key = v8::String::new(scope, "prototype")?;
    let prototype = v8::Local::<v8::Object>::try_from(constructor.get(scope, key.into())?).ok()?;
    let key = v8::String::new(scope, "set")?;
    let set = v8::Local::<v8::Function>::try_from(prototype.get(scope, key.into())?).ok()?;
    state.cycle_intrinsics = Some(Intrinsics {
        constructor: Rc::new(v8::Global::new(scope, constructor)),
        set: Rc::new(v8::Global::new(scope, set)),
    });
    Some(())
}

/// Prepares all graph/recovery storage before dropping any strong slab root.
/// The caller must close this handle scope before entering GC.
pub(crate) fn prepare(
    scope: &mut v8::PinScope<'_, '_>,
    state: &Rc<RefCell<WorldState>>,
    context: &SharedContext,
) -> Option<()> {
    if state.borrow().cycle.is_some() {
        return None;
    }
    // A dead wrapper's Weak tombstone stays in the identity map until reuse or
    // teardown. Only provisional strong roots can be transferred to the
    // graph; including tombstones would make a later checkpoint fail recovery.
    let handles: Vec<u64> = state
        .borrow()
        .wrappers
        .iter()
        .filter_map(|(&bits, entry)| entry.root.as_ref().map(|_| bits))
        .collect();
    // Event wrappers have no provisional native root, but an existing Event
    // can expose its target's native node subtree. The shared plan validates
    // each generational Event record without materializing a new wrapper.
    let mut plan_handles = handles.clone();
    plan_handles.extend(state.borrow().wrappers.iter().filter_map(|(&bits, entry)| {
        (entry.root.is_none()
            && tenta_js::bindings::host::decode(bits).tag == tenta_js::bindings::host::TAG_EVENT)
            .then_some(bits)
    }));
    let mut orphaned: HashSet<u64> = handles
        .iter()
        .copied()
        .filter(|bits| {
            tenta_js::bindings::host::decode(*bits)
                .node_anchor()
                .is_some()
        })
        .collect();
    let plan = Plan::with_wrappers(context, &plan_handles);
    let css_plan =
        tenta_js::bindings::host::cssom::graph::snapshot_wrapper_graph(context, &handles)?;
    // CSS records and standalone targets have provisional host roots. Each
    // ownership plan removes its members; stale host keys remain candidates.
    let mut orphaned_hosts: HashSet<u64> = handles
        .iter()
        .copied()
        .filter(|bits| {
            tenta_js::bindings::host::decode(*bits).tag == tenta_js::bindings::host::TAG_HOST
        })
        .collect();
    let potentially_eligible: HashSet<u64> = {
        let state = state.borrow();
        plan.references
            .iter()
            .filter_map(|(&bits, &refs)| {
                state
                    .values
                    .get(bits as usize)
                    .and_then(Option::as_ref)
                    .filter(|slot| slot.refs as usize == refs)
                    .map(|_| bits)
            })
            .collect()
    };
    for group in &plan.groups {
        for bits in &group.wrappers {
            orphaned.remove(bits);
            orphaned_hosts.remove(bits);
        }
    }
    let wrapper_count = orphaned.len()
        + plan
            .groups
            .iter()
            .filter(|group| !group.native_root)
            .map(|group| group.wrappers.len())
            .sum::<usize>()
        + orphaned_hosts.len();
    if potentially_eligible.is_empty() && wrapper_count == 0 {
        return Some(());
    }
    let (constructor_root, set_root) = {
        let state = state.borrow();
        let intrinsics = state.cycle_intrinsics.as_ref()?;
        (
            Rc::clone(&intrinsics.constructor),
            Rc::clone(&intrinsics.set),
        )
    };
    let constructor = v8::Local::new(scope, constructor_root.as_ref());
    let set = v8::Local::new(scope, set_root.as_ref());
    let graph = constructor.new_instance(scope, &[])?;
    healthy(scope)?;
    let mut candidates = Vec::with_capacity(potentially_eligible.len());
    let mut eligible = HashSet::new();
    for &bits in &potentially_eligible {
        let root = {
            let state = state.borrow();
            Rc::clone(state.values.get(bits as usize)?.as_ref()?.global.as_ref()?)
        };
        let value = v8::Local::new(scope, root.as_ref());
        // CustomEvent.detail may be a primitive. It cannot form a JS object
        // cycle, so leave that value strongly stored until Event reclamation.
        let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
            continue;
        };
        healthy(scope)?;
        eligible.insert(bits);
        candidates.push(Candidate {
            bits,
            weak: v8::Weak::new(scope, object),
            restored: None,
        });
    }
    let mut wrappers = Vec::with_capacity(wrapper_count);
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
    // Build one payload per Event: a second WeakMap.set with the same key
    // would overwrite either its dispatch target or its owned value edge.
    let mut event_payloads = HashMap::with_capacity(outgoing_events.len());
    for bits in outgoing_events {
        let payload = v8::Object::new(scope);
        let null = v8::null(scope);
        if payload.set_prototype(scope, null.into()) != Some(true) {
            return None;
        }
        healthy(scope)?;
        event_payloads.insert(bits, payload);
    }
    for (group_index, group) in plan.groups.iter().enumerate() {
        if group.native_root {
            continue;
        }
        let values = v8::Object::new(scope);
        let mut index = 0;
        for &bits in &group.wrappers {
            let (generation, weak, has_root) = {
                let state = state.borrow();
                let entry = state.wrappers.get(&bits)?;
                (
                    entry.generation,
                    Rc::clone(&entry.weak),
                    entry.root.is_some(),
                )
            };
            if !has_root {
                return None;
            }
            let wrapper = weak.to_local(scope)?;
            let key = v8::String::new(scope, &index.to_string())?;
            if values.create_data_property(scope, key.into(), wrapper.into()) != Some(true) {
                return None;
            }
            healthy(scope)?;
            set.call(scope, graph.into(), &[wrapper.into(), values.into()])?;
            healthy(scope)?;
            wrappers.push(WrapperCandidate {
                bits,
                generation,
                weak,
                restored: None,
            });
            index += 1;
        }
        for &bits in &group.callbacks {
            if !eligible.contains(&bits) {
                continue;
            }
            let root = {
                let state = state.borrow();
                Rc::clone(state.values.get(bits as usize)?.as_ref()?.global.as_ref()?)
            };
            let value = v8::Local::new(scope, root.as_ref());
            let key = v8::String::new(scope, &index.to_string())?;
            // DefineOwnProperty bypasses inherited numeric setters, including
            // ones installed on Object.prototype by the page.
            if values.create_data_property(scope, key.into(), value) != Some(true) {
                return None;
            }
            healthy(scope)?;
            index += 1;
        }
        // Event.target is one outgoing edge. Keep it in the Event's private
        // payload until all other Event-owned edges are ready to publish.
        for bits in incoming_events.remove(&group_index).unwrap_or_default() {
            let payload = *event_payloads.get(&bits)?;
            let key = v8::String::new(scope, "target")?;
            if payload.create_data_property(scope, key.into(), values.into()) != Some(true) {
                return None;
            }
            healthy(scope)?;
        }
    }
    for edge in &plan.event_value_edges {
        if !eligible.contains(&edge.value) {
            continue;
        }
        let root = {
            let world = state.borrow();
            Rc::clone(
                world
                    .values
                    .get(edge.value as usize)?
                    .as_ref()?
                    .global
                    .as_ref()?,
            )
        };
        let value = v8::Local::new(scope, root.as_ref());
        let payload = *event_payloads.get(&edge.event)?;
        let key = v8::String::new(scope, "owned")?;
        if payload.create_data_property(scope, key.into(), value) != Some(true) {
            return None;
        }
        healthy(scope)?;
    }
    // One directed ephemeron per existing Event wrapper. Neither a target nor
    // relatedTarget can retain a dead Event through this private graph.
    for (bits, payload) in event_payloads {
        let weak = {
            let world = state.borrow();
            Rc::clone(&world.wrappers.get(&bits)?.weak)
        };
        if let Some(event) = weak.to_local(scope) {
            healthy(scope)?;
            set.call(scope, graph.into(), &[event.into(), payload.into()])?;
            healthy(scope)?;
        } else {
            healthy(scope)?;
        }
    }
    // Build every private CSS group value before any provisional root is
    // dropped. Directed edges carry one already-materialized representative
    // from a target group; that representative's own WeakMap edge exposes the
    // target group's remaining wrappers. No wrapper is minted by collection.
    let mut css_values = Vec::with_capacity(css_plan.groups.len());
    for _ in &css_plan.groups {
        let values = v8::Object::new(scope);
        healthy(scope)?;
        let null = v8::null(scope);
        if values.set_prototype(scope, null.into()) != Some(true) {
            return None;
        }
        healthy(scope)?;
        css_values.push(values);
    }
    for (group_index, group) in css_plan.groups.iter().enumerate() {
        let values = css_values[group_index];
        let mut key_index = 0;
        for &bits in &group.wrappers {
            if !orphaned_hosts.remove(&bits) {
                // The shared snapshot may include only provisionally rooted
                // CSS records, and each may belong to exactly one group.
                return None;
            }
            let (generation, weak, has_root) = {
                let world = state.borrow();
                let entry = world.wrappers.get(&bits)?;
                (
                    entry.generation,
                    Rc::clone(&entry.weak),
                    entry.root.is_some(),
                )
            };
            if !has_root {
                return None;
            }
            let wrapper = weak.to_local(scope)?;
            healthy(scope)?;
            let key = v8::String::new(scope, &key_index.to_string())?;
            healthy(scope)?;
            if values.create_data_property(scope, key.into(), wrapper.into()) != Some(true) {
                return None;
            }
            healthy(scope)?;
            set.call(scope, graph.into(), &[wrapper.into(), values.into()])?;
            healthy(scope)?;
            wrappers.push(WrapperCandidate {
                bits,
                generation,
                weak,
                restored: None,
            });
            key_index += 1;
        }
        for &target in &group.targets {
            let bits = *css_plan.groups.get(target)?.wrappers.first()?;
            let weak = {
                let world = state.borrow();
                let entry = world.wrappers.get(&bits)?;
                entry.root.as_ref()?;
                Rc::clone(&entry.weak)
            };
            let wrapper = weak.to_local(scope)?;
            healthy(scope)?;
            let key = v8::String::new(scope, &key_index.to_string())?;
            healthy(scope)?;
            if values.create_data_property(scope, key.into(), wrapper.into()) != Some(true) {
                return None;
            }
            healthy(scope)?;
            key_index += 1;
        }
    }
    // Stale node or host handles have no valid native component and cannot keep
    // a provisional root indefinitely. Retain recovery without graph edges.
    for bits in orphaned.into_iter().chain(orphaned_hosts) {
        let (generation, weak, has_root) = {
            let state = state.borrow();
            let entry = state.wrappers.get(&bits)?;
            (
                entry.generation,
                Rc::clone(&entry.weak),
                entry.root.is_some(),
            )
        };
        if !has_root {
            return None;
        }
        wrappers.push(WrapperCandidate {
            bits,
            generation,
            weak,
            restored: None,
        });
    }
    let graph = v8::Global::new(scope, graph);
    healthy(scope)?;
    let wrapper_ids: Vec<_> = wrappers
        .iter()
        .map(|entry| (entry.bits, entry.generation))
        .collect();
    // Publish recovery ownership before dropping a single strong root.
    let mut world = state.borrow_mut();
    world.cycle = Some(Collection {
        plan,
        _graph: graph,
        candidates,
        wrappers,
    });
    drop(world);
    {
        let state = state.borrow();
        if eligible.iter().any(|bits| {
            state
                .values
                .get(*bits as usize)
                .and_then(Option::as_ref)
                .and_then(|slot| slot.global.as_ref())
                .is_none()
        }) {
            return None;
        }
    }
    for bits in &eligible {
        let root = state
            .borrow_mut()
            .values
            .get_mut(*bits as usize)?
            .as_mut()?
            .global
            .take()?;
        drop(root);
        healthy(scope)?;
    }
    {
        let state = state.borrow();
        if wrapper_ids.iter().any(|(bits, generation)| {
            state
                .wrappers
                .get(bits)
                .is_none_or(|entry| entry.generation != *generation || entry.root.is_none())
        }) {
            return None;
        }
    }
    for (bits, generation) in wrapper_ids {
        let root = {
            let mut state = state.borrow_mut();
            let entry = state.wrappers.get_mut(&bits)?;
            if entry.generation != generation {
                return None;
            }
            entry.root.take()?
        };
        drop(root);
        healthy(scope)?;
    }
    healthy(scope)
}

pub(crate) fn restore(
    scope: &mut v8::PinScope<'_, '_>,
    state: &Rc<RefCell<WorldState>>,
) -> Option<()> {
    let Some(collection) = state.borrow_mut().cycle.take() else {
        return Some(());
    };
    let mut recovery = Recovery {
        state,
        collection: Some(collection),
    };
    let collection = recovery.collection.as_mut()?;
    let mut dead_callbacks = HashSet::with_capacity(collection.candidates.len());
    for candidate in &mut collection.candidates {
        if let Some(value) = candidate.weak.to_local(scope) {
            candidate.restored = Some(v8::Global::new(scope, v8::Local::<v8::Value>::from(value)));
        } else {
            dead_callbacks.insert(candidate.bits);
        }
        healthy(scope)?;
    }
    for candidate in &mut collection.wrappers {
        if let Some(value) = candidate.weak.to_local(scope) {
            candidate.restored = Some(v8::Global::new(scope, value));
        }
        healthy(scope)?;
    }
    // Validate every destination before draining registrations or moving any
    // restored root. A failure retains the transaction intact and poisons the
    // owning engine at the outer native recovery boundary.
    {
        let world = state.borrow();
        for candidate in &collection.candidates {
            if candidate.restored.is_some()
                && world
                    .values
                    .get(candidate.bits as usize)
                    .and_then(Option::as_ref)
                    .is_none_or(|slot| slot.global.is_some())
            {
                return None;
            }
        }
        for candidate in &collection.wrappers {
            if candidate.restored.is_some()
                && world.wrappers.get(&candidate.bits).is_none_or(|entry| {
                    entry.generation != candidate.generation || entry.root.is_some()
                })
            {
                return None;
            }
            if candidate.restored.is_none() && world.hosts.contains_key(&candidate.bits) {
                // A dead weak wrapper must have run its V8 finalizer before
                // native host ownership can be released or reused.
                return None;
            }
        }
    }
    let released = collection.plan.drain_collected(&dead_callbacks);
    if tenta_js::bindings::native_callback_failure(scope).is_some() {
        return None;
    }
    {
        let mut world = state.borrow_mut();
        for candidate in &mut collection.candidates {
            if let Some(value) = candidate.restored.take() {
                world.values[candidate.bits as usize]
                    .as_mut()
                    .expect("validated callback slot")
                    .global = Some(Rc::new(value));
            }
        }
        for candidate in &mut collection.wrappers {
            if let Some(value) = candidate.restored.take() {
                world
                    .wrappers
                    .get_mut(&candidate.bits)
                    .expect("validated wrapper generation")
                    .root = Some(value);
            }
        }
    }
    for bits in released {
        let root = state.borrow_mut().take_released_value(bits);
        drop(root);
        if tenta_js::bindings::native_callback_failure(scope).is_some() {
            return None;
        }
    }
    // Drop the graph and weak recovery handles only after all survivors are
    // rooted again and the world borrow has ended.
    drop(recovery.collection.take());
    tenta_js::bindings::native_callback_failure(scope)
        .is_none()
        .then_some(())
}

/// One closed recovery checkpoint. The collection lease serializes watchdog
/// delivery with entry/reset but is not held as a mutex across V8 GC. The
/// callback's outer scope may remain open; each temporary preparation and
/// restoration scope closes before the next phase.
pub(crate) fn checkpoint(
    scope: &mut v8::PinScope<'_, '_>,
    state: &Rc<RefCell<WorldState>>,
    context: &SharedContext,
    termination: &std::sync::Arc<TerminationGate>,
    sweep: bool,
) -> Result<bool, ()> {
    // An embedding caller can still hold a document borrow across an engine
    // entry. A closed checkpoint must not turn that unrelated borrow conflict
    // into a collector panic before the intended host callback runs. Defer
    // collection without resetting pressure; admission will enforce the hard
    // bound if the caller keeps the document borrowed.
    if context
        .documents
        .all()
        .iter()
        .any(|document| document.doc.try_borrow_mut().is_err())
    {
        return Ok(true);
    }
    let Some(_lease) = termination.begin_collection() else {
        return Ok(false);
    };
    loop {
        let released_before = super::reclaim_dead_bookkeeping(scope, state, &context.documents)?;
        if tenta_js::bindings::native_callback_failure(scope).is_some() {
            return Err(());
        }
        drain_swept_listeners(scope, state, context)?;
        {
            v8::scope!(let prepare_scope, scope);
            prepare(prepare_scope, state, context).ok_or(())?;
        }
        if tenta_js::bindings::native_callback_failure(scope).is_some() {
            return Err(());
        }
        scope.low_memory_notification();
        if tenta_js::bindings::native_callback_failure(scope).is_some() {
            return Err(());
        }
        {
            v8::scope!(let restore_scope, scope);
            restore(restore_scope, state).ok_or(())?;
        }
        if tenta_js::bindings::native_callback_failure(scope).is_some() {
            return Err(());
        }
        let released_after = super::reclaim_dead_bookkeeping(scope, state, &context.documents)?;
        if tenta_js::bindings::native_callback_failure(scope).is_some() {
            return Err(());
        }
        drain_swept_listeners(scope, state, context)?;
        if !released_before && !released_after {
            break;
        }
    }
    if sweep {
        // SharedSink pins every raw parser-owned NodeId until the paused
        // parser resumes or drops. Its tree and any native NodeHandle are
        // therefore represented as native roots in Plan; unrelated detached
        // script-created nodes can be reclaimed here without a page-loop wait.
        for document in context.documents.all() {
            document.doc.borrow_mut().sweep_detached();
            if tenta_js::bindings::native_callback_failure(scope).is_some() {
                return Err(());
            }
        }
        super::reclaim_dead_bookkeeping(scope, state, &context.documents)?;
        drain_swept_listeners(scope, state, context)?;
    }
    state.borrow_mut().new_wrapper_materializations = 0;
    Ok(true)
}

fn drain_swept_listeners(
    scope: &v8::Isolate,
    state: &Rc<RefCell<WorldState>>,
    context: &SharedContext,
) -> Result<(), ()> {
    for document in context.documents.all() {
        let callbacks = document.take_swept_listeners();
        if tenta_js::bindings::native_callback_failure(scope).is_some() {
            return Err(());
        }
        for callback in callbacks {
            let root = state.borrow_mut().take_released_value(callback.0);
            drop(root);
            if tenta_js::bindings::native_callback_failure(scope).is_some() {
                return Err(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use tenta_js::bindings::host::SharedContext;

    fn new_cycle_engine() -> Engine {
        Engine::with_document(std::rc::Rc::new(std::cell::RefCell::new(
            tenta_dom::Document::new(),
        )))
    }
    fn cycle_context(engine: &Engine) -> SharedContext {
        engine.dom.as_ref().unwrap().ctx.clone()
    }
    fn retains_failed_cycle(engine: &Engine) -> bool {
        engine.dom.as_ref().unwrap().state.borrow().cycle.is_some()
    }
    fn expected_cycle_boundary() -> &'static str {
        "wrapper graph collection"
    }

    #[test]
    fn an_external_shared_document_borrow_defers_gc_without_poisoning() {
        use tenta_js::engine::PageEngine;

        let document = std::rc::Rc::new(std::cell::RefCell::new(tenta_dom::Document::new()));
        let mut engine = Engine::with_document(document.clone());
        let before = engine.gc_epoch();
        let held = document.borrow();
        engine.trim_memory();
        assert_eq!(engine.gc_epoch(), before);
        assert_eq!(engine.native_failure(), None);
        drop(held);
        engine.trim_memory();
        assert!(engine.gc_epoch() > before);
        assert_eq!(engine.native_failure(), None);
    }

    #[test]
    fn watchdog_request_during_nonempty_graph_recovers_before_termination() {
        let mut engine = new_cycle_engine();
        assert_eq!(
            engine
                .evaluate("globalThis.kept=document.createElement('div');kept.note='alive';'ready'")
                .unwrap(),
            "ready"
        );
        let (state, context) = {
            let dom = engine.dom.as_ref().unwrap();
            (Rc::clone(&dom.state), dom.ctx.clone())
        };
        let termination = std::sync::Arc::clone(&engine.termination);
        let _entry = engine.entered();
        let lease = termination.begin_collection().expect("no request yet");
        {
            v8::scope!(let handle_scope, &mut engine.isolate);
            let context_local = v8::Local::new(handle_scope, &engine.context);
            let mut context_scope = v8::ContextScope::new(handle_scope, context_local);
            v8::scope!(let scope, &mut context_scope);
            prepare(scope, &state, &context).expect("publish nonempty graph");
        }
        assert!(state.borrow().cycle.is_some(), "wrapper graph is active");
        termination.request();
        engine.isolate.low_memory_notification();
        {
            v8::scope!(let handle_scope, &mut engine.isolate);
            let context_local = v8::Local::new(handle_scope, &engine.context);
            let mut context_scope = v8::ContextScope::new(handle_scope, context_local);
            v8::scope!(let scope, &mut context_scope);
            restore(scope, &state).expect("restore before delivering termination");
        }
        drop(lease);
        assert!(termination.take_requested_and_reset());
        drop(_entry);
        assert_eq!(engine.evaluate("kept.note").unwrap(), "alive");
        assert_eq!(engine.native_failure(), None);
    }
    include!("../../../tenta-js/tests/support/collector_failure.rs");
}
