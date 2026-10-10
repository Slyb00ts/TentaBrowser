//! Native preflight and publication for one wrapper world's observer journal.
//!
//! This type owns no `SharedDocument` or JavaScript value. A Document and its
//! world can share it through `Arc<Mutex<_>>` without creating a Document
//! ownership cycle. The world schedules notification only after releasing
//! the Document borrow; this module never calls script or an engine API.

use std::collections::VecDeque;
use std::mem::size_of;
use std::sync::Arc;

use hashbrown::HashMap;

use tenta_core::{
    Error, NativeMemoryBudget, NativeMemoryComponent, NativeMemoryReservation, Result,
};

use crate::Document;

use super::charged_deque::ChargedDeque;
use super::charged_hash::reserve_map;
use super::observer_journal::{
    InstalledTransients, JournalError, ObserverId, ObserverJournal, ObserverNode, PreparedMutation,
    PreparedOwnedMutation, PreparedTransients, reserve_charged_vec,
};
use super::observer_ownership::{acquire_prepared_operation, budget_error};
use super::observer_pins::ObserverPins;
use super::observer_registry::{ObserverRegistration, RegistrationPhase};
use super::observer_semantics::SemanticPlan;
use super::observer_semantics::SemanticStep;

/// One native-only owner for a wrapper world. Each stable registration owns
/// its physical `NodeHandle`s; the agent owns no duplicate pin ledger.
pub struct ObserverAgent {
    journal: ObserverJournal,
    root_registration: Arc<ObserverRegistration>,
    root_document_id: Option<u64>,
    root_attached: bool,
    child_registrations: HashMap<u64, Arc<ObserverRegistration>>,
    budget: NativeMemoryBudget,
    notify_demand: NotifyDemand,
    failed_preparation: Option<PreparedObserverOperation>,
    native_poisoned: bool,
    teardown_started: bool,
    // Last field: the child-node map drops before its high-water charge.
    child_registrations_charge: NativeMemoryReservation,
}

/// Native journal ownership moved out of the agent before releasing its
/// documents. Drop this outside the agent lock and every Document borrow.
/// Physical NodeHandles remain in each stable registration node until the
/// caller releases them through the original Document.
#[must_use = "drop native observer work outside the agent and Document borrows"]
pub struct NativeObserverTeardownWork {
    _failed_preparation: Option<PreparedObserverOperation>,
    _journal: ObserverJournal,
}

/// Fully reserved before the DOM effect. The caller installs transients just
/// before unlink, then commits records after success or aborts before effect.
/// An effect that can fail after changing native state needs an operation-
/// specific partial-commit hook; it cannot discard this whole transaction.
pub struct PreparedObserverOperation {
    document_id: u64,
    queue_notification: bool,
    notify_demand_before: NotifyDemand,
    effect_started: bool,
    awaiting_unlink: Option<ObserverNode>,
    owned: VecDeque<PreparedOwnedMutation>,
    transients: VecDeque<PreparedTransients>,
    steps: VecDeque<SemanticStep>,
    installed: Vec<InstalledTransients>,
    // Last fields: all three buffers drop before their page charges.
    _owned_charge: NativeMemoryReservation,
    _transients_charge: NativeMemoryReservation,
    _installed_charge: NativeMemoryReservation,
    _steps_charge: NativeMemoryReservation,
}

impl PreparedObserverOperation {
    pub(super) fn is_complete(&self) -> bool {
        self.awaiting_unlink.is_none()
            && self.transients.is_empty()
            && self.owned.is_empty()
            && self.steps.is_empty()
    }

    #[cfg(test)]
    pub(super) fn discard_next_step_for_test(&mut self) {
        self.steps.pop_front();
    }
}

/// The native request is distinct from the pending observer set. DOM queues
/// the microtask even when no registration selects a record, and takeRecords
/// does not cancel a previously requested job.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotifyDemand {
    #[default]
    Idle,
    Requested,
    Enqueued,
}

/// Read-only edges owned by this world's native observer journal. Targets
/// are weak: the directed edge keeps the observer alive only if the target
/// is live in the collector's graph. Pending observers are native roots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObserverGraphEdge {
    Target {
        target: ObserverNode,
        observer: ObserverId,
    },
    Pending(ObserverId),
}

