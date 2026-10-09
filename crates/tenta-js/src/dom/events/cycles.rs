//! Native node/standalone-target wrappers, listeners, and Event.target ownership.
//!
//! Engines supply the actual ephemeron graph and garbage collection. This plan
//! records candidate registrations, lifetime roots, and directed target edges;
//! it never creates wrappers, weakens a native handle, or sweeps the arena.

use std::collections::{HashMap, HashSet};

use tenta_core::{NodeId, NodeRef};
use tenta_dom::Document;

use super::{HostKey, ListenerId, TargetKey};
use crate::bindings::AnyValue;
use crate::bindings::host::{HostRecord, SharedContext, TAG_EVENT, TAG_HOST, TAG_NODE, decode};

struct Registration {
    context: SharedContext,
    target: TargetKey,
    id: ListenerId,
    callback: u64,
}

/// A node lifetime subtree or standalone target represented by wrappers/listeners.
pub struct Group {
    /// Stored callback identities, including each candidate registration's owned reference.
    pub callbacks: Vec<u64>,
    /// Already materialized wrappers sharing this native lifetime.
    pub wrappers: Vec<u64>,
    /// A connected tree, native NodeHandle or foreign-world wrapper retains
    /// this group independently of the collecting engine's JS heap.
    pub native_root: bool,
    /// Native claims across all worlds. Compared with this plan's wrappers
    /// only after all local node-anchored wrappers have joined their groups.
    wrapper_claims: usize,
}

impl Group {
    fn independently_rooted(&self) -> bool {
        self.native_root || self.wrapper_claims > self.wrappers.len()
    }
}

/// A materialized event can expose one target's native lifetime group.
/// The edge is one-way: a target does not retain its dispatched events.
pub struct EventTargetEdge {
    /// Existing event wrapper handle, including its slab generation.
    pub event: u64,
    /// Index in [`Plan::groups`] containing the live target.
    pub target_group: usize,
}

/// One materialized Event's owned stored-value edge. The value is held by
/// exactly one EventRecord reference, even if other records intern the same JS
/// object. The backend decides whether the value can be weakened.
pub struct EventValueEdge {
    /// Existing event wrapper handle, including its slab generation.
    pub event: u64,
    /// Existing stored-value slot with one reference owned by this Event.
    pub value: u64,
}

/// A snapshot built at a native safe point without active host borrows.
///
/// Node subtrees and standalone EventTargets form groups. Materialized events
/// retain their target group through a directed edge. Only groups without
/// native lifetime roots participate in weakening. Other host targets and JS-mediated pins
/// (for example a Range or snapshot) need additional ownership information.
pub struct Plan {
    /// Lifetime groups, including rooted wrapper groups with no listeners.
    pub groups: Vec<Group>,
    /// Directed native Event.target edges between existing wrappers.
    pub event_target_edges: Vec<EventTargetEdge>,
    /// Event-owned CustomEvent.detail / MouseEvent.relatedTarget values.
    pub event_value_edges: Vec<EventValueEdge>,
    /// Number of candidate registrations or EventRecords owning each stored value.
    pub references: HashMap<u64, usize>,
    roots: HashMap<(u64, NodeRef), usize>,
    registrations: Vec<Registration>,
}

