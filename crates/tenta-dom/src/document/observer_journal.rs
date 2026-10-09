//! Native MutationObserver selection and ordered record queues.
//!
//! This module contains no JavaScript values or document borrows. The caller
//! supplies exact-generation node references and a mutation-time ancestry
//! snapshot. It prepares complete records before changing the tree, then
//! publishes them after the mutation succeeds. The document adapter owns
//! preflight memory reservation and node-pin acquisition.

use hashbrown::{HashMap, HashSet};
use tenta_core::{
    Atom, NativeMemoryBudget, NativeMemoryComponent, NativeMemoryReservation, NodeRef,
};

use super::charged_deque::{CapacityError, ChargedDeque};
use super::charged_hash::{reserve_map, reserve_set};
use super::observer_ownership::OwnedObserverRecord;

/// An exact node within one native Document. Arena indices and generations
/// can match across auxiliary documents, so the document id is mandatory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObserverNode {
    pub document_id: u64,
    pub node: NodeRef,
}

impl ObserverNode {
    pub const fn new(document_id: u64, node: NodeRef) -> Self {
        Self { document_id, node }
    }
}

/// Identity supplied by the page's observer host record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObserverId(pub u64);

/// The validated, inferred options used by the native selector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObserveOptions {
    pub child_list: bool,
    pub attributes: bool,
    pub character_data: bool,
    pub subtree: bool,
    pub attribute_old_value: bool,
    pub character_data_old_value: bool,
    /// `Some([])` intentionally filters out every attribute.
    pub attribute_filter: Option<Vec<Atom>>,
}

/// Dictionary presence must survive Web IDL conversion for the inference and
/// explicit-false checks in DOM Standard §4.3.1.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObserveOptionsInput {
    pub child_list: bool,
    pub attributes: Option<bool>,
    pub character_data: Option<bool>,
    pub subtree: bool,
    pub attribute_old_value: Option<bool>,
    pub character_data_old_value: Option<bool>,
    pub attribute_filter: Option<Vec<Atom>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionsError {
    NoMutationType,
    AttributesDisabled,
    CharacterDataDisabled,
}

impl ObserveOptions {
    pub fn normalize(input: ObserveOptionsInput) -> Result<Self, OptionsError> {
        let attributes = input
            .attributes
            .unwrap_or(input.attribute_old_value.is_some() || input.attribute_filter.is_some());
        let character_data = input
            .character_data
            .unwrap_or(input.character_data_old_value.is_some());
        if !input.child_list && !attributes && !character_data {
            return Err(OptionsError::NoMutationType);
        }
        if !attributes
            && (input.attribute_old_value == Some(true) || input.attribute_filter.is_some())
        {
            return Err(OptionsError::AttributesDisabled);
        }
        if !character_data && input.character_data_old_value == Some(true) {
            return Err(OptionsError::CharacterDataDisabled);
        }
        Ok(Self {
            child_list: input.child_list,
            attributes,
            character_data,
            subtree: input.subtree,
            attribute_old_value: input.attribute_old_value.unwrap_or(false),
            character_data_old_value: input.character_data_old_value.unwrap_or(false),
            attribute_filter: input.attribute_filter,
        })
    }
}

fn copy_nodes(nodes: &[ObserverNode]) -> Result<Vec<ObserverNode>, JournalError> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(nodes.len())
        .map_err(|_| JournalError::Allocation)?;
    copy.extend_from_slice(nodes);
    Ok(copy)
}

fn copy_old_value(value: Option<&str>) -> Result<Option<String>, JournalError> {
    value
        .map(|value| {
            let mut copy = String::new();
            copy.try_reserve_exact(value.len())
                .map_err(|_| JournalError::Allocation)?;
            copy.push_str(value);
            Ok(copy)
        })
        .transpose()
}

fn copy_options(options: &ObserveOptions) -> Result<ObserveOptions, JournalError> {
    let attribute_filter = options
        .attribute_filter
        .as_deref()
        .map(copy_atoms)
        .transpose()?;
    Ok(ObserveOptions {
        child_list: options.child_list,
        attributes: options.attributes,
        character_data: options.character_data,
        subtree: options.subtree,
        attribute_old_value: options.attribute_old_value,
        character_data_old_value: options.character_data_old_value,
        attribute_filter,
    })
}

fn filter_bytes(options: &ObserveOptions) -> Result<usize, JournalError> {
    options.attribute_filter.as_ref().map_or(Ok(0), |filter| {
        filter
            .capacity()
            .checked_mul(std::mem::size_of::<Atom>())
            .ok_or(JournalError::Allocation)
    })
}

fn charge_owned_filter(
    options: &ObserveOptions,
    budget: &NativeMemoryBudget,
) -> Result<NativeMemoryReservation, JournalError> {
    let mut charge = budget.begin(NativeMemoryComponent::MutationObserver);
    let bytes = filter_bytes(options).inspect_err(|_| {
        charge.fail_allocation();
    })?;
    if !charge.charge(bytes) {
        return Err(JournalError::Budget);
    }
    Ok(charge)
}

