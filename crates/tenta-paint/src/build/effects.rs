//! Opacity groups and transforms.

use tenta_layout::{Fragment, Rect};

use crate::dl::Item;

/// The groups [`open`] pushed, which [`close`] must pop in reverse order.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct EffectScope {
    pub opacity: bool,
    pub transform: bool,
}

/// Pushes the transform and opacity group of `frag`.
pub(super) fn open(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) -> EffectScope {
    let _ = (frag, border_box, items);
    todo!("P2b")
}

/// Pops the groups [`open`] pushed.
pub(super) fn close(scope: EffectScope, items: &mut Vec<Item>) {
    let _ = (scope, items);
    todo!("P2b")
}
