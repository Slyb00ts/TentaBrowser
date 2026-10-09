//! Overflow clips.

use tenta_layout::{Fragment, Rect};

use crate::dl::Item;

/// Whether [`open`] pushed a clip that [`close`] must pop.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct ClipScope(pub bool);

/// Pushes the overflow clip of `frag` on its padding box, when it has one.
pub(super) fn open(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) -> ClipScope {
    let _ = (frag, border_box, items);
    todo!("P2b")
}

/// Pops the clip [`open`] pushed.
pub(super) fn close(scope: ClipScope, items: &mut Vec<Item>) {
    let _ = (scope, items);
    todo!("P2b")
}
