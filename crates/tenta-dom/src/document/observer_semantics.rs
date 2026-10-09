//! Mutation-time semantic facts for the native observer journal.
//!
//! Capture runs after Document mutation validation and before changing links,
//! attributes, or character data. It does not consume the diagnostic sink's
//! low-level records. Replace-all captures one grouped parent record before effects.

use std::mem::size_of;
use tenta_core::atom::atoms;

use tenta_core::{
    Error, NativeMemoryBudget, NativeMemoryComponent, NativeMemoryReservation, NodeId, Result,
};

use crate::mutation::{Mutation, ReplaceAllWith};
use crate::{Document, NodeKind};

use super::observer_journal::{MutationFacts, ObserverJournal, ObserverNode, RecordType};
use super::observer_ownership::budget_error;

/// Test only the native target-to-root registrations before allocating an
/// ancestry snapshot or cloning an attribute/character-data old value. The
/// caller has already validated the mutation. Diagnostic sinks are separate
/// and continue receiving their existing low-level records.
pub fn may_observe_mutation(
    document: &Document,
    journal: &ObserverJournal,
    mutation: &Mutation,
) -> bool {
    let interested = |target, kind, name, namespace| {
        let path = std::iter::successors(Some(target), |current| {
            let parent = document.parent(*current);
            parent.is_some().then_some(parent)
        })
        .map(|node| key(document, node));
        journal.may_observe_path(path, kind, name, namespace)
    };
    let removal_interested = |old_parent| {
        let path = std::iter::successors(Some(old_parent), |current| {
            let parent = document.parent(*current);
            parent.is_some().then_some(parent)
        })
        .map(|node| key(document, node));
        interested(old_parent, RecordType::ChildList, None, None)
            || journal.may_install_transients(path)
    };
    match mutation {
        Mutation::Append { parent, child } | Mutation::InsertBefore { parent, child, .. } => {
            if document.kind(*child) == NodeKind::Fragment {
                document.children(*child).next().is_some()
                    && (removal_interested(*child)
                        || interested(*parent, RecordType::ChildList, None, None))
            } else {
                let old_parent = document.parent(*child);
                (old_parent.is_some() && removal_interested(old_parent))
                    || interested(*parent, RecordType::ChildList, None, None)
            }
        }
        Mutation::Remove { child } => {
            let old_parent = document.parent(*child);
            old_parent.is_some() && removal_interested(old_parent)
        }
        Mutation::ReplaceChild {
            parent, new_child, ..
        } => {
            let source = if document.kind(*new_child) == NodeKind::Fragment {
                document
                    .children(*new_child)
                    .next()
                    .is_some()
                    .then_some(*new_child)
            } else {
                let old_parent = document.parent(*new_child);
                old_parent.is_some().then_some(old_parent)
            };
            source.is_some_and(&removal_interested) || removal_interested(*parent)
        }
        Mutation::ReplaceAll {
            parent,
            replacement,
        } => {
            let removed = document.children(*parent).next().is_some();
            let added = match replacement {
                ReplaceAllWith::Empty => false,
                ReplaceAllWith::Text(_) => true,
                ReplaceAllWith::ChildrenOf(source) => document.children(*source).next().is_some(),
            };
            (removed && removal_interested(*parent))
                || (added && interested(*parent, RecordType::ChildList, None, None))
                || (added
                    && match replacement {
                        ReplaceAllWith::ChildrenOf(source) => removal_interested(*source),
                        _ => false,
                    })
        }
        Mutation::AddMissingAttributes {
            element,
            attributes,
        } => attributes.iter().enumerate().any(|(index, attribute)| {
            document.parser_attribute_is_missing(*element, attributes, index)
                && interested(
                    *element,
                    RecordType::Attributes,
                    Some(attribute.name.local),
                    (attribute.name.ns != atoms::EMPTY).then_some(attribute.name.ns),
                )
        }),
        Mutation::SetAttribute { element, name, .. }
        | Mutation::ReplaceAttribute { element, name, .. } => interested(
            *element,
            RecordType::Attributes,
            Some(name.local),
            (name.ns != atoms::EMPTY).then_some(name.ns),
        ),
        Mutation::RemoveAttribute { element, name } => {
            document
                .attribute_ns(*element, name.ns, name.local)
                .is_some()
                && interested(
                    *element,
                    RecordType::Attributes,
                    Some(name.local),
                    (name.ns != atoms::EMPTY).then_some(name.ns),
                )
        }
        Mutation::SetText { node, .. } | Mutation::ReplaceData { node, .. } => {
            interested(*node, RecordType::CharacterData, None, None)
        }
        Mutation::AppendTextData { node, text } => {
            !text.is_empty() && interested(*node, RecordType::CharacterData, None, None)
        }
        Mutation::ReparentChildren { from, to } => {
            document.children(*from).next().is_some()
                && (removal_interested(*from) || interested(*to, RecordType::ChildList, None, None))
        }
    }
}