fn copy_options_charged(
    options: &ObserveOptions,
    budget: &NativeMemoryBudget,
) -> Result<(ObserveOptions, NativeMemoryReservation), JournalError> {
    let mut charge = budget.begin(NativeMemoryComponent::MutationObserver);
    let minimum = options
        .attribute_filter
        .as_ref()
        .map_or(Some(0), |filter| {
            filter.len().checked_mul(std::mem::size_of::<Atom>())
        })
        .ok_or_else(|| {
            charge.fail_allocation();
            JournalError::Allocation
        })?;
    if !charge.charge(minimum) {
        return Err(JournalError::Budget);
    }
    let copy = copy_options(options).inspect_err(|_| {
        charge.fail_allocation();
    })?;
    let actual = filter_bytes(&copy).inspect_err(|_| {
        charge.fail_allocation();
    })?;
    if actual > minimum && !charge.charge(actual - minimum) {
        return Err(JournalError::Budget);
    }
    Ok((copy, charge))
}

fn copy_atoms(atoms: &[Atom]) -> Result<Vec<Atom>, JournalError> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(atoms.len())
        .map_err(|_| JournalError::Allocation)?;
    copy.extend_from_slice(atoms);
    Ok(copy)
}

pub(super) fn reserve_charged_vec<T>(
    values: &mut Vec<T>,
    charge: &mut NativeMemoryReservation,
    count: usize,
) -> Result<(), JournalError> {
    debug_assert_eq!(values.capacity(), 0, "this helper reserves a new vector");
    let minimum = count.checked_mul(std::mem::size_of::<T>()).ok_or_else(|| {
        charge.fail_allocation();
        JournalError::Allocation
    })?;
    if !charge.charge(minimum) {
        return Err(JournalError::Budget);
    }
    values.try_reserve_exact(count).map_err(|_| {
        charge.fail_allocation();
        JournalError::Allocation
    })?;
    let actual = values
        .capacity()
        .checked_mul(std::mem::size_of::<T>())
        .ok_or_else(|| {
            charge.fail_allocation();
            JournalError::Allocation
        })?;
    if actual > minimum && !charge.charge(actual - minimum) {
        return Err(JournalError::Budget);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordType {
    ChildList,
    Attributes,
    CharacterData,
}

/// One semantic record. Lists and sibling references are captured before the
/// mutation changes the native tree; the backend materializes Node wrappers
/// lazily from these exact references.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticRecord {
    pub record_type: RecordType,
    pub target: ObserverNode,
    pub added_nodes: Vec<ObserverNode>,
    pub removed_nodes: Vec<ObserverNode>,
    pub previous_sibling: Option<ObserverNode>,
    pub next_sibling: Option<ObserverNode>,
    pub attribute_name: Option<Atom>,
    pub attribute_namespace: Option<Atom>,
    pub old_value: Option<String>,
}

impl SemanticRecord {
    /// The record's native node references, including duplicates. The pin
    /// owner deduplicates by exact generation and tracks logical owners.
    pub fn referenced_nodes(&self) -> impl Iterator<Item = ObserverNode> + '_ {
        std::iter::once(self.target)
            .chain(self.added_nodes.iter().copied())
            .chain(self.removed_nodes.iter().copied())
            .chain(self.previous_sibling)
            .chain(self.next_sibling)
    }

    /// The retained record payload after publication. This deliberately
    /// excludes the journal's shared queue/map capacity and the native pin
    /// ledger, which have separately lived owners and charges.
    pub fn retained_payload_bytes(&self) -> Result<usize, JournalError> {
        let nodes = self
            .added_nodes
            .capacity()
            .checked_add(self.removed_nodes.capacity())
            .and_then(|count| count.checked_mul(std::mem::size_of::<ObserverNode>()))
            .ok_or(JournalError::Allocation)?;
        std::mem::size_of::<Self>()
            .checked_add(nodes)
            .and_then(|bytes| {
                bytes.checked_add(self.old_value.as_ref().map_or(0, String::capacity))
            })
            .ok_or(JournalError::Allocation)
    }
}

/// A mutation-time snapshot provided by the Document adapter. `ancestors`
/// starts with `target` and proceeds towards its root. For a removal this is
/// the old chain, captured before detachment.
#[derive(Clone, Debug)]
pub struct MutationFacts {
    pub record_type: RecordType,
    pub target: ObserverNode,
    pub ancestors: Vec<ObserverNode>,
    pub added_nodes: Vec<ObserverNode>,
    pub removed_nodes: Vec<ObserverNode>,
    pub previous_sibling: Option<ObserverNode>,
    pub next_sibling: Option<ObserverNode>,
    pub attribute_name: Option<Atom>,
    pub attribute_namespace: Option<Atom>,
    pub old_value: Option<String>,
}

