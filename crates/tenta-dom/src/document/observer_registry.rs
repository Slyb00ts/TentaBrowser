//! Native attachment list for independent observer worlds sharing a Document.
//!
//! The Document owns this small header. Each world owns its registration node,
//! while the header keeps nodes alive after that world leaves until their
//! physical NodeHandles can be released through the original Document. There
//! is no SharedDocument, JavaScript value, or strong agent edge in this graph.

use std::mem::size_of;
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::Mutex;
use tenta_core::{
    Error, NativeMemoryBudget, NativeMemoryComponent, NativeMemoryReservation, Result,
};

use crate::Document;

use super::observer_agent::ObserverAgent;
use super::observer_agent::PreparedObserverOperation;
use super::observer_ownership::{OwnedObserverList, OwnedObserverRecord, budget_error};
use super::observer_pins::ObserverPins;
use super::observer_semantics::CapturedSemanticPlan;

type WeakAgent = Weak<Mutex<ObserverAgent>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationPhase {
    Unbound,
    Active,
    Retired,
    Drained,
}

struct RegistrationState {
    phase: RegistrationPhase,
    // Retired nodes remain as tombstones until a mutable Document seam drains
    // them. This link remains valid for a walk that already holds the node.
    next: Option<Arc<ObserverRegistration>>,
    pins: ObserverPins,
    participating: bool,
    prepared: Option<NativeMutationSlot>,
}

/// Native-only operation ownership during one Document mutation. It has no
/// strong agent, Document, or engine reference, so retirement cannot create a
/// cycle or silently drop an unconsumed pin lease.
pub(super) struct NativeMutationSlot {
    pub(super) operation: PreparedObserverOperation,
    pub(super) captured: Option<CapturedSemanticPlan>,
}

/// One stable native attachment, unique to a (world, Document) pair. An alias
/// context in the same world reuses this node. Its pin ledger and byte charge
/// survive world teardown until the Document can be borrowed mutably.
pub struct ObserverRegistration {
    document_id: OnceLock<u64>,
    registry: OnceLock<Weak<ObserverRegistry>>,
    agent: OnceLock<WeakAgent>,
    state: Mutex<RegistrationState>,
    // Last field: child-node bytes stay charged until pins/links are gone.
    _charge: Option<NativeMemoryReservation>,
}

impl ObserverRegistration {
    /// Root shell construction preserves the existing infallible public engine
    /// constructor. This fixed Arc allocation is outside the partial native
    /// ledger, just like the existing HostContext/Rc shell.
    pub fn root(budget: NativeMemoryBudget) -> Arc<Self> {
        Arc::new(Self {
            document_id: OnceLock::new(),
            registry: OnceLock::new(),
            agent: OnceLock::new(),
            state: Mutex::new(RegistrationState {
                phase: RegistrationPhase::Unbound,
                next: None,
                pins: ObserverPins::new(budget),
                participating: false,
                prepared: None,
            }),
            _charge: None,
        })
    }

    /// A minted Document needs a distinct native attachment. Component quota
    /// admission precedes publication; stable std Arc allocation itself still
    /// has the repository's existing nonrecoverable allocator-OOM boundary.
    pub fn child(budget: NativeMemoryBudget) -> Result<Arc<Self>> {
        let mut charge = budget.begin(NativeMemoryComponent::MutationObserver);
        if !charge.charge(size_of::<Self>()) {
            return Err(budget_error(&budget));
        }
        Ok(Arc::new(Self {
            document_id: OnceLock::new(),
            registry: OnceLock::new(),
            agent: OnceLock::new(),
            state: Mutex::new(RegistrationState {
                phase: RegistrationPhase::Unbound,
                next: None,
                pins: ObserverPins::new(budget),
                participating: false,
                prepared: None,
            }),
            _charge: Some(charge),
        }))
    }

    pub fn document_id(&self) -> Option<u64> {
        self.document_id.get().copied()
    }