impl Plan {
    /// Snapshots node-anchoring wrappers, listener registrations, and target
    /// edges of materialized Event wrappers. Native pins are checked once per
    /// lifetime root, including groups with no listeners.
    pub fn with_wrappers(context: &SharedContext, wrappers: &[u64]) -> Self {
        let mut plan = Self {
            groups: Vec::new(),
            event_target_edges: Vec::new(),
            event_value_edges: Vec::new(),
            references: HashMap::new(),
            roots: HashMap::new(),
            registrations: Vec::new(),
        };
        let mut by_context: HashMap<u16, Vec<u64>> = HashMap::new();
        let mut event_wrappers: HashMap<u16, Vec<u64>> = HashMap::new();
        let mut host_wrappers: HashMap<u16, Vec<u64>> = HashMap::new();
        let mut host_groups: HashMap<(u16, HostKey), usize> = HashMap::new();
        let mut event_targets = Vec::new();
        let mut contexts = HashMap::new();
        for &bits in wrappers {
            let decoded = decode(bits);
            if decoded.node_anchor().is_some() {
                by_context.entry(decoded.slot).or_default().push(bits);
            } else if decoded.tag == TAG_EVENT {
                event_wrappers.entry(decoded.slot).or_default().push(bits);
            } else if decoded.tag == TAG_HOST {
                host_wrappers.entry(decoded.slot).or_default().push(bits);
            }
        }
        let all_contexts = context.documents.all();
        // Aliases of one Document use different slots in the same engine.
        // Gather all of their wrappers before classifying any listener;
        // a later alias must not look like a foreign engine's root.
        for context in &all_contexts {
            let Some(wrappers) = by_context.remove(&context.slot) else {
                continue;
            };
            let doc = context.doc.borrow();
            for bits in wrappers {
                let decoded = decode(bits);
                let Some(node) = decoded.node_anchor() else {
                    continue;
                };
                if !doc.is_live(node)
                    || doc.node_ref(node).generation != u32::from(decoded.generation)
                {
                    continue;
                }
                let group = plan.group_for_node(&doc, node);
                plan.groups[group].wrappers.push(bits);
            }
        }
        for context in all_contexts {
            let doc = context.doc.borrow();
            for bits in host_wrappers.remove(&context.slot).unwrap_or_default() {
                let key = decode(bits).host_key();
                if !matches!(
                    context.host_records.borrow().get(key),
                    Some(HostRecord::EventTarget)
                ) {
                    continue;
                }
                let group = plan.group_for_host(&mut host_groups, context.slot, key);
                plan.groups[group].wrappers.push(bits);
            }
            let listeners = context.listeners.borrow();
            for (&target, entry) in &listeners.targets {
                if entry.listeners.is_empty() {
                    continue;
                }
                let group = match target {
                    TargetKey::Node(node) if doc.resolve(node).is_ok() => {
                        plan.group_for_node(&doc, node.id)
                    }
                    TargetKey::Host(key)
                        if matches!(
                            context.host_records.borrow().get(key),
                            Some(HostRecord::EventTarget)
                        ) =>
                    {
                        plan.group_for_host(&mut host_groups, context.slot, key)
                    }
                    _ => continue,
                };
                if plan.groups[group].independently_rooted() {
                    continue;
                }
                for listener in &entry.listeners {
                    let bits = listener.callback.0;
                    plan.groups[group].callbacks.push(bits);
                    *plan.references.entry(bits).or_default() += 1;
                    plan.registrations.push(Registration {
                        context: context.clone(),
                        target,
                        id: listener.id,
                        callback: bits,
                    });
                }
            }
            drop(listeners);
            drop(doc);
            {
                let events = context.events.borrow();
                for bits in event_wrappers.remove(&context.slot).unwrap_or_default() {
                    let Some(record) = events.get(decode(bits).event_key()) else {
                        continue;
                    };
                    if let Some(target) = record.target {
                        event_targets.push((bits, target));
                    }
                    if let Some(value) = record.kind.owned_stored_value()
                        && value != AnyValue::NULL.0
                    {
                        *plan.references.entry(value).or_default() += 1;
                        plan.event_value_edges
                            .push(EventValueEdge { event: bits, value });
                    }
                }
            }
            contexts.insert(context.slot, context);
        }
        // Target handles can belong to a different document. Read them only
        // after every per-context borrow above has ended.
        for (event, target) in event_targets {
            let decoded = decode(target);
            let Some(context) = contexts.get(&decoded.slot).cloned() else {
                continue;
            };
            let target_group = match decoded.tag {
                TAG_NODE => {
                    let doc = context.doc.borrow();
                    if !doc.is_live(decoded.node_id)
                        || doc.node_ref(decoded.node_id).generation != u32::from(decoded.generation)
                    {
                        continue;
                    }
                    plan.group_for_node(&doc, decoded.node_id)
                }
                TAG_HOST
                    if matches!(
                        context.host_records.borrow().get(decoded.host_key()),
                        Some(HostRecord::EventTarget)
                    ) =>
                {
                    plan.group_for_host(&mut host_groups, context.slot, decoded.host_key())
                }
                _ => continue,
            };
            plan.event_target_edges.push(EventTargetEdge {
                event,
                target_group,
            });
        }
        for group in &mut plan.groups {
            // Independent engines cannot trace one another's JS heaps. Keep
            // the whole native lifetime and its per-world state while another
            // engine owns a wrapper; cross-engine cycles remain conservative.
            group.native_root = group.independently_rooted();
        }
        plan
    }

