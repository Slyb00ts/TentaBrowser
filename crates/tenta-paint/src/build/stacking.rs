//! Stacking contexts and CSS 2.1 Appendix E painting order.

use tenta_layout::{FragId, FragmentTree, Rect};

use super::{background, border, clip, effects, marker, shadow, text};
use crate::PaintContext;
use crate::dl::Item;

/// Paints fragment `id` and its subtree into `items`. `origin` is the document
/// position of the parent's border box.
///
/// The call order here marks where each kind of item is produced; the painting
/// order itself is decided in this module.
pub(super) fn paint_fragment(
    frags: &FragmentTree,
    id: FragId,
    origin: (f32, f32),
    cx: &PaintContext,
    items: &mut Vec<Item>,
) {
    let frag = frags.get(id);
    let border_box = Rect::new(
        origin.0 + frag.rect.x,
        origin.1 + frag.rect.y,
        frag.rect.w,
        frag.rect.h,
    );
    let effect_scope = effects::open(frag, border_box, items);
    shadow::emit(frag, border_box, items);
    background::emit(frag, border_box, cx, items);
    border::emit(frag, border_box, items);
    marker::emit(frag, border_box, items);
    let clipped = clip::open(frag, border_box, items);
    text::emit(frag, border_box, items);
    for &child in &frag.children {
        paint_fragment(frags, child, (border_box.x, border_box.y), cx, items);
    }
    clip::close(clipped, items);
    effects::close(effect_scope, items);
}