/// One removed subtree whose old ancestor registrations become transient on
/// its detached root before the native links change.
#[derive(Clone, Debug)]
pub struct TransientRemoval {
    pub removed_root: ObserverNode,
    pub old_parent_ancestors: Vec<ObserverNode>,
}

/// Native effect order, independent of the separately prepared record and
/// transient buffers. The Document adapter advances each step only at its
/// matching unlink or DOM queue-record seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticStep {
    InstallTransient(ObserverNode),
    QueueRecord(ObserverNode),
}

#[derive(Debug, Default)]
pub struct SemanticPlan {
    /// In the DOM-defined queueing order, independent of diagnostic messages.
    pub records: Vec<MutationFacts>,
    pub transient_removals: Vec<TransientRemoval>,
    pub steps: Vec<SemanticStep>,
    /// A semantic queue request is independent of matching registrations.
    /// A validated mutation can request an empty notification job.
    pub request_notification: bool,
}

/// The semantic fact buffers stay charged until both observer preparation and
/// the native mutation have finished. The reservation is deliberately last.
#[derive(Debug)]
pub struct CapturedSemanticPlan {
    plan: SemanticPlan,
    _charge: NativeMemoryReservation,
}

impl CapturedSemanticPlan {
    pub fn plan(&self) -> &SemanticPlan {
        &self.plan
    }
}

struct CaptureBudget {
    budget: Option<NativeMemoryBudget>,
    charge: Option<NativeMemoryReservation>,
}

impl CaptureBudget {
    #[cfg(test)]
    fn unmetered() -> Self {
        Self {
            budget: None,
            charge: None,
        }
    }

    fn metered(budget: &NativeMemoryBudget) -> Self {
        Self {
            budget: Some(budget.clone()),
            charge: Some(budget.begin(NativeMemoryComponent::MutationObserver)),
        }
    }

    fn allocation_error(&mut self) -> Error {
        if let (Some(budget), Some(charge)) = (&self.budget, &mut self.charge) {
            charge.fail_allocation();
            budget_error(budget)
        } else {
            Error::invalid("observer semantic capture allocation failed")
        }
    }

