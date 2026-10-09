//! Page-budget and exact native ownership of one queued observer record.
//!
//! The shared journal selects semantic records; this owner moves with each
//! individual record through pending, JS publication, and finalizer cleanup.
//! It never charges an entire batch to a single surviving record.

use std::mem::size_of;

use tenta_core::{
    BudgetKind, Error, NativeMemoryBudget, NativeMemoryComponent, NativeMemoryReservation, Result,
};

use crate::Document;

use super::observer_journal::{
    ObserverNode, PreparedMutation, PreparedOwnedMutation, SemanticRecord, reserve_charged_vec,
};
use super::observer_pins::{ObserverPins, PinError, PinLease};

#[derive(Debug)]
pub struct OwnedObserverRecord {
    record: SemanticRecord,
    pin: PinLease,
    // Last field: it outlives record buffers and pin metadata until the
    // complete owner is dropped outside the Document borrow.
    _charge: NativeMemoryReservation,
}

impl OwnedObserverRecord {
    /// Charge the retained record and lease before acquiring a Document
    /// handle or changing the tree. The lease's actual vector capacity is
    /// charged before returning the new owner.
    pub fn acquire_pending(
        budget: &NativeMemoryBudget,
        pins: &mut ObserverPins,
        document: &mut Document,
        record: SemanticRecord,
    ) -> Result<Self> {
        if !budget.same_page(pins.budget()) {
            return Err(Error::invalid(
                "observer pin ledger belongs to another page budget",
            ));
        }
        let mut charge = budget.begin(NativeMemoryComponent::MutationObserver);
        let bytes = nominal_retained_bytes(&record).ok_or_else(|| {
            charge.fail_allocation();
            budget_error(budget)
        })?;
        if !charge.charge(bytes) {
            return Err(budget_error(budget));
        }
        let mut pin = match pins.acquire_pending_record_classified(document, &record) {
            Ok(pin) => pin,
            Err(PinError::Allocation) => {
                charge.fail_allocation();
                return Err(budget_error(budget));
            }
            Err(PinError::Budget(error)) => return Err(error),
            Err(PinError::Native(error)) => return Err(error),
        };
        let actual_pin_bytes = match pin.retained_payload_bytes() {
            Ok(bytes) => bytes,
            Err(_) => {
                charge.fail_allocation();
                pins.release(document, &mut pin)
                    .expect("a newly acquired pin rolls back before DOM mutation");
                return Err(budget_error(budget));
            }
        };
        let nominal_pin_bytes = record
            .referenced_nodes()
            .count()
            .checked_mul(size_of::<ObserverNode>())
            .and_then(|bytes| bytes.checked_add(size_of::<PinLease>()));
        let Some(nominal_pin_bytes) = nominal_pin_bytes else {
            charge.fail_allocation();
            pins.release(document, &mut pin)
                .expect("a newly acquired pin rolls back before DOM mutation");
            return Err(budget_error(budget));
        };
        let extra_bytes = actual_pin_bytes.saturating_sub(nominal_pin_bytes);
        if extra_bytes != 0 && !charge.charge(extra_bytes) {
            pins.release(document, &mut pin)
                .expect("a newly acquired pin rolls back before DOM mutation");
            return Err(budget_error(budget));
        }
        Ok(Self {
            record,
            pin,
            _charge: charge,
        })
    }

    pub fn record(&self) -> &SemanticRecord {
        &self.record
    }

    pub fn pin(&self) -> &PinLease {
        &self.pin
    }

    /// Transfer the already-owned physical handle from pending/native root
    /// to conditional JS-record ownership only after wrapper and directed
    /// graph edges have both been published.
    pub fn publish(&mut self, pins: &mut ObserverPins) -> Result<()> {
        pins.publish_record(&mut self.pin)
    }

    /// The caller drops the *whole* owner after releasing its Document borrow.
    /// A failure does not change physical pin counts. Native poison retains
    /// that rooted ledger entry until teardown; a discarded record's own
    /// buffers and reservation can then drop together.
    pub fn release(&mut self, pins: &mut ObserverPins, document: &mut Document) -> Result<()> {
        pins.release(document, &mut self.pin)
    }

    /// A moved batch releases only its still-unpublished owner on failure.
    /// The original registration supplies the pin ledger without borrowing
    /// Document; physical handles drain at the next mutable seam.
    pub fn release_pending_without_document(&mut self, pins: &mut ObserverPins) -> Result<()> {
        pins.release_pending(&mut self.pin)
    }

    /// Finalizer path. The stable registration supplies its original pin
    /// ledger; no Document borrow or native allocation is required.
    pub fn release_published(&mut self, pins: &mut ObserverPins) -> Result<()> {
        pins.release_published(&mut self.pin)
    }
}

