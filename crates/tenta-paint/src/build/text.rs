//! Text, decoration and `text-shadow` items.

use tenta_layout::{Fragment, Rect};

use crate::dl::Item;

/// Pushes the text, decoration and text-shadow items of a text fragment;
/// does nothing for other fragment kinds.
pub(super) fn emit(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
    let _ = (frag, border_box, items);
    todo!("P2b")
}
