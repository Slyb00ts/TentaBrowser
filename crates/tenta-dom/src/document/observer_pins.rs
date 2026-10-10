//! Exact-generation native node ownership for queued observer records.
//!
//! This is one physical `NodeHandle` per referenced node in a Document,
//! regardless of how many records mention it. Pending record owners are
//! unconditional native roots. Published record/list owners are conditional
//! on their JavaScript wrappers in the shared graph transaction.

use hashbrown::{HashMap, HashSet};
use tenta_core::{
    Error, NativeMemoryBudget, NativeMemoryComponent, NativeMemoryReservation, Result,
};

use crate::{Document, NodeHandle};

use super::charged_deque::CapacityError;
use super::charged_hash::{reserve_map, reserve_set};
use super::observer_journal::{ObserverNode, SemanticRecord};
use super::observer_ownership::budget_error;

struct PinEntry {
    handle: NodeHandle,
    pending_owners: usize,
    conditional_owners: usize,
}

#[derive(Debug)]
pub enum PinError {
    Allocation,
    Budget(Error),
    Native(Error),
}

impl From<PinError> for Error {
    fn from(error: PinError) -> Self {
        match error {
            PinError::Allocation => Error::invalid("observer pin allocation failed"),
            PinError::Budget(error) => error,
            PinError::Native(error) => error,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PinPhase {
    Pending,
    Conditional,
    Released,
}

/// Exact native references owned by one queued record, published record, or
/// static record list. Deduplication happens before the DOM effect or JS
/// publication. Finalizer release only walks this already-owned slice.
#[derive(Debug)]
pub struct PinLease {
    nodes: Vec<ObserverNode>,
    phase: PinPhase,
}

impl PinLease {
    pub fn is_pending(&self) -> bool {
        self.phase == PinPhase::Pending
    }

    /// Native bytes retained by this owner, excluding the separately lived
    /// physical pin ledger entry. The caller keeps the corresponding page
    /// budget reservation until this lease is released.
    pub fn retained_payload_bytes(&self) -> Result<usize> {
        self.nodes
            .capacity()
            .checked_mul(std::mem::size_of::<ObserverNode>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .ok_or_else(|| Error::invalid("observer pin lease size overflow"))
    }
}

/// One ledger belongs to one native Document. Its parent must call
/// `release_all` before dropping the Document arena.
pub struct ObserverPins {
    entries: HashMap<ObserverNode, PinEntry>,
    // The Document every entry belongs to, set when the first entry of an
    // empty ledger is inserted. It answers "does this ledger belong to this
    // Document" in constant time; scanning the entries per record made queueing
    // N records cost O(N^2).
    owner_document: Option<u64>,
    // Exact physical entries with no logical owner. Their handles remain in
    // this same ledger until a mutable Document maintenance seam drains them.
    deferred_zero: usize,
    budget: NativeMemoryBudget,
    // Last field: physical handle entries and their hash buckets drop first.
    entries_charge: NativeMemoryReservation,
}

impl ObserverPins {
    pub fn new(budget: NativeMemoryBudget) -> Self {
        let entries_charge = budget.begin(NativeMemoryComponent::MutationObserver);
        Self {
            entries: HashMap::new(),
            owner_document: None,
            deferred_zero: 0,
            budget,
            entries_charge,
        }
    }

    pub fn budget(&self) -> &NativeMemoryBudget {
        &self.budget
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub fn owner_counts(&self, node: ObserverNode) -> Option<(usize, usize)> {
        self.entries
            .get(&node)
            .map(|entry| (entry.pending_owners, entry.conditional_owners))
    }

    /// One physical handle can be subtracted from native-root classification
    /// only if every pending/native owner is gone and a published JS-owned
    /// record or list still has a conditional owner. External handles are
    /// independent and must never be subtracted here.
    pub fn conditional_handle_subtraction(&self, node: ObserverNode) -> u16 {
        u16::from(
            self.entries
                .get(&node)
                .is_some_and(|entry| entry.pending_owners == 0 && entry.conditional_owners != 0),
        )
    }

    /// Preflight one record's native lifetime before its tree mutation. This
    /// operation is all-or-nothing: failed acquisition releases each new
    /// handle, leaving all existing logical owner counts unchanged.
    #[cfg(test)]
    pub fn acquire_pending_record(
        &mut self,
        document: &mut Document,
        record: &SemanticRecord,
    ) -> Result<PinLease> {
        self.acquire_pending_record_classified(document, record)
            .map_err(Into::into)
    }

    /// The charged owner uses the typed allocation cause to mark the shared
    /// page budget sticky without misclassifying stale or foreign nodes.
    pub fn acquire_pending_record_classified(
        &mut self,
        document: &mut Document,
        record: &SemanticRecord,
    ) -> Result<PinLease, PinError> {
        let nodes = unique_references(record, &self.budget)?;
        if !self.entries.is_empty()
            && self
                .owner_document
                .is_some_and(|owner| owner != document.document_id())
        {
            return Err(PinError::Native(Error::invalid(
                "observer pin ledger belongs to another document",
            )));
        }
        for node in &nodes {
            if node.document_id != document.document_id() {
                return Err(PinError::Native(Error::invalid(
                    "observer record references another document",
                )));
            }
            // Reject a stale generation before growing the retained pin map.
            // The later handle acquisition still performs its own fallible
            // capacity check and rolls back previously acquired handles.
            document.resolve(node.node).map_err(PinError::Native)?;
            if self
                .entries
                .get(node)
                .is_some_and(|entry| entry.pending_owners == usize::MAX)
            {
                return Err(PinError::Native(Error::invalid(
                    "observer pending owner count overflow",
                )));
            }
        }
        let new_entries = nodes
            .iter()
            .filter(|node| !self.entries.contains_key(*node))
            .count();
        // Count only entries that existed before this acquisition. Freshly
        // inserted entries also start at (0, 0), but were never deferred.
        let reused_deferred = nodes
            .iter()
            .filter(|node| {
                self.entries
                    .get(*node)
                    .is_some_and(|entry| entry.pending_owners == 0 && entry.conditional_owners == 0)
            })
            .count();
        assert!(
            self.deferred_zero >= reused_deferred,
            "observer deferred pin count drifted before acquisition"
        );
        reserve_map(
            &mut self.entries,
            &mut self.entries_charge,
            &self.budget,
            new_entries,
        )
        .map_err(|error| match error {
            CapacityError::Budget => PinError::Budget(budget_error(&self.budget)),
            CapacityError::Allocation => PinError::Allocation,
        })?;

        let mut acquired_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let requested_bytes = nodes
            .len()
            .checked_mul(std::mem::size_of::<(ObserverNode, NodeHandle)>())
            .ok_or_else(|| {
                acquired_charge.fail_allocation();
                PinError::Allocation
            })?;
        if !acquired_charge.charge(requested_bytes) {
            return Err(PinError::Budget(budget_error(&self.budget)));
        }
        let mut acquired = Vec::new();
        acquired.try_reserve_exact(nodes.len()).map_err(|_| {
            acquired_charge.fail_allocation();
            PinError::Allocation
        })?;
        let actual_bytes = acquired
            .capacity()
            .checked_mul(std::mem::size_of::<(ObserverNode, NodeHandle)>())
            .ok_or_else(|| {
                acquired_charge.fail_allocation();
                PinError::Allocation
            })?;
        if actual_bytes > requested_bytes && !acquired_charge.charge(actual_bytes - requested_bytes)
        {
            return Err(PinError::Budget(budget_error(&self.budget)));
        }
        for node in &nodes {
            if self.entries.contains_key(node) {
                continue;
            }
            let result = document
                .resolve(node.node)
                .and_then(|id| document.handle(id));
            match result {
                Ok(handle) => acquired.push((*node, handle)),
                Err(error) => {
                    for (_, handle) in acquired {
                        document.release(handle);
                    }
                    return Err(PinError::Native(error));
                }
            }
        }
        for (node, handle) in acquired {
            if self.entries.is_empty() {
                self.owner_document = Some(node.document_id);
            }
            let previous = self.entries.insert(
                node,
                PinEntry {
                    handle,
                    pending_owners: 0,
                    conditional_owners: 0,
                },
            );
            debug_assert!(previous.is_none());
        }
        for node in &nodes {
            self.entries
                .get_mut(node)
                .expect("preflight inserted each exact-generation pin")
                .pending_owners += 1;
        }
        // All fallible acquisition work has finished. No failed attempt may
        // consume the deferred count or change a prior logical owner.
        self.deferred_zero -= reused_deferred;
        Ok(PinLease {
            nodes,
            phase: PinPhase::Pending,
        })
    }

    /// Acquire every record owner in one semantic DOM operation before its
    /// first tree edit. A later invalid reference or handle-capacity refusal
    /// releases all earlier leases without allocating in the rollback path.
    #[cfg(test)]
    pub fn acquire_pending_batch<'a>(
        &mut self,
        document: &mut Document,
        records: impl ExactSizeIterator<Item = &'a SemanticRecord>,
    ) -> Result<Vec<PinLease>> {
        let mut leases = Vec::new();
        leases
            .try_reserve_exact(records.len())
            .map_err(|_| Error::invalid("observer pin batch allocation failed"))?;
        for record in records {
            match self.acquire_pending_record(document, record) {
                Ok(lease) => leases.push(lease),
                Err(error) => {
                    for lease in &mut leases {
                        self.release(document, lease)
                            .expect("preflighted pending lease rolls back exactly");
                    }
                    return Err(error);
                }
            }
        }
        Ok(leases)
    }

    /// Commit one pending owner into a JS-owned record/list owner after the
    /// exact wrapper and outgoing graph edge have been published. No new
    /// physical handle is acquired, including at the u16 handle cap.
    pub fn publish_record(&mut self, lease: &mut PinLease) -> Result<()> {
        if lease.phase != PinPhase::Pending {
            return Err(Error::invalid("observer pin is not pending at publication"));
        }
        for node in &lease.nodes {
            let entry = self
                .entries
                .get(node)
                .ok_or_else(|| Error::invalid("observer pin missing at publication"))?;
            if entry.pending_owners == 0 || entry.conditional_owners == usize::MAX {
                return Err(Error::invalid(
                    "observer pin owner count invalid at publication",
                ));
            }
        }
        for node in &lease.nodes {
            let entry = self
                .entries
                .get_mut(node)
                .expect("publication preflighted pin");
            entry.pending_owners -= 1;
            entry.conditional_owners += 1;
        }
        lease.phase = PinPhase::Conditional;
        Ok(())
    }

    /// Undo an unpublished host-conversion result without dropping the native
    /// batch. A backend may use this only while it still owns every partially
    /// constructed JS wrapper and before returning the result to page code.
    #[cfg(test)]
    pub fn revert_publication(&mut self, lease: &mut PinLease) -> Result<()> {
        if lease.phase != PinPhase::Conditional {
            return Err(Error::invalid(
                "observer pin is not conditional at rollback",
            ));
        }
        for node in &lease.nodes {
            let entry = self
                .entries
                .get(node)
                .ok_or_else(|| Error::invalid("observer pin missing during rollback"))?;
            if entry.conditional_owners == 0 || entry.pending_owners == usize::MAX {
                return Err(Error::invalid(
                    "observer pin owner count invalid during rollback",
                ));
            }
        }
        for node in &lease.nodes {
            let entry = self
                .entries
                .get_mut(node)
                .expect("rollback preflighted pin");
            entry.conditional_owners -= 1;
            entry.pending_owners += 1;
        }
        lease.phase = PinPhase::Pending;
        Ok(())
    }

    /// A separately materialized static list owns only its own members.
    /// `[]` acquires no owner and cannot retain its parent record's target or
    /// siblings. Every member must already be pinned by its live record.
    pub(super) fn retain_published_list(
        &mut self,
        members: &[ObserverNode],
        charge: &mut NativeMemoryReservation,
    ) -> Result<PinLease> {
        let nominal = members
            .len()
            .checked_mul(std::mem::size_of::<ObserverNode>())
            .ok_or_else(|| {
                charge.fail_allocation();
                budget_error(&self.budget)
            })?;
        if !charge.charge(nominal) {
            return Err(budget_error(&self.budget));
        }
        let nodes = unique_nodes(members.iter().copied(), members.len(), &self.budget).map_err(
            |error| match error {
                PinError::Allocation => {
                    charge.fail_allocation();
                    budget_error(&self.budget)
                }
                PinError::Budget(error) | PinError::Native(error) => error,
            },
        )?;
        let actual = nodes
            .capacity()
            .checked_mul(std::mem::size_of::<ObserverNode>())
            .ok_or_else(|| {
                charge.fail_allocation();
                budget_error(&self.budget)
            })?;
        if actual > nominal && !charge.charge(actual - nominal) {
            return Err(budget_error(&self.budget));
        }
        for node in &nodes {
            let entry = self
                .entries
                .get(node)
                .ok_or_else(|| Error::invalid("record list member has no physical pin"))?;
            if entry.conditional_owners == 0 || entry.conditional_owners == usize::MAX {
                return Err(Error::invalid(
                    "record list has no publishable owner capacity",
                ));
            }
        }
        for node in &nodes {
            self.entries
                .get_mut(node)
                .expect("list member pin was preflighted")
                .conditional_owners += 1;
        }
        Ok(PinLease {
            nodes,
            phase: PinPhase::Conditional,
        })
    }

    /// Drop an unpublished batch owner without touching Document. Its physical
    /// handle remains a native root until the next mutable maintenance seam.
    pub fn release_pending(&mut self, lease: &mut PinLease) -> Result<()> {
        if lease.phase != PinPhase::Pending {
            return Err(Error::invalid("observer pin is not pending at release"));
        }
        self.release_logical(lease)
    }

    /// Drop one published record or list owner without touching Document. The
    /// exact physical handle remains in this ledger if it has no other owner;
    /// `drain_released` will release it at a mutable Document maintenance seam.
    /// A failed or repeated release leaves all counts and the lease unchanged.
    pub fn release_published(&mut self, lease: &mut PinLease) -> Result<()> {
        if lease.phase != PinPhase::Conditional {
            return Err(Error::invalid("observer pin is not published at release"));
        }
        self.release_logical(lease)
    }

    fn release_logical(&mut self, lease: &mut PinLease) -> Result<()> {
        if lease.phase == PinPhase::Released {
            return Err(Error::invalid("observer pin lease already released"));
        }
        for node in &lease.nodes {
            let entry = self
                .entries
                .get(node)
                .ok_or_else(|| Error::invalid("observer pin missing at release"))?;
            if entry.handle.document_id() != node.document_id {
                return Err(Error::invalid("observer pin belongs to another document"));
            }
            let count = if lease.phase == PinPhase::Conditional {
                entry.conditional_owners
            } else {
                entry.pending_owners
            };
            if count == 0 {
                return Err(Error::invalid("observer pin owner count underflow"));
            }
        }
        for node in &lease.nodes {
            let entry = self.entries.get_mut(node).expect("release preflighted pin");
            if lease.phase == PinPhase::Conditional {
                entry.conditional_owners -= 1;
            } else {
                entry.pending_owners -= 1;
            }
            if entry.pending_owners == 0 && entry.conditional_owners == 0 {
                self.deferred_zero = self
                    .deferred_zero
                    .checked_add(1)
                    .expect("deferred observer pin count fits the retained map");
            }
        }
        lease.phase = PinPhase::Released;
        Ok(())
    }

    /// Native preparation still releases its handles immediately while the
    /// original Document is mutably available. Publication/finalization uses
    /// `release_published` instead, without a Document borrow.
    pub fn release(&mut self, document: &mut Document, lease: &mut PinLease) -> Result<()> {
        for node in &lease.nodes {
            let entry = self
                .entries
                .get(node)
                .ok_or_else(|| Error::invalid("observer pin missing at release"))?;
            if entry.handle.document_id() != document.document_id() {
                return Err(Error::invalid("observer pin released in another document"));
            }
        }
        if lease.phase == PinPhase::Conditional {
            self.release_published(lease)?;
        } else {
            self.release_pending(lease)?;
        }
        for node in &lease.nodes {
            if self
                .entries
                .get(node)
                .is_some_and(|entry| entry.pending_owners == 0 && entry.conditional_owners == 0)
            {
                let entry = self.entries.remove(node).expect("zero-owner pin exists");
                self.deferred_zero = self
                    .deferred_zero
                    .checked_sub(1)
                    .expect("immediate observer pin release was deferred exactly once");
                document.release(entry.handle);
            }
        }
        Ok(())
    }

    /// Release physical handles whose last published owner was finalized
    /// while Document was borrowed. No allocation, JavaScript, or foreign
    /// ledger participates. Map capacity stays charged until ledger teardown.
    pub fn drain_released(&mut self, document: &mut Document) -> Result<usize> {
        if self.deferred_zero == 0 {
            return Ok(0);
        }
        if self
            .owner_document
            .is_some_and(|owner| owner != document.document_id())
        {
            return Err(Error::invalid(
                "observer pin drained through another document",
            ));
        }
        let mut drained = 0;
        for (_, entry) in self
            .entries
            .extract_if(|_, entry| entry.pending_owners == 0 && entry.conditional_owners == 0)
        {
            document.release(entry.handle);
            drained += 1;
        }
        assert_eq!(
            drained, self.deferred_zero,
            "observer deferred pin count drifted"
        );
        self.deferred_zero = 0;
        Ok(drained)
    }

    /// Teardown releases all physical handles through their original owner.
    /// No callback or engine allocation occurs while the Document is borrowed.
    pub fn release_all(&mut self, document: &mut Document) {
        for (_, entry) in self.entries.drain() {
            document.release(entry.handle);
        }
        self.deferred_zero = 0;
    }
}

#[cfg(test)]
impl Default for ObserverPins {
    fn default() -> Self {
        Self::new(NativeMemoryBudget::with_limits(
            usize::MAX,
            usize::MAX,
            usize::MAX,
            0,
        ))
    }
}

fn unique_references(
    record: &SemanticRecord,
    budget: &NativeMemoryBudget,
) -> Result<Vec<ObserverNode>, PinError> {
    unique_nodes(
        record.referenced_nodes(),
        record.referenced_nodes().count(),
        budget,
    )
}

fn unique_nodes(
    references: impl Iterator<Item = ObserverNode>,
    count: usize,
    budget: &NativeMemoryBudget,
) -> Result<Vec<ObserverNode>, PinError> {
    let mut seen_charge = budget.begin(NativeMemoryComponent::MutationObserver);
    let mut seen = HashSet::new();
    reserve_set(&mut seen, &mut seen_charge, budget, count).map_err(|error| match error {
        CapacityError::Budget => PinError::Budget(budget_error(budget)),
        CapacityError::Allocation => PinError::Allocation,
    })?;
    let mut nodes = Vec::new();
    nodes.try_reserve(count).map_err(|_| PinError::Allocation)?;
    for node in references {
        if seen.insert(node) {
            nodes.push(node);
        }
    }
    Ok(nodes)
}

#[cfg(test)]
#[path = "observer_pins/tests.rs"]
mod tests;
