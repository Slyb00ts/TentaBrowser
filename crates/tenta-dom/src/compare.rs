//! Comparing nodes: `isEqualNode` and `compareDocumentPosition`
//! (<https://dom.spec.whatwg.org/#concept-node-equals>,
//! <https://dom.spec.whatwg.org/#dom-node-comparedocumentposition>).

use tenta_core::NodeId;

use crate::arena::NodeKind;
use crate::document::Document;

/// `DOCUMENT_POSITION_DISCONNECTED`
pub const DISCONNECTED: u16 = 0x01;
/// `DOCUMENT_POSITION_PRECEDING`
pub const PRECEDING: u16 = 0x02;
/// `DOCUMENT_POSITION_FOLLOWING`
pub const FOLLOWING: u16 = 0x04;
/// `DOCUMENT_POSITION_CONTAINS`
pub const CONTAINS: u16 = 0x08;
/// `DOCUMENT_POSITION_CONTAINED_BY`
pub const CONTAINED_BY: u16 = 0x10;
/// `DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC`
pub const IMPLEMENTATION_SPECIFIC: u16 = 0x20;

/// The attributes of an element as comparable triples, sorted: equality does
/// not depend on their order.
fn attributes(doc: &Document, element: NodeId) -> Vec<(String, String, String)> {
    let mut list: Vec<(String, String, String)> = doc
        .attributes(element)
        .map(|(name, value)| {
            (
                name.ns.as_str().to_owned(),
                name.local.as_str().to_owned(),
                value.to_owned(),
            )
        })
        .collect();
    list.sort();
    list
}

/// Whether node `a` of `left` equals node `b` of `right`: the same kind, the
/// same kind-specific data, the same attributes in any order, and children
/// that are pairwise equal. The two may be in different documents.
pub fn is_equal_node(left: &Document, a: NodeId, right: &Document, b: NodeId) -> bool {
    // An explicit stack: equality of deep trees must not recurse natively.
    let mut pending = vec![(a, b)];
    while let Some((a, b)) = pending.pop() {
        let kind = left.kind(a);
        if kind != right.kind(b) {
            return false;
        }
        let same = match kind {
            NodeKind::Element => {
                left.namespace(a) == right.namespace(b)
                    && left.element_prefix(a) == right.element_prefix(b)
                    && left.tag(a) == right.tag(b)
                    && attributes(left, a) == attributes(right, b)
            }
            NodeKind::Text | NodeKind::CData | NodeKind::Comment => {
                left.text_data(a) == right.text_data(b)
            }
            NodeKind::ProcessingInstruction => {
                left.processing_instruction(a) == right.processing_instruction(b)
            }
            NodeKind::DocType => left.doctype(a) == right.doctype(b),
            _ => true,
        };
        if !same {
            return false;
        }
        let mut children_a = left.children(a);
        let mut children_b = right.children(b);
        loop {
            match (children_a.next(), children_b.next()) {
                (Some(child_a), Some(child_b)) => pending.push((child_a, child_b)),
                (None, None) => break,
                _ => return false,
            }
        }
    }
    true
}

/// `node.compareDocumentPosition(other)` for two nodes of one document: where
/// `other` is relative to `node`.
pub fn compare_document_position(doc: &Document, node: NodeId, other: NodeId) -> u16 {
    if node == other {
        return 0;
    }
    if doc.root_of(node) != doc.root_of(other) {
        // Disconnected trees have no order; the standard asks only that the
        // answer be consistent, which the arena's own order is.
        let direction = if other.raw() < node.raw() {
            PRECEDING
        } else {
            FOLLOWING
        };
        return DISCONNECTED | IMPLEMENTATION_SPECIFIC | direction;
    }
    if doc.ancestors(node).any(|ancestor| ancestor == other) {
        return CONTAINS | PRECEDING;
    }
    if doc.ancestors(other).any(|ancestor| ancestor == node) {
        return CONTAINED_BY | FOLLOWING;
    }
    if doc.precedes(other, node) {
        PRECEDING
    } else {
        FOLLOWING
    }
}
