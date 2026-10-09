//! Effect-time bridge from a Document mutation to every attached observer world.
//!
//! Each stable registration owns its prepared native operation and capture.
//! An empty world needs only its identity and notification demand. Neither
//! path adds a pin ledger or Document backedge, or enters JS under its locks.

use std::sync::Arc;

use parking_lot::Mutex;
use tenta_core::NativeMemoryBudget;
use tenta_core::{Error, NodeId, Result};

use super::observer_agent::ObserverAgent;
use super::observer_journal::ObserverNode;
use super::observer_ownership::budget_error;
use super::observer_registry::{
    NativeMutationSlot, ObserverAttachment, ObserverRegistration, ObserverRegistry,
    RegistrationPhase,
};
use super::observer_semantics::{
    CapturedSemanticPlan, SemanticPlan, capture_validated_charged, may_observe_mutation,
    requests_notification,
};
use super::{
    Document, Mutation, NodeKind, hierarchy_request, index_size, not_found, utf16_to_byte,
};
use crate::mutation::ReplaceAllWith;

pub type SharedObserverAgent = Arc<Mutex<ObserverAgent>>;

/// Prepared record/pin ownership stays in each stable registration node. A
/// sole empty world instead keeps only its admitted identity and demand.
pub(super) struct ObservedMutation {
    registry: Arc<ObserverRegistry>,
    empty: Option<EmptyObservedMutation>,
    shared_effect_started: bool,
    #[cfg(test)]
    fail_after_world_hook: Option<usize>,
    #[cfg(test)]
    fail_after_world_cleanup: Option<usize>,
}

/// One admitted world with no normal or transient registrations has no native
/// record owners to prepare. Keep its exact identity through the effect and
/// unwind, without publishing an empty operation in its registration slot.
struct EmptyObservedMutation {
    registration: Arc<ObserverRegistration>,
    agent: SharedObserverAgent,
    request_notification: bool,
}

impl EmptyObservedMutation {
    fn check_pre_effect(&self) -> Result<()> {
        let state = self.agent.lock();
        let document_id = self
            .registration
            .document_id()
            .ok_or_else(|| Error::Crashed {
                reason: "empty observer mutation lost its Document identity".to_owned(),
            })?;
        state.check_document_id(document_id)?;
        if state.budget().first_failure().is_some() {
            return Err(budget_error(state.budget()));
        }
        if !state
            .registration_for(document_id)
            .is_some_and(|registration| Arc::ptr_eq(registration, &self.registration))
        {
            return Err(Error::Crashed {
                reason: "empty observer mutation changed its registration".to_owned(),
            });
        }
        if self.registration.phase() != RegistrationPhase::Active {
            return Err(Error::Closed);
        }
        Ok(())
    }
}

impl Document {
    fn ensure_observer_registry(&mut self) -> &Arc<ObserverRegistry> {
        let document_id = self.document_id();
        self.observer_registry
            .get_or_insert_with(|| ObserverRegistry::new(document_id))
    }

    /// Surface a participating world's sticky native or page-memory failure
    /// at parser and native entry points that do not return through `mutate`.
    /// The check does not allocate records or enter either JS engine.
    pub fn check_observer_health(&self) -> Result<()> {
        let Some(registry) = &self.observer_registry else {
            return Ok(());
        };
        registry.for_each_active(|_, agent| {
            let state = agent.lock();
            state.check_document(self)?;
            if state.budget().first_failure().is_some() {
                return Err(budget_error(state.budget()));
            }
            Ok(())
        })
    }

    /// Construct and publish a private root world before any engine script
    /// context is exposed. Taking no caller-supplied agent is the freshness
    /// guarantee: this fixed registration cannot already belong to a Document.
    pub fn new_observer_world_owned(
        &mut self,
        budget: NativeMemoryBudget,
    ) -> (SharedObserverAgent, ObserverAttachment) {
        let agent = Arc::new(Mutex::new(ObserverAgent::new(budget)));
        let registration = agent.lock().bind_fresh_root_document(self.document_id());
        let registry = self.ensure_observer_registry();
        registry.attach_fresh_root(&registration, Arc::downgrade(&agent));
        let owner = ObserverAttachment::new(Arc::clone(registry), registration);
        (agent, owner)
    }

