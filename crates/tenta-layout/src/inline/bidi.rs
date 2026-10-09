//! Bidirectional reordering of line content.

use crate::fragment::FragId;
use crate::types::Direction;

/// Reorders the fragments of one line from logical to visual order for the
/// paragraph `direction`.
pub(super) fn reorder_line(direction: Direction, children: &mut [FragId]) {
    let _ = (direction, children);
    todo!("L3b")
}