    fn reserve<T>(&mut self, values: &mut Vec<T>, additional: usize) -> Result<()> {
        let needed = values
            .len()
            .checked_add(additional)
            .ok_or_else(|| self.allocation_error())?;
        if needed <= values.capacity() {
            return Ok(());
        }
        let old_capacity = values.capacity();
        let preferred = old_capacity.saturating_mul(2).max(4).max(needed);
        let mut requested = preferred;
        if self.charge.is_some() {
            let preferred_bytes = preferred.checked_mul(size_of::<T>());
            let optional = preferred > needed
                && preferred_bytes.is_some_and(|bytes| {
                    self.charge
                        .as_mut()
                        .expect("metered capture has a reservation")
                        .try_charge_optional(bytes)
                });
            if !optional {
                requested = needed;
                let Some(bytes) = requested.checked_mul(size_of::<T>()) else {
                    return Err(self.allocation_error());
                };
                if !self
                    .charge
                    .as_mut()
                    .expect("metered capture has a reservation")
                    .charge(bytes)
                {
                    return Err(budget_error(
                        self.budget.as_ref().expect("metered capture has a budget"),
                    ));
                }
            }
        }
        if values.try_reserve_exact(requested - values.len()).is_err() {
            return Err(self.allocation_error());
        }
        if self.charge.is_some() {
            let Some(actual) = values.capacity().checked_mul(size_of::<T>()) else {
                return Err(self.allocation_error());
            };
            let Some(charged) = requested.checked_mul(size_of::<T>()) else {
                return Err(self.allocation_error());
            };
            let budget = self.budget.as_ref().expect("metered capture has a budget");
            let charge = self
                .charge
                .as_mut()
                .expect("metered capture has a reservation");
            if actual > charged && !charge.charge(actual - charged) {
                return Err(budget_error(budget));
            }
        }
        Ok(())
    }

    fn owned_string(&mut self, value: &str) -> Result<String> {
        let mut copy = String::new();
        if let (Some(budget), Some(charge)) = (&self.budget, &mut self.charge)
            && !charge.charge(value.len())
        {
            return Err(budget_error(budget));
        }
        copy.try_reserve_exact(value.len())
            .map_err(|_| self.allocation_error())?;
        if let (Some(budget), Some(charge)) = (&self.budget, &mut self.charge) {
            let extra = copy.capacity().saturating_sub(value.len());
            if extra != 0 && !charge.charge(extra) {
                return Err(budget_error(budget));
            }
        }
        copy.push_str(value);
        Ok(copy)
    }
}

impl SemanticPlan {
    /// Allocation-free fast path for a validated operation when no observer
    /// record or transient registration needs a native snapshot.
    pub fn notification_only(request_notification: bool) -> Self {
        Self {
            records: Vec::new(),
            transient_removals: Vec::new(),
            steps: Vec::new(),
            request_notification,
        }
    }

    fn push_record(&mut self, record: MutationFacts, capture: &mut CaptureBudget) -> Result<()> {
        capture.reserve(&mut self.records, 1)?;
        capture.reserve(&mut self.steps, 1)?;
        self.steps.push(SemanticStep::QueueRecord(record.target));
        self.records.push(record);
        Ok(())
    }

    fn push_transient(
        &mut self,
        removal: TransientRemoval,
        capture: &mut CaptureBudget,
    ) -> Result<()> {
        capture.reserve(&mut self.transient_removals, 1)?;
        capture.reserve(&mut self.steps, 1)?;
        self.steps
            .push(SemanticStep::InstallTransient(removal.removed_root));
        self.transient_removals.push(removal);
        Ok(())
    }
}

/// Whether the validated operation queues a DOM mutation record, even if no
/// registration selects it. The caller must finish validation before asking
/// and commit this request only after the native effect succeeds.
pub fn requests_notification(document: &Document, mutation: &Mutation) -> bool {
    match mutation {
        Mutation::Append { child, .. } | Mutation::InsertBefore { child, .. } => {
            document.kind(*child) != NodeKind::Fragment
                || document.children(*child).next().is_some()
        }
        Mutation::Remove { child } => document.parent(*child).is_some(),
        Mutation::ReplaceChild { .. }
        | Mutation::SetAttribute { .. }
        | Mutation::ReplaceAttribute { .. }
        | Mutation::SetText { .. }
        | Mutation::ReplaceData { .. } => true,
        Mutation::ReplaceAll {
            parent,
            replacement,
        } => {
            document.children(*parent).next().is_some()
                || match replacement {
                    ReplaceAllWith::Empty => false,
                    ReplaceAllWith::Text(_) => true,
                    ReplaceAllWith::ChildrenOf(source) => {
                        document.children(*source).next().is_some()
                    }
                }
        }
        Mutation::AppendTextData { text, .. } => !text.is_empty(),
        Mutation::AddMissingAttributes {
            element,
            attributes,
        } => attributes
            .iter()
            .enumerate()
            .any(|(index, _)| document.parser_attribute_is_missing(*element, attributes, index)),
        Mutation::RemoveAttribute { element, name } => document
            .attribute_ns(*element, name.ns, name.local)
            .is_some(),
        Mutation::ReparentChildren { from, .. } => document.children(*from).next().is_some(),
    }
}

