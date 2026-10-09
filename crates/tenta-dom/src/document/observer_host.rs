//! Narrow native ownership seam for a future MutationObserver host record.
//!
//! This module does not run JavaScript or enqueue engine microtasks. Its
//! batches keep the journal's moved records paired with their original stable
//! (world, Document) registrations until each record is published or released.

use std::sync::Arc;

use tenta_core::{Atom, Error, NativeMemoryComponent, NativeMemoryReservation, Result};

use super::Document;
use super::charged_deque::ChargedDeque;
use super::observer_hooks::SharedObserverAgent;
use super::observer_journal::{
    ObserveOptions, ObserveOptionsInput, ObserverId, ObserverNode, OptionsError, RecordType,
    SemanticRecord, reserve_charged_vec,
};
use super::observer_ownership::{OwnedObserverList, OwnedObserverRecord};
use super::observer_registry::{ObserverRegistration, RegistrationPhase};

/// Native validation is distinct from Web IDL's option-conversion TypeError.
#[derive(Debug)]
pub enum NativeObserveError {
    Options(OptionsError),
    Native(Error),
}

/// Native identity is supplied by the complete generational host key. The
/// binding owns the callback value and must finalize this observer by calling
/// `forget`; this native handle never roots an observed target.
pub struct NativeObserver {
    agent: SharedObserverAgent,
    id: ObserverId,
}

impl NativeObserver {
    pub fn new(agent: SharedObserverAgent, id: ObserverId) -> Result<Self> {
        agent.lock().check_host_access()?;
        Ok(Self { agent, id })
    }

    pub fn id(&self) -> ObserverId {
        self.id
    }

    /// Admit an inert host Node, such as Attr, without inventing an arena
    /// identity. The binding retains the exact weak host key itself. Attr
    /// has no children and is never an attribute mutation's native target.
    /// Options still use the same presence-sensitive normalization as every
    /// other Node, and a closed or poisoned world cannot admit a target.
    pub fn observe_inert(
        &self,
        document: &Document,
        input: ObserveOptionsInput,
    ) -> std::result::Result<(), NativeObserveError> {
        let agent = self.agent.lock();
        agent
            .check_host_access()
            .map_err(NativeObserveError::Native)?;
        agent
            .check_document(document)
            .map_err(NativeObserveError::Native)?;
        let registration = agent
            .registration_for(document.document_id())
            .ok_or(NativeObserveError::Native(Error::Closed))?;
        let options = ObserveOptions::normalize(input).map_err(NativeObserveError::Options)?;
        registration
            .with_active(|| drop(options))
            .map_err(NativeObserveError::Native)
    }

    pub fn observe(
        &self,
        document: &Document,
        target: ObserverNode,
        input: ObserveOptionsInput,
    ) -> std::result::Result<(), NativeObserveError> {
        let mut agent = self.agent.lock();
        agent
            .check_host_access()
            .map_err(NativeObserveError::Native)?;
        agent
            .check_document(document)
            .map_err(NativeObserveError::Native)?;
        if target.document_id != document.document_id() {
            return Err(NativeObserveError::Native(Error::invalid(
                "observer target belongs to another Document",
            )));
        }
        document
            .resolve(target.node)
            .map_err(NativeObserveError::Native)?;
        let registration = agent
            .registration_for(target.document_id)
            .cloned()
            .ok_or(NativeObserveError::Native(Error::Closed))?;
        let options = ObserveOptions::normalize(input).map_err(NativeObserveError::Options)?;
        let outcome = registration
            .with_active(|| agent.journal_mut().observe(self.id, target, options))
            .map_err(NativeObserveError::Native)?;
        outcome.map_err(|error| NativeObserveError::Native(agent.journal_error(error)))
    }

    pub fn take_records(&self) -> Result<NativeRecordBatch> {
        self.move_batch(BatchMove::Take)
    }

    pub fn disconnect(&self) -> Result<NativeRecordBatch> {
        self.move_batch(BatchMove::Disconnect)
    }

    /// Delivery calls this immediately before this observer's callback.
    /// Transients are removed by the existing journal operation.
    pub fn drain_for_notify(&self) -> Result<NativeRecordBatch> {
        self.move_batch(BatchMove::Notify)
    }

    /// Finalization removes weak registrations and returns the same owned
    /// batch contract as takeRecords. A host may drop this batch after leaving
    /// all slab, agent, and Document guards.
    pub fn forget(&self) -> Result<NativeRecordBatch> {
        self.move_batch(BatchMove::Forget)
    }