impl ObserverAgent {
    pub fn new(budget: NativeMemoryBudget) -> Self {
        let child_registrations_charge = budget.begin(NativeMemoryComponent::MutationObserver);
        Self {
            journal: ObserverJournal::new(budget.clone()),
            root_registration: ObserverRegistration::root(budget.clone()),
            root_document_id: None,
            root_attached: false,
            child_registrations: HashMap::new(),
            budget,
            notify_demand: NotifyDemand::Idle,
            failed_preparation: None,
            native_poisoned: false,
            teardown_started: false,
            child_registrations_charge,
        }
    }

    /// Only the Document-owned fresh-world factory calls this immediately
    /// after `new`, before the agent or root node can be observed elsewhere.
    /// The fixed root shell has no quota admission or fallible publication.
    pub(super) fn bind_fresh_root_document(
        &mut self,
        document_id: u64,
    ) -> Arc<ObserverRegistration> {
        debug_assert!(self.root_document_id.is_none());
        debug_assert!(!self.root_attached);
        self.root_document_id = Some(document_id);
        self.root_attached = true;
        Arc::clone(&self.root_registration)
    }

    /// Name the root Document at world installation, before admitting any
    /// minted document. The infallible root node cannot be assigned to an
    /// arbitrary child merely because that child attached first.
    pub fn bind_root_document(&mut self, document: &Document) -> Result<()> {
        let id = document.document_id();
        if self.teardown_started {
            return Err(Error::invalid("observer agent is tearing down"));
        }
        match self.root_document_id {
            Some(existing) if existing != id => {
                Err(Error::invalid("observer root Document is already bound"))
            }
            _ => {
                self.root_document_id = Some(id);
                Ok(())
            }
        }
    }

    /// Prepare one native registration before publishing a HostContext slot.
    /// Binding it to the Document registry is the later world-installation
    /// transaction; aliases of the same Document reuse this registration.
    pub fn attach_document(&mut self, document: &Document) -> Result<()> {
        if self.teardown_started {
            return Err(Error::invalid("observer agent is tearing down"));
        }
        let id = document.document_id();
        if self.registration_for(id).is_some() {
            return Ok(());
        }
        let Some(root_id) = self.root_document_id else {
            return Err(Error::invalid("observer root Document is not bound"));
        };
        if id == root_id {
            self.root_registration.claim_document_id(id)?;
            self.root_attached = true;
            return Ok(());
        }
        let child = ObserverRegistration::child(self.budget.clone())?;
        child.claim_document_id(id)?;
        reserve_map(
            &mut self.child_registrations,
            &mut self.child_registrations_charge,
            &self.budget,
            1,
        )
        .map_err(|error| self.journal_error(error.into()))?;
        self.child_registrations.insert(id, child);
        Ok(())
    }

    pub(super) fn registration_for(&self, document_id: u64) -> Option<&Arc<ObserverRegistration>> {
        if self.root_attached && self.root_document_id == Some(document_id) {
            Some(&self.root_registration)
        } else {
            self.child_registrations.get(&document_id)
        }
    }

    /// Roll back only a newly prepared attachment that never entered the
    /// Document registry. Retained map capacity keeps its honest high-water
    /// charge; the unpublished child node and its charge are released.
    pub(super) fn rollback_unpublished_attachment(&mut self, document_id: u64) -> Result<()> {
        let registration = self
            .registration_for(document_id)
            .ok_or_else(|| Error::invalid("observer Document was not attached"))?;
        if registration.phase() != RegistrationPhase::Unbound
            || !registration.with_pins_read(ObserverPins::is_empty)
        {
            return Err(Error::Crashed {
                reason: "published observer attachment cannot be rolled back".to_owned(),
            });
        }
        if self.root_document_id == Some(document_id) {
            self.root_attached = false;
        } else {
            self.child_registrations.remove(&document_id);
        }
        Ok(())
    }

    /// Access the exact physical ledger without manufacturing a second owner.
    /// This native helper also supports unbound isolated Document tests.
    #[cfg(test)]
    pub(super) fn with_pins_for<R>(
        &self,
        document_id: u64,
        use_pins: impl FnOnce(&mut ObserverPins) -> R,
    ) -> Option<R> {
        Some(self.registration_for(document_id)?.with_pins(use_pins))
    }