fn one<T>(value: T, capture: &mut CaptureBudget) -> Result<Vec<T>> {
    let mut values = Vec::new();
    capture.reserve(&mut values, 1)?;
    values.push(value);
    Ok(values)
}

fn copy_nodes(nodes: &[ObserverNode], capture: &mut CaptureBudget) -> Result<Vec<ObserverNode>> {
    let mut copy = Vec::new();
    capture.reserve(&mut copy, nodes.len())?;
    copy.extend_from_slice(nodes);
    Ok(copy)
}

fn child_nodes(
    document: &Document,
    node: NodeId,
    capture: &mut CaptureBudget,
) -> Result<Vec<NodeId>> {
    let mut children = Vec::new();
    capture.reserve(&mut children, document.children(node).count())?;
    for child in document.children(node) {
        children.push(child);
    }
    Ok(children)
}

fn keys(
    document: &Document,
    nodes: &[NodeId],
    capture: &mut CaptureBudget,
) -> Result<Vec<ObserverNode>> {
    let mut result = Vec::new();
    capture.reserve(&mut result, nodes.len())?;
    result.extend(nodes.iter().map(|node| key(document, *node)));
    Ok(result)
}

fn key(document: &Document, node: NodeId) -> ObserverNode {
    ObserverNode::new(document.document_id(), document.node_ref(node))
}

fn optional_key(document: &Document, node: NodeId) -> Option<ObserverNode> {
    node.is_some().then(|| key(document, node))
}

fn ancestry(
    document: &Document,
    target: NodeId,
    capture: &mut CaptureBudget,
) -> Result<Vec<ObserverNode>> {
    let mut result = Vec::new();
    let mut count = 0usize;
    let mut current = target;
    while current.is_some() {
        count = count
            .checked_add(1)
            .ok_or_else(|| capture.allocation_error())?;
        current = document.parent(current);
    }
    capture.reserve(&mut result, count)?;
    let mut current = target;
    while current.is_some() {
        result.push(key(document, current));
        current = document.parent(current);
    }
    Ok(result)
}

fn removal(
    document: &Document,
    node: NodeId,
    plan: &mut SemanticPlan,
    capture: &mut CaptureBudget,
) -> Result<()> {
    let parent = document.parent(node);
    if !parent.is_some() {
        return Ok(());
    }
    let old_ancestors = ancestry(document, parent, capture)?;
    plan.push_transient(
        TransientRemoval {
            removed_root: key(document, node),
            old_parent_ancestors: copy_nodes(&old_ancestors, capture)?,
        },
        capture,
    )?;
    plan.push_record(
        MutationFacts::child_list(
            key(document, parent),
            old_ancestors,
            Vec::new(),
            one(key(document, node), capture)?,
            optional_key(document, document.previous_sibling(node)),
            optional_key(document, document.next_sibling(node)),
        ),
        capture,
    )
}

/// Capture a validated public mutation. The caller must not let page code run
/// between validation, this snapshot, pin/byte preflight, and native mutation.
/// `ReparentChildren` records each native unlink/link pair for the parser's
/// reparenting primitive. Replace-all must use its own semantic hook instead
/// of this primitive's per-child records.
#[cfg(test)]
pub fn capture_validated(document: &Document, mutation: &Mutation) -> Result<SemanticPlan> {
    capture_validated_inner(document, mutation, &mut CaptureBudget::unmetered())
}

