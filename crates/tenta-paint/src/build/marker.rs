//! List markers.

use tenta_layout::{Fragment, Rect};

use crate::dl::Item;

/// Pushes the shape of a list-marker fragment; does nothing for other kinds.
pub(super) fn emit(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
    let _ = (frag, border_box, items);
    todo!("P2b")
}