    #[cfg(test)]
    pub(super) fn with_pins_for_read<R>(
        &self,
        document_id: u64,
        read: impl FnOnce(&ObserverPins) -> R,
    ) -> Option<R> {
        Some(self.registration_for(document_id)?.with_pins_read(read))
    }

    pub(super) fn journal(&self) -> &ObserverJournal {
        &self.journal
    }

    pub(super) fn budget(&self) -> &NativeMemoryBudget {
        &self.budget
    }

    pub(super) fn journal_mut(&mut self) -> &mut ObserverJournal {
        &mut self.journal
    }

    pub fn notify_demand(&self) -> NotifyDemand {
        self.notify_demand
    }

    /// Whether any observer has a record waiting for delivery. A microtask
    /// asked for by a mutation that no observer selected has nothing to do, and
    /// browsers queue none: the host enqueues only when this is true.
    pub fn has_pending_observers(&self) -> bool {
        self.journal.pending_observers().next().is_some()
    }

    /// Inspect native graph relationships without allocating, borrowing a
    /// Document, acquiring node handles, or materializing wrappers. The
    /// visitor must not call script or reenter the agent/registration locks.
    /// Duplicate target edges are allowed, including transient registrations.
    /// After begin_notify, its moved batch is the delivery adapter's root.
    pub fn visit_gc_edges(
        &self,
        mut visit: impl FnMut(ObserverGraphEdge) -> Result<()>,
    ) -> Result<()> {
        self.check_host_access()?;
        for (target, observer) in self.journal.target_observers() {
            let Some(registration) = self.registration_for(target.document_id) else {
                return Err(Error::Crashed {
                    reason: "observer graph target lost its original registration".to_owned(),
                });
            };
            match registration.with_active(|| visit(ObserverGraphEdge::Target { target, observer }))
            {
                Ok(outcome) => outcome?,
                Err(Error::Closed) => {}
                Err(error) => return Err(error),
            }
        }
        for observer in self.journal.pending_observers() {
            visit(ObserverGraphEdge::Pending(observer))?;
        }
        Ok(())
    }

    /// Subtract only this world's one active physical observer handle, and
    /// only when all its owners are published records/lists. Pending owners,
    /// foreign worlds, stale generations and retired undrained registrations
    /// cannot make a real native root disappear from cycle classification.
    pub fn conditional_handle_subtraction(&self, node: ObserverNode) -> Result<u16> {
        self.check_host_access()?;
        Ok(self
            .registration_for(node.document_id)
            .map_or(0, |registration| {
                registration.conditional_handle_subtraction(node)
            }))
    }

    /// The engine calls this only after its native microtask enqueue has
    /// succeeded. A failed enqueue leaves Requested and the owned work intact.
    pub fn mark_notify_enqueued(&mut self) -> Result<()> {
        self.check_native_health()?;
        if self.notify_demand != NotifyDemand::Requested {
            return Err(Error::invalid("observer notification was not requested"));
        }
        self.notify_demand = NotifyDemand::Enqueued;
        Ok(())
    }

    /// Called by the engine's enqueued job before invoking page callbacks.
    /// Resetting first lets a mutation inside callback A request a later job.
    pub fn begin_notify(&mut self) -> Result<ChargedDeque<ObserverId>> {
        self.check_native_health()?;
        if self.notify_demand != NotifyDemand::Enqueued {
            return Err(Error::invalid("observer notification job was not enqueued"));
        }
        self.notify_demand = NotifyDemand::Idle;
        Ok(self.journal.begin_notify())
    }

    pub(super) fn check_document(&self, document: &Document) -> Result<()> {
        self.check_document_id(document.document_id())
    }

    pub(super) fn check_document_id(&self, document_id: u64) -> Result<()> {
        self.check_native_health()?;
        if self.teardown_started {
            return Err(Error::Crashed {
                reason: "observer agent is tearing down".to_owned(),
            });
        }
        if self.registration_for(document_id).is_none() {
            return Err(Error::invalid(
                "observer Document is not attached to its agent",
            ));
        }
        if self.failed_preparation.is_some() {
            return Err(Error::Crashed {
                reason: "observer agent retains a failed native preparation".to_owned(),
            });
        }
        Ok(())
    }