/// Capture the same semantic facts while charging their buffers to the page
/// before any DOM effect. The returned owner must outlive native preparation
/// and the mutation; dropping it releases the fact-buffer charge last.
pub fn capture_validated_charged(
    document: &Document,
    mutation: &Mutation,
    budget: &NativeMemoryBudget,
) -> Result<CapturedSemanticPlan> {
    if budget.first_failure().is_some() {
        return Err(budget_error(budget));
    }
    let mut capture = CaptureBudget::metered(budget);
    let plan = capture_validated_inner(document, mutation, &mut capture)?;
    Ok(CapturedSemanticPlan {
        plan,
        _charge: capture.charge.take().expect("metered capture has a charge"),
    })
}

fn capture_validated_inner(
    document: &Document,
    mutation: &Mutation,
    capture: &mut CaptureBudget,
) -> Result<SemanticPlan> {
    let default_plan = SemanticPlan::default();
    let mut plan = SemanticPlan {
        request_notification: requests_notification(document, mutation),
        ..default_plan
    };
    match mutation {
        Mutation::Append { parent, child } => {
            plan_insert(document, *parent, *child, NodeId::NONE, &mut plan, capture)?;
        }
        Mutation::InsertBefore {
            parent,
            child,
            before,
        } => plan_insert(document, *parent, *child, *before, &mut plan, capture)?,
        Mutation::Remove { child } => removal(document, *child, &mut plan, capture)?,
        Mutation::ReplaceChild {
            parent,
            new_child,
            old_child,
        } => plan_replace(
            document, *parent, *new_child, *old_child, &mut plan, capture,
        )?,
        Mutation::ReplaceAll {
            parent,
            replacement,
        } => {
            plan_replace_all(document, *parent, *replacement, &mut plan, capture)?;
        }
        Mutation::SetAttribute { element, name, .. }
        | Mutation::ReplaceAttribute { element, name, .. } => {
            plan.push_record(
                MutationFacts::attributes(
                    key(document, *element),
                    ancestry(document, *element, capture)?,
                    name.local,
                    (name.ns != atoms::EMPTY).then_some(name.ns),
                    document
                        .attribute_ns(*element, name.ns, name.local)
                        .map(|value| capture.owned_string(value))
                        .transpose()?,
                ),
                capture,
            )?;
        }
        Mutation::AddMissingAttributes {
            element,
            attributes,
        } => {
            for (index, attribute) in attributes.iter().enumerate() {
                if !document.parser_attribute_is_missing(*element, attributes, index) {
                    continue;
                }
                plan.push_record(
                    MutationFacts::attributes(
                        key(document, *element),
                        ancestry(document, *element, capture)?,
                        attribute.name.local,
                        (attribute.name.ns != atoms::EMPTY).then_some(attribute.name.ns),
                        None,
                    ),
                    capture,
                )?;
            }
        }
        Mutation::RemoveAttribute { element, name } => {
            if let Some(old_value) = document.attribute_ns(*element, name.ns, name.local) {
                plan.push_record(
                    MutationFacts::attributes(
                        key(document, *element),
                        ancestry(document, *element, capture)?,
                        name.local,
                        (name.ns != atoms::EMPTY).then_some(name.ns),
                        Some(capture.owned_string(old_value)?),
                    ),
                    capture,
                )?;
            }
        }
        Mutation::SetText { node, .. } | Mutation::ReplaceData { node, .. } => {
            let old_value = document.text_data(*node).ok_or_else(|| {
                Error::invalid("observer character-data capture requires a validated node")
            })?;
            plan.push_record(
                MutationFacts::character_data(
                    key(document, *node),
                    ancestry(document, *node, capture)?,
                    capture.owned_string(old_value)?,
                ),
                capture,
            )?;
        }
        Mutation::AppendTextData { node, text } if !text.is_empty() => {
            let old_value = document.text_data(*node).ok_or_else(|| {
                Error::invalid("observer character-data capture requires a validated node")
            })?;
            plan.push_record(
                MutationFacts::character_data(
                    key(document, *node),
                    ancestry(document, *node, capture)?,
                    capture.owned_string(old_value)?,
                ),
                capture,
            )?;
        }
        Mutation::AppendTextData { .. } => {}
        Mutation::ReparentChildren { from, to } => {
            plan_reparent_children(document, *from, *to, &mut plan, capture)?;
        }
    }
    Ok(plan)
}