    /// Native agent preparation can own records and pins before a Document
    /// registry publishes this node. Bind its exact Document identity at that
    /// earlier attachment so an unpublished owner can still drain safely.
    pub(super) fn claim_document_id(&self, document_id: u64) -> Result<()> {
        if *self.document_id.get_or_init(|| document_id) != document_id {
            return Err(Error::invalid(
                "observer registration belongs to another Document",
            ));
        }
        Ok(())
    }

    pub fn phase(&self) -> RegistrationPhase {
        self.state.lock().phase
    }

    pub fn agent(&self) -> Option<Arc<Mutex<ObserverAgent>>> {
        self.agent.get()?.upgrade()
    }

    /// Publish an already-owned native operation with no allocation. The
    /// Document still holds the original borrow and no effect has occurred.
    pub(super) fn publish_prepared(&self, prepared: &mut Option<NativeMutationSlot>) -> Result<()> {
        let mut state = self.state.lock();
        if state.phase != RegistrationPhase::Active || state.participating {
            return Err(Error::Crashed {
                reason: "observer registration has an overlapping native mutation".to_owned(),
            });
        }
        state.prepared = prepared.take();
        if state.prepared.is_none() {
            return Err(Error::Crashed {
                reason: "observer registration received no native preparation".to_owned(),
            });
        }
        state.participating = true;
        Ok(())
    }

    pub(super) fn with_prepared<R>(
        &self,
        use_prepared: impl FnOnce(&mut NativeMutationSlot) -> R,
    ) -> Result<R> {
        let mut state = self.state.lock();
        let prepared = state.prepared.as_mut().ok_or_else(|| Error::Crashed {
            reason: "observer registration lost its native preparation".to_owned(),
        })?;
        Ok(use_prepared(prepared))
    }

    /// Lend the still-published owner and its exact physical pins together.
    /// Failure cleanup must not take the owner out of this slot before every
    /// release succeeds, and must not re-lock this registration for its pins.
    pub(super) fn with_prepared_and_pins<R>(
        &self,
        use_both: impl FnOnce(&mut NativeMutationSlot, &mut ObserverPins) -> R,
    ) -> Result<R> {
        let mut state = self.state.lock();
        let RegistrationState { prepared, pins, .. } = &mut *state;
        let owner = prepared.as_mut().ok_or_else(|| Error::Crashed {
            reason: "observer registration lost its native preparation".to_owned(),
        })?;
        Ok(use_both(owner, pins))
    }

    pub(super) fn take_prepared(&self) -> Option<NativeMutationSlot> {
        self.state.lock().prepared.take()
    }

    pub(super) fn participating(&self) -> bool {
        self.state.lock().participating
    }

    /// Call only after a complete no-failure pass over every participant.
    /// There is no fallible work left, so clearing markers cannot create a
    /// partially poisoned prefix after the first world commits.
    pub(super) fn finish_mutation(&self) {
        self.state.lock().participating = false;
    }

    /// The caller already holds its Document borrow, then its agent lock.
    /// Never hold the registry header lock while running this closure.
    #[cfg(test)]
    pub fn with_active_pins<R>(&self, use_pins: impl FnOnce(&mut ObserverPins) -> R) -> Result<R> {
        let mut state = self.state.lock();
        if state.phase != RegistrationPhase::Active {
            return Err(Error::invalid("observer registration is not active"));
        }
        Ok(use_pins(&mut state.pins))
    }

    /// Agent-internal access while this registration is prepared, active, or
    /// being torn down. The agent owns the operation; publication uses the
    /// stricter active-only accessor above.
    pub(super) fn with_pins<R>(&self, use_pins: impl FnOnce(&mut ObserverPins) -> R) -> R {
        use_pins(&mut self.state.lock().pins)
    }

    pub(super) fn with_pins_read<R>(&self, read: impl FnOnce(&ObserverPins) -> R) -> R {
        read(&self.state.lock().pins)
    }