    /// Check host access without borrowing a Document or mutating the journal.
    /// Poison, teardown and page-budget failure are sticky: this read-only
    /// check never clears them, including after the failing owner was dropped.
    pub fn check_host_access(&self) -> Result<()> {
        self.check_native_health()?;
        if self.teardown_started {
            return Err(Error::Crashed {
                reason: "observer agent is tearing down".to_owned(),
            });
        }
        if self.failed_preparation.is_some() {
            return Err(Error::Crashed {
                reason: "observer agent retains a failed native preparation".to_owned(),
            });
        }
        if self.budget.first_failure().is_some() {
            return Err(budget_error(&self.budget));
        }
        Ok(())
    }

    /// A world without registrations still receives a semantic notification
    /// request. This does not clear an earlier request or an enqueued job.
    pub(super) fn request_empty_notification(&mut self, requested: bool) {
        if requested && self.notify_demand == NotifyDemand::Idle {
            self.notify_demand = NotifyDemand::Requested;
        }
    }

    pub(super) fn check_native_health(&self) -> Result<()> {
        if self.native_poisoned {
            return Err(Error::Crashed {
                reason: "native observer ownership is poisoned".to_owned(),
            });
        }
        Ok(())
    }

    /// A failure after a shared Document effect poisons every participating
    /// world, including worlds whose engine was not executing at the time.
    pub(super) fn poison_shared_effect(&mut self) {
        self.native_poisoned = true;
    }

    /// Fail closed when a binding loses an exact prepared ownership identity.
    /// Use only for native publication/ownership invariants, never ordinary
    /// page receiver or option errors. Original registration pins survive for
    /// the existing teardown drain, and every later normal access must stop.
    pub fn poison_host_ownership(&mut self) {
        self.poison_shared_effect();
    }

    pub(super) fn journal_error(&self, error: JournalError) -> Error {
        match error {
            JournalError::Allocation => {
                self.budget
                    .begin(NativeMemoryComponent::MutationObserver)
                    .fail_allocation();
                budget_error(&self.budget)
            }
            JournalError::Budget => budget_error(&self.budget),
            JournalError::ObserverGone => {
                Error::invalid("observer disappeared during native mutation preparation")
            }
        }
    }

    /// Complete fallible selection, queue/transient capacity, and exact node
    /// lifetime acquisition before any native tree or value change.
    pub fn prepare(
        &mut self,
        document: &mut Document,
        plan: &SemanticPlan,
    ) -> Result<PreparedObserverOperation> {
        self.check_document(document)?;
        let document_id = document.document_id();
        let queue_notification = plan.request_notification;

        let mut prepared_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut prepared = Vec::<PreparedMutation>::new();
        reserve_charged_vec(&mut prepared, &mut prepared_charge, plan.records.len())
            .map_err(|error| self.journal_error(error))?;
        for facts in &plan.records {
            prepared.push(
                self.journal
                    .prepare(facts)
                    .map_err(|error| self.journal_error(error))?,
            );
        }
        let mut transients_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut transients = Vec::<PreparedTransients>::new();
        reserve_charged_vec(
            &mut transients,
            &mut transients_charge,
            plan.transient_removals.len(),
        )
        .map_err(|error| self.journal_error(error))?;
        for removal in &plan.transient_removals {
            transients.push(
                self.journal
                    .prepare_transients(removal.removed_root, &removal.old_parent_ancestors)
                    .map_err(|error| self.journal_error(error))?,
            );
        }
        self.journal
            .reserve_for_operation(&mut prepared, &transients)
            .map_err(|error| self.journal_error(error))?;
        self.journal
            .reserve_transients(&mut transients)
            .map_err(|error| self.journal_error(error))?;

        let mut installed_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut installed = Vec::<InstalledTransients>::new();
        if let Err(error) =
            reserve_charged_vec(&mut installed, &mut installed_charge, transients.len())
        {
            self.journal.discard_reserved_transients(&transients);
            return Err(self.journal_error(error));
        }

        let mut steps_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut steps = Vec::new();
        if let Err(error) = reserve_charged_vec(&mut steps, &mut steps_charge, plan.steps.len()) {
            self.journal.discard_reserved_transients(&transients);
            return Err(self.journal_error(error));
        }
        steps.extend_from_slice(&plan.steps);

        let mut owned_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let Some(nominal) = prepared
            .len()
            .checked_mul(size_of::<PreparedOwnedMutation>())
        else {
            owned_charge.fail_allocation();
            self.journal.discard_reserved_transients(&transients);
            return Err(budget_error(&self.budget));
        };
        if !owned_charge.charge(nominal) {
            self.journal.discard_reserved_transients(&transients);
            return Err(budget_error(&self.budget));
        }
        let registration = Arc::clone(
            self.registration_for(document_id)
                .expect("attached observer Document has a registration"),
        );
        let owned = match registration
            .with_pins(|pins| acquire_prepared_operation(&self.budget, pins, document, prepared))
        {
            Ok(owned) => owned,
            Err(error) => {
                self.journal.discard_reserved_transients(&transients);
                return Err(error);
            }
        };
        let actual = owned
            .capacity()
            .checked_mul(size_of::<PreparedOwnedMutation>());
        let extra_error = match actual {
            Some(actual) if actual > nominal && !owned_charge.charge(actual - nominal) => {
                Some(budget_error(&self.budget))
            }
            None => {
                owned_charge.fail_allocation();
                Some(budget_error(&self.budget))
            }
            _ => None,
        };
        let operation = PreparedObserverOperation {
            document_id,
            queue_notification,
            notify_demand_before: self.notify_demand,
            effect_started: false,
            awaiting_unlink: None,
            owned: VecDeque::from(owned),
            transients: VecDeque::from(transients),
            steps: VecDeque::from(steps),
            installed,
            _owned_charge: owned_charge,
            _transients_charge: transients_charge,
            _installed_charge: installed_charge,
            _steps_charge: steps_charge,
        };
        if let Some(error) = extra_error {
            self.abort(document, operation)?;
            return Err(error);
        }
        Ok(operation)
    }

