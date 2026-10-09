//! Overflow clips.

use tenta_layout::{Fragment, OverflowKind, Rect};

use super::border::{inset_radii, inset_rect, resolve_radii};
use crate::dl::Item;

/// Half the extent of the clip on an axis that does not clip. Large enough to
/// cover any page, small enough that device coordinates stay exact in `f32`.
const UNBOUNDED: f32 = 1.0e6;

/// Whether [`open`] pushed a clip that [`close`] must pop.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct ClipScope(pub bool);

/// Pushes the overflow clip of `frag` on its padding box.
///
/// The fragment is flagged as clipping, so that flag is authoritative: an
/// axis whose `overflow` is `visible` stays unbounded while the other clips,
/// and a fragment whose styles are both `visible` (replaced content, for
/// instance) clips both axes. `hidden`, `clip`, `scroll` and `auto` cut at the
/// same edge; the scroll offset of a scroll container is not applied yet. The
/// inner border radii only round the clip when both axes clip, since a clip
/// open on one side has no corner there.
pub(super) fn open(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) -> ClipScope {
    let style = &frag.paint;
    let mut clip_x = style.overflow_x != OverflowKind::Visible;
    let mut clip_y = style.overflow_y != OverflowKind::Visible;
    if !clip_x && !clip_y {
        (clip_x, clip_y) = (true, true);
    }
    let padding_box = inset_rect(border_box, &frag.border);
    let radii = if clip_x && clip_y {
        inset_radii(resolve_radii(&style.border.radii, border_box), &frag.border)
    } else {
        [0.0; 8]
    };
    let (x, w) = if clip_x {
        (padding_box.x, padding_box.w)
    } else {
        (-UNBOUNDED, 2.0 * UNBOUNDED)
    };
    let (y, h) = if clip_y {
        (padding_box.y, padding_box.h)
    } else {
        (-UNBOUNDED, 2.0 * UNBOUNDED)
    };
    items.push(Item::PushClip {
        rect: Rect::new(x, y, w, h),
        radii,
    });
    ClipScope(true)
}