/// A record's independently retained static node list owns only its members.
/// The ordered snapshot and its reservation outlive the exact PinLease buffer,
/// including after the record itself is finalized. An empty list retains no
/// native node bytes.
pub struct OwnedObserverList {
    members: Vec<ObserverNode>,
    pin: PinLease,
    // Last field: both the ordered snapshot and pin metadata drop first.
    _charge: NativeMemoryReservation,
}

impl OwnedObserverList {
    pub fn retain(pins: &mut ObserverPins, members: &[ObserverNode]) -> Result<Self> {
        let budget = pins.budget().clone();
        let mut charge = budget.begin(NativeMemoryComponent::MutationObserver);
        let mut snapshot = Vec::new();
        reserve_charged_vec(&mut snapshot, &mut charge, members.len())
            .map_err(|_| budget_error(&budget))?;
        snapshot.extend_from_slice(members);
        let pin = pins.retain_published_list(members, &mut charge)?;
        Ok(Self {
            members: snapshot,
            pin,
            _charge: charge,
        })
    }

    pub fn members(&self) -> &[ObserverNode] {
        &self.members
    }

    /// A retained list can outlive its record and release only its members.
    /// The caller drops the whole list after this logical release succeeds.
    pub fn release_published(&mut self, pins: &mut ObserverPins) -> Result<()> {
        pins.release_published(&mut self.pin)
    }
}

/// Convert every selected record in one validated DOM operation to an exact
/// queued owner before changing the tree. The journal must have reserved all
/// queue slots first. Failure releases every earlier pin and charge; no
/// partially owned record can be committed or silently dropped.
pub fn acquire_prepared_operation(
    budget: &NativeMemoryBudget,
    pins: &mut ObserverPins,
    document: &mut Document,
    mutations: Vec<PreparedMutation>,
) -> Result<Vec<PreparedOwnedMutation>> {
    let mut owned = Vec::new();
    if owned.try_reserve_exact(mutations.len()).is_err() {
        budget
            .begin(NativeMemoryComponent::MutationObserver)
            .fail_allocation();
        return Err(budget_error(budget));
    }
    for mutation in mutations {
        assert!(mutation.reserved, "observer queues must be reserved first");
        let mut entries_charge = budget.begin(NativeMemoryComponent::MutationObserver);
        let mut entries = Vec::new();
        if reserve_charged_vec(&mut entries, &mut entries_charge, mutation.entries.len()).is_err() {
            release_prepared(&mut owned, pins, document);
            return Err(budget_error(budget));
        }
        for (observer, record) in mutation.entries {
            match OwnedObserverRecord::acquire_pending(budget, pins, document, record) {
                Ok(record) => entries.push((observer, record)),
                Err(error) => {
                    release_entries(&mut entries, pins, document);
                    release_prepared(&mut owned, pins, document);
                    return Err(error);
                }
            }
        }
        owned.push(PreparedOwnedMutation {
            entries,
            reserved: true,
            _entries_charge: entries_charge,
        });
    }
    Ok(owned)
}

fn release_prepared(
    prepared: &mut Vec<PreparedOwnedMutation>,
    pins: &mut ObserverPins,
    document: &mut Document,
) {
    for mutation in prepared {
        release_entries(&mut mutation.entries, pins, document);
    }
}

fn release_entries(
    entries: &mut Vec<(super::observer_journal::ObserverId, OwnedObserverRecord)>,
    pins: &mut ObserverPins,
    document: &mut Document,
) {
    for (_, record) in entries {
        record
            .release(pins, document)
            .expect("newly acquired observer owner rolls back exactly");
    }
}

fn nominal_retained_bytes(record: &SemanticRecord) -> Option<usize> {
    let record_buffers = record
        .retained_payload_bytes()
        .ok()?
        .checked_sub(size_of::<SemanticRecord>())?;
    let maximum_pin_nodes = record
        .referenced_nodes()
        .count()
        .checked_mul(size_of::<ObserverNode>())?;
    size_of::<OwnedObserverRecord>()
        .checked_add(record_buffers)?
        .checked_add(maximum_pin_nodes)
}

pub(super) fn budget_error(budget: &NativeMemoryBudget) -> Error {
    let failure = budget.first_failure();
    Error::BudgetExceeded {
        kind: failure.map_or(BudgetKind::PageMemory, |failure| failure.kind),
        component: Some(
            failure
                .map_or(NativeMemoryComponent::MutationObserver, |failure| {
                    failure.component
                })
                .name()
                .to_owned(),
        ),
    }
}

#[cfg(test)]
#[path = "observer_ownership/tests.rs"]
mod tests;