    /// Publish only the next prepared transient set, immediately before its
    /// exact native unlink. A mismatch leaves the operation unchanged.
    pub fn install_next_transient(
        &mut self,
        operation: &mut PreparedObserverOperation,
        removed_root: ObserverNode,
    ) -> Result<()> {
        if operation.document_id != removed_root.document_id
            || operation.awaiting_unlink.is_some()
            || operation.steps.front() != Some(&SemanticStep::InstallTransient(removed_root))
            || operation
                .transients
                .front()
                .is_none_or(|next| next.removed_root() != removed_root)
        {
            return Err(Error::invalid(
                "observer transient does not match the next native unlink",
            ));
        }
        let prepared = operation
            .transients
            .pop_front()
            .expect("the next exact transient was checked");
        operation.steps.pop_front();
        let installed = self.journal.commit_transients(prepared);
        if installed.activated_count() != 0 && self.notify_demand == NotifyDemand::Idle {
            self.notify_demand = NotifyDemand::Requested;
        }
        operation.installed.push(installed);
        operation.awaiting_unlink = Some(removed_root);
        Ok(())
    }

    /// Called immediately after the matching native links changed. A later
    /// failure can no longer roll back this operation as if it had no effect.
    pub fn confirm_unlink(
        &mut self,
        operation: &mut PreparedObserverOperation,
        removed_root: ObserverNode,
    ) -> Result<()> {
        if operation.awaiting_unlink != Some(removed_root)
            || operation
                .installed
                .last()
                .is_none_or(|entry| entry.removed_root() != removed_root)
        {
            return Err(Error::invalid(
                "observer native unlink confirmation does not match preparation",
            ));
        }
        operation.awaiting_unlink = None;
        operation.effect_started = true;
        Ok(())
    }

    /// Queue the next already-owned record after its matching native effect.
    /// This preserves pending-observer activation order among records and
    /// transient registrations inside a composite DOM operation.
    pub fn queue_next_record(
        &mut self,
        operation: &mut PreparedObserverOperation,
        target: ObserverNode,
    ) -> Result<()> {
        if operation.document_id != target.document_id
            || operation.awaiting_unlink.is_some()
            || operation.steps.front() != Some(&SemanticStep::QueueRecord(target))
            || operation.owned.is_empty()
        {
            return Err(Error::invalid(
                "observer record does not match the next native queue step",
            ));
        }
        operation.steps.pop_front();
        operation.effect_started = true;
        let record = operation
            .owned
            .pop_front()
            .expect("the next exact record was checked");
        self.journal.commit(record);
        if self.notify_demand == NotifyDemand::Idle {
            self.notify_demand = NotifyDemand::Requested;
        }
        Ok(())
    }