    fn group_for_host(
        &mut self,
        groups: &mut HashMap<(u16, HostKey), usize>,
        slot: u16,
        key: HostKey,
    ) -> usize {
        *groups.entry((slot, key)).or_insert_with(|| {
            let index = self.groups.len();
            self.groups.push(Group {
                callbacks: Vec::new(),
                wrappers: Vec::new(),
                native_root: false,
                wrapper_claims: 0,
            });
            index
        })
    }

    fn group_for_node(&mut self, doc: &Document, node: NodeId) -> usize {
        // Connected flags already identify the document lifetime root. Avoid
        // walking every ancestor for each wrapper in a deep connected tree.
        let root = if doc.is_connected(node) {
            doc.document_node()
        } else {
            doc.lifetime_root(node)
        };
        let key = (doc.document_id(), doc.node_ref(root));
        if let Some(&index) = self.roots.get(&key) {
            return index;
        }
        let (native_root, wrapper_claims) = if doc.is_connected(root) {
            (true, 0)
        } else {
            doc.subtree_retention(root)
        };
        let index = self.groups.len();
        self.groups.push(Group {
            callbacks: Vec::new(),
            wrappers: Vec::new(),
            native_root,
            wrapper_claims,
        });
        self.roots.insert(key, index);
        index
    }

    /// Removes exact collected registrations before their stored-value slots
    /// may be released or reused. No script runs during this operation.
    /// Returns one owned reference for each removed listener.
    pub fn drain_collected(&self, dead: &HashSet<u64>) -> Vec<u64> {
        let mut released = Vec::with_capacity(self.registrations.len());
        for registration in &self.registrations {
            if !dead.contains(&registration.callback) {
                continue;
            }
            let context = &registration.context;
            let removed = context
                .listeners
                .borrow_mut()
                .remove_by_id_at(registration.target, registration.id);
            assert_eq!(
                removed.map(|callback| callback.0),
                Some(registration.callback),
                "listener ownership changed during cycle collection"
            );
            context
                .handlers
                .borrow_mut()
                .retain(|(target, _), id| *target != registration.target || *id != registration.id);
            released.push(registration.callback);
        }
        released
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tenta_core::atom::atoms;

    use crate::bindings::host::{HostContext, encode_node};
    use crate::dom::events::CallbackRef;
    use crate::event_loop::{Clock, MonoTime, SharedClock};

    #[test]
    fn same_world_aliases_are_local_and_only_foreign_claims_root_their_listeners() {
        let document = Rc::new(RefCell::new(Document::new()));
        let root = HostContext::new(
            document.clone(),
            SharedClock::new(Clock::virtual_at(MonoTime::ZERO)),
            tenta_net::CookieJar::new(),
            crate::storage::PageStorage::isolated(),
        );
        let alias = root.new_child(document.clone()).unwrap();
        let node = {
            let mut doc = document.borrow_mut();
            let id = doc.create_html_element(atoms::DIV);
            let node = doc.node_ref(id);
            doc.retain_wrapper(node).unwrap();
            doc.retain_wrapper(node).unwrap();
            node
        };
        let wrappers = [encode_node(root.slot, node), encode_node(alias.slot, node)];
        for (context, callback) in [(&root, 7), (&alias, 9)] {
            context
                .listeners
                .borrow_mut()
                .add_at(
                    TargetKey::Node(node),
                    "probe".to_owned(),
                    CallbackRef(callback),
                    false,
                    false,
                    false,
                )
                .unwrap();
        }
        let local = Plan::with_wrappers(&root, &wrappers);
        assert_eq!(local.groups.len(), 1);
        assert_eq!(local.groups[0].wrappers.len(), 2);
        assert!(!local.groups[0].native_root);
        assert_eq!(local.references.get(&7), Some(&1));
        assert_eq!(local.references.get(&9), Some(&1));

        document.borrow_mut().retain_wrapper(node).unwrap();
        let foreign = Plan::with_wrappers(&root, &wrappers);
        assert!(foreign.groups[0].native_root);
        assert!(foreign.references.is_empty());
        document.borrow_mut().release_wrapper(node);
        let collectible = Plan::with_wrappers(&root, &wrappers);
        assert!(!collectible.groups[0].native_root);
        let mut released = collectible.drain_collected(&HashSet::from([7, 9]));
        released.sort_unstable();
        assert_eq!(released, [7, 9]);
        document.borrow_mut().release_wrapper(node);
        document.borrow_mut().release_wrapper(node);
        assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 1);
        root.documents.clear_minted();
    }
}