    pub(super) fn conditional_handle_subtraction(
        &self,
        node: super::observer_journal::ObserverNode,
    ) -> u16 {
        if self.document_id() != Some(node.document_id) {
            return 0;
        }
        let state = self.state.lock();
        if state.phase != RegistrationPhase::Active {
            return 0;
        }
        state.pins.conditional_handle_subtraction(node)
    }

    /// Serialize a host observe() update against borrow-free retirement.
    /// The caller already holds the agent lock and does no page-script work.
    pub(super) fn with_active<R>(&self, use_active: impl FnOnce() -> R) -> Result<R> {
        let state = self.state.lock();
        if state.phase != RegistrationPhase::Active {
            return Err(Error::Closed);
        }
        Ok(use_active())
    }

    /// Commit one already-materialized record to conditional ownership. The
    /// caller must have published its wrapper and directed graph edges first.
    pub(super) fn publish_record(&self, record: &mut OwnedObserverRecord) -> Result<()> {
        if self.document_id() != Some(record.record().target.document_id) {
            return Err(Error::invalid(
                "observer record belongs to another Document",
            ));
        }
        let mut state = self.state.lock();
        if state.phase != RegistrationPhase::Active {
            return Err(Error::Closed);
        }
        record.publish(&mut state.pins)
    }

    /// A static list owns an ordered snapshot and only its exact members.
    pub(super) fn retain_record_list(
        &self,
        members: &[super::observer_journal::ObserverNode],
    ) -> Result<OwnedObserverList> {
        let mut state = self.state.lock();
        if state.phase != RegistrationPhase::Active {
            return Err(Error::Closed);
        }
        OwnedObserverList::retain(&mut state.pins, members)
    }

    /// Batch abandonment releases only unpublished logical owners. A drained
    /// registration already released every physical pin at world teardown.
    pub(super) fn release_pending_record(&self, record: &mut OwnedObserverRecord) -> Result<()> {
        if self.document_id() != Some(record.record().target.document_id) {
            return Err(Error::invalid(
                "observer record belongs to another Document",
            ));
        }
        let mut state = self.state.lock();
        if state.phase == RegistrationPhase::Drained {
            return Ok(());
        }
        if state.phase == RegistrationPhase::Unbound {
            return Err(Error::invalid("observer registration was never attached"));
        }
        record.release_pending_without_document(&mut state.pins)
    }

    pub(super) fn release_published_record(&self, record: &mut OwnedObserverRecord) -> Result<()> {
        if self.document_id() != Some(record.record().target.document_id) {
            return Err(Error::invalid(
                "observer record belongs to another Document",
            ));
        }
        let mut state = self.state.lock();
        if state.phase == RegistrationPhase::Drained {
            return Ok(());
        }
        if state.phase == RegistrationPhase::Unbound {
            return Err(Error::invalid("observer registration was never attached"));
        }
        record.release_published(&mut state.pins)
    }

    pub(super) fn release_published_list(&self, list: &mut OwnedObserverList) -> Result<()> {
        let mut state = self.state.lock();
        if state.phase == RegistrationPhase::Drained {
            return Ok(());
        }
        if state.phase == RegistrationPhase::Unbound {
            return Err(Error::invalid("observer registration was never attached"));
        }
        list.release_published(&mut state.pins)
    }

    /// Explicit native teardown can precede the last JS record/list owner.
    /// Mark the registration drained in the same transaction as releasing its
    /// physical pins, so later owner Drop never touches an empty ledger.
    pub(super) fn drain_on_teardown(&self, document: &mut Document) -> Result<()> {
        if self.document_id() != Some(document.document_id()) {
            return Err(Error::invalid("observer teardown used another Document"));
        }
        let mut state = self.state.lock();
        // A native-only agent may have prepared records before this node was
        // published into a Document registry. Its identity was claimed when
        // the agent attached, and those physical pins still need teardown.
        if state.phase != RegistrationPhase::Drained {
            state.pins.release_all(document);
            state.phase = RegistrationPhase::Drained;
        }
        Ok(())
    }