fn plan_replace_all(
    document: &Document,
    parent: NodeId,
    replacement: ReplaceAllWith,
    plan: &mut SemanticPlan,
    capture: &mut CaptureBudget,
) -> Result<()> {
    let removed = child_nodes(document, parent, capture)?;
    let added = match replacement {
        ReplaceAllWith::Empty => Vec::new(),
        ReplaceAllWith::Text(node) => one(node, capture)?,
        ReplaceAllWith::ChildrenOf(source) => child_nodes(document, source, capture)?,
    };
    if removed.is_empty() && added.is_empty() {
        return Ok(());
    }
    let parent_ancestors = ancestry(document, parent, capture)?;
    for child in &removed {
        plan.push_transient(
            TransientRemoval {
                removed_root: key(document, *child),
                old_parent_ancestors: copy_nodes(&parent_ancestors, capture)?,
            },
            capture,
        )?;
    }
    if let ReplaceAllWith::ChildrenOf(source) = replacement
        && !added.is_empty()
    {
        let source_ancestors = ancestry(document, source, capture)?;
        for child in &added {
            plan.push_transient(
                TransientRemoval {
                    removed_root: key(document, *child),
                    old_parent_ancestors: copy_nodes(&source_ancestors, capture)?,
                },
                capture,
            )?;
        }
        // Fragment insertion queues its own removal record even when the
        // insertion into the destination suppresses individual records. A
        // parser root is private, so this prepared record has no JS receiver.
        plan.push_record(
            MutationFacts::child_list(
                key(document, source),
                source_ancestors,
                Vec::new(),
                keys(document, &added, capture)?,
                None,
                None,
            ),
            capture,
        )?;
    }
    plan.push_record(
        MutationFacts::child_list(
            key(document, parent),
            parent_ancestors,
            keys(document, &added, capture)?,
            keys(document, &removed, capture)?,
            None,
            None,
        ),
        capture,
    )
}

fn plan_reparent_children(
    document: &Document,
    from: NodeId,
    to: NodeId,
    plan: &mut SemanticPlan,
    capture: &mut CaptureBudget,
) -> Result<()> {
    let children = child_nodes(document, from, capture)?;
    if children.is_empty() {
        return Ok(());
    }
    let source_ancestors = ancestry(document, from, capture)?;
    let destination_ancestors = ancestry(document, to, capture)?;
    let mut destination_previous = document.last_child(to);
    for (index, child) in children.iter().copied().enumerate() {
        let next = children.get(index + 1).copied().unwrap_or_else(|| {
            if from == to && children.len() > 1 {
                children[0]
            } else {
                NodeId::NONE
            }
        });
        plan.push_transient(
            TransientRemoval {
                removed_root: key(document, child),
                old_parent_ancestors: copy_nodes(&source_ancestors, capture)?,
            },
            capture,
        )?;
        plan.push_record(
            MutationFacts::child_list(
                key(document, from),
                copy_nodes(&source_ancestors, capture)?,
                Vec::new(),
                one(key(document, child), capture)?,
                None,
                optional_key(document, next),
            ),
            capture,
        )?;
        let previous = if from == to && index == 0 && destination_previous == child {
            NodeId::NONE
        } else {
            destination_previous
        };
        plan.push_record(
            MutationFacts::child_list(
                key(document, to),
                copy_nodes(&destination_ancestors, capture)?,
                one(key(document, child), capture)?,
                Vec::new(),
                optional_key(document, previous),
                None,
            ),
            capture,
        )?;
        destination_previous = child;
    }
    Ok(())
}