    fn move_batch(&self, mode: BatchMove) -> Result<NativeRecordBatch> {
        let (records, registrations, registration_charge) = {
            let mut agent = self.agent.lock();
            agent.check_host_access()?;
            let budget = agent.budget().clone();
            let mut registration_charge = budget.begin(NativeMemoryComponent::MutationObserver);
            let mut registrations = Vec::new();
            let count = agent.journal().queued_count(self.id);
            reserve_charged_vec(&mut registrations, &mut registration_charge, count)
                .map_err(|error| agent.journal_error(error))?;
            let mut corrupt = false;
            let mut closed = false;
            if let Some(queued) = agent.journal().queued_records(self.id) {
                for record in queued {
                    if !record.pin().is_pending() {
                        corrupt = true;
                        break;
                    }
                    let Some(registration) =
                        agent.registration_for(record.record().target.document_id)
                    else {
                        corrupt = true;
                        break;
                    };
                    if registration.phase() != RegistrationPhase::Active {
                        closed = true;
                        break;
                    }
                    registrations.push(Some(Arc::clone(registration)));
                }
            }
            if corrupt {
                agent.poison_shared_effect();
                return Err(Error::Crashed {
                    reason: "queued observer record lost its native owner".to_owned(),
                });
            }
            if closed {
                return Err(Error::Closed);
            }
            let records = match mode {
                BatchMove::Take => agent.journal_mut().take_records(self.id),
                BatchMove::Disconnect => agent.journal_mut().disconnect(self.id),
                BatchMove::Notify => agent.journal_mut().drain_for_notify(self.id),
                BatchMove::Forget => agent.journal_mut().forget_observer(self.id),
            };
            (records, registrations, registration_charge)
        };
        Ok(NativeRecordBatch {
            records,
            registrations,
            next: 0,
            agent: Arc::clone(&self.agent),
            // Last field: the registration vector is freed before its charge.
            _registration_charge: registration_charge,
        })
    }
}

#[derive(Clone, Copy)]
enum BatchMove {
    Take,
    Disconnect,
    Notify,
    Forget,
}

/// One authoritative moved queue. The host publishes its front record's
/// wrapper and directed graph edges before calling `commit_published_front`.
/// A failed publication leaves every uncommitted record here; Drop releases
/// only those pending records without borrowing Document.
#[must_use = "publish records or drop the batch outside native and engine locks"]
pub struct NativeRecordBatch {
    records: ChargedDeque<OwnedObserverRecord>,
    registrations: Vec<Option<Arc<ObserverRegistration>>>,
    next: usize,
    agent: SharedObserverAgent,
    _registration_charge: NativeMemoryReservation,
}

impl NativeRecordBatch {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn front(&self) -> Option<NativeRecordView<'_>> {
        self.records
            .front()
            .map(|record| record_view(record.record()))
    }

    pub fn commit_published_front(&mut self) -> Result<PublishedObserverRecord> {
        let mut agent = self.agent.lock();
        agent.check_host_access()?;
        let Some(registration) = self.registrations.get(self.next).and_then(Option::as_ref) else {
            if self.records.is_empty() {
                return Err(Error::invalid("observer batch has no record to publish"));
            }
            agent.poison_shared_effect();
            return Err(Error::Crashed {
                reason: "observer batch lost its original registration".to_owned(),
            });
        };
        if registration.phase() != RegistrationPhase::Active {
            return Err(Error::Closed);
        }
        let Some(record) = self.records.front_mut() else {
            agent.poison_shared_effect();
            return Err(Error::Crashed {
                reason: "observer batch registration has no record".to_owned(),
            });
        };
        if let Err(error) = registration.publish_record(record) {
            if !matches!(error, Error::Closed) {
                agent.poison_shared_effect();
            }
            return Err(error);
        }
        drop(agent);
        let record = self
            .records
            .pop_front()
            .expect("published front remains queued");
        let registration = self.registrations[self.next]
            .take()
            .expect("published record retains its original registration");
        self.next += 1;
        Ok(PublishedObserverRecord {
            record: Some(record),
            registration,
        })
    }
}

impl Drop for NativeRecordBatch {
    fn drop(&mut self) {
        let mut poisoned = false;
        while let Some(mut record) = self.records.pop_front() {
            let registration = self.registrations.get_mut(self.next).and_then(Option::take);
            self.next += 1;
            match registration {
                Some(registration) => {
                    if registration.release_pending_record(&mut record).is_err() {
                        // Logical release preflights without changing counts.
                        // Its physical pin remains rooted in the original
                        // registration until poisoned-world teardown.
                        poisoned = true;
                    }
                }
                None => {
                    poisoned = true;
                }
            }
        }
        if poisoned {
            self.agent.lock().poison_shared_effect();
        }
    }
}