    /// Native graph preparation may inspect this ledger without acquiring a
    /// Document borrow, but only while its world still owns an active node.
    #[cfg(test)]
    pub fn with_active_pins_read<R>(&self, read: impl FnOnce(&ObserverPins) -> R) -> Result<R> {
        let state = self.state.lock();
        if state.phase != RegistrationPhase::Active {
            return Err(Error::invalid("observer registration is not active"));
        }
        Ok(read(&state.pins))
    }
}

impl Drop for ObserverRegistration {
    fn drop(&mut self) {
        // A walk can retain a retired tombstone whose saved successors are
        // no longer in the header. Unlink a uniquely owned tail iteratively
        // so the last captured head cannot recursively drop thousands of
        // registration nodes. A shared successor remains with its other owner.
        let mut next = self.state.get_mut().next.take();
        while let Some(node) = next {
            let Some(mut owned) = Arc::into_inner(node) else {
                break;
            };
            next = owned.state.get_mut().next.take();
        }
    }
}

#[derive(Default)]
struct RegistryLists {
    active: Option<Arc<ObserverRegistration>>,
}

/// This header is reachable from Document and each live world attachment. It
/// contains no Document backedge. Strong `next` links point only to stable
/// registration nodes, so a departed middle world cannot hide a later one.
pub struct ObserverRegistry {
    document_id: u64,
    lists: Mutex<RegistryLists>,
}

/// One host-side owner of an already published (world, Document) registration.
/// Aliased host contexts share this owner. Its fallback Drop only marks the
/// stable native node retired; physical pins stay in the Document registry
/// until the next mutable Document seam or Document destruction.
pub struct ObserverAttachment {
    registry: Arc<ObserverRegistry>,
    registration: Arc<ObserverRegistration>,
}

impl ObserverAttachment {
    pub(super) fn new(
        registry: Arc<ObserverRegistry>,
        registration: Arc<ObserverRegistration>,
    ) -> Self {
        debug_assert_eq!(registration.document_id(), Some(registry.document_id));
        debug_assert_eq!(registration.phase(), RegistrationPhase::Active);
        Self {
            registry,
            registration,
        }
    }

    pub fn document_id(&self) -> u64 {
        self.registry.document_id
    }

    pub fn retire(&self) -> bool {
        // This owner can only be built after attach succeeds. The registry
        // header and exact node stay alive together, so retirement needs no
        // fallible lookup, Document borrow, or allocation.
        let _header = self.registry.lists.lock();
        let mut state = self.registration.state.lock();
        if state.phase == RegistrationPhase::Active {
            state.phase = RegistrationPhase::Retired;
            true
        } else {
            false
        }
    }
}

impl Drop for ObserverAttachment {
    fn drop(&mut self) {
        self.retire();
    }
}

impl ObserverRegistry {
    pub fn new(document_id: u64) -> Arc<Self> {
        Arc::new(Self {
            document_id,
            lists: Mutex::new(RegistryLists::default()),
        })
    }

    /// Link a root node created inside the Document's fresh-world factory.
    /// No caller supplies an existing agent or node, and the fixed shell has
    /// already been allocated, so this path has no quota/error result.
    pub(super) fn attach_fresh_root(
        self: &Arc<Self>,
        node: &Arc<ObserverRegistration>,
        agent: WeakAgent,
    ) {
        let mut lists = self.lists.lock();
        let mut state = node.state.lock();
        debug_assert_eq!(state.phase, RegistrationPhase::Unbound);
        debug_assert!(node.document_id.get().is_none());
        debug_assert!(node.registry.get().is_none());
        debug_assert!(node.agent.get().is_none());
        node.document_id.get_or_init(|| self.document_id);
        node.registry.get_or_init(|| Arc::downgrade(self));
        node.agent.get_or_init(|| agent);
        state.next = lists.active.take();
        state.phase = RegistrationPhase::Active;
        lists.active = Some(Arc::clone(node));
    }