fn plan_insert(
    document: &Document,
    parent: NodeId,
    child: NodeId,
    before: NodeId,
    plan: &mut SemanticPlan,
    capture: &mut CaptureBudget,
) -> Result<()> {
    let reference_child = if before == child {
        document.next_sibling(child)
    } else {
        before
    };
    let is_fragment = document.kind(child) == NodeKind::Fragment;
    let added: Vec<NodeId> = if is_fragment {
        child_nodes(document, child, capture)?
    } else {
        one(child, capture)?
    };
    if added.is_empty() {
        return Ok(());
    }
    if is_fragment {
        for node in &added {
            plan.push_transient(
                TransientRemoval {
                    removed_root: key(document, *node),
                    old_parent_ancestors: ancestry(document, child, capture)?,
                },
                capture,
            )?;
        }
        // DOM insert removes the fragment's children first, then queues one
        // fragment record even when the parent insertion is suppressed.
        plan.push_record(
            MutationFacts::child_list(
                key(document, child),
                ancestry(document, child, capture)?,
                Vec::new(),
                keys(document, &added, capture)?,
                None,
                None,
            ),
            capture,
        )?;
    } else {
        removal(document, child, plan, capture)?;
    }
    let previous = if reference_child.is_some() {
        document.previous_sibling(reference_child)
    } else {
        document.last_child(parent)
    };
    // DOM insert captures previousSibling before adopting the moved node.
    // It can therefore be the node being moved itself.
    plan.push_record(
        MutationFacts::child_list(
            key(document, parent),
            ancestry(document, parent, capture)?,
            keys(document, &added, capture)?,
            Vec::new(),
            optional_key(document, previous),
            optional_key(document, reference_child),
        ),
        capture,
    )
}

fn plan_replace(
    document: &Document,
    parent: NodeId,
    new_child: NodeId,
    old_child: NodeId,
    plan: &mut SemanticPlan,
    capture: &mut CaptureBudget,
) -> Result<()> {
    let previous = document.previous_sibling(old_child);
    let mut next = document.next_sibling(old_child);
    if next == new_child {
        next = document.next_sibling(new_child);
    }
    let is_fragment = document.kind(new_child) == NodeKind::Fragment;
    let added: Vec<NodeId> = if is_fragment {
        child_nodes(document, new_child, capture)?
    } else {
        one(new_child, capture)?
    };
    if is_fragment {
        // The native replacement removes the old child before inserting the
        // fragment's children. The fragment record is queued separately, but
        // transient installation must follow actual unlink order.
        if new_child != old_child {
            plan.push_transient(
                TransientRemoval {
                    removed_root: key(document, old_child),
                    old_parent_ancestors: ancestry(document, parent, capture)?,
                },
                capture,
            )?;
        }
        if !added.is_empty() {
            for node in &added {
                plan.push_transient(
                    TransientRemoval {
                        removed_root: key(document, *node),
                        old_parent_ancestors: ancestry(document, new_child, capture)?,
                    },
                    capture,
                )?;
            }
            plan.push_record(
                MutationFacts::child_list(
                    key(document, new_child),
                    ancestry(document, new_child, capture)?,
                    Vec::new(),
                    keys(document, &added, capture)?,
                    None,
                    None,
                ),
                capture,
            )?;
        }
    } else {
        // Adoption removes an already-parented replacement first. When the
        // replacement is the old child itself, the final combined record's
        // removedNodes is empty, matching DOM replace step 7.
        removal(document, new_child, plan, capture)?;
        if new_child != old_child {
            plan.push_transient(
                TransientRemoval {
                    removed_root: key(document, old_child),
                    old_parent_ancestors: ancestry(document, parent, capture)?,
                },
                capture,
            )?;
        }
    }
    plan.push_record(
        MutationFacts::child_list(
            key(document, parent),
            ancestry(document, parent, capture)?,
            keys(document, &added, capture)?,
            if new_child != old_child {
                one(key(document, old_child), capture)?
            } else {
                Vec::new()
            },
            optional_key(document, previous),
            optional_key(document, next),
        ),
        capture,
    )
}

#[cfg(test)]
#[path = "observer_semantics/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "observer_semantics/charged_tests.rs"]
mod charged_tests;
