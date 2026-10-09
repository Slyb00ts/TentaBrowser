//! Native reachability among already materialized constructed-CSSOM wrappers.
//!
//! This is metadata for the engines' existing wrapper collection transaction.
//! It never creates a HostRecord or a JavaScript wrapper. A component contains
//! objects that can re-expose each other through CSSOM getters; target edges
//! preserve only the direction of other reachable objects.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use tenta_css::CssRuleHandle;

use super::{CssSheetState, HostObject, HostRecord, SharedContext, TAG_HOST, decode};

/// Whether a host must receive a provisional CSS graph root at wrapper
/// publication. Inspect the native variant, not an interface-name spelling:
/// a future CSS subclass cannot silently escape admission this way.
pub fn is_css_host(host: &HostObject) -> bool {
    let HostObject::Host { ctx, key } = host else {
        return false;
    };
    matches!(
        ctx.host_records.borrow().get(*key),
        Some(
            HostRecord::CssSheet(_)
                | HostRecord::CssRuleList(_)
                | HostRecord::CssStyleRule(_)
                | HostRecord::CssRuleStyle(_)
        )
    )
}

/// One strongly reachable set of existing wrapper handles.
pub struct CssWrapperGroup {
    pub wrappers: Vec<u64>,
    /// Indices in [`CssWrapperGraph::groups`] reachable in one direction.
    pub targets: Vec<usize>,
}

/// CSS ownership snapshot for the same GC transaction as native node groups.
pub struct CssWrapperGraph {
    pub groups: Vec<CssWrapperGroup>,
}

enum RecordKind {
    Sheet,
    List(Option<CssRuleHandle>),
    Rule(CssRuleHandle),
    Style(CssRuleHandle),
}

struct Record {
    bits: u64,
    kind: RecordKind,
}

struct SheetRecords {
    state: Rc<CssSheetState>,
    records: Vec<Record>,
}

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
enum Component {
    /// The sheet and its current rule tree. Each current rule has a sheet
    /// link, and the sheet can reach each rule through cssRules.
    Current,
    /// A removed rule and descendants still in its own child list.
    Detached(usize),
    /// An empty list has no item from which to navigate to its native owner.
    EmptyList(u64),
}

#[derive(Default)]
struct WorkingGroup {
    wrappers: Vec<u64>,
    targets: HashSet<usize>,
    reaches_sheet: bool,
}

struct Builder {
    indices: HashMap<Component, usize>,
    groups: Vec<WorkingGroup>,
}

impl Builder {
    fn new() -> Self {
        Self {
            indices: HashMap::new(),
            groups: Vec::new(),
        }
    }

    fn group(&mut self, component: Component) -> usize {
        if let Some(&index) = self.indices.get(&component) {
            return index;
        }
        let index = self.groups.len();
        self.groups.push(WorkingGroup::default());
        self.indices.insert(component, index);
        index
    }

    fn finish(self) -> Vec<CssWrapperGroup> {
        // A private native component can be an intermediate getter result
        // without having any current JS wrapper. Project through it, rather
        // than letting the absence of its wrapper erase a reachable expando.
        let mut published = Vec::new();
        let mut remap = vec![None; self.groups.len()];
        for (old, group) in self.groups.iter().enumerate() {
            if !group.wrappers.is_empty() {
                remap[old] = Some(published.len());
                published.push(CssWrapperGroup {
                    wrappers: group.wrappers.clone(),
                    targets: Vec::new(),
                });
            }
        }
        for (old, source) in self.groups.iter().enumerate() {
            let Some(source_index) = remap[old] else {
                continue;
            };
            let mut pending: Vec<usize> = source.targets.iter().copied().collect();
            let mut seen = HashSet::new();
            while let Some(target) = pending.pop() {
                if !seen.insert(target) || target == old {
                    continue;
                }
                if let Some(index) = remap[target] {
                    published[source_index].targets.push(index);
                } else {
                    pending.extend(self.groups[target].targets.iter().copied());
                }
            }
            published[source_index].targets.sort_unstable();
            published[source_index].targets.dedup();
        }
        published
    }
}

