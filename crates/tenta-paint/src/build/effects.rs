//! Opacity groups and transforms.

use std::collections::BTreeMap;

use tenta_layout::{Affine, BlendMode, FragId, Fragment, FragmentTree, Rect};

use super::text;
use crate::dl::Item;

/// The groups [`open`] pushed, which [`close`] must pop in reverse order.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct EffectScope {
    pub opacity: bool,
    pub transform: bool,
}

/// Pushes the transform and opacity group of `frag`.
///
/// Display-list items carry absolute document coordinates, so a CSS transform
/// is conjugated by the transform origin, `translate(origin) * M *
/// translate(-origin)`, instead of relying on a local coordinate space. The
/// transform is the outer group: the opacity layer is then allocated in the
/// transformed space, where the content's footprint is already known.
///
/// `filter` and `mix-blend-mode` are not drawn: the display list has no item
/// for them yet, and the stacking walk already gives those boxes their own
/// context so the content still paints in the right order.
pub(super) fn open(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) -> EffectScope {
    let style = &frag.paint;
    let transform = style
        .transform
        .map(|matrix| {
            let origin = (
                border_box.x + style.transform_origin.0.resolve(border_box.w),
                border_box.y + style.transform_origin.1.resolve(border_box.h),
            );
            Affine::translation(origin.0, origin.1)
                .after(&matrix)
                .after(&Affine::translation(-origin.0, -origin.1))
                .to_array()
        })
        .filter(|array| array.iter().all(|v| v.is_finite()));
    if let Some(array) = transform {
        items.push(Item::PushTransform(array));
    }
    let opacity = style.opacity > 0.0 && style.opacity < 1.0;
    if opacity {
        items.push(Item::PushOpacity(style.opacity));
    }
    EffectScope {
        opacity,
        transform: transform.is_some(),
    }
}

/// Pops the groups [`open`] pushed.
pub(super) fn close(scope: EffectScope, items: &mut Vec<Item>) {
    if scope.opacity {
        items.push(Item::PopOpacity);
    }
    if scope.transform {
        items.push(Item::PopTransform);
    }
}