    /// Install this world's fixed root and return its host-side lifetime
    /// owner. The returned owner must be shared by aliases of this Document.
    pub fn attach_root_observer_agent_owned(
        &mut self,
        agent: SharedObserverAgent,
    ) -> Result<ObserverAttachment> {
        agent.lock().bind_root_document(self)?;
        self.attach_observer_agent_owned(agent)
    }

    /// Attach a minted Document before publishing its host slot. Native-only
    /// callers may still use `attach_observer_agent` without a host owner.
    pub fn attach_observer_agent_owned(
        &mut self,
        agent: SharedObserverAgent,
    ) -> Result<ObserverAttachment> {
        self.attach_observer_agent(Arc::clone(&agent))?;
        let registration = agent
            .lock()
            .registration_for(self.document_id())
            .cloned()
            .ok_or_else(|| Error::Crashed {
                reason: "observer attachment lost its native registration".to_owned(),
            })?;
        Ok(ObserverAttachment::new(
            Arc::clone(self.ensure_observer_registry()),
            registration,
        ))
    }

    pub(super) fn poison_observer_preparation(&self) {
        let Some(registry) = &self.observer_registry else {
            return;
        };
        let mut observed = ObservedMutation {
            registry: Arc::clone(registry),
            empty: None,
            shared_effect_started: false,
            #[cfg(test)]
            fail_after_world_hook: None,
            #[cfg(test)]
            fail_after_world_cleanup: None,
        };
        observed.retain_after_panic();
    }

    /// Bind the page's fixed root registration before any child context is
    /// published. This is the infallible root shell, not a quota reservation.
    pub fn attach_root_observer_agent(&mut self, agent: SharedObserverAgent) -> Result<()> {
        agent.lock().bind_root_document(self)?;
        self.attach_observer_agent(agent)
    }

