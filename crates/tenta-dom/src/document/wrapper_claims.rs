//! Generation-checked JavaScript wrapper ownership, separate from Rust handles.
//!
//! Like the DOM arena and its other lifetime indexes, this sparse table is
//! reported by Document::memory_report, outside the partial native page ledger.
//! Reserve fallibly before changing ownership; a backend must treat allocation
//! failure as fatal and must never publish a wrapper without its claim.

use tenta_core::{NodeId, NodeRef};

use super::{Document, NodeFlags};

/// A wrapper claim could not be acquired. Existing claims remain unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapperClaimError {
    /// The arena slot was freed or reused before publication.
    Stale,
    /// The exact count or the sparse table's allocation could not grow.
    Capacity,
}

impl Document {
    /// Acquires one claim for a newly published wrapper in any world. Node,
    /// NodeList, HTMLCollection and other node-anchored wrappers each own one
    /// claim. A cache hit owns no additional claim. Every acquired claim must
    /// be released exactly once after finalization or world teardown.
    pub fn retain_wrapper(&mut self, node: NodeRef) -> Result<(), WrapperClaimError> {
        let id = self.resolve(node).map_err(|_| WrapperClaimError::Stale)?;
        let slot = self.index(id);
        if !self.arena.nodes.flags[slot].contains(NodeFlags::HAS_WRAPPER) {
            self.arena.nodes.flags[slot].insert(NodeFlags::HAS_WRAPPER);
            self.invalidate_keep(id);
            return Ok(());
        }
        let extra = self.extra_wrapper_claims.get(&id).copied().unwrap_or(0);
        // Include the first claim in the overflow check before publishing any
        // change. No count is saturated or silently discarded.
        extra.checked_add(2).ok_or(WrapperClaimError::Capacity)?;
        if extra == 0 {
            self.extra_wrapper_claims
                .try_reserve(1)
                .map_err(|_| WrapperClaimError::Capacity)?;
        }
        self.extra_wrapper_claims.insert(id, extra + 1);
        Ok(())
    }

    /// Releases one owned claim. Stale generations and unclaimed nodes are
    /// ignored; a late finalizer must never affect a recycled arena slot.
    /// Returns whether a claim was released.
    pub fn release_wrapper(&mut self, node: NodeRef) -> bool {
        let Ok(id) = self.resolve(node) else {
            return false;
        };
        let slot = self.index(id);
        if !self.arena.nodes.flags[slot].contains(NodeFlags::HAS_WRAPPER) {
            return false;
        }
        if let Some(extra) = self.extra_wrapper_claims.get_mut(&id) {
            if *extra > 1 {
                *extra -= 1;
            } else {
                self.extra_wrapper_claims.remove(&id);
                if self.extra_wrapper_claims.is_empty() {
                    // Release the retained allocation after the last multiply
                    // wrapped node returns to its single-claim fast path.
                    self.extra_wrapper_claims = Default::default();
                }
            }
        } else {
            self.arena.nodes.flags[slot].remove(NodeFlags::HAS_WRAPPER);
            self.invalidate_keep(id);
        }
        true
    }

    /// Number of published wrappers anchored to this live node across worlds.
    pub fn wrapper_count(&self, id: NodeId) -> usize {
        if !self.arena.is_live(id) || !self.has_wrapper(id) {
            0
        } else {
            1 + self.extra_wrapper_claims.get(&id).copied().unwrap_or(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tenta_core::atom::atoms;

    #[test]
    fn extra_claim_storage_is_reported_and_released_at_the_single_claim_boundary() {
        let mut doc = Document::new();
        let id = doc.create_html_element(atoms::DIV);
        let node = doc.node_ref(id);
        let before = doc.memory_report().indexes;
        doc.retain_wrapper(node).unwrap();
        assert_eq!(doc.memory_report().indexes, before);
        doc.retain_wrapper(node).unwrap();
        let allocation = doc.extra_wrapper_claims.allocation_size();
        assert!(allocation > 0);
        assert_eq!(doc.memory_report().indexes, before + allocation);
        doc.release_wrapper(node);
        assert_eq!(doc.wrapper_count(id), 1);
        assert_eq!(doc.extra_wrapper_claims.allocation_size(), 0);
        assert_eq!(doc.memory_report().indexes, before);
        doc.release_wrapper(node);
    }

    #[test]
    fn wrapper_claim_overflow_preserves_every_existing_claim() {
        let mut doc = Document::new();
        let id = doc.create_html_element(atoms::DIV);
        let node = doc.node_ref(id);
        doc.retain_wrapper(node).unwrap();
        doc.extra_wrapper_claims.insert(id, usize::MAX - 1);
        assert_eq!(doc.retain_wrapper(node), Err(WrapperClaimError::Capacity));
        assert_eq!(doc.wrapper_count(id), usize::MAX);
        assert!(doc.release_wrapper(node));
        assert_eq!(doc.wrapper_count(id), usize::MAX - 1);
    }
}