impl MutationFacts {
    pub fn child_list(
        target: ObserverNode,
        ancestors: Vec<ObserverNode>,
        added_nodes: Vec<ObserverNode>,
        removed_nodes: Vec<ObserverNode>,
        previous_sibling: Option<ObserverNode>,
        next_sibling: Option<ObserverNode>,
    ) -> Self {
        Self {
            record_type: RecordType::ChildList,
            target,
            ancestors,
            added_nodes,
            removed_nodes,
            previous_sibling,
            next_sibling,
            attribute_name: None,
            attribute_namespace: None,
            old_value: None,
        }
    }

    pub fn attributes(
        target: ObserverNode,
        ancestors: Vec<ObserverNode>,
        name: Atom,
        namespace: Option<Atom>,
        old_value: Option<String>,
    ) -> Self {
        Self {
            record_type: RecordType::Attributes,
            target,
            ancestors,
            added_nodes: Vec::new(),
            removed_nodes: Vec::new(),
            previous_sibling: None,
            next_sibling: None,
            attribute_name: Some(name),
            attribute_namespace: namespace,
            old_value,
        }
    }

    pub fn character_data(
        target: ObserverNode,
        ancestors: Vec<ObserverNode>,
        old_value: String,
    ) -> Self {
        Self {
            record_type: RecordType::CharacterData,
            target,
            ancestors,
            added_nodes: Vec::new(),
            removed_nodes: Vec::new(),
            previous_sibling: None,
            next_sibling: None,
            attribute_name: None,
            attribute_namespace: None,
            old_value: Some(old_value),
        }
    }
}

#[derive(Debug)]
struct Registration {
    id: u64,
    observer: ObserverId,
    options: ObserveOptions,
    /// The immediate registered observer copied by a removal. Nested
    /// transients must not be flattened to the original observed node.
    transient_source: Option<u64>,
    // Last field: the copied filter buffer drops before its page charge.
    filter_charge: NativeMemoryReservation,
}

impl Registration {
    fn interest(
        &self,
        record_type: RecordType,
        attribute_name: Option<Atom>,
        attribute_namespace: Option<Atom>,
        depth: usize,
    ) -> Option<bool> {
        if depth != 0 && !self.options.subtree {
            return None;
        }
        match record_type {
            RecordType::ChildList if self.options.child_list => Some(false),
            RecordType::Attributes if self.options.attributes => {
                if let Some(filter) = &self.options.attribute_filter
                    && (attribute_namespace.is_some()
                        || !attribute_name.is_some_and(|name| filter.contains(&name)))
                {
                    return None;
                }
                Some(self.options.attribute_old_value)
            }
            RecordType::CharacterData if self.options.character_data => {
                Some(self.options.character_data_old_value)
            }
            _ => None,
        }
    }
}

/// A fully owned mutation prepared before the DOM operation. The caller must
/// obtain its byte reservation and deduplicated node pins before mutation,
/// then call `commit` only after successful native mutation.
#[derive(Debug)]
pub struct PreparedMutation {
    pub(super) entries: Vec<(ObserverId, SemanticRecord)>,
    pub(super) reserved: bool,
    // The copied semantic payload and entry array stay charged until the
    // native operation commits, aborts, or transfers each record to a queued
    // owner. The queued owner acquires an independent charge first.
    pub(super) _charge: NativeMemoryReservation,
}

pub struct PreparedOwnedMutation {
    pub(super) entries: Vec<(ObserverId, OwnedObserverRecord)>,
    pub(super) reserved: bool,
    // Last field: entry storage drops before its independent page charge.
    pub(super) _entries_charge: NativeMemoryReservation,
}

/// Transient registrations prepared before a native removal. Preparation and
/// capacity reservation are fallible; installation and rollback allocate no
/// memory. The caller must roll back only if the tree removal did not occur.
#[derive(Debug)]
pub struct PreparedTransients {
    removed_root: ObserverNode,
    additions: Vec<Registration>,
    ids: Vec<TransientActivation>,
    reserved: bool,
    // The two buffers remain charged through native commit/rollback. Their
    // per-registration attribute filters own independent charges.
    _additions_charge: NativeMemoryReservation,
    _ids_charge: NativeMemoryReservation,
}

pub struct InstalledTransients {
    removed_root: ObserverNode,
    ids: Vec<TransientActivation>,
    _ids_charge: NativeMemoryReservation,
}

#[derive(Clone, Copy, Debug)]
struct TransientActivation {
    registration_id: u64,
    observer: ObserverId,
    newly_pending: bool,
}

impl InstalledTransients {
    pub fn removed_root(&self) -> ObserverNode {
        self.removed_root
    }

    pub fn activated_count(&self) -> usize {
        self.ids.len()
    }
}

impl PreparedMutation {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub fn records(&self) -> impl Iterator<Item = &SemanticRecord> {
        self.entries.iter().map(|(_, record)| record)
    }
}

impl PreparedTransients {
    pub fn removed_root(&self) -> ObserverNode {
        self.removed_root
    }
}