    /// Attach one precreated node before its HostContext slot is published.
    /// A same-world alias returns `false` and does not duplicate observations.
    /// No allocation occurs while the header is locked.
    pub fn attach(
        self: &Arc<Self>,
        node: &Arc<ObserverRegistration>,
        agent: WeakAgent,
    ) -> Result<bool> {
        // Lock the header before the node, including identity checks and
        // OnceLock initialization. Two worlds may try to attach the same
        // unbound Arc concurrently; checking outside this lock would let both
        // publish one `next` pointer into different headers.
        let mut lists = self.lists.lock();
        let mut state = node.state.lock();
        if let Some(id) = node.document_id()
            && id != self.document_id
        {
            return Err(Error::invalid(
                "observer registration belongs to another Document",
            ));
        }
        if let Some(registered) = node.registry.get()
            && !registered.ptr_eq(&Arc::downgrade(self))
        {
            return Err(Error::invalid(
                "observer registration belongs to another registry",
            ));
        }
        if let Some(registered) = node.agent.get()
            && !registered.ptr_eq(&agent)
        {
            return Err(Error::invalid(
                "observer registration belongs to another world",
            ));
        }
        if agent.upgrade().is_none() {
            return Err(Error::invalid("observer world ended before attachment"));
        }
        if state.phase == RegistrationPhase::Active {
            return Ok(false);
        }
        if state.phase != RegistrationPhase::Unbound {
            return Err(Error::invalid(
                "retired observer registration cannot be republished",
            ));
        }
        if node.document_id.get().is_none() && node.document_id.set(self.document_id).is_err() {
            return Err(Error::invalid("observer document binding raced"));
        }
        if node.registry.get().is_none() && node.registry.set(Arc::downgrade(self)).is_err() {
            return Err(Error::invalid("observer registry binding raced"));
        }
        if node.agent.get().is_none() && node.agent.set(agent).is_err() {
            return Err(Error::invalid("observer world binding raced"));
        }
        state.next = lists.active.take();
        state.phase = RegistrationPhase::Active;
        lists.active = Some(Arc::clone(node));
        Ok(true)
    }

    /// Start an allocation-free traversal. The caller clones each next-node
    /// link before entering an agent and skips a node retired in the meantime.
    pub fn active_head(&self) -> Option<Arc<ObserverRegistration>> {
        self.lists.lock().active.clone()
    }

    pub fn active_next(node: &ObserverRegistration) -> Option<Arc<ObserverRegistration>> {
        let state = node.state.lock();
        // A walk can hold a node that was retired after `active_head`. Keep
        // its old successor until every such walk releases the node.
        state.next.clone()
    }

    /// Visit active consumers without a temporary Vec or a held header lock.
    /// Capture each successor before entering a world: the callback may retire
    /// that world or another one while preserving every later consumer.
    pub fn for_each_active(
        &self,
        mut visit: impl FnMut(&Arc<ObserverRegistration>, Arc<Mutex<ObserverAgent>>) -> Result<()>,
    ) -> Result<()> {
        let mut current = self.active_head();
        while let Some(node) = current {
            current = Self::active_next(&node);
            if node.phase() == RegistrationPhase::Active {
                if let Some(agent) = node.agent() {
                    visit(&node, agent)?;
                } else {
                    // The world disappeared before its teardown callback.
                    // Keep its physical handles in the tombstone for the next
                    // mutable Document drain; do not skip later consumers.
                    self.retire(&node)?;
                }
            }
        }
        Ok(())
    }

    /// Walk every world that entered this exact Document mutation, including
    /// a retired tombstone still owning prepared native work. The caller does
    /// not hold the header lock while entering an agent or running a closure.
    pub(super) fn for_each_participating(
        &self,
        mut visit: impl FnMut(
            &Arc<ObserverRegistration>,
            Option<Arc<Mutex<ObserverAgent>>>,
        ) -> Result<()>,
    ) -> Result<()> {
        let mut current = self.active_head();
        while let Some(node) = current {
            current = Self::active_next(&node);
            if node.participating() {
                visit(&node, node.agent())?;
            }
        }
        Ok(())
    }