    /// Attach an independent world's native observer agent before publishing
    /// its HostContext slot. An alias reuses the same document/world node.
    pub fn attach_observer_agent(&mut self, agent: SharedObserverAgent) -> Result<()> {
        let (registration, existed) = {
            let mut state = agent.lock();
            let existed = state.registration_for(self.document_id()).is_some();
            state.attach_document(self)?;
            (
                Arc::clone(
                    state
                        .registration_for(self.document_id())
                        .expect("successful attach publishes one native registration"),
                ),
                existed,
            )
        };
        let was_unattached = self.observer_registry.is_none();
        let attached = self
            .ensure_observer_registry()
            .attach(&registration, Arc::downgrade(&agent));
        if let Err(error) = attached {
            if was_unattached {
                self.observer_registry = None;
            }
            if !existed {
                agent
                    .lock()
                    .rollback_unpublished_attachment(self.document_id())?;
            }
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn prepare_observed_mutation(
        &mut self,
        mutation: &Mutation,
    ) -> Result<Option<ObservedMutation>> {
        let Some(registry) = self.observer_registry.clone() else {
            return Ok(None);
        };
        let mut prepared_any = false;
        let prepared = registry.for_each_active(|registration, agent| {
            let mut state = agent.lock();
            state.check_document(self)?;
            let budget = state.budget().clone();
            if budget.first_failure().is_some() {
                return Err(budget_error(&budget));
            }
            let captured = if may_observe_mutation(self, state.journal(), mutation) {
                Some(capture_validated_charged(self, mutation, &budget)?)
            } else {
                None
            };
            let empty = SemanticPlan::notification_only(requests_notification(self, mutation));
            let plan = captured.as_ref().map_or(&empty, CapturedSemanticPlan::plan);
            let operation = state.prepare(self, plan)?;
            let mut owner = Some(NativeMutationSlot {
                operation,
                captured,
            });
            if let Err(error) = registration.publish_prepared(&mut owner) {
                // Publication refused before any DOM effect. The local owner
                // still holds every lease and can be released exactly.
                let owner = owner.expect("failed publication retained native owner");
                state.abort(self, owner.operation)?;
                return Err(error);
            }
            prepared_any = true;
            Ok(())
        });
        let mut observed = ObservedMutation {
            registry,
            empty: None,
            shared_effect_started: false,
            #[cfg(test)]
            fail_after_world_hook: None,
            #[cfg(test)]
            fail_after_world_cleanup: None,
        };
        if let Err(error) = prepared {
            observed.finish_failure(self)?;
            return Err(error);
        }
        Ok(prepared_any.then_some(observed))
    }
}

#[cfg(test)]
#[path = "observer_hooks/tests.rs"]
mod tests;

impl ObservedMutation {
    pub(super) fn without_records(
        registry: Arc<ObserverRegistry>,
        registration: Arc<ObserverRegistration>,
        agent: SharedObserverAgent,
        request_notification: bool,
    ) -> Self {
        let empty = EmptyObservedMutation {
            registration,
            agent,
            request_notification,
        };
        // Document::mutate checks this owner immediately before its first
        // effect. No allocation or native mutation occurs between here and
        // that gate, so a second identical check only repeats its locks.
        Self {
            registry,
            empty: Some(empty),
            shared_effect_started: false,
            #[cfg(test)]
            fail_after_world_hook: None,
            #[cfg(test)]
            fail_after_world_cleanup: None,
        }
    }

    fn key(document: &Document, node: NodeId) -> ObserverNode {
        ObserverNode::new(document.document_id(), document.node_ref(node))
    }

    /// Check every participating world's sticky native/byte failure before
    /// the next native effect. Other engines remain dormant under this borrow.
    pub(super) fn check_pre_effect(&self) -> Result<()> {
        if let Some(empty) = &self.empty {
            return empty.check_pre_effect();
        }
        self.registry.for_each_participating(|_, agent| {
            let agent = agent.ok_or_else(|| Error::Crashed {
                reason: "observer world ended during a native mutation".to_owned(),
            })?;
            let state = agent.lock();
            state.check_native_health()?;
            if state.budget().first_failure().is_some() {
                return Err(budget_error(state.budget()));
            }
            Ok(())
        })
    }

    /// Every world installs its prepared transient before the same unlink.
    pub(super) fn before_unlink(&mut self, document: &Document, node: NodeId) -> Result<()> {
        self.check_pre_effect()?;
        if self.empty.is_some() {
            return Ok(());
        }
        let key = Self::key(document, node);
        self.registry.for_each_participating(|registration, agent| {
            let agent = agent.expect("participating observer world remains alive");
            let mut state = agent.lock();
            registration
                .with_prepared(|owner| {
                    if owner.captured.is_some() {
                        state.install_next_transient(&mut owner.operation, key)
                    } else {
                        Ok(())
                    }
                })
                .unwrap_or_else(|_| panic!("observer transient owner disappeared"))
                .unwrap_or_else(|_| panic!("observer transient preparation diverged from the DOM"));
            Ok(())
        })
    }

    /// A link already changed. Mark the shared transaction before visiting
    /// any world, so a panic between two hooks retains and poisons them all.
    pub(super) fn after_unlink(&mut self, document: &Document, node: NodeId) -> Result<()> {
        self.shared_effect_started = true;
        if self.empty.is_some() {
            return Ok(());
        }
        let key = Self::key(document, node);
        self.registry.for_each_participating(|registration, agent| {
            let agent = agent.expect("participating observer world remains alive");
            let mut state = agent.lock();
            registration
                .with_prepared(|owner| {
                    state.note_native_effect(&mut owner.operation);
                    if owner.captured.is_some() {
                        state.confirm_unlink(&mut owner.operation, key)
                    } else {
                        Ok(())
                    }
                })
                .unwrap_or_else(|_| panic!("observer unlink owner disappeared"))
                .unwrap_or_else(|_| panic!("observer unlink confirmation diverged from the DOM"));
            Ok(())
        })
    }

    /// Queue each world's next already-owned record after the exact effect.
    pub(super) fn after_record_effect(
        &mut self,
        document: &Document,
        target: NodeId,
    ) -> Result<()> {
        self.shared_effect_started = true;
        if let Some(empty) = &mut self.empty {
            // A native record-producing effect supplies authoritative demand.
            // Attribute removal uses this instead of pre-scanning for presence.
            empty.request_notification = true;
            return Ok(());
        }
        let key = Self::key(document, target);
        #[cfg(test)]
        let mut visited = 0usize;
        #[cfg(test)]
        let fail_after = self.fail_after_world_hook;
        self.registry.for_each_participating(|registration, agent| {
            let agent = agent.expect("participating observer world remains alive");
            let mut state = agent.lock();
            registration
                .with_prepared(|owner| {
                    state.note_native_effect(&mut owner.operation);
                    if owner.captured.is_some() {
                        state.queue_next_record(&mut owner.operation, key)
                    } else {
                        Ok(())
                    }
                })
                .unwrap_or_else(|_| panic!("observer record owner disappeared"))
                .unwrap_or_else(|_| panic!("observer record step diverged from the DOM"));
            #[cfg(test)]
            {
                visited += 1;
                if fail_after == Some(visited) {
                    panic!("injected observer failure between world record hooks");
                }
            }
            Ok(())
        })
    }

    pub(super) fn note_effect(&mut self) {
        self.shared_effect_started = true;
        if self.empty.is_some() {
            return;
        }
        self.registry
            .for_each_participating(|registration, agent| {
                let agent = agent.expect("participating observer world remains alive");
                let mut state = agent.lock();
                registration
                    .with_prepared(|owner| state.note_native_effect(&mut owner.operation))
                    .expect("observer effect owner disappeared");
                Ok(())
            })
            .expect("observer effect traversal diverged");
    }

    /// Validate the entire transaction before consuming any owner. Keep all
    /// participant markers until every agent's nonallocating commit finishes.
    pub(super) fn commit(&mut self) {
        if let Some(empty) = &self.empty {
            let mut state = empty.agent.lock();
            if empty.registration.phase() == RegistrationPhase::Active {
                state.request_empty_notification(empty.request_notification);
            }
            return;
        }
        self.registry
            .for_each_participating(|registration, _| {
                let complete = registration
                    .with_prepared(|owner| owner.operation.is_complete())
                    .expect("observer commit owner disappeared");
                assert!(
                    complete,
                    "observer mutation ended before all native steps completed"
                );
                Ok(())
            })
            .expect("observer commit traversal diverged");
        self.registry
            .for_each_participating(|registration, agent| {
                let agent = agent.expect("participating observer world remains alive");
                let owner = registration
                    .take_prepared()
                    .expect("validated observer owner is still published");
                agent.lock().commit(owner.operation);
                drop(owner.captured);
                Ok(())
            })
            .expect("observer commit traversal diverged");
        self.clear_participation();
    }

    fn clear_participation(&self) {
        self.registry
            .for_each_participating(|registration, _| {
                registration.finish_mutation();
                Ok(())
            })
            .expect("observer participation clear is infallible");
    }

    /// Abort before effects, or finish the same partial native effect in each
    /// world. A failure leaves unconsumed node owners published and poisons
    /// every participant; no later callback may deliver a partial success.
    pub(super) fn finish_failure(&mut self, document: &mut Document) -> Result<()> {
        if let Some(empty) = &self.empty {
            let mut state = empty.agent.lock();
            // Retirement has no prepared record/pin owner to roll back. A
            // retained Arc must not request a job for the departed world.
            if empty.registration.phase() != RegistrationPhase::Active {
                return Ok(());
            }
            if let Err(error) = state.check_document(document) {
                state.poison_shared_effect();
                return Err(error);
            }
            if self.shared_effect_started {
                state.request_empty_notification(empty.request_notification);
            }
            return Ok(());
        }
        #[cfg(test)]
        let mut visited = 0usize;
        #[cfg(test)]
        let fail_after = self.fail_after_world_cleanup;
        let result = self.registry.for_each_participating(|registration, agent| {
            let agent = agent.ok_or_else(|| Error::Crashed {
                reason: "observer world ended before native failure cleanup".to_owned(),
            })?;
            let mut state = agent.lock();
            let released = registration.with_prepared_and_pins(|owner, pins| {
                if self.shared_effect_started {
                    state.finish_partial_failure_borrowed(document, &mut owner.operation, pins)
                } else {
                    state.abort_borrowed(document, &mut owner.operation, pins)
                }
            })?;
            drop(state);
            released?;
            // Only a fully cleaned owner may leave its slot. Until every
            // participant succeeds, all markers remain for global poison.
            let owner = registration
                .take_prepared()
                .expect("cleaned observer owner remains published");
            drop(owner);
            #[cfg(test)]
            {
                visited += 1;
                if fail_after == Some(visited) {
                    panic!("injected observer failure between world cleanups");
                }
            }
            Ok(())
        });
        if let Err(error) = result {
            self.retain_after_panic();
            return Err(error);
        }
        self.clear_participation();
        Ok(())
    }

    /// Node slots keep every unconsumed native owner through this unwind. The
    /// marker persists for teardown, while all living participating agents
    /// receive sticky native poison, even if one already queued its records.
    pub(super) fn retain_after_panic(&mut self) {
        if let Some(empty) = &self.empty {
            empty.agent.lock().poison_shared_effect();
            return;
        }
        let _ = self.registry.for_each_participating(|_, agent| {
            if let Some(agent) = agent {
                agent.lock().poison_shared_effect();
            }
            Ok(())
        });
    }
}

impl Document {
    /// Repeat the mutation's ordinary validation before observer capture.
    /// Existing native methods retain their own checks so unattached
    /// Documents keep the current exact behavior and error messages.
    pub(super) fn validate_observed_mutation(&self, mutation: &Mutation) -> Result<()> {
        match mutation {
            Mutation::Append { parent, child } => {
                self.validate_insert(*parent, *child, NodeId::NONE)
            }
            Mutation::InsertBefore {
                parent,
                child,
                before,
            } => self.validate_insert(*parent, *child, *before),
            Mutation::Remove { child } => {
                if self.arena.is_live(*child) {
                    Ok(())
                } else {
                    Err(hierarchy_request("remove of a stale node"))
                }
            }
            Mutation::ReplaceChild {
                parent,
                new_child,
                old_child,
            } => {
                if self.parent(*old_child) != *parent {
                    return Err(not_found(
                        "the node to replace is not a child of the parent",
                    ));
                }
                self.validate_insert_excluding(*parent, *new_child, *old_child, *old_child)
            }
            Mutation::ReplaceAll {
                parent,
                replacement,
            } => self.validate_replace_all(*parent, *replacement),
            Mutation::SetAttribute {
                element,
                name,
                value,
            }
            | Mutation::ReplaceAttribute {
                element,
                name,
                value,
            } => {
                if self.kind(*element) != NodeKind::Element {
                    return Err(hierarchy_request("attributes belong to elements"));
                }
                self.validate_xml_attribute_value(value)?;
                self.validate_xml_name(name.local.as_str())
            }
            Mutation::RemoveAttribute { element, .. } => {
                if self.kind(*element) == NodeKind::Element {
                    Ok(())
                } else {
                    Err(hierarchy_request("attributes belong to elements"))
                }
            }
            // The parser historically treats a non-element as an empty batch.
            Mutation::AddMissingAttributes { .. } => Ok(()),
            Mutation::SetText { node, .. } | Mutation::ReplaceData { node, .. }
                if !self.kind(*node).is_character_data_subclass() =>
            {
                Err(hierarchy_request("only character data nodes hold text"))
            }
            Mutation::ReplaceData { node, offset, .. } => {
                let current = self.text_data(*node).unwrap_or_default();
                utf16_to_byte(current, *offset)
                    .map(|_| ())
                    .ok_or_else(|| index_size("offset is greater than the node's length"))
            }
            Mutation::SetText { .. } => Ok(()),
            Mutation::AppendTextData { node, .. } => {
                if self.kind(*node) == NodeKind::Text {
                    Ok(())
                } else {
                    Err(hierarchy_request("parser text append requires a text node"))
                }
            }
            Mutation::ReparentChildren { from, to } => {
                if !self.arena.is_live(*from) || !self.arena.is_live(*to) {
                    return Err(hierarchy_request("reparent with a stale node"));
                }
                if !self.kind(*to).is_container() {
                    return Err(hierarchy_request("reparent target cannot have children"));
                }
                if self.is_inclusive_ancestor(*from, *to) && from != to {
                    return Err(hierarchy_request(
                        "cannot reparent a node's children into its own subtree",
                    ));
                }
                Ok(())
            }
        }
    }

    pub(super) fn apply_observed_mutation(
        &mut self,
        mutation: Mutation,
        observed: &mut ObservedMutation,
    ) -> Result<()> {
        match mutation {
            Mutation::Append { parent, child } => {
                self.observed_insert(parent, child, NodeId::NONE, false, observed)
            }
            Mutation::InsertBefore {
                parent,
                child,
                before,
            } => self.observed_insert(parent, child, before, false, observed),
            Mutation::Remove { child } => self.observed_remove(child, true, observed),
            Mutation::ReplaceChild {
                parent,
                new_child,
                old_child,
            } => {
                if self.kind(new_child) == NodeKind::Fragment {
                    self.preflight_range_removals(
                        std::iter::once(old_child).chain(self.children(new_child)),
                    )?;
                } else if new_child == old_child {
                    self.preflight_range_removals([new_child])?;
                } else {
                    self.preflight_range_removals([new_child, old_child])?;
                }
                let mut anchor = self.next_sibling(old_child);
                if anchor == new_child {
                    anchor = self.next_sibling(new_child);
                }
                // The source adoption activates before the replaced child's
                // transient, matching the ordered semantic plan.
                self.observed_unlink(new_child, true, observed)?;
                if self.parent(old_child).is_some() {
                    self.observed_remove(old_child, false, observed)?;
                }
                self.observed_insert(parent, new_child, anchor, true, observed)
            }
            Mutation::ReplaceAll {
                parent,
                replacement,
            } => self.observed_replace_all(parent, replacement, observed),
            Mutation::SetAttribute {
                element,
                name,
                value,
            } => {
                observed.check_pre_effect()?;
                self.set_attribute(element, name, &value, false)?;
                observed.after_record_effect(self, element)
            }
            Mutation::ReplaceAttribute {
                element,
                name,
                value,
            } => {
                observed.check_pre_effect()?;
                self.set_attribute(element, name, &value, true)?;
                observed.after_record_effect(self, element)
            }
            Mutation::RemoveAttribute { element, name } => {
                observed.check_pre_effect()?;
                if self.remove_attribute(element, name)? {
                    observed.after_record_effect(self, element)?;
                }
                Ok(())
            }
            Mutation::AddMissingAttributes {
                element,
                attributes,
            } => {
                observed.check_pre_effect()?;
                let added = self.add_missing_attributes_unobserved(element, attributes);
                for _ in 0..added {
                    observed.after_record_effect(self, element)?;
                }
                Ok(())
            }
            Mutation::SetText { node, text } => {
                observed.check_pre_effect()?;
                self.set_text(node, &text)?;
                observed.after_record_effect(self, node)
            }
            Mutation::AppendTextData { node, text } => {
                observed.check_pre_effect()?;
                self.append_parser_text_data(node, &text)?;
                if text.is_empty() {
                    observed.note_effect();
                    Ok(())
                } else {
                    observed.after_record_effect(self, node)
                }
            }
            Mutation::ReplaceData {
                node,
                offset,
                count,
                data,
            } => {
                observed.check_pre_effect()?;
                self.replace_text_data(node, offset, count, &data)?;
                observed.after_record_effect(self, node)
            }
            Mutation::ReparentChildren { from, to } => {
                self.observed_reparent_children(from, to, observed)
            }
        }
    }

    fn observed_replace_all(
        &mut self,
        parent: NodeId,
        replacement: ReplaceAllWith,
        observed: &mut ObservedMutation,
    ) -> Result<()> {
        // Observer preparation can itself pin the parent after the initial
        // validation. Recheck compound range capacity with those pins held
        // before the first unlink; a refusal aborts every prepared world.
        self.validate_replace_all(parent, replacement)?;
        let removed = self.children(parent).count();
        for _ in 0..removed {
            let child = self.first_child(parent);
            self.observed_unlink(child, false, observed)?;
        }
        let added = match replacement {
            ReplaceAllWith::Empty => 0,
            ReplaceAllWith::Text(node) => {
                observed.check_pre_effect()?;
                self.link(parent, node, NodeId::NONE);
                observed.note_effect();
                1
            }
            ReplaceAllWith::ChildrenOf(source) => {
                let count = self.children(source).count();
                for _ in 0..count {
                    let child = self.first_child(source);
                    self.observed_unlink(child, false, observed)?;
                    observed.check_pre_effect()?;
                    self.link(parent, child, NodeId::NONE);
                    observed.note_effect();
                }
                if count != 0 {
                    observed.after_record_effect(self, source)?;
                }
                count
            }
        };
        if removed != 0 || added != 0 {
            observed.after_record_effect(self, parent)?;
            self.bump();
        }
        Ok(())
    }

    /// Returns an attached node's source childList record at this exact
    /// unlink only when the semantic operation calls for that record.
    fn observed_unlink(
        &mut self,
        child: NodeId,
        queue_source: bool,
        observed: &mut ObservedMutation,
    ) -> Result<()> {
        let old_parent = self.parent(child);
        if old_parent.is_some() {
            observed.before_unlink(self, child)?;
        }
        self.unlink(child)?;
        if old_parent.is_some() {
            observed.after_unlink(self, child)?;
            if queue_source {
                observed.after_record_effect(self, old_parent)?;
            }
        }
        Ok(())
    }

    fn observed_insert(
        &mut self,
        parent: NodeId,
        child: NodeId,
        before: NodeId,
        replacing: bool,
        observed: &mut ObservedMutation,
    ) -> Result<()> {
        self.validate_insert(parent, child, before)?;
        let before = if before == child {
            self.next_sibling(child)
        } else {
            before
        };
        if self.kind(child) == NodeKind::Fragment {
            // The fragment loses its first child on each move. A fixed initial
            // count visits exactly the original children without a second,
            // uncharged native vector after semantic capture.
            let child_count = self.children(child).count();
            self.preflight_range_removals(self.children(child))?;
            for _ in 0..child_count {
                let node = self.first_child(child);
                self.observed_unlink(node, false, observed)?;
                observed.check_pre_effect()?;
                self.link(parent, node, before);
                observed.note_effect();
            }
            if child_count != 0 {
                observed.after_record_effect(self, child)?;
            }
            if child_count != 0 || replacing {
                observed.after_record_effect(self, parent)?;
            }
            self.bump();
            return Ok(());
        }
        self.observed_unlink(child, true, observed)?;
        observed.check_pre_effect()?;
        self.link(parent, child, before);
        observed.after_record_effect(self, parent)?;
        self.bump();
        Ok(())
    }

    fn observed_remove(
        &mut self,
        child: NodeId,
        queue_source: bool,
        observed: &mut ObservedMutation,
    ) -> Result<()> {
        if !self.arena.is_live(child) {
            return Err(hierarchy_request("remove of a stale node"));
        }
        self.observed_unlink(child, queue_source, observed)?;
        self.bump();
        Ok(())
    }

    fn observed_reparent_children(
        &mut self,
        from: NodeId,
        to: NodeId,
        observed: &mut ObservedMutation,
    ) -> Result<()> {
        if !self.arena.is_live(from) || !self.arena.is_live(to) {
            return Err(hierarchy_request("reparent with a stale node"));
        }
        if !self.kind(to).is_container() {
            return Err(hierarchy_request("reparent target cannot have children"));
        }
        if self.is_inclusive_ancestor(from, to) && from != to {
            return Err(hierarchy_request(
                "cannot reparent a node's children into its own subtree",
            ));
        }
        // `from == to` rotates the first child to the end on each iteration;
        // the fixed original count still visits each original child once.
        let child_count = self.children(from).count();
        self.preflight_range_removals(self.children(from))?;
        for _ in 0..child_count {
            let child = self.first_child(from);
            self.observed_unlink(child, true, observed)?;
            observed.check_pre_effect()?;
            self.link(to, child, NodeId::NONE);
            observed.after_record_effect(self, to)?;
        }
        self.bump();
        Ok(())
    }
}
