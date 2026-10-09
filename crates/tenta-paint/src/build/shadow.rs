//! `box-shadow` items.

use tenta_layout::{Fragment, Rect};

use crate::dl::Item;

/// Pushes the outer and inset `box-shadow` items of `frag`.
pub(super) fn emit(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
    let _ = (frag, border_box, items);
    todo!("P2a")
}