    /// The allocation-free no-registration path still needs to remember a
    /// completed native effect when a later step of a compound mutation fails.
    pub(super) fn note_native_effect(&mut self, operation: &mut PreparedObserverOperation) {
        operation.effect_started = true;
    }

    /// Called after the validated native effect. A task boundary observes
    /// the journal's pending set and queues exactly one notification job.
    pub fn commit(&mut self, operation: PreparedObserverOperation) -> bool {
        assert!(
            operation.awaiting_unlink.is_none()
                && operation.transients.is_empty()
                && operation.owned.is_empty()
                && operation.steps.is_empty(),
            "transients must be installed before effect"
        );
        let requested = operation.queue_notification;
        if requested && self.notify_demand == NotifyDemand::Idle {
            self.notify_demand = NotifyDemand::Requested;
        }
        requested
    }

    /// Abort only when no DOM effect occurred. The standalone native entry
    /// retains its owner on either an ordinary failure or a panic.
    pub fn abort(
        &mut self,
        document: &mut Document,
        mut operation: PreparedObserverOperation,
    ) -> Result<()> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.check_document(document)?;
            let registration = Arc::clone(
                self.registration_for(operation.document_id)
                    .expect("attached observer Document has a registration"),
            );
            registration.with_pins(|pins| self.abort_borrowed(document, &mut operation, pins))
        }));
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.retain_panicked_operation(operation);
                Err(error)
            }
            Err(panic) => {
                self.retain_panicked_operation(operation);
                std::panic::resume_unwind(panic)
            }
        }
    }

    /// The shared Document fan-out keeps this operation in its registration
    /// slot until all releases succeed. The caller lends the same slot's pin
    /// ledger under one lock and poisons every participant on failure.
    pub(super) fn abort_borrowed(
        &mut self,
        document: &mut Document,
        operation: &mut PreparedObserverOperation,
        pins: &mut ObserverPins,
    ) -> Result<()> {
        self.check_document(document)?;
        if operation.document_id != document.document_id() {
            return Err(Error::invalid(
                "observer operation belongs to another document",
            ));
        }
        if operation.effect_started {
            return Err(Error::invalid(
                "observer operation changed the DOM before abort",
            ));
        }
        for installed in operation.installed.drain(..).rev() {
            self.journal.rollback_transients(installed);
        }
        let (first, second) = operation.transients.as_slices();
        self.journal.discard_reserved_transients(first);
        self.journal.discard_reserved_transients(second);
        for mutation in &mut operation.owned {
            for (_, record) in &mut mutation.entries {
                record.release(pins, document)?;
            }
        }
        self.notify_demand = operation.notify_demand_before;
        Ok(())
    }

    /// Finish a compound mutation whose earlier native effects succeeded but
    /// whose next unlink failed before changing links. Already queued records
    /// and confirmed transients remain published. Only the transient installed
    /// for the failed unlink and the unperformed suffix are rolled back. The
    /// caller must confirm every successful unlink before another fallible
    /// action, so `awaiting_unlink` always denotes an unchanged tree edge.
    ///
    /// The page boundary reports the original native error after this cleanup;
    /// it must not treat a partially changed DOM as an atomic failed mutation.
    pub fn finish_partial_failure(
        &mut self,
        document: &mut Document,
        mut operation: PreparedObserverOperation,
    ) -> Result<()> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.check_document(document)?;
            let registration = Arc::clone(
                self.registration_for(operation.document_id)
                    .expect("attached observer Document has a registration"),
            );
            registration.with_pins(|pins| {
                self.finish_partial_failure_borrowed(document, &mut operation, pins)
            })
        }));
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.retain_panicked_operation(operation);
                Err(error)
            }
            Err(panic) => {
                self.retain_panicked_operation(operation);
                std::panic::resume_unwind(panic)
            }
        }
    }

    /// Finish one world's portion of a partially changed shared Document
    /// while the registration still owns its operation and physical pins.
    pub(super) fn finish_partial_failure_borrowed(
        &mut self,
        document: &mut Document,
        operation: &mut PreparedObserverOperation,
        pins: &mut ObserverPins,
    ) -> Result<()> {
        if !operation.effect_started {
            return self.abort_borrowed(document, operation, pins);
        }
        self.check_document(document)?;
        if operation.document_id != document.document_id() {
            return Err(Error::invalid(
                "observer operation belongs to another document",
            ));
        }
        if let Some(root) = operation.awaiting_unlink.take() {
            let installed = operation
                .installed
                .pop()
                .expect("an unconfirmed unlink has an installed transient");
            assert_eq!(installed.removed_root(), root);
            self.journal.rollback_transients(installed);
        }
        let (first, second) = operation.transients.as_slices();
        self.journal.discard_reserved_transients(first);
        self.journal.discard_reserved_transients(second);
        for mutation in &mut operation.owned {
            for (_, record) in &mut mutation.entries {
                record.release(pins, document)?;
            }
        }
        if operation.queue_notification && self.notify_demand == NotifyDemand::Idle {
            self.notify_demand = NotifyDemand::Requested;
        }
        Ok(())
    }

    /// A native panic is caught by the enclosing callback boundary. Retain
    /// the still-owned operation so its physical pins cannot be silently
    /// orphaned while the page is poisoned and torn down.
    pub(super) fn retain_panicked_operation(&mut self, operation: PreparedObserverOperation) {
        assert!(
            self.failed_preparation.is_none(),
            "one observer operation may be active per Document borrow"
        );
        self.failed_preparation = Some(operation);
    }

    /// Close native observer entry and move queued/failed work out under the
    /// short agent lock. This allocates no record or Document container. The
    /// world drops the returned work only after releasing the lock, then
    /// borrows each original Document before calling the pin-release method.
    /// JS callback and notification entry must already be disabled.
    pub fn begin_teardown(&mut self) -> Result<NativeObserverTeardownWork> {
        if self.teardown_started {
            return Err(Error::invalid("observer teardown has already started"));
        }
        Ok(self.start_teardown())
    }

    /// HostContext teardown can follow an earlier native teardown request.
    /// The first caller receives the work owner; later calls have nothing to
    /// move and can still retire their stable Document registrations.
    pub fn begin_teardown_if_needed(&mut self) -> Option<NativeObserverTeardownWork> {
        (!self.teardown_started).then(|| self.start_teardown())
    }

    fn start_teardown(&mut self) -> NativeObserverTeardownWork {
        self.teardown_started = true;
        self.notify_demand = NotifyDemand::Idle;
        NativeObserverTeardownWork {
            _failed_preparation: self.failed_preparation.take(),
            _journal: std::mem::replace(
                &mut self.journal,
                ObserverJournal::new(self.budget.clone()),
            ),
        }
    }

    /// Teardown only: the engine has disabled callback entry and will drop
    /// all pending, cursor, and published lease owners without normal release.
    /// Call once for each original Document while its arena still exists. A
    /// registry-owned retired node instead defers this call until a mutable
    /// Document seam; this method never creates or removes another ledger.
    pub fn release_document_pins_on_teardown(&mut self, document: &mut Document) -> Result<()> {
        if !self.teardown_started {
            return Err(Error::invalid("observer teardown has not started"));
        }
        let Some(registration) = self.registration_for(document.document_id()) else {
            return Err(Error::invalid("observer Document was not attached"));
        };
        registration.drain_on_teardown(document)
    }

    /// Called after every attached Document's physical pins were released.
    pub fn finish_teardown(&mut self) -> Result<()> {
        if !self.teardown_started {
            return Err(Error::invalid("observer teardown has not started"));
        }
        if !self
            .root_registration
            .with_pins_read(ObserverPins::is_empty)
            || self
                .child_registrations
                .values()
                .any(|registration| !registration.with_pins_read(ObserverPins::is_empty))
        {
            return Err(Error::Crashed {
                reason: "observer teardown retains physical Document pins".to_owned(),
            });
        }
        self.failed_preparation = None;
        self.teardown_started = true;
        self.notify_demand = NotifyDemand::Idle;
        Ok(())
    }
}

#[cfg(test)]
#[path = "observer_agent/tests.rs"]
mod tests;