/// Native registration and record selection for one similar-origin agent.
/// The page backend supplies callback ownership and the FIFO notify job.
#[derive(Debug)]
pub struct ObserverJournal {
    registrations: HashMap<ObserverNode, ChargedDeque<Registration>>,
    records: HashMap<ObserverId, ChargedDeque<OwnedObserverRecord>>,
    pending: ChargedDeque<ObserverId>,
    pending_members: HashSet<ObserverId>,
    registration_count: usize,
    next_registration_id: u64,
    budget: NativeMemoryBudget,
    // Last fields: the three shared hash allocations die before their
    // high-water charges, including after removals leave retained capacity.
    registration_map_charge: tenta_core::NativeMemoryReservation,
    record_map_charge: tenta_core::NativeMemoryReservation,
    pending_members_charge: tenta_core::NativeMemoryReservation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalError {
    Allocation,
    Budget,
    ObserverGone,
}

impl From<CapacityError> for JournalError {
    fn from(error: CapacityError) -> Self {
        match error {
            CapacityError::Budget => Self::Budget,
            CapacityError::Allocation => Self::Allocation,
        }
    }
}

#[cfg(test)]
impl Default for ObserverJournal {
    fn default() -> Self {
        Self::new(NativeMemoryBudget::with_limits(
            usize::MAX,
            usize::MAX,
            usize::MAX,
            0,
        ))
    }
}

impl ObserverJournal {
    pub fn new(budget: NativeMemoryBudget) -> Self {
        let registration_map_charge =
            budget.begin(tenta_core::NativeMemoryComponent::MutationObserver);
        let record_map_charge = budget.begin(tenta_core::NativeMemoryComponent::MutationObserver);
        let pending_members_charge =
            budget.begin(tenta_core::NativeMemoryComponent::MutationObserver);
        Self {
            registrations: HashMap::new(),
            records: HashMap::new(),
            pending: ChargedDeque::new(budget.clone()),
            pending_members: HashSet::new(),
            registration_count: 0,
            next_registration_id: 0,
            budget,
            registration_map_charge,
            record_map_charge,
            pending_members_charge,
        }
    }

    fn next_registration_id(&mut self) -> u64 {
        let id = self.next_registration_id;
        self.next_registration_id = id
            .checked_add(1)
            .expect("observer registration id overflow");
        id
    }

    #[inline]
    pub fn has_registrations(&self) -> bool {
        self.registration_count != 0
    }

    pub(super) fn target_observers(&self) -> impl Iterator<Item = (ObserverNode, ObserverId)> + '_ {
        self.registrations
            .iter()
            .flat_map(|(target, registrations)| {
                registrations
                    .iter()
                    .map(move |entry| (*target, entry.observer))
            })
    }