fn component(rule: &CssRuleHandle, top_rules: &HashSet<usize>) -> Component {
    let root = rule.graph_component_identity();
    if top_rules.contains(&root) {
        Component::Current
    } else {
        Component::Detached(root)
    }
}

/// Read existing generational records into a directed CSS ownership plan.
///
/// `wrapper_bits` must contain only wrappers with provisional engine roots;
/// a weak tombstone is not a candidate for ownership transfer. A borrow
/// conflict fails the checkpoint before any root is weakened.
pub fn snapshot_wrapper_graph(
    root: &SharedContext,
    wrapper_bits: &[u64],
) -> Option<CssWrapperGraph> {
    let contexts: HashMap<_, _> = root
        .documents
        .all()
        .into_iter()
        .map(|context| (context.slot, context))
        .collect();
    let mut sheets: HashMap<usize, SheetRecords> = HashMap::new();
    for &bits in wrapper_bits {
        let decoded = decode(bits);
        if decoded.tag != TAG_HOST {
            continue;
        }
        let Some(context) = contexts.get(&decoded.slot) else {
            continue;
        };
        let records = context.host_records.try_borrow().ok()?;
        let (state, kind) = match records.get(decoded.host_key()) {
            Some(HostRecord::CssSheet(state)) => (Rc::clone(state), RecordKind::Sheet),
            Some(HostRecord::CssRuleList(list)) => (
                Rc::clone(&list.sheet),
                RecordKind::List(list.parent.clone()),
            ),
            Some(HostRecord::CssStyleRule(rule)) => {
                (Rc::clone(&rule.sheet), RecordKind::Rule(rule.rule.clone()))
            }
            Some(HostRecord::CssRuleStyle(rule)) => {
                (Rc::clone(&rule.sheet), RecordKind::Style(rule.rule.clone()))
            }
            _ => continue,
        };
        drop(records);
        let id = Rc::as_ptr(&state) as usize;
        sheets
            .entry(id)
            .or_insert_with(|| SheetRecords {
                state,
                records: Vec::new(),
            })
            .records
            .push(Record { bits, kind });
    }

    let mut result = CssWrapperGraph { groups: Vec::new() };
    for sheet in sheets.into_values() {
        let top_rules: HashSet<_> = sheet
            .state
            .sheet
            .try_borrow()
            .ok()?
            .graph_top_rule_identities()
            .into_iter()
            .collect();
        let mut builder = Builder::new();
        let current = builder.group(Component::Current);
        let mut checked_sheet_links = HashSet::new();
        for record in sheet.records {
            let group = match record.kind {
                RecordKind::Sheet => current,
                RecordKind::List(None) if top_rules.is_empty() => {
                    let empty = builder.group(Component::EmptyList(record.bits));
                    builder.groups[current].targets.insert(empty);
                    empty
                }
                RecordKind::List(None) => current,
                RecordKind::List(Some(parent)) if parent.child_count() == 0 => {
                    let owner = builder.group(component(&parent, &top_rules));
                    let empty = builder.group(Component::EmptyList(record.bits));
                    builder.groups[owner].targets.insert(empty);
                    empty
                }
                RecordKind::List(Some(parent)) => {
                    let owner = builder.group(component(&parent, &top_rules));
                    if owner != current && checked_sheet_links.insert(owner) {
                        builder.groups[owner].reaches_sheet = parent
                            .graph_component_root()
                            .graph_reaches_parent_style_sheet();
                    }
                    owner
                }
                RecordKind::Rule(rule) | RecordKind::Style(rule) => {
                    let owner = builder.group(component(&rule, &top_rules));
                    if owner != current && checked_sheet_links.insert(owner) {
                        builder.groups[owner].reaches_sheet = rule
                            .graph_component_root()
                            .graph_reaches_parent_style_sheet();
                    }
                    owner
                }
            };
            builder.groups[group].wrappers.push(record.bits);
        }
        for index in 0..builder.groups.len() {
            if index != current && builder.groups[index].reaches_sheet {
                builder.groups[index].targets.insert(current);
            }
        }
        let base = result.groups.len();
        let groups = builder.finish();
        for mut group in groups {
            for target in &mut group.targets {
                *target += base;
            }
            result.groups.push(group);
        }
    }
    Some(result)
}
