//! Background layers, gradients and canvas propagation.

use tenta_layout::{Fragment, Rect};

use crate::PaintContext;
use crate::dl::Item;

/// Pushes the background items of `frag`, topmost layer first.
pub(super) fn emit(frag: &Fragment, border_box: Rect, cx: &PaintContext, items: &mut Vec<Item>) {
    let _ = (frag, border_box, cx, items);
    todo!("P2a")
}