    /// Retire a departing world without a SharedDocument borrow. Its node
    /// remains as a tombstone in the active chain until a later mutable
    /// Document seam or Document::drop drains its physical pins and unlinks it.
    pub fn retire(&self, target: &Arc<ObserverRegistration>) -> Result<bool> {
        if target.document_id() != Some(self.document_id)
            || !target
                .registry
                .get()
                .is_some_and(|registry| std::ptr::eq(registry.as_ptr(), self))
        {
            return Err(Error::invalid(
                "observer registration belongs to another Document",
            ));
        }
        let lists = self.lists.lock();
        let phase = target.state.lock().phase;
        if phase == RegistrationPhase::Retired || phase == RegistrationPhase::Drained {
            return Ok(false);
        }
        if phase != RegistrationPhase::Active {
            return Err(Error::invalid(
                "unbound observer registration cannot retire",
            ));
        }
        let mut cursor = lists.active.clone();
        while let Some(current) = cursor {
            let next = current.state.lock().next.clone();
            if Arc::ptr_eq(&current, target) {
                current.state.lock().phase = RegistrationPhase::Retired;
                return Ok(true);
            }
            cursor = next;
        }
        Err(Error::invalid(
            "active observer registration is absent from its registry",
        ))
    }

    /// Drain zero-owner published pins and retired registrations through the
    /// exact original Document. Mutation and sweep call this with a mutable
    /// Document, after finalizers may have run under an external borrow. The
    /// header lock is never held across `Document::release` or node drop.
    /// The return count describes retired registrations, as before.
    pub fn drain_retired(&self, document: &mut Document) -> Result<usize> {
        if document.document_id() != self.document_id {
            return Err(Error::invalid(
                "observer registry belongs to another Document",
            ));
        }
        let mut current = self.active_head();
        while let Some(node) = current {
            current = Self::active_next(&node);
            let mut state = node.state.lock();
            if state.phase == RegistrationPhase::Active {
                state.pins.drain_released(document)?;
            }
        }
        let mut drained = 0usize;
        while let Some(node) = self.pop_retired() {
            {
                let mut state = node.state.lock();
                state.pins.release_all(document);
                state.phase = RegistrationPhase::Drained;
            }
            drained += 1;
        }
        Ok(drained)
    }

    /// Native Document-drop fallback. Engine contexts normally keep their
    /// Documents alive until world teardown; a direct native owner may not.
    pub fn drain_all_on_document_drop(&self, document: &mut Document) {
        debug_assert_eq!(document.document_id(), self.document_id);
        while let Some(node) = self.pop_active() {
            {
                let mut state = node.state.lock();
                state.pins.release_all(document);
                state.phase = RegistrationPhase::Drained;
            }
        }
    }

    fn pop_retired(&self) -> Option<Arc<ObserverRegistration>> {
        let mut lists = self.lists.lock();
        let mut previous: Option<Arc<ObserverRegistration>> = None;
        let mut cursor = lists.active.clone();
        while let Some(node) = cursor {
            let state = node.state.lock();
            let next = state.next.clone();
            let retired = matches!(
                state.phase,
                RegistrationPhase::Retired | RegistrationPhase::Drained
            );
            drop(state);
            if retired {
                // Keep the removed node's `next`: an in-flight traversal may
                // already hold it. The header bypasses it without creating a
                // second strong-link chain or an Arc cycle.
                match previous {
                    Some(previous) => previous.state.lock().next = next,
                    None => lists.active = next,
                }
                return Some(node);
            }
            previous = Some(node);
            cursor = next;
        }
        None
    }

    fn pop_active(&self) -> Option<Arc<ObserverRegistration>> {
        let mut lists = self.lists.lock();
        let node = lists.active.take()?;
        lists.active = node.state.lock().next.take();
        Some(node)
    }
}

impl Drop for ObserverRegistry {
    fn drop(&mut self) {
        let lists = self.lists.get_mut();
        while let Some(node) = lists.active.take() {
            lists.active = node.state.lock().next.take();
        }
    }
}

#[cfg(test)]
#[path = "observer_registry/tests.rs"]
mod tests;