/// Pops the clip [`open`] pushed.
pub(super) fn close(scope: ClipScope, items: &mut Vec<Item>) {
    if scope.0 {
        items.push(Item::PopClip);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tenta_core::NodeId;
    use tenta_layout::{Edges, FragFlags, FragmentKind, LengthPercent, PaintStyle};

    use super::*;

    fn clipping(edit: impl FnOnce(&mut Fragment)) -> Fragment {
        let mut frag = Fragment::new(
            NodeId(2),
            FragmentKind::Box,
            Arc::new(PaintStyle::default()),
            Rect::new(0.0, 0.0, 100.0, 60.0),
        );
        frag.flags = FragFlags::CLIPS_OVERFLOW;
        edit(&mut frag);
        frag
    }

    fn with_style(frag: &mut Fragment, edit: impl FnOnce(&mut PaintStyle)) {
        let mut style = (*frag.paint).clone();
        edit(&mut style);
        frag.paint = Arc::new(style);
    }

    fn pushed(frag: &Fragment, border_box: Rect) -> (ClipScope, Vec<Item>) {
        let mut items = Vec::new();
        let scope = open(frag, border_box, &mut items);
        (scope, items)
    }

    #[test]
    fn a_hidden_box_clips_to_its_padding_box() {
        let frag = clipping(|f| {
            f.border = Edges {
                top: 1.0,
                right: 2.0,
                bottom: 3.0,
                left: 4.0,
            };
            with_style(f, |s| {
                s.overflow_x = OverflowKind::Hidden;
                s.overflow_y = OverflowKind::Hidden;
            });
        });
        let (scope, items) = pushed(&frag, Rect::new(10.0, 20.0, 100.0, 60.0));
        assert_eq!(scope, ClipScope(true));
        assert_eq!(
            items,
            vec![Item::PushClip {
                rect: Rect::new(14.0, 21.0, 94.0, 56.0),
                radii: [0.0; 8],
            }]
        );
    }

    #[test]
    fn the_clip_uses_the_inner_border_radii() {
        let frag = clipping(|f| {
            f.border = Edges::all(4.0);
            with_style(f, |s| {
                s.overflow_x = OverflowKind::Clip;
                s.overflow_y = OverflowKind::Clip;
                s.border.radii = [
                    LengthPercent::px(10.0),
                    LengthPercent::px(10.0),
                    LengthPercent::px(3.0),
                    LengthPercent::px(3.0),
                    LengthPercent::fraction(0.5),
                    LengthPercent::px(0.0),
                    LengthPercent::px(0.0),
                    LengthPercent::px(0.0),
                ];
            });
        });
        let (_, items) = pushed(&frag, Rect::new(0.0, 0.0, 100.0, 60.0));
        let Item::PushClip { radii, .. } = &items[0] else {
            panic!("expected a clip");
        };
        // 10 - 4, a radius smaller than the border collapses to square, and
        // 50% of the width is 50.
        assert_eq!(*radii, [6.0, 6.0, 0.0, 0.0, 46.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn only_the_clipping_axis_is_bounded() {
        let frag = clipping(|f| {
            with_style(f, |s| {
                s.overflow_x = OverflowKind::Clip;
                s.overflow_y = OverflowKind::Visible;
                s.border.radii = [LengthPercent::px(8.0); 8];
            });
        });
        let (_, items) = pushed(&frag, Rect::new(10.0, 20.0, 100.0, 60.0));
        assert_eq!(
            items,
            vec![Item::PushClip {
                rect: Rect::new(10.0, -UNBOUNDED, 100.0, 2.0 * UNBOUNDED),
                radii: [0.0; 8],
            }]
        );
        let frag = clipping(|f| {
            with_style(f, |s| s.overflow_y = OverflowKind::Clip);
        });
        let (_, items) = pushed(&frag, Rect::new(10.0, 20.0, 100.0, 60.0));
        assert_eq!(
            items,
            vec![Item::PushClip {
                rect: Rect::new(-UNBOUNDED, 20.0, 2.0 * UNBOUNDED, 60.0),
                radii: [0.0; 8],
            }]
        );
    }

    #[test]
    fn visible_on_one_axis_computes_to_auto_and_clips_both() {
        // Stylo computes `overflow-x: visible; overflow-y: hidden` as
        // auto / hidden, so a fragment never reaches the builder with a
        // visible axis next to a scrolling one.
        let frag = clipping(|f| {
            with_style(f, |s| {
                s.overflow_x = OverflowKind::Auto;
                s.overflow_y = OverflowKind::Hidden;
            });
        });
        let (_, items) = pushed(&frag, Rect::new(10.0, 20.0, 100.0, 60.0));
        assert_eq!(
            items,
            vec![Item::PushClip {
                rect: Rect::new(10.0, 20.0, 100.0, 60.0),
                radii: [0.0; 8],
            }]
        );
    }

    #[test]
    fn a_flagged_fragment_with_visible_overflow_still_clips_both_axes() {
        let frag = clipping(|_| {});
        let (_, items) = pushed(&frag, Rect::new(5.0, 6.0, 30.0, 40.0));
        assert_eq!(
            items,
            vec![Item::PushClip {
                rect: Rect::new(5.0, 6.0, 30.0, 40.0),
                radii: [0.0; 8],
            }]
        );
    }

    #[test]
    fn a_box_swallowed_by_its_borders_clips_everything() {
        let frag = clipping(|f| {
            f.border = Edges::all(10.0);
            with_style(f, |s| {
                s.overflow_x = OverflowKind::Hidden;
                s.overflow_y = OverflowKind::Hidden;
            });
        });
        let (_, items) = pushed(&frag, Rect::new(0.0, 0.0, 12.0, 12.0));
        let Item::PushClip { rect, .. } = &items[0] else {
            panic!("expected a clip");
        };
        assert_eq!((rect.w, rect.h), (0.0, 0.0));
    }

    #[test]
    fn close_pops_only_what_open_pushed() {
        let mut items = Vec::new();
        close(ClipScope(false), &mut items);
        assert!(items.is_empty());
        close(ClipScope(true), &mut items);
        assert_eq!(items, vec![Item::PopClip]);
    }
}