/// Immutable mutation-time values. A static node list owns its own ordered
/// members separately; this view does not lend mutable queue/ledger access.
pub struct NativeRecordView<'a> {
    pub record_type: RecordType,
    pub target: ObserverNode,
    pub added_nodes: &'a [ObserverNode],
    pub removed_nodes: &'a [ObserverNode],
    pub previous_sibling: Option<ObserverNode>,
    pub next_sibling: Option<ObserverNode>,
    pub attribute_name: Option<Atom>,
    pub attribute_namespace: Option<Atom>,
    pub old_value: Option<&'a str>,
}

fn record_view(record: &SemanticRecord) -> NativeRecordView<'_> {
    NativeRecordView {
        record_type: record.record_type,
        target: record.target,
        added_nodes: &record.added_nodes,
        removed_nodes: &record.removed_nodes,
        previous_sibling: record.previous_sibling,
        next_sibling: record.next_sibling,
        attribute_name: record.attribute_name,
        attribute_namespace: record.attribute_namespace,
        old_value: record.old_value.as_deref(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObserverListKind {
    Added,
    Removed,
}

/// One published record keeps its exact original registration even if its
/// observer disconnects, a different world retires, or the queue is drained.
pub struct PublishedObserverRecord {
    record: Option<OwnedObserverRecord>,
    registration: Arc<ObserverRegistration>,
}

impl PublishedObserverRecord {
    pub fn view(&self) -> Result<NativeRecordView<'_>> {
        self.record
            .as_ref()
            .map(|record| record_view(record.record()))
            .ok_or_else(|| Error::invalid("observer record was already released"))
    }

    pub fn retain_list(&self, kind: ObserverListKind) -> Result<PublishedObserverList> {
        let agent = self.registration.agent().ok_or_else(|| Error::Crashed {
            reason: "observer world is no longer available".to_owned(),
        })?;
        let mut guard = agent.lock();
        guard.check_host_access()?;
        let record = self
            .record
            .as_ref()
            .ok_or_else(|| Error::invalid("observer record was already released"))?;
        let members = match kind {
            ObserverListKind::Added => &record.record().added_nodes,
            ObserverListKind::Removed => &record.record().removed_nodes,
        };
        if self.registration.phase() != RegistrationPhase::Active {
            return Err(Error::Closed);
        }
        let list = match self.registration.retain_record_list(members) {
            Ok(list) => list,
            Err(error) => {
                if !matches!(error, Error::BudgetExceeded { .. } | Error::Closed) {
                    guard.poison_shared_effect();
                }
                return Err(error);
            }
        };
        drop(guard);
        Ok(PublishedObserverList {
            list: Some(list),
            registration: Arc::clone(&self.registration),
        })
    }

    pub fn release(&mut self) -> Result<()> {
        let Some(record) = self.record.as_mut() else {
            return Ok(());
        };
        if let Err(error) = self.registration.release_published_record(record) {
            if let Some(agent) = self.registration.agent() {
                agent.lock().poison_shared_effect();
            }
            return Err(error);
        }
        self.record.take();
        Ok(())
    }
}

impl Drop for PublishedObserverRecord {
    fn drop(&mut self) {
        if self.release().is_err() {
            // A failed all-or-nothing logical release leaves the physical
            // entry rooted until teardown. This owner and its own byte charge
            // still drop together, while the world becomes unusable.
            if let Some(agent) = self.registration.agent() {
                agent.lock().poison_shared_effect();
            }
        }
    }
}

/// A retained static list owns only its ordered members. It can outlive the
/// parent record and releases logical owners without a Document borrow.
pub struct PublishedObserverList {
    list: Option<OwnedObserverList>,
    registration: Arc<ObserverRegistration>,
}

impl PublishedObserverList {
    pub fn members(&self) -> Result<&[ObserverNode]> {
        self.list
            .as_ref()
            .map(OwnedObserverList::members)
            .ok_or_else(|| Error::invalid("observer list was already released"))
    }

    pub fn release(&mut self) -> Result<()> {
        let Some(list) = self.list.as_mut() else {
            return Ok(());
        };
        if let Err(error) = self.registration.release_published_list(list) {
            if let Some(agent) = self.registration.agent() {
                agent.lock().poison_shared_effect();
            }
            return Err(error);
        }
        self.list.take();
        Ok(())
    }
}

impl Drop for PublishedObserverList {
    fn drop(&mut self) {
        if self.release().is_err()
            && let Some(agent) = self.registration.agent()
        {
            agent.lock().poison_shared_effect();
        }
    }
}

#[cfg(test)]
#[path = "observer_host/tests.rs"]
mod tests;