    pub(super) fn pending_observers(&self) -> impl Iterator<Item = ObserverId> + '_ {
        self.pending.iter().copied()
    }

    /// Allocation-free precheck for the Document's fast path. The caller
    /// yields the target and then its native ancestors without materializing
    /// a vector or copying an old attribute/character-data value.
    pub fn may_observe_path(
        &self,
        path: impl IntoIterator<Item = ObserverNode>,
        record_type: RecordType,
        attribute_name: Option<Atom>,
        attribute_namespace: Option<Atom>,
    ) -> bool {
        if !self.has_registrations() {
            return false;
        }
        path.into_iter().enumerate().any(|(depth, node)| {
            self.registrations.get(&node).is_some_and(|registrations| {
                registrations.iter().any(|registration| {
                    registration
                        .interest(record_type, attribute_name, attribute_namespace, depth)
                        .is_some()
                })
            })
        })
    }

    /// Removing a node inherits subtree registrations from the old parent
    /// chain even when none of them requests a child-list record. The
    /// transient stays active on the removed subtree until notification.
    pub fn may_install_transients(
        &self,
        old_parent_path: impl IntoIterator<Item = ObserverNode>,
    ) -> bool {
        self.has_registrations()
            && old_parent_path.into_iter().any(|node| {
                self.registrations.get(&node).is_some_and(|registrations| {
                    registrations
                        .iter()
                        .any(|registration| registration.options.subtree)
                })
            })
    }

    pub fn observe(
        &mut self,
        observer: ObserverId,
        target: ObserverNode,
        options: ObserveOptions,
    ) -> Result<(), JournalError> {
        let matching = self.registrations.get(&target).map_or(0, |list| {
            list.iter()
                .filter(|entry| entry.observer == observer)
                .count()
        });
        let mut updates_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut updates = Vec::new();
        reserve_charged_vec(&mut updates, &mut updates_charge, matching)?;
        if matching != 0 {
            for registration in self
                .registrations
                .get(&target)
                .expect("matching target exists")
            {
                if registration.observer == observer {
                    updates.push((
                        registration.id,
                        Some(copy_options_charged(&options, &self.budget)?),
                    ));
                }
            }
        }
        let new_filter_charge = if matching == 0 {
            Some(charge_owned_filter(&options, &self.budget)?)
        } else {
            None
        };
        if !self.records.contains_key(&observer) {
            reserve_map(
                &mut self.records,
                &mut self.record_map_charge,
                &self.budget,
                1,
            )
            .map_err(JournalError::from)?;
        }
        if matching == 0 {
            self.registration_count
                .checked_add(1)
                .ok_or(JournalError::Allocation)?;
            self.next_registration_id
                .checked_add(1)
                .ok_or(JournalError::Allocation)?;
            if let Some(list) = self.registrations.get_mut(&target) {
                list.try_reserve(1).map_err(JournalError::from)?;
            } else {
                reserve_map(
                    &mut self.registrations,
                    &mut self.registration_map_charge,
                    &self.budget,
                    1,
                )
                .map_err(JournalError::from)?;
                let mut list = ChargedDeque::new(self.budget.clone());
                list.try_reserve(1).map_err(JournalError::from)?;
                self.registrations.insert(target, list);
            }
        }
        self.records
            .entry(observer)
            .or_insert_with(|| ChargedDeque::new(self.budget.clone()));
        if matching == 0 {
            let id = self.next_registration_id();
            self.registrations
                .get_mut(&target)
                .expect("target capacity was prepared")
                .push_back_prepared(Registration {
                    id,
                    observer,
                    options,
                    transient_source: None,
                    filter_charge: new_filter_charge.expect("new filter charge was prepared"),
                });
            self.registration_count += 1;
        } else {
            let list = self.registrations.get_mut(&target).expect("target exists");
            for (registration, (_, update)) in list
                .iter_mut()
                .filter(|entry| entry.observer == observer)
                .zip(updates.iter_mut())
            {
                let (mut options, mut filter_charge) =
                    update.take().expect("prepared options exist");
                std::mem::swap(&mut registration.options, &mut options);
                std::mem::swap(&mut registration.filter_charge, &mut filter_charge);
                *update = Some((options, filter_charge));
            }
            for (source, old) in updates {
                drop(old);
                self.remove_transients_from_source(observer, source);
            }
        }
        Ok(())
    }

    /// Copy subtree registrations from the old parent chain into an owned
    /// pre-removal transaction. No journal state changes on allocation error.
    pub fn prepare_transients(
        &self,
        removed_root: ObserverNode,
        old_ancestors: &[ObserverNode],
    ) -> Result<PreparedTransients, JournalError> {
        let count = old_ancestors.iter().try_fold(0usize, |count, ancestor| {
            let on_ancestor = self.registrations.get(ancestor).map_or(0, |registrations| {
                registrations
                    .iter()
                    .filter(|registration| registration.options.subtree)
                    .count()
            });
            count
                .checked_add(on_ancestor)
                .ok_or(JournalError::Allocation)
        })?;
        let mut additions_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut additions = Vec::new();
        reserve_charged_vec(&mut additions, &mut additions_charge, count)?;
        for ancestor in old_ancestors {
            if let Some(registrations) = self.registrations.get(ancestor) {
                for registration in registrations {
                    if registration.options.subtree {
                        let (options, filter_charge) =
                            copy_options_charged(&registration.options, &self.budget)?;
                        additions.push(Registration {
                            id: 0,
                            observer: registration.observer,
                            options,
                            transient_source: Some(registration.id),
                            filter_charge,
                        });
                    }
                }
            }
        }
        let mut ids_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut ids = Vec::new();
        reserve_charged_vec(&mut ids, &mut ids_charge, count)?;
        Ok(PreparedTransients {
            removed_root,
            additions,
            ids,
            reserved: false,
            _additions_charge: additions_charge,
            _ids_charge: ids_charge,
        })
    }

    /// Reserve all transient lists for one DOM operation cumulatively before
    /// any unlink. Existing-list capacity changes have no semantic effect;
    /// absent lists are built off-journal and inserted only after all fallible
    /// allocations complete.
    pub fn reserve_transients(
        &mut self,
        operations: &mut [PreparedTransients],
    ) -> Result<(), JournalError> {
        let total = operations.iter().try_fold(0usize, |sum, operation| {
            sum.checked_add(operation.additions.len())
                .ok_or(JournalError::Allocation)
        })?;
        self.registration_count
            .checked_add(total)
            .ok_or(JournalError::Allocation)?;
        self.next_registration_id
            .checked_add(u64::try_from(total).map_err(|_| JournalError::Allocation)?)
            .ok_or(JournalError::Allocation)?;
        let mut counts_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut counts = HashMap::<ObserverNode, usize>::new();
        reserve_map(&mut counts, &mut counts_charge, &self.budget, total)
            .map_err(JournalError::from)?;
        for operation in operations.iter() {
            if !operation.additions.is_empty() {
                let count = counts.entry(operation.removed_root).or_default();
                *count = count
                    .checked_add(operation.additions.len())
                    .ok_or(JournalError::Allocation)?;
            }
        }
        let mut absent_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut absent = HashMap::<ObserverNode, ChargedDeque<Registration>>::new();
        reserve_map(&mut absent, &mut absent_charge, &self.budget, counts.len())
            .map_err(JournalError::from)?;
        for (root, count) in counts {
            if let Some(list) = self.registrations.get_mut(&root) {
                list.try_reserve(count).map_err(JournalError::from)?;
            } else {
                let mut list = ChargedDeque::new(self.budget.clone());
                list.try_reserve(count).map_err(JournalError::from)?;
                absent.insert(root, list);
            }
        }
        reserve_map(
            &mut self.registrations,
            &mut self.registration_map_charge,
            &self.budget,
            absent.len(),
        )
        .map_err(JournalError::from)?;
        self.registrations.extend(absent);
        for operation in operations {
            operation.reserved = true;
        }
        Ok(())
    }

    /// Discard a reserved operation that failed before its first unlink.
    /// Capacity on preexisting lists remains reusable; unused new map keys
    /// do not accumulate after repeated failed native validations.
    pub fn discard_reserved_transients(&mut self, operations: &[PreparedTransients]) {
        for operation in operations {
            if self
                .registrations
                .get(&operation.removed_root)
                .is_some_and(|list| list.is_empty())
            {
                self.registrations.remove(&operation.removed_root);
            }
        }
    }

    /// Install prepared transients without allocation immediately before a
    /// native unlink. A failed operation with no unlink can roll this back.
    pub fn commit_transients(&mut self, mut operation: PreparedTransients) -> InstalledTransients {
        assert!(
            operation.reserved,
            "transient capacity must be reserved before unlink"
        );
        if operation.additions.is_empty() {
            return InstalledTransients {
                removed_root: operation.removed_root,
                ids: operation.ids,
                _ids_charge: operation._ids_charge,
            };
        }
        let list = self
            .registrations
            .get_mut(&operation.removed_root)
            .expect("transient target list was reserved");
        for mut registration in operation.additions.drain(..) {
            let id = self.next_registration_id;
            self.next_registration_id += 1;
            registration.id = id;
            let observer = registration.observer;
            list.push_back_prepared(registration);
            self.registration_count += 1;
            let newly_pending = self.pending_members.insert(observer);
            if newly_pending {
                self.pending.push_back_prepared(observer);
            }
            operation.ids.push(TransientActivation {
                registration_id: id,
                observer,
                newly_pending,
            });
        }
        InstalledTransients {
            removed_root: operation.removed_root,
            ids: operation.ids,
            _ids_charge: operation._ids_charge,
        }
    }

    /// Roll back only when the corresponding native removal did not occur.
    /// Once unlinked, these registrations must survive until notify instead.
    pub fn rollback_transients(&mut self, installed: InstalledTransients) {
        if let Some(list) = self.registrations.get_mut(&installed.removed_root) {
            let before = list.len();
            list.retain(|entry| {
                !installed
                    .ids
                    .iter()
                    .any(|activation| activation.registration_id == entry.id)
            });
            self.registration_count -= before - list.len();
            if list.is_empty() {
                self.registrations.remove(&installed.removed_root);
            }
        }
        for activation in installed.ids {
            if activation.newly_pending {
                self.pending_members.remove(&activation.observer);
                self.pending
                    .retain(|observer| *observer != activation.observer);
            }
        }
    }

    #[cfg(test)]
    pub fn install_transients(
        &mut self,
        removed_root: ObserverNode,
        old_ancestors: &[ObserverNode],
    ) {
        let mut prepared = self
            .prepare_transients(removed_root, old_ancestors)
            .unwrap();
        self.reserve_for_operation(&mut [], std::slice::from_ref(&prepared))
            .unwrap();
        self.reserve_transients(std::slice::from_mut(&mut prepared))
            .unwrap();
        self.commit_transients(prepared);
    }

    /// Select interested observers in target-to-root/node-list order. A
    /// later matching registration can promote oldValue for the same observer.
    pub fn prepare(&self, facts: &MutationFacts) -> Result<PreparedMutation, JournalError> {
        if !self.has_registrations() {
            return Ok(PreparedMutation {
                entries: Vec::new(),
                reserved: true,
                _charge: self.budget.begin(NativeMemoryComponent::MutationObserver),
            });
        }
        debug_assert_eq!(facts.ancestors.first(), Some(&facts.target));
        let on_path = facts.ancestors.iter().try_fold(0usize, |count, ancestor| {
            let registrations = self
                .registrations
                .get(ancestor)
                .map_or(0, |list| list.len());
            count
                .checked_add(registrations)
                .ok_or(JournalError::Allocation)
        })?;
        let mut interested_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut interested = Vec::<(ObserverId, bool)>::new();
        let mut indices_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut indices = HashMap::<ObserverId, usize>::new();
        reserve_charged_vec(&mut interested, &mut interested_charge, on_path)?;
        reserve_map(&mut indices, &mut indices_charge, &self.budget, on_path)
            .map_err(JournalError::from)?;
        for (depth, node) in facts.ancestors.iter().enumerate() {
            let Some(registrations) = self.registrations.get(node) else {
                continue;
            };
            for registration in registrations {
                let Some(old_value) = registration.interest(
                    facts.record_type,
                    facts.attribute_name,
                    facts.attribute_namespace,
                    depth,
                ) else {
                    continue;
                };
                if let Some(&index) = indices.get(&registration.observer) {
                    interested[index].1 |= old_value;
                } else {
                    indices.insert(registration.observer, interested.len());
                    interested.push((registration.observer, old_value));
                }
            }
        }
        let mut charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut entries = Vec::new();
        reserve_charged_vec(&mut entries, &mut charge, interested.len())?;
        for (observer, wants_old_value) in interested {
            let minimum = facts
                .added_nodes
                .len()
                .checked_add(facts.removed_nodes.len())
                .and_then(|count| count.checked_mul(std::mem::size_of::<ObserverNode>()))
                .and_then(|bytes| {
                    bytes.checked_add(if wants_old_value {
                        facts.old_value.as_ref().map_or(0, String::len)
                    } else {
                        0
                    })
                })
                .ok_or_else(|| {
                    charge.fail_allocation();
                    JournalError::Allocation
                })?;
            if !charge.charge(minimum) {
                return Err(JournalError::Budget);
            }
            let record = SemanticRecord {
                record_type: facts.record_type,
                target: facts.target,
                added_nodes: copy_nodes(&facts.added_nodes).inspect_err(|_| {
                    charge.fail_allocation();
                })?,
                removed_nodes: copy_nodes(&facts.removed_nodes).inspect_err(|_| {
                    charge.fail_allocation();
                })?,
                previous_sibling: facts.previous_sibling,
                next_sibling: facts.next_sibling,
                attribute_name: facts.attribute_name,
                attribute_namespace: facts.attribute_namespace,
                old_value: if wants_old_value {
                    copy_old_value(facts.old_value.as_deref()).inspect_err(|_| {
                        charge.fail_allocation();
                    })?
                } else {
                    None
                },
            };
            let actual = record
                .retained_payload_bytes()
                .inspect_err(|_| {
                    charge.fail_allocation();
                })?
                .checked_sub(std::mem::size_of::<SemanticRecord>())
                .ok_or_else(|| {
                    charge.fail_allocation();
                    JournalError::Allocation
                })?;
            if actual > minimum && !charge.charge(actual - minimum) {
                return Err(JournalError::Budget);
            }
            entries.push((observer, record));
        }
        Ok(PreparedMutation {
            entries,
            reserved: false,
            _charge: charge,
        })
    }

    /// Reserve an entire semantic DOM operation, not one record at a time.
    /// Replace and fragment operations can queue several records for the same
    /// observer while its queue length is unchanged during preflight.
    #[cfg(test)]
    pub fn reserve_for_commits(
        &mut self,
        mutations: &mut [PreparedMutation],
    ) -> Result<(), JournalError> {
        self.reserve_for_operation(mutations, &[])
    }

    /// Reserve the union of record and transient activations before a DOM
    /// operation starts. Reserving these independently would undercount when
    /// both first activate different observers in one composite operation.
    pub fn reserve_for_operation(
        &mut self,
        mutations: &mut [PreparedMutation],
        transients: &[PreparedTransients],
    ) -> Result<(), JournalError> {
        let record_total = mutations.iter().try_fold(0usize, |total, mutation| {
            total
                .checked_add(mutation.entries.len())
                .ok_or(JournalError::Allocation)
        })?;
        let transient_total = transients.iter().try_fold(0usize, |total, transient| {
            total
                .checked_add(transient.additions.len())
                .ok_or(JournalError::Allocation)
        })?;
        let total = record_total
            .checked_add(transient_total)
            .ok_or(JournalError::Allocation)?;
        let mut counts_charge = self.budget.begin(NativeMemoryComponent::MutationObserver);
        let mut counts = HashMap::<ObserverId, usize>::new();
        reserve_map(&mut counts, &mut counts_charge, &self.budget, total)
            .map_err(JournalError::from)?;
        for mutation in mutations.iter() {
            for (observer, _) in &mutation.entries {
                let count = counts.entry(*observer).or_default();
                *count = count.checked_add(1).ok_or(JournalError::Allocation)?;
            }
        }
        for transient in transients {
            for registration in &transient.additions {
                counts.entry(registration.observer).or_default();
            }
        }
        let newly_pending = counts
            .keys()
            .filter(|observer| !self.pending_members.contains(*observer))
            .count();
        self.pending
            .try_reserve(newly_pending)
            .map_err(JournalError::from)?;
        reserve_set(
            &mut self.pending_members,
            &mut self.pending_members_charge,
            &self.budget,
            newly_pending,
        )
        .map_err(JournalError::from)?;
        for (observer, count) in counts {
            let records = self
                .records
                .get_mut(&observer)
                .ok_or(JournalError::ObserverGone)?;
            if count != 0 {
                records.try_reserve(count).map_err(JournalError::from)?;
            }
        }
        for mutation in mutations {
            mutation.reserved = true;
        }
        Ok(())
    }

    /// Publish a prepared mutation in pending-set insertion order. Native
    /// mutation code must reserve the queues and own all referenced nodes
    /// before changing the tree; this method deliberately contains no JS call.
    pub fn commit(&mut self, mutation: PreparedOwnedMutation) {
        assert!(
            mutation.reserved,
            "observer record capacity must be reserved before mutation"
        );
        for (observer, record) in mutation.entries {
            self.records
                .get_mut(&observer)
                .expect("prepared observer must remain registered during native mutation")
                .push_back_prepared(record);
            if self.pending_members.insert(observer) {
                self.pending.push_back_prepared(observer);
            }
        }
    }

    /// Clone and clear the ordered pending set at notify start. The caller
    /// drains each observer only immediately before invoking that callback.
    pub fn begin_notify(&mut self) -> ChargedDeque<ObserverId> {
        self.pending_members.clear();
        self.pending.take()
    }

    /// Drain one observer just before its callback. Transients disappear even
    /// when `take_records` had already emptied the record queue.
    pub fn drain_for_notify(&mut self, observer: ObserverId) -> ChargedDeque<OwnedObserverRecord> {
        self.remove_transients_for_observer(observer);
        self.take_records(observer)
    }

    /// Read-only preflight for a registration-bound publication batch. The
    /// caller holds the agent lock and takes the queue only after every
    /// original Document registration has been retained and charged.
    pub(super) fn queued_records(
        &self,
        observer: ObserverId,
    ) -> Option<&ChargedDeque<OwnedObserverRecord>> {
        self.records.get(&observer)
    }

    pub fn take_records(&mut self, observer: ObserverId) -> ChargedDeque<OwnedObserverRecord> {
        self.records
            .get_mut(&observer)
            .map(ChargedDeque::take)
            .unwrap_or_else(|| ChargedDeque::new(self.budget.clone()))
    }

    pub fn queued_count(&self, observer: ObserverId) -> usize {
        self.records
            .get(&observer)
            .map_or(0, |records| records.len())
    }

    pub fn disconnect(&mut self, observer: ObserverId) -> ChargedDeque<OwnedObserverRecord> {
        self.remove_registrations_for_observer(observer);
        self.take_records(observer)
    }

    /// Called when the exact host observer dies, after callback ownership is
    /// released. Pending ordered-set entries are harmless and notify empty.
    pub fn forget_observer(&mut self, observer: ObserverId) -> ChargedDeque<OwnedObserverRecord> {
        let records = self.disconnect(observer);
        self.records.remove(&observer);
        records
    }

    /// Page teardown releases each queued owner without running callbacks.
    /// A failed native release leaves that exact owner at the front for the
    /// sticky-poison teardown path instead of dropping its pin/charge metadata.
    #[cfg(test)]
    pub fn release_queued_without_delivery(
        &mut self,
        mut release: impl FnMut(&mut OwnedObserverRecord) -> tenta_core::Result<()>,
    ) -> tenta_core::Result<()> {
        for records in self.records.values_mut() {
            while let Some(record) = records.front_mut() {
                release(record)?;
                records.pop_front();
            }
        }
        self.pending.clear();
        self.pending_members.clear();
        Ok(())
    }

    #[cfg(test)]
    pub fn forget_freed_node(&mut self, node: ObserverNode) {
        if let Some(list) = self.registrations.remove(&node) {
            self.registration_count -= list.len();
        }
        // A transient registration on a surviving removed subtree remains
        // active until notify, re-observe, or disconnect even if its original
        // observed ancestor is collected in the meantime. Its source is an
        // exact identity for later removal, not a liveness requirement.
    }

    fn remove_transients_from_source(&mut self, observer: ObserverId, source: u64) {
        for list in self.registrations.values_mut() {
            let before = list.len();
            list.retain(|entry| {
                !(entry.observer == observer && entry.transient_source == Some(source))
            });
            self.registration_count -= before - list.len();
        }
        self.registrations.retain(|_, list| !list.is_empty());
    }

    fn remove_transients_for_observer(&mut self, observer: ObserverId) {
        for list in self.registrations.values_mut() {
            let before = list.len();
            list.retain(|entry| !(entry.observer == observer && entry.transient_source.is_some()));
            self.registration_count -= before - list.len();
        }
        self.registrations.retain(|_, list| !list.is_empty());
    }

    fn remove_registrations_for_observer(&mut self, observer: ObserverId) {
        for list in self.registrations.values_mut() {
            let before = list.len();
            list.retain(|entry| entry.observer != observer);
            self.registration_count -= before - list.len();
        }
        self.registrations.retain(|_, list| !list.is_empty());
    }
}

#[cfg(test)]
#[path = "observer_journal/tests.rs"]
mod tests;