/// Counts, by name, the painted fragments that use style the display list
/// cannot express yet: `filter`, `mix-blend-mode` and what the text emitter
/// leaves out. Fragments the painter skips (fully transparent subtrees) are
/// not counted. `clip-path` and `overflow-clip-margin` are not counted because
/// fragments do not carry them.
pub(super) fn count_unsupported(frags: &FragmentTree) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    let mut pending: Vec<FragId> = vec![frags.root];
    while let Some(id) = pending.pop() {
        let frag = frags.get(id);
        if frag.paint.opacity <= 0.0 {
            continue;
        }
        if !frag.paint.filter.is_empty() {
            *counts.entry("Filter").or_default() += 1;
        }
        if frag.paint.blend != BlendMode::Normal {
            *counts.entry("BlendMode").or_default() += 1;
        }
        text::count_unsupported(frag, &mut counts);
        pending.extend(frag.children.iter().copied());
    }
    counts
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tenta_core::NodeId;
    use tenta_layout::{FragmentKind, LengthPercent, PaintStyle};

    use super::*;

    fn fragment(edit: impl FnOnce(&mut PaintStyle)) -> Fragment {
        let mut style = PaintStyle::default();
        edit(&mut style);
        Fragment::new(
            NodeId(2),
            FragmentKind::Box,
            Arc::new(style),
            Rect::new(0.0, 0.0, 40.0, 20.0),
        )
    }

    fn rotation(degrees: f32) -> Affine {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Affine {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }

    fn matrix(items: &[Item]) -> Affine {
        let Item::PushTransform([a, b, c, d, e, f]) = items[0] else {
            panic!("expected a transform, got {items:?}");
        };
        Affine { a, b, c, d, e, f }
    }

    fn near(actual: (f32, f32), expected: (f32, f32)) {
        assert!(
            (actual.0 - expected.0).abs() < 1e-3 && (actual.1 - expected.1).abs() < 1e-3,
            "{actual:?} is not {expected:?}"
        );
    }

    #[test]
    fn opacity_brackets_the_group() {
        let frag = fragment(|s| s.opacity = 0.5);
        let mut items = Vec::new();
        let scope = open(&frag, Rect::new(0.0, 0.0, 40.0, 20.0), &mut items);
        assert_eq!(items, vec![Item::PushOpacity(0.5)]);
        close(scope, &mut items);
        assert_eq!(items, vec![Item::PushOpacity(0.5), Item::PopOpacity]);
    }

    #[test]
    fn a_rotated_box_keeps_its_centre_and_swings_its_corners() {
        let frag = fragment(|s| {
            s.transform = Some(rotation(90.0));
        });
        let mut items = Vec::new();
        open(&frag, Rect::new(100.0, 200.0, 40.0, 20.0), &mut items);
        let m = matrix(&items);
        // The default origin is the centre (120, 210) and stays put.
        near(m.apply(120.0, 210.0), (120.0, 210.0));
        // The top-left corner is 20 left and 10 above the centre; a quarter
        // turn clockwise sends it 10 right of and 20 above the centre.
        near(m.apply(100.0, 200.0), (130.0, 190.0));
        near(m.apply(140.0, 220.0), (110.0, 230.0));
    }

    #[test]
    fn the_origin_is_resolved_against_the_border_box() {
        let frag = fragment(|s| {
            s.transform = Some(Affine::scale(2.0, 2.0));
            s.transform_origin = (LengthPercent::px(0.0), LengthPercent::fraction(1.0));
        });
        let mut items = Vec::new();
        open(&frag, Rect::new(100.0, 200.0, 40.0, 20.0), &mut items);
        let m = matrix(&items);
        // The bottom-left corner is the fixed point of the scale.
        near(m.apply(100.0, 220.0), (100.0, 220.0));
        near(m.apply(140.0, 200.0), (180.0, 180.0));
    }

    #[test]
    fn transform_wraps_the_opacity_group_and_closes_last() {
        let frag = fragment(|s| {
            s.opacity = 0.25;
            s.transform = Some(Affine::translation(5.0, 0.0));
        });
        let mut items = Vec::new();
        let scope = open(&frag, Rect::new(0.0, 0.0, 40.0, 20.0), &mut items);
        assert!(matches!(
            items[..],
            [Item::PushTransform(_), Item::PushOpacity(_)]
        ));
        close(scope, &mut items);
        assert!(matches!(items[2..], [Item::PopOpacity, Item::PopTransform]));
    }

    #[test]
    fn opaque_fragments_without_a_transform_push_nothing() {
        let frag = fragment(|s| {
            s.opacity = 1.0;
            s.filter = vec![tenta_layout::FilterOp::Invert(1.0)];
        });
        let mut items = Vec::new();
        let scope = open(&frag, Rect::new(0.0, 0.0, 40.0, 20.0), &mut items);
        assert!(items.is_empty());
        close(scope, &mut items);
        assert!(items.is_empty());
    }

    #[test]
    fn filters_and_blend_modes_are_counted_unless_their_subtree_is_invisible() {
        use tenta_layout::{FilterOp, FragmentBuilder};
        let paint = |edit: fn(&mut PaintStyle)| {
            let mut style = PaintStyle::default();
            edit(&mut style);
            Arc::new(style)
        };
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let mut builder = FragmentBuilder::new();
        let mut push = |parent: Option<FragId>, paint| {
            let id = builder.push(Fragment::new(NodeId(2), FragmentKind::Box, paint, rect));
            if let Some(parent) = parent {
                builder.add_child(parent, id);
            }
            id
        };
        let root = push(None, paint(|_| {}));
        push(Some(root), paint(|s| s.filter = vec![FilterOp::Blur(2.0)]));
        push(Some(root), paint(|s| s.blend = BlendMode::Multiply));
        let ghost = push(Some(root), paint(|s| s.opacity = 0.0));
        push(
            Some(ghost),
            paint(|s| s.filter = vec![FilterOp::Invert(1.0)]),
        );
        let tree = builder.finish(root, rect, rect);
        let counts = count_unsupported(&tree);
        assert_eq!(counts.get("Filter"), Some(&1));
        assert_eq!(counts.get("BlendMode"), Some(&1));
        assert_eq!(counts.len(), 2);
    }

    #[test]
    fn a_non_finite_transform_is_dropped() {
        let frag = fragment(|s| s.transform = Some(Affine::scale(f32::NAN, 1.0)));
        let mut items = Vec::new();
        let scope = open(&frag, Rect::new(0.0, 0.0, 40.0, 20.0), &mut items);
        assert!(items.is_empty());
        assert!(!scope.transform);
    }
}
