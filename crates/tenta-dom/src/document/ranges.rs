//! Live boundary points participate in the document's mutation and lifetime
//! paths, independently of the JavaScript engine that owns the range.

use hashbrown::HashMap;
use tenta_core::{NodeId, NodeRef, Result};

use super::{Document, NodeHandle, utf16_len};
use crate::NodeKind;
use crate::error::{index_size, invalid_state};

/// A generational node reference and a UTF-16 or child-list offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeBoundary {
    pub node: NodeRef,
    pub offset: u32,
}

/// A document-specific live-range registration. Keys are never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LiveRangeId {
    document: u64,
    serial: u64,
}

struct LiveRange {
    points: [RangeBoundary; 2],
    handles: [NodeHandle; 2],
}

#[derive(Default)]
pub(super) struct LiveRanges {
    entries: HashMap<LiveRangeId, LiveRange>,
    next: u64,
}

impl LiveRanges {
    pub(super) fn memory_bytes(&self) -> usize {
        self.entries.capacity() * (size_of::<LiveRangeId>() + size_of::<LiveRange>() + 1)
    }
}

impl Document {
    /// Registers a range's endpoints and retains their detached subtrees.
    /// The caller establishes boundary order; both points must share a root.
    /// Release the registration with [`Self::release_live_range`].
    pub fn register_live_range(
        &mut self,
        start: RangeBoundary,
        end: RangeBoundary,
    ) -> Result<LiveRangeId> {
        for point in [start, end] {
            let node = self.resolve(point.node)?;
            let length = if self.kind(node).is_character_data_subclass() {
                utf16_len(self.text_data(node).unwrap_or_default())
            } else {
                self.children(node).count() as u32
            };
            if self.kind(node) == NodeKind::DocType || point.offset > length {
                return Err(index_size("invalid live range boundary"));
            }
        }
        if self.root_of(start.node.id) != self.root_of(end.node.id) {
            return Err(invalid_state("live range endpoints have different roots"));
        }
        let next = self
            .live_ranges
            .next
            .checked_add(1)
            .ok_or_else(|| invalid_state("live range registrations exhausted"))?;
        let first = self.handle(start.node.id)?;
        let second = match self.handle(end.node.id) {
            Ok(handle) => handle,
            Err(error) => {
                self.release(first);
                return Err(error);
            }
        };
        let id = LiveRangeId {
            document: self.document_id,
            serial: next,
        };
        self.live_ranges.next = next;
        self.live_ranges.entries.insert(
            id,
            LiveRange {
                points: [start, end],
                handles: [first, second],
            },
        );
        Ok(id)
    }

    /// Reads the boundary points after every preceding DOM mutation.
    pub fn live_range_points(&self, id: LiveRangeId) -> Option<[RangeBoundary; 2]> {
        self.live_ranges.entries.get(&id).map(|range| range.points)
    }

    /// Releases a registration and its node handles, once only.
    pub fn release_live_range(&mut self, id: LiveRangeId) {
        if let Some(range) = self.live_ranges.entries.remove(&id) {
            for handle in range.handles {
                self.release(handle);
            }
        }
    }

    /// Checks all removals of a compound move before its first tree edit.
    /// A boundary can move only to an ancestor, so counting the original
    /// descendants for each removal bounds the handles its parent will need.
    /// The exclusive document borrow prevents new registrations between this
    /// check and the removals. Individual removal still acquires its handles
    /// transactionally before editing any points.
    pub(super) fn preflight_range_removals(
        &self,
        children: impl IntoIterator<Item = NodeId>,
    ) -> Result<()> {
        if self.live_ranges.entries.is_empty() {
            return Ok(());
        }
        let mut required: HashMap<NodeId, usize> = HashMap::new();
        for child in children {
            let parent = self.parent(child);
            if !parent.is_some() {
                continue;
            }
            for range in self.live_ranges.entries.values() {
                for point in range.points {
                    if self.is_inclusive_ancestor(child, point.node.id) {
                        let count = required.entry(parent).or_default();
                        *count += 1;
                        if *count > usize::from(u16::MAX - self.handle_count(parent)) {
                            return Err(invalid_state(
                                "live range fixup exceeds node handle capacity",
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn adjust_ranges_for_insertion(&mut self, parent: NodeId, before: NodeId) {
        // Appending cannot displace a valid boundary: no offset exceeds the
        // parent's old length. Documents with no ranges do no extra traversal.
        if self.live_ranges.entries.is_empty() || !before.is_some() {
            return;
        }
        let index = self
            .children(parent)
            .position(|node| node == before)
            .unwrap_or(0) as u32;
        for range in self.live_ranges.entries.values_mut() {
            for point in &mut range.points {
                if point.node.id == parent && point.offset > index {
                    point.offset += 1;
                }
            }
        }
    }

    pub(super) fn adjust_ranges_for_removal(
        &mut self,
        child: NodeId,
        parent: NodeId,
    ) -> Result<()> {
        if self.live_ranges.entries.is_empty() {
            return Ok(());
        }
        let index = self
            .children(parent)
            .position(|node| node == child)
            .unwrap_or(0) as u32;
        let parent_ref = self.node_ref(parent);
        // Acquire every replacement handle before changing any boundary. A
        // handle-capacity error must leave both the tree and all ranges intact.
        let affected: Vec<_> = self
            .live_ranges
            .entries
            .iter()
            .flat_map(|(id, range)| {
                range.points.iter().enumerate().filter_map(|(side, point)| {
                    self.is_inclusive_ancestor(child, point.node.id)
                        .then_some((*id, side))
                })
            })
            .collect();
        let mut replacements = Vec::with_capacity(affected.len());
        for (id, side) in affected {
            match self.handle(parent) {
                Ok(handle) => replacements.push((id, side, handle)),
                Err(error) => {
                    for (_, _, handle) in replacements {
                        self.release(handle);
                    }
                    return Err(error);
                }
            }
        }
        for (id, side, handle) in replacements {
            let range = self
                .live_ranges
                .entries
                .get_mut(&id)
                .expect("registered range");
            range.points[side] = RangeBoundary {
                node: parent_ref,
                offset: index,
            };
            let previous = std::mem::replace(&mut range.handles[side], handle);
            self.release(previous);
        }
        for range in self.live_ranges.entries.values_mut() {
            for point in &mut range.points {
                if point.node.id == parent && point.offset > index {
                    point.offset -= 1;
                }
            }
        }
        Ok(())
    }

    pub(super) fn adjust_ranges_for_data(
        &mut self,
        node: NodeId,
        offset: u32,
        count: u32,
        inserted: u32,
    ) {
        for range in self.live_ranges.entries.values_mut() {
            for point in &mut range.points {
                if point.node.id == node && point.offset > offset {
                    point.offset = if point.offset <= offset + count {
                        offset
                    } else {
                        point.offset - count + inserted
                    };
                }
            }
        }
    }
}

impl Document {
    pub(super) fn release_all_live_ranges(&mut self) {
        for (_, range) in std::mem::take(&mut self.live_ranges.entries) {
            for handle in range.handles {
                self.release(handle);
            }
        }
    }
}
