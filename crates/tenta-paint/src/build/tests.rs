//! Painting-order, geometry and raster tests over hand-built fragment trees.
//!
//! Text, overflow clips and opacity/transform groups belong to the seams that
//! are not implemented yet, so these trees avoid them; inline content is
//! represented by plain inline boxes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tenta_core::NodeId;
use tenta_layout::{
    Affine, BackgroundContent, BackgroundLayer, BackgroundSize, BorderStyle, BoxArea, Color,
    ColorStop, Edges, FragFlags, FragId, Fragment, FragmentBuilder, FragmentKind, FragmentTree,
    GradientPaint, LengthPercent, OutlinePaint, PaintStyle, PositionKind, Rect, RepeatKind,
    ShadowPaint,
};

use super::clip::ClipScope;
use super::effects::EffectScope;
use super::stacking::{Seams, creates_context, paint_with};
use super::*;
use crate::dl::Item;
use crate::encode::{PngProfile, encode_png};
use crate::image::ImageStore;
use crate::raster::{Pixmap, RasterOptions, RasterOutput, rasterize};

fn tag(n: u8) -> Color {
    Color::from_rgba8(n, 0, 0, 255)
}

fn layer(color: Color) -> BackgroundLayer {
    BackgroundLayer {
        content: BackgroundContent::Color(color),
        repeat: (RepeatKind::Repeat, RepeatKind::Repeat),
        position: (LengthPercent::default(), LengthPercent::default()),
        size: BackgroundSize::Auto,
        origin: BoxArea::BorderBox,
        clip: BoxArea::BorderBox,
    }
}

fn clipped_layer(color: Color, clip: BoxArea) -> BackgroundLayer {
    BackgroundLayer {
        clip,
        ..layer(color)
    }
}

fn style(edit: impl FnOnce(&mut PaintStyle)) -> PaintStyle {
    let mut style = PaintStyle::default();
    edit(&mut style);
    style
}

fn filled(color: Color) -> PaintStyle {
    style(|s| s.background = vec![layer(color)])
}

fn positioned(color: Color, z: Option<i32>) -> PaintStyle {
    style(|s| {
        s.background = vec![layer(color)];
        s.position = PositionKind::Absolute;
        s.z_index = z;
    })
}

/// A fragment tree under construction.
struct Scene {
    builder: FragmentBuilder,
    root: FragId,
}

impl Scene {
    fn new() -> Self {
        Self::with_root(PaintStyle::default())
    }

    fn with_root(root_style: PaintStyle) -> Self {
        let mut builder = FragmentBuilder::new();
        let root = builder.push(Fragment::new(
            NodeId(1),
            FragmentKind::Box,
            Arc::new(root_style),
            Rect::new(0.0, 0.0, 800.0, 600.0),
        ));
        Scene { builder, root }
    }

    fn add_kind(
        &mut self,
        parent: FragId,
        kind: FragmentKind,
        rect: Rect,
        paint: PaintStyle,
        flags: FragFlags,
    ) -> FragId {
        let mut frag = Fragment::new(NodeId(2), kind, Arc::new(paint), rect);
        frag.flags = flags;
        let id = self.builder.push(frag);
        self.builder.add_child(parent, id);
        id
    }

    fn add_clipping(&mut self, parent: FragId, rect: Rect, color: Color) -> FragId {
        self.add_kind(
            parent,
            FragmentKind::Box,
            rect,
            filled(color),
            FragFlags::CLIPS_OVERFLOW,
        )
    }

    fn add(&mut self, parent: FragId, rect: Rect, paint: PaintStyle) -> FragId {
        self.add_kind(parent, FragmentKind::Box, rect, paint, FragFlags::empty())
    }

    fn float(&mut self, parent: FragId, rect: Rect, paint: PaintStyle) -> FragId {
        self.add_kind(parent, FragmentKind::Box, rect, paint, FragFlags::FLOAT)
    }

    /// A line box holding one inline box, returned.
    fn inline_box(&mut self, parent: FragId, rect: Rect, paint: PaintStyle) -> FragId {
        let line = self.add_kind(
            parent,
            FragmentKind::Line,
            Rect::new(0.0, rect.y, 800.0, rect.h),
            PaintStyle::default(),
            FragFlags::empty(),
        );
        self.add(line, Rect::new(rect.x, 0.0, rect.w, rect.h), paint)
    }

    fn edit(&mut self, id: FragId, edit: impl FnOnce(&mut Fragment)) {
        edit(self.builder.get_mut(id));
    }

    fn finish(self) -> FragmentTree {
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        self.builder.finish(self.root, viewport, viewport)
    }
}

/// Stand-ins for the clip, effect, text and marker emitters that record what
/// the walk asks for: clips and groups as pushes and pops, text as a fill of
/// the text colour.
struct Recorder;

impl Seams for Recorder {
    fn open_clip(&self, _frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) -> ClipScope {
        items.push(Item::PushClip {
            rect: border_box,
            radii: [0.0; 8],
        });
        ClipScope(true)
    }

    fn close_clip(&self, _scope: ClipScope, items: &mut Vec<Item>) {
        items.push(Item::PopClip);
    }

    fn open_effects(&self, frag: &Fragment, _bb: Rect, items: &mut Vec<Item>) -> EffectScope {
        let scope = EffectScope {
            opacity: frag.paint.opacity < 1.0,
            transform: frag.paint.transform.is_some(),
        };
        if let Some(affine) = frag.paint.transform {
            items.push(Item::PushTransform(affine.to_array()));
        }
        if scope.opacity {
            items.push(Item::PushOpacity(frag.paint.opacity));
        }
        scope
    }

    fn close_effects(&self, scope: EffectScope, items: &mut Vec<Item>) {
        if scope.opacity {
            items.push(Item::PopOpacity);
        }
        if scope.transform {
            items.push(Item::PopTransform);
        }
    }

    fn text(&self, frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
        items.push(Item::FillRect {
            rect: border_box,
            color: frag.paint.color,
        });
    }

    fn marker(&self, frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
        self.text(frag, border_box, items);
    }
}

fn build_recorded(tree: &FragmentTree) -> DisplayList {
    let images = ImageStore::new();
    DisplayList {
        items: paint_with(tree, &PaintContext::new(&images), Recorder),
        bounds: tree.scroll_extent,
        ..DisplayList::default()
    }
}

fn build_list(tree: &FragmentTree) -> DisplayList {
    let images = ImageStore::new();
    build(tree, &PaintContext::new(&images))
}

/// The red channel of every fill, which is how tests tag boxes.
fn tags(list: &DisplayList) -> Vec<u8> {
    list.items
        .iter()
        .filter_map(|item| match item {
            Item::FillRect { color, .. } | Item::FillRRect { color, .. } => {
                Some(color.to_premultiplied_rgba8()[0])
            }
            _ => None,
        })
        .collect()
}

fn fills(list: &DisplayList) -> Vec<Rect> {
    list.items
        .iter()
        .filter_map(|item| match item {
            Item::FillRect { rect, .. } | Item::FillRRect { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect()
}

#[test]
fn appendix_e_order_for_a_mixed_tree() {
    let mut scene = Scene::new();
    let root = scene.root;
    // Children are added in a scrambled source order on purpose.
    scene.add(
        root,
        Rect::new(0.0, 0.0, 50.0, 50.0),
        positioned(tag(7), Some(2)),
    );
    scene.add(
        root,
        Rect::new(0.0, 0.0, 50.0, 50.0),
        positioned(tag(6), None),
    );
    scene.float(root, Rect::new(0.0, 60.0, 20.0, 20.0), filled(tag(5)));
    let block = scene.add(root, Rect::new(0.0, 100.0, 300.0, 40.0), filled(tag(3)));
    scene.inline_box(block, Rect::new(5.0, 0.0, 30.0, 10.0), filled(tag(4)));
    scene.add(
        root,
        Rect::new(0.0, 0.0, 50.0, 50.0),
        positioned(tag(2), Some(-1)),
    );
    scene.add(
        root,
        Rect::new(0.0, 0.0, 50.0, 50.0),
        positioned(tag(8), Some(1)),
    );
    let list = build_list(&scene.finish());
    assert_eq!(tags(&list), vec![2, 3, 5, 4, 6, 8, 7]);
    assert!(list.is_balanced());
}

#[test]
fn a_context_below_a_positioned_auto_box_joins_the_enclosing_context() {
    let mut scene = Scene::new();
    let root = scene.root;
    let auto = scene.add(
        root,
        Rect::new(0.0, 0.0, 100.0, 100.0),
        positioned(tag(6), None),
    );
    scene.add(
        auto,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        positioned(tag(9), Some(-1)),
    );
    scene.add(
        auto,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        positioned(tag(10), Some(3)),
    );
    scene.add(root, Rect::new(0.0, 0.0, 50.0, 50.0), filled(tag(3)));
    scene.add(
        root,
        Rect::new(0.0, 0.0, 50.0, 50.0),
        positioned(tag(11), Some(1)),
    );
    let list = build_list(&scene.finish());
    // The auto box paints with the z-index 0 layer; its z-index children sort
    // against the root's other contexts.
    assert_eq!(tags(&list), vec![9, 3, 6, 11, 10]);
}

#[test]
fn equal_z_indexes_keep_tree_order_and_negatives_sort_ascending() {
    let mut scene = Scene::new();
    let root = scene.root;
    for (t, z) in [(1, 2), (2, 1), (3, 2), (4, -1), (5, -3), (6, -1)] {
        scene.add(
            root,
            Rect::new(0.0, 0.0, 5.0, 5.0),
            positioned(tag(t), Some(z)),
        );
    }
    assert_eq!(tags(&build_list(&scene.finish())), vec![5, 4, 6, 2, 1, 3]);
}

#[test]
fn all_block_backgrounds_precede_all_inline_content() {
    let mut scene = Scene::new();
    let root = scene.root;
    let first = scene.add(root, Rect::new(0.0, 0.0, 300.0, 20.0), filled(tag(1)));
    scene.inline_box(first, Rect::new(0.0, 0.0, 10.0, 10.0), filled(tag(3)));
    let second = scene.add(root, Rect::new(0.0, 20.0, 300.0, 20.0), filled(tag(2)));
    scene.inline_box(second, Rect::new(0.0, 0.0, 10.0, 10.0), filled(tag(4)));
    assert_eq!(tags(&build_list(&scene.finish())), vec![1, 2, 3, 4]);
}

#[test]
fn an_inline_block_paints_its_blocks_floats_and_inlines_atomically() {
    let mut scene = Scene::new();
    let root = scene.root;
    let host = scene.add(root, Rect::new(0.0, 0.0, 300.0, 40.0), filled(tag(1)));
    let line = scene.add_kind(
        host,
        FragmentKind::Line,
        Rect::new(0.0, 0.0, 300.0, 40.0),
        PaintStyle::default(),
        FragFlags::empty(),
    );
    let atomic = scene.add_kind(
        line,
        FragmentKind::Box,
        Rect::new(0.0, 0.0, 100.0, 40.0),
        filled(tag(2)),
        FragFlags::PAINT_ATOMIC,
    );
    let inner = scene.add(atomic, Rect::new(0.0, 0.0, 100.0, 20.0), filled(tag(3)));
    scene.inline_box(inner, Rect::new(0.0, 0.0, 10.0, 10.0), filled(tag(4)));
    scene.inline_box(line, Rect::new(120.0, 0.0, 10.0, 10.0), filled(tag(5)));
    assert_eq!(tags(&build_list(&scene.finish())), vec![1, 2, 3, 4, 5]);
}

#[test]
fn box_parts_paint_shadow_background_inset_border_in_order() {
    let mut scene = Scene::new();
    let id = scene.add(
        scene.root,
        Rect::new(10.0, 10.0, 100.0, 50.0),
        style(|s| {
            s.background = vec![layer(tag(1))];
            s.border.styles = [BorderStyle::Solid; 4];
            s.box_shadow = vec![
                ShadowPaint {
                    offset: (1.0, 2.0),
                    blur: 3.0,
                    spread: 0.0,
                    color: tag(2),
                    inset: true,
                },
                ShadowPaint {
                    offset: (4.0, 4.0),
                    blur: 0.0,
                    spread: 1.0,
                    color: tag(3),
                    inset: false,
                },
            ];
        }),
    );
    scene.edit(id, |f| f.border = Edges::all(2.0));
    let list = build_list(&scene.finish());
    let kinds: Vec<&str> = list
        .items
        .iter()
        .map(|item| match item {
            Item::BoxShadow { inset: false, .. } => "outer",
            Item::BoxShadow { inset: true, .. } => "inset",
            Item::FillRect { .. } => "fill",
            Item::Border { .. } => "border",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["outer", "fill", "inset", "border"]);
    let Item::BoxShadow {
        rect,
        offset,
        spread,
        ..
    } = &list.items[0]
    else {
        unreachable!()
    };
    assert_eq!(*rect, Rect::new(10.0, 10.0, 100.0, 50.0));
    assert_eq!((*offset, *spread), ((4.0, 4.0), 1.0));
    let Item::BoxShadow { rect, .. } = &list.items[2] else {
        unreachable!()
    };
    assert_eq!(*rect, Rect::new(12.0, 12.0, 96.0, 46.0));
}

#[test]
fn the_root_background_takes_the_canvas() {
    let mut scene = Scene::with_root(filled(tag(1)));
    let body = scene.add(scene.root, Rect::new(8.0, 8.0, 100.0, 50.0), filled(tag(2)));
    let _ = body;
    let list = build_list(&scene.finish());
    assert_eq!(tags(&list), vec![1, 2]);
    assert_eq!(fills(&list)[0], Rect::new(0.0, 0.0, 800.0, 600.0));
    assert_eq!(fills(&list)[1], Rect::new(8.0, 8.0, 100.0, 50.0));
}

#[test]
fn the_flagged_body_background_propagates_when_the_root_has_none() {
    let mut scene = Scene::new();
    scene.add_kind(
        scene.root,
        FragmentKind::Box,
        Rect::new(8.0, 8.0, 100.0, 50.0),
        filled(tag(2)),
        FragFlags::CANVAS_SOURCE,
    );
    let list = build_list(&scene.finish());
    // One canvas fill and no second paint by the body itself.
    assert_eq!(tags(&list), vec![2]);
    assert_eq!(fills(&list), vec![Rect::new(0.0, 0.0, 800.0, 600.0)]);
}

#[test]
fn a_transparent_root_and_body_leave_the_canvas_alone() {
    let mut scene = Scene::with_root(filled(Color::TRANSPARENT));
    scene.add(
        scene.root,
        Rect::new(8.0, 8.0, 100.0, 50.0),
        filled(Color::TRANSPARENT),
    );
    assert!(build_list(&scene.finish()).items.is_empty());
}

#[test]
fn the_canvas_covers_content_scrolled_past_the_viewport() {
    let mut scene = Scene::with_root(filled(tag(1)));
    scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        PaintStyle::default(),
    );
    let mut tree = scene.finish();
    tree.scroll_extent = Rect::new(0.0, 0.0, 800.0, 3000.0);
    assert_eq!(
        fills(&build_list(&tree))[0],
        Rect::new(0.0, 0.0, 800.0, 3000.0)
    );
}

#[test]
fn background_clip_selects_the_box() {
    let mut scene = Scene::new();
    let id = scene.add(
        scene.root,
        Rect::new(10.0, 20.0, 100.0, 60.0),
        style(|s| {
            s.background = vec![
                clipped_layer(tag(3), BoxArea::ContentBox),
                clipped_layer(tag(2), BoxArea::PaddingBox),
                clipped_layer(tag(1), BoxArea::BorderBox),
            ];
        }),
    );
    scene.edit(id, |f| {
        f.border = Edges {
            top: 2.0,
            right: 4.0,
            bottom: 6.0,
            left: 8.0,
        };
        f.padding = Edges::all(5.0);
    });
    let list = build_list(&scene.finish());
    assert_eq!(tags(&list), vec![1, 2, 3]);
    assert_eq!(
        fills(&list),
        vec![
            Rect::new(10.0, 20.0, 100.0, 60.0),
            Rect::new(18.0, 22.0, 88.0, 52.0),
            Rect::new(23.0, 27.0, 78.0, 42.0),
        ]
    );
}

#[test]
fn percentage_radii_resolve_per_axis_and_shrink_for_inner_clips() {
    let mut scene = Scene::new();
    let id = scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 100.0, 50.0),
        style(|s| {
            s.background = vec![
                clipped_layer(tag(2), BoxArea::PaddingBox),
                clipped_layer(tag(1), BoxArea::BorderBox),
            ];
            s.border.radii = [LengthPercent::fraction(0.5); 8];
        }),
    );
    scene.edit(id, |f| f.border = Edges::all(10.0));
    let list = build_list(&scene.finish());
    let radii: Vec<[f32; 8]> = list
        .items
        .iter()
        .filter_map(|item| match item {
            Item::FillRRect { radii, .. } => Some(*radii),
            _ => None,
        })
        .collect();
    assert_eq!(radii[0], [50.0, 25.0, 50.0, 25.0, 50.0, 25.0, 50.0, 25.0]);
    assert_eq!(radii[1], [40.0, 15.0, 40.0, 15.0, 40.0, 15.0, 40.0, 15.0]);
}

#[test]
fn overlapping_radii_shrink_together() {
    let mut scene = Scene::new();
    scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 100.0, 100.0),
        style(|s| {
            s.background = vec![layer(tag(1))];
            s.border.radii = [LengthPercent::px(80.0); 8];
        }),
    );
    let list = build_list(&scene.finish());
    assert!(matches!(
        list.items[0],
        Item::FillRRect { radii, .. } if radii == [50.0; 8]
    ));
}

#[test]
fn border_items_carry_widths_colours_styles_and_radii() {
    let mut scene = Scene::new();
    let id = scene.add(
        scene.root,
        Rect::new(5.0, 6.0, 80.0, 40.0),
        style(|s| {
            s.border.styles = [
                BorderStyle::Solid,
                BorderStyle::None,
                BorderStyle::Dashed,
                BorderStyle::Hidden,
            ];
            s.border.colors = [tag(1), tag(2), tag(3), tag(4)];
            s.border.radii[0] = LengthPercent::px(6.0);
            s.border.radii[1] = LengthPercent::fraction(0.25);
        }),
    );
    scene.edit(id, |f| {
        f.border = Edges {
            top: 1.0,
            right: 2.0,
            bottom: 3.0,
            left: 4.0,
        }
    });
    let list = build_list(&scene.finish());
    let [
        Item::Border {
            rect,
            widths,
            colors,
            styles,
            radii,
        },
    ] = &list.items[..]
    else {
        panic!("expected exactly one border item, got {:?}", list.items);
    };
    assert_eq!(*rect, Rect::new(5.0, 6.0, 80.0, 40.0));
    assert_eq!(
        *widths,
        Edges {
            top: 1.0,
            right: 0.0,
            bottom: 3.0,
            left: 0.0
        }
    );
    assert_eq!(colors[0], tag(1));
    assert_eq!(colors[2], tag(3));
    assert_eq!(styles[1], BorderStyle::None);
    assert_eq!(styles[2], BorderStyle::Dashed);
    assert_eq!(radii[0], 6.0);
    assert_eq!(radii[1], 10.0);
}

#[test]
fn borders_without_visible_sides_emit_nothing() {
    let mut scene = Scene::new();
    let hidden = scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        style(|s| s.border.styles = [BorderStyle::None; 4]),
    );
    scene.edit(hidden, |f| f.border = Edges::all(3.0));
    let clear = scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        style(|s| {
            s.border.styles = [BorderStyle::Solid; 4];
            s.border.colors = [Color::TRANSPARENT; 4];
        }),
    );
    scene.edit(clear, |f| f.border = Edges::all(3.0));
    assert!(build_list(&scene.finish()).items.is_empty());
}

fn outlined(color: Color) -> PaintStyle {
    style(|s| {
        s.background = vec![layer(color)];
        s.outline = Some(OutlinePaint {
            width: 2.0,
            style: BorderStyle::Solid,
            color: tag(9),
            offset: 1.0,
        });
    })
}

/// `B` for an outline, a tag for a fill.
fn outline_sequence(list: &DisplayList) -> Vec<String> {
    list.items
        .iter()
        .filter_map(|item| match item {
            Item::Border { .. } => Some("B".to_string()),
            Item::FillRect { color, .. } => Some(color.to_premultiplied_rgba8()[0].to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn in_flow_outlines_follow_the_flow_and_precede_positioned_layers() {
    let mut scene = Scene::new();
    let root = scene.root;
    scene.add(root, Rect::new(10.0, 10.0, 20.0, 20.0), outlined(tag(1)));
    scene.add(root, Rect::new(40.0, 10.0, 20.0, 20.0), filled(tag(3)));
    scene.add(
        root,
        Rect::new(50.0, 10.0, 20.0, 20.0),
        positioned(tag(2), Some(1)),
    );
    let list = build_list(&scene.finish());
    assert_eq!(outline_sequence(&list), ["1", "3", "B", "2"]);
    let Item::Border { rect, widths, .. } = &list.items[2] else {
        unreachable!()
    };
    assert_eq!(*rect, Rect::new(7.0, 7.0, 26.0, 26.0));
    assert_eq!(*widths, Edges::all(2.0));
}

#[test]
fn a_group_paints_its_content_then_in_flow_outlines_then_its_own() {
    let mut scene = Scene::new();
    let group = scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 100.0, 100.0),
        style(|s| {
            s.background = vec![layer(tag(1))];
            s.position = PositionKind::Relative;
            s.outline = outlined(tag(0)).outline;
        }),
    );
    scene.add(group, Rect::new(5.0, 5.0, 20.0, 20.0), outlined(tag(2)));
    scene.add(group, Rect::new(30.0, 5.0, 20.0, 20.0), filled(tag(3)));
    let list = build_list(&scene.finish());
    assert_eq!(outline_sequence(&list), ["1", "2", "3", "B", "B"]);
    let Item::Border { rect, .. } = &list.items[4] else {
        unreachable!()
    };
    assert_eq!(*rect, Rect::new(-3.0, -3.0, 106.0, 106.0));
}

#[test]
fn hidden_boxes_paint_nothing_but_visible_children_do() {
    let mut scene = Scene::new();
    let hidden = style(|s| {
        s.visibility_hidden = true;
        s.background = vec![layer(tag(1))];
        s.border.styles = [BorderStyle::Solid; 4];
    });
    let parent = scene.add(scene.root, Rect::new(0.0, 0.0, 100.0, 100.0), hidden);
    scene.edit(parent, |f| f.border = Edges::all(1.0));
    scene.add(parent, Rect::new(10.0, 10.0, 20.0, 20.0), filled(tag(2)));
    let list = build_list(&scene.finish());
    assert_eq!(tags(&list), vec![2]);
    assert!(
        !list
            .items
            .iter()
            .any(|item| matches!(item, Item::Border { .. }))
    );
}

#[test]
fn transparent_subtrees_are_skipped_entirely() {
    let mut scene = Scene::new();
    let gone = scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 100.0, 100.0),
        style(|s| {
            s.opacity = 0.0;
            s.background = vec![layer(tag(1))];
        }),
    );
    scene.add(
        gone,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        positioned(tag(2), Some(5)),
    );
    scene.add(scene.root, Rect::new(0.0, 0.0, 10.0, 10.0), filled(tag(3)));
    assert_eq!(tags(&build_list(&scene.finish())), vec![3]);
}

#[test]
fn coordinates_are_absolute_page_positions() {
    let mut scene = Scene::new();
    let outer = scene.add(
        scene.root,
        Rect::new(10.0, 20.0, 200.0, 200.0),
        PaintStyle::default(),
    );
    let inner = scene.add(
        outer,
        Rect::new(5.0, 6.0, 100.0, 100.0),
        PaintStyle::default(),
    );
    scene.add(inner, Rect::new(1.0, 2.0, 10.0, 10.0), filled(tag(1)));
    assert_eq!(
        fills(&build_list(&scene.finish())),
        vec![Rect::new(16.0, 28.0, 10.0, 10.0)]
    );
}

#[test]
fn empty_boxes_and_clear_backgrounds_emit_nothing() {
    let mut scene = Scene::new();
    scene.add(scene.root, Rect::new(0.0, 0.0, 0.0, 10.0), filled(tag(1)));
    scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        filled(Color::TRANSPARENT),
    );
    assert!(build_list(&scene.finish()).items.is_empty());
}

#[test]
fn context_creation_follows_the_style() {
    let creates = |edit: &dyn Fn(&mut PaintStyle)| {
        let mut scene = Scene::new();
        let id = scene.add(scene.root, Rect::default(), style(|s| edit(s)));
        let tree = scene.finish();
        creates_context(tree.get(id))
    };
    assert!(!creates(&|_| {}));
    assert!(!creates(&|s| s.z_index = Some(3)));
    assert!(!creates(&|s| s.position = PositionKind::Relative));
    assert!(creates(&|s| {
        s.position = PositionKind::Relative;
        s.z_index = Some(0);
    }));
    assert!(creates(&|s| s.opacity = 0.5));
    assert!(creates(&|s| s.transform = Some(Affine::IDENTITY)));
    assert!(creates(&|s| s.position = PositionKind::Fixed));
    assert!(creates(&|s| s.isolate = true));
    assert!(creates(
        &|s| s.filter = vec![tenta_layout::FilterOp::Blur(1.0)]
    ));
}

fn gradient_style(layers: Vec<BackgroundLayer>) -> PaintStyle {
    style(|s| s.background = layers)
}

fn gradient_layer(size: BackgroundSize, repeat: RepeatKind) -> BackgroundLayer {
    BackgroundLayer {
        content: BackgroundContent::Gradient(GradientPaint {
            angle_deg: 90.0,
            stops: vec![
                ColorStop {
                    position: LengthPercent::fraction(0.0),
                    color: tag(1),
                },
                ColorStop {
                    position: LengthPercent::fraction(1.0),
                    color: tag(2),
                },
            ],
            repeating: false,
        }),
        repeat: (repeat, repeat),
        size,
        ..layer(Color::TRANSPARENT)
    }
}

#[test]
fn a_covering_gradient_is_one_item_with_the_clip_radii() {
    let mut scene = Scene::new();
    scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 100.0, 40.0),
        style(|s| {
            s.background = vec![gradient_layer(BackgroundSize::Auto, RepeatKind::Repeat)];
            s.border.radii = [LengthPercent::px(5.0); 8];
        }),
    );
    let list = build_list(&scene.finish());
    let [
        Item::LinearGradient {
            rect,
            radii,
            angle,
            stops,
            repeat,
        },
    ] = &list.items[..]
    else {
        panic!("{:?}", list.items);
    };
    assert_eq!(*rect, Rect::new(0.0, 0.0, 100.0, 40.0));
    assert_eq!((*radii, *angle, *repeat), ([5.0; 8], 90.0, false));
    assert_eq!(stops, &vec![(0.0, tag(1)), (1.0, tag(2))]);
}

#[test]
fn tiled_gradients_emit_each_tile_inside_one_clip() {
    let mut scene = Scene::new();
    let size =
        BackgroundSize::Explicit(Some(LengthPercent::px(40.0)), Some(LengthPercent::px(40.0)));
    scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 100.0, 40.0),
        gradient_style(vec![gradient_layer(size, RepeatKind::Repeat)]),
    );
    let list = build_list(&scene.finish());
    let tiles: Vec<Rect> = list
        .items
        .iter()
        .filter_map(|item| match item {
            Item::LinearGradient { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(
        tiles,
        vec![
            Rect::new(0.0, 0.0, 40.0, 40.0),
            Rect::new(40.0, 0.0, 40.0, 40.0),
            Rect::new(80.0, 0.0, 40.0, 40.0),
        ]
    );
    assert!(matches!(list.items.first(), Some(Item::PushClip { .. })));
    assert!(matches!(list.items.last(), Some(Item::PopClip)));
    assert!(list.is_balanced());
}

#[test]
fn no_repeat_and_space_place_tiles_inside_the_area() {
    let mut scene = Scene::new();
    let size =
        BackgroundSize::Explicit(Some(LengthPercent::px(30.0)), Some(LengthPercent::px(40.0)));
    let mut once = gradient_layer(size, RepeatKind::NoRepeat);
    once.position = (LengthPercent::fraction(1.0), LengthPercent::default());
    let spaced = gradient_layer(size, RepeatKind::Space);
    scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 100.0, 40.0),
        gradient_style(vec![once]),
    );
    scene.add(
        scene.root,
        Rect::new(0.0, 100.0, 100.0, 40.0),
        gradient_style(vec![spaced]),
    );
    let list = build_list(&scene.finish());
    let xs: Vec<(f32, f32)> = list
        .items
        .iter()
        .filter_map(|item| match item {
            Item::LinearGradient { rect, .. } => Some((rect.x, rect.y)),
            _ => None,
        })
        .collect();
    // One tile flush right; then three tiles spaced to span the 100px area.
    assert_eq!(
        xs,
        vec![(70.0, 0.0), (0.0, 100.0), (35.0, 100.0), (70.0, 100.0)]
    );
    assert!(list.is_balanced());
}

#[test]
fn image_layers_are_not_emitted_without_a_resolvable_image() {
    let mut scene = Scene::new();
    let mut image = layer(Color::TRANSPARENT);
    image.content = BackgroundContent::Image(tenta_layout::ImageKey(Arc::from("a.png")));
    scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        style(|s| s.background = vec![image, layer(tag(1))]),
    );
    assert_eq!(tags(&build_list(&scene.finish())), vec![1]);
}

/// `[`/`]` are clip push/pop, `<o`/`o>` opacity, `<t`/`t>` transform, `B` a
/// border, and a fill is its tag.
fn sequence(list: &DisplayList) -> Vec<String> {
    list.items
        .iter()
        .map(|item| match item {
            Item::FillRect { color, .. } | Item::FillRRect { color, .. } => {
                color.to_premultiplied_rgba8()[0].to_string()
            }
            Item::PushClip { .. } => "[".into(),
            Item::PopClip => "]".into(),
            Item::PushOpacity(_) => "<o".into(),
            Item::PopOpacity => "o>".into(),
            Item::PushTransform(_) => "<t".into(),
            Item::PopTransform => "t>".into(),
            Item::Border { .. } => "B".into(),
            other => format!("{other:?}"),
        })
        .collect()
}

#[test]
fn a_positive_z_child_is_still_clipped_by_the_box_it_was_hoisted_out_of() {
    let mut scene = Scene::new();
    let root = scene.root;
    scene.add(root, Rect::new(0.0, 0.0, 10.0, 10.0), filled(tag(4)));
    let clip = scene.add_clipping(root, Rect::new(20.0, 0.0, 50.0, 50.0), tag(1));
    scene.add(clip, Rect::new(0.0, 0.0, 10.0, 10.0), filled(tag(3)));
    scene.add(
        clip,
        Rect::new(5.0, 5.0, 80.0, 80.0),
        positioned(tag(2), Some(1)),
    );
    let list = build_recorded(&scene.finish());
    // The unclipped sibling is outside; the in-flow child and the hoisted child
    // share the clip, and the clip is the parent's padding box.
    assert_eq!(sequence(&list), ["4", "1", "[", "3", "2", "]"]);
    assert!(matches!(
        list.items[2],
        Item::PushClip { rect, .. } if rect == Rect::new(20.0, 0.0, 50.0, 50.0)
    ));
}

#[test]
fn nested_clips_open_outermost_first_and_close_in_reverse() {
    let mut scene = Scene::new();
    let outer = scene.add_clipping(scene.root, Rect::new(10.0, 10.0, 200.0, 200.0), tag(1));
    let inner = scene.add_clipping(outer, Rect::new(5.0, 5.0, 100.0, 100.0), tag(2));
    scene.add(
        inner,
        Rect::new(0.0, 0.0, 300.0, 300.0),
        positioned(tag(3), Some(1)),
    );
    let list = build_recorded(&scene.finish());
    assert_eq!(sequence(&list), ["1", "[", "2", "[", "3", "]", "]"]);
    assert!(matches!(
        (&list.items[1], &list.items[3]),
        (Item::PushClip { rect: a, .. }, Item::PushClip { rect: b, .. })
            if *a == Rect::new(10.0, 10.0, 200.0, 200.0) && *b == Rect::new(15.0, 15.0, 100.0, 100.0)
    ));
}

#[test]
fn a_float_inside_a_clip_is_clipped_and_negative_z_children_too() {
    let mut scene = Scene::new();
    let clip = scene.add_clipping(scene.root, Rect::new(0.0, 0.0, 50.0, 50.0), tag(1));
    scene.float(clip, Rect::new(0.0, 0.0, 10.0, 10.0), filled(tag(2)));
    scene.add(
        clip,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        positioned(tag(3), Some(-1)),
    );
    let list = build_recorded(&scene.finish());
    // The negative-z child is painted before the clip box's own background
    // (it joins the root context) but still under the clip.
    assert_eq!(sequence(&list), ["[", "3", "]", "1", "[", "2", "]"]);
}

#[test]
fn siblings_under_one_clip_share_a_single_push_pop_pair() {
    let mut scene = Scene::new();
    let clip = scene.add_clipping(scene.root, Rect::new(0.0, 0.0, 100.0, 100.0), tag(1));
    for t in 2..=6 {
        scene.add(clip, Rect::new(0.0, 0.0, 10.0, 10.0), filled(tag(t)));
    }
    scene.add(
        clip,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        positioned(tag(7), Some(2)),
    );
    let list = build_recorded(&scene.finish());
    assert_eq!(
        sequence(&list),
        ["1", "[", "2", "3", "4", "5", "6", "7", "]"]
    );
}

#[test]
fn clips_sit_outside_an_effect_group_they_enclose_and_inside_one_they_belong_to() {
    let mut scene = Scene::new();
    let clip = scene.add_clipping(scene.root, Rect::new(0.0, 0.0, 100.0, 100.0), tag(1));
    let group = scene.add(
        clip,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        style(|s| {
            s.background = vec![layer(tag(2))];
            s.position = PositionKind::Relative;
            s.opacity = 0.5;
        }),
    );
    scene.edit(group, |f| f.flags |= FragFlags::CLIPS_OVERFLOW);
    scene.add(group, Rect::new(0.0, 0.0, 5.0, 5.0), filled(tag(3)));
    scene.add(
        scene.root,
        Rect::new(0.0, 0.0, 5.0, 5.0),
        style(|s| {
            s.background = vec![layer(tag(4))];
            s.transform = Some(Affine::translation(3.0, 4.0));
        }),
    );
    let list = build_recorded(&scene.finish());
    assert_eq!(
        sequence(&list),
        [
            "1", "[", "<o", "2", "[", "3", "]", "o>", "]", "<t", "4", "t>"
        ]
    );
    assert!(matches!(
        list.items[9],
        Item::PushTransform([1.0, 0.0, 0.0, 1.0, 3.0, 4.0])
    ));
}

#[test]
fn text_paints_above_all_block_backgrounds() {
    let mut scene = Scene::new();
    let root = scene.root;
    let first = scene.add(root, Rect::new(0.0, 0.0, 100.0, 20.0), filled(tag(1)));
    let line = scene.add_kind(
        first,
        FragmentKind::Line,
        Rect::new(0.0, 0.0, 100.0, 20.0),
        PaintStyle::default(),
        FragFlags::empty(),
    );
    scene.add_kind(
        line,
        FragmentKind::Text(tenta_layout::TextFragment {
            text_range: 0..0,
            runs: Default::default(),
            baseline: 0.0,
        }),
        Rect::new(0.0, 0.0, 10.0, 10.0),
        style(|s| s.color = tag(7)),
        FragFlags::empty(),
    );
    scene.add(root, Rect::new(0.0, 20.0, 100.0, 20.0), filled(tag(2)));
    assert_eq!(sequence(&build_recorded(&scene.finish())), ["1", "2", "7"]);
}

#[test]
fn z_zero_layers_keep_tree_order_through_positioned_auto_parents() {
    let mut scene = Scene::new();
    let root = scene.root;
    let auto = scene.add(
        root,
        Rect::new(0.0, 0.0, 100.0, 100.0),
        positioned(tag(1), None),
    );
    scene.add(
        auto,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        positioned(tag(2), Some(0)),
    );
    let inner = scene.add(
        auto,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        positioned(tag(3), None),
    );
    scene.add(
        inner,
        Rect::new(0.0, 0.0, 5.0, 5.0),
        positioned(tag(4), Some(0)),
    );
    scene.add(
        root,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        positioned(tag(5), Some(0)),
    );
    assert_eq!(tags(&build_list(&scene.finish())), vec![1, 2, 3, 4, 5]);
}

#[test]
fn a_centred_no_repeat_gradient_on_the_body_positions_against_the_root() {
    let mut scene = Scene::new();
    let mut layer = gradient_layer(
        BackgroundSize::Explicit(Some(LengthPercent::px(20.0)), Some(LengthPercent::px(20.0))),
        RepeatKind::NoRepeat,
    );
    layer.position = (LengthPercent::fraction(0.5), LengthPercent::fraction(0.5));
    scene.add_kind(
        scene.root,
        FragmentKind::Box,
        Rect::new(8.0, 8.0, 100.0, 50.0),
        gradient_style(vec![layer]),
        FragFlags::CANVAS_SOURCE,
    );
    let list = build_list(&scene.finish());
    let [Item::LinearGradient { rect, .. }] = &list.items[..] else {
        panic!("{:?}", list.items);
    };
    assert_eq!(*rect, Rect::new(390.0, 290.0, 20.0, 20.0));
}

#[test]
fn an_unflagged_first_child_does_not_feed_the_canvas() {
    let mut scene = Scene::new();
    scene.add(scene.root, Rect::new(0.0, 0.0, 10.0, 10.0), filled(tag(2)));
    assert_eq!(
        fills(&build_list(&scene.finish())),
        vec![Rect::new(0.0, 0.0, 10.0, 10.0)]
    );
}

/// Builds a random tree and checks the invariants of the painting order.
#[test]
fn random_trees_build_balanced_lists_in_layer_and_tree_order() {
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move |bound: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % bound
    };
    for _ in 0..200 {
        let mut scene = Scene::new();
        let mut ids = vec![scene.root];
        let mut parents = vec![usize::MAX];
        let mut kids: Vec<Vec<usize>> = vec![Vec::new()];
        for index in 1..=60usize {
            let parent = next(ids.len() as u64) as usize;
            let paint = style(|s| {
                let mut layers = vec![layer(tag(index as u8))];
                if next(4) == 0 {
                    layers.insert(
                        0,
                        gradient_layer(
                            BackgroundSize::Explicit(Some(LengthPercent::px(7.0)), None),
                            RepeatKind::Repeat,
                        ),
                    );
                }
                s.background = layers;
                if next(3) == 0 {
                    s.position = PositionKind::Absolute;
                    s.z_index = [None, Some(-1), Some(0), Some(1), Some(2)][next(5) as usize];
                }
                if next(4) == 0 {
                    s.border.radii = [LengthPercent::px(4.0); 8];
                }
                s.opacity = [1.0, 1.0, 1.0, 0.5, 0.0][next(5) as usize];
                if next(8) == 0 {
                    s.transform = Some(Affine::translation(1.0, 1.0));
                }
            });
            let rect = Rect::new(next(30) as f32, next(30) as f32, 20.0, 20.0);
            let mut flags = FragFlags::empty();
            if next(5) == 0 {
                flags |= FragFlags::FLOAT;
            }
            if next(3) == 0 {
                flags |= FragFlags::CLIPS_OVERFLOW;
            }
            let id = scene.add_kind(ids[parent], FragmentKind::Box, rect, paint, flags);
            ids.push(id);
            parents.push(parent);
            kids.push(Vec::new());
            kids[parent].push(index);
        }
        let tree = scene.finish();
        let list = build_recorded(&tree);
        assert!(list.is_balanced());

        let mut preorder = vec![0usize; ids.len()];
        let mut stack = vec![0usize];
        let mut counter = 0;
        while let Some(node) = stack.pop() {
            preorder[node] = counter;
            counter += 1;
            stack.extend(kids[node].iter().rev());
        }
        let context_of = |mut node: usize| {
            node = parents[node];
            while node != 0 && !creates_context(tree.get(ids[node])) {
                node = parents[node];
            }
            node
        };
        let key = |node: usize| {
            let frag = tree.get(ids[node]);
            if creates_context(frag) {
                Some(frag.paint.z_index.unwrap_or(0))
            } else if frag.paint.position != PositionKind::Static {
                Some(0)
            } else {
                None
            }
        };
        let position = |node: usize| {
            list.items.iter().position(|item| {
                matches!(item, Item::FillRect { color, .. } | Item::FillRRect { color, .. }
                    if color.to_premultiplied_rgba8()[0] == node as u8)
            })
        };
        let layered: Vec<usize> = (1..ids.len())
            .filter(|&n| key(n).is_some() && position(n).is_some())
            .collect();
        for &a in &layered {
            for &b in &layered {
                if context_of(a) != context_of(b) {
                    continue;
                }
                if (key(a), preorder[a]) < (key(b), preorder[b]) {
                    assert!(position(a) < position(b), "{a} should paint before {b}");
                }
            }
        }
    }
}

fn px(pix: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * pix.width() + x) * 4) as usize;
    pix.data()[offset..offset + 4]
        .try_into()
        .expect("four channels")
}

fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .expect("valid PNG header");
    let mut buffer = vec![0; reader.output_buffer_size().expect("a frame fits in memory")];
    let info = reader.next_frame(&mut buffer).expect("valid PNG frame");
    buffer.truncate(info.buffer_size());
    (info.width, info.height, buffer)
}

/// Compares with the blessed golden, within one unit per channel;
/// `UPDATE_GOLDEN=1` rewrites it.
fn assert_golden(name: &str, pix: &Pixmap) {
    let encoded = encode_png(pix, PngProfile::Fast);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/render-goldens")
        .join(format!("{name}.png"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &encoded).expect("golden is writable");
        return;
    }
    let golden = std::fs::read(&path)
        .unwrap_or_else(|err| panic!("missing golden {}: {err}", path.display()));
    let (w, h, expected) = decode_png(&golden);
    let (actual_w, actual_h, actual) = decode_png(&encoded);
    assert_eq!(
        (actual_w, actual_h),
        (w, h),
        "golden {name} has another size"
    );
    let worst = expected
        .iter()
        .zip(&actual)
        .map(|(e, a)| e.abs_diff(*a))
        .max()
        .unwrap_or(0);
    assert!(worst <= 1, "golden {name} differs by {worst} per channel");
}

fn render(tree: &FragmentTree, width: u32, height: u32) -> RasterOutput {
    let list = build_list(tree);
    let opts = RasterOptions {
        width,
        height,
        scale: 1.0,
        origin: (0.0, 0.0),
        background: None,
    };
    rasterize(&list, &opts, &ImageStore::new()).expect("valid size")
}

fn small_tree(root_style: PaintStyle) -> Scene {
    let mut scene = Scene::with_root(root_style);
    scene.edit(scene.root, |f| f.rect = Rect::new(0.0, 0.0, 120.0, 80.0));
    scene
}

#[test]
fn composed_boxes_rasterise_to_the_golden() {
    let mut scene = small_tree(filled(Color::from_rgba8(240, 240, 240, 255)));
    let root = scene.root;
    let rgb = |r, g, b| Color::from_rgba8(r, g, b, 255);
    // A rounded blue card with a dark border and a content-box inset layer.
    let card = scene.add(
        root,
        Rect::new(10.0, 10.0, 70.0, 50.0),
        style(|s| {
            s.background = vec![
                clipped_layer(rgb(255, 255, 255), BoxArea::ContentBox),
                clipped_layer(rgb(40, 90, 200), BoxArea::PaddingBox),
            ];
            s.border.styles = [BorderStyle::Solid; 4];
            s.border.colors = [rgb(20, 20, 60); 4];
            s.border.radii = [LengthPercent::px(12.0); 8];
            s.box_shadow = vec![ShadowPaint {
                offset: (2.0, 2.0),
                blur: 4.0,
                spread: 0.0,
                color: rgb(0, 0, 0),
                inset: false,
            }];
        }),
    );
    scene.edit(card, |f| {
        f.border = Edges::all(4.0);
        f.padding = Edges::all(8.0);
    });
    // A green square under the card, and a red one over it.
    scene.add(
        root,
        Rect::new(60.0, 30.0, 40.0, 40.0),
        positioned(rgb(30, 160, 60), Some(-1)),
    );
    scene.add(
        root,
        Rect::new(70.0, 15.0, 30.0, 30.0),
        positioned(rgb(220, 30, 30), Some(1)),
    );
    let out = render(&scene.finish(), 120, 80);
    let pix = out.pixmap;
    assert_eq!(
        out.stats.skipped.get("BoxShadow"),
        Some(&1),
        "skipped, and counted"
    );
    assert_eq!(px(&pix, 2, 2), [240, 240, 240, 255]);
    assert_eq!(px(&pix, 40, 35), [255, 255, 255, 255], "content box");
    assert_eq!(px(&pix, 20, 20), [40, 90, 200, 255], "padding ring");
    assert_eq!(px(&pix, 12, 35), [20, 20, 60, 255], "border");
    assert_eq!(px(&pix, 11, 11), [240, 240, 240, 255], "rounded-off corner");
    assert_eq!(
        px(&pix, 90, 60),
        [30, 160, 60, 255],
        "negative z, outside the card"
    );
    assert_eq!(
        px(&pix, 65, 55),
        [40, 90, 200, 255],
        "card is above the negative z box"
    );
    assert_eq!(px(&pix, 80, 25), [220, 30, 30, 255], "positive z on top");
    assert_golden("build-composed", &pix);
}

#[test]
fn the_body_background_fills_the_whole_canvas_in_the_golden() {
    let mut scene = small_tree(PaintStyle::default());
    scene.add_kind(
        scene.root,
        FragmentKind::Box,
        Rect::new(8.0, 8.0, 30.0, 20.0),
        filled(Color::from_rgba8(250, 220, 120, 255)),
        FragFlags::CANVAS_SOURCE,
    );
    let pix = render(&scene.finish(), 60, 40).pixmap;
    for (x, y) in [(0, 0), (59, 0), (0, 39), (59, 39), (20, 15)] {
        assert_eq!(px(&pix, x, y), [250, 220, 120, 255], "pixel {x},{y}");
    }
    assert_golden("build-canvas-body", &pix);
}

#[test]
fn a_tiled_gradient_over_a_grey_box_with_an_inset_shadow_rasterises_to_the_golden() {
    let mut scene = small_tree(filled(Color::from_rgba8(250, 250, 250, 255)));
    let size =
        BackgroundSize::Explicit(Some(LengthPercent::px(20.0)), Some(LengthPercent::px(20.0)));
    let grey = Color::from_rgba8(160, 160, 160, 255);
    scene.add(
        scene.root,
        Rect::new(10.0, 10.0, 60.0, 40.0),
        style(|s| {
            s.background = vec![gradient_layer(size, RepeatKind::Repeat), layer(grey)];
            s.box_shadow = vec![ShadowPaint {
                offset: (0.0, 0.0),
                blur: 6.0,
                spread: 0.0,
                color: Color::from_rgba8(0, 0, 0, 255),
                inset: true,
            }];
        }),
    );
    let out = render(&scene.finish(), 80, 60);
    let skipped: Vec<_> = out.stats.skipped.iter().map(|(k, v)| (*k, *v)).collect();
    assert_eq!(skipped, [("BoxShadow", 1)]);
    // The tiles are drawn: the centre shows the gradient, not the grey layer.
    assert_ne!(px(&out.pixmap, 40, 30), [160, 160, 160, 255]);
    assert_golden("build-tiled-gradient-inset", &out.pixmap);
}

/// `cargo nextest run -p tenta-paint --release --run-ignored only
/// --no-capture build_5000_boxes`
#[test]
#[ignore = "timing run, release only"]
fn build_5000_boxes_timing() {
    let mut scene = Scene::new();
    let root = scene.root;
    let mut parent = root;
    for i in 0..5000u32 {
        if i % 50 == 0 {
            parent = scene.add(root, Rect::new(0.0, i as f32, 800.0, 100.0), filled(tag(1)));
            if i % 100 == 0 {
                scene.edit(parent, |f| f.flags |= FragFlags::CLIPS_OVERFLOW);
            }
        }
        let rect = Rect::new((i % 40) as f32 * 10.0, (i % 7) as f32 * 3.0, 9.0, 9.0);
        let paint = style(|s| {
            s.background = vec![layer(tag((i % 200) as u8))];
            s.border.styles = [BorderStyle::Solid; 4];
            if i % 10 == 0 {
                s.border.radii = [LengthPercent::px(3.0); 8];
            }
            if i % 25 == 0 {
                s.position = PositionKind::Absolute;
                s.z_index = Some((i % 5) as i32);
            }
            if i % 125 == 0 {
                s.opacity = 0.5;
            }
        });
        let id = scene.add(parent, rect, paint);
        scene.edit(id, |f| f.border = Edges::all(1.0));
    }
    let tree = scene.finish();
    let mut micros: Vec<f64> = (0..30)
        .map(|_| {
            let start = Instant::now();
            let list = build_recorded(&tree);
            let elapsed = start.elapsed().as_secs_f64() * 1e6;
            assert!(list.items.len() > 5000);
            assert!(list.is_balanced());
            elapsed
        })
        .collect();
    micros.sort_by(f64::total_cmp);
    println!(
        "build 5000 boxes (with clips and opacity groups): p50 {:.0} us, p95 {:.0} us over {} runs",
        micros[micros.len() / 2],
        micros[micros.len() * 95 / 100],
        micros.len()
    );
}

// ------------------------------------------------------------------ images

mod images {
    use std::collections::HashMap;

    use tenta_layout::{ImageKey, IntrinsicSize, ObjectFit, ReplacedKind};

    use super::*;
    use crate::build::{DrawableImage, ImageBindings};
    use crate::image::{DecodeParams, DecodedImage, ImageId, StoreKey};

    #[derive(Default)]
    struct Bindings {
        nodes: HashMap<NodeId, DrawableImage>,
        urls: HashMap<ImageKey, DrawableImage>,
        broken: Vec<NodeId>,
    }

    impl ImageBindings for Bindings {
        fn node_image(&self, node: NodeId) -> Option<DrawableImage> {
            self.nodes.get(&node).copied()
        }

        fn url_image(&self, key: &ImageKey) -> Option<DrawableImage> {
            self.urls.get(key).copied()
        }

        fn node_broken(&self, node: NodeId) -> bool {
            self.broken.contains(&node)
        }
    }

    fn natural(w: f32, h: f32) -> IntrinsicSize {
        IntrinsicSize {
            width: Some(w),
            height: Some(h),
            ratio: Some(w / h),
        }
    }

    /// A store holding one 2x2 image, and its handle.
    fn store() -> (ImageStore, ImageId) {
        let mut store = ImageStore::new();
        let image = DecodedImage {
            width: 2,
            height: 2,
            rgba: vec![255; 16].into(),
            source_width: 2,
            source_height: 2,
        };
        let key = StoreKey {
            url: ImageKey("test:image".into()),
            params: DecodeParams::default(),
        };
        let id = store.insert(key, image, 1.0).expect("fits");
        (store, id)
    }

    fn list(tree: &FragmentTree, store: &ImageStore, bindings: &Bindings) -> DisplayList {
        build(
            tree,
            &PaintContext {
                images: store,
                bindings,
            },
        )
    }

    fn images_of(list: &DisplayList) -> Vec<Rect> {
        list.items
            .iter()
            .filter_map(|item| match item {
                Item::Image { dest, .. } => Some(*dest),
                _ => None,
            })
            .collect()
    }

    /// A replaced image fragment with `border` and `padding` of 2 and 3.
    fn image_tree(fit: ObjectFit, radius: f32) -> FragmentTree {
        let mut scene = Scene::new();
        let id = scene.add_kind(
            scene.root,
            FragmentKind::Replaced(ReplacedKind::Image),
            Rect::new(10.0, 10.0, 100.0, 60.0),
            style(|s| {
                s.object_fit = fit;
                s.border.radii = [LengthPercent::px(radius); 8];
            }),
            FragFlags::empty(),
        );
        scene.edit(id, |f| {
            f.node = NodeId(42);
            f.border = Edges::all(2.0);
            f.padding = Edges::all(3.0);
        });
        scene.finish()
    }

    fn bound(store_id: ImageId, size: (f32, f32)) -> Bindings {
        let mut bindings = Bindings::default();
        bindings.nodes.insert(
            NodeId(42),
            DrawableImage {
                id: store_id,
                natural: natural(size.0, size.1),
            },
        );
        bindings
    }

    #[test]
    fn a_replaced_image_fills_its_content_box() {
        let (store, id) = store();
        let out = list(
            &image_tree(ObjectFit::Fill, 0.0),
            &store,
            &bound(id, (2.0, 2.0)),
        );
        // 100x60 border box, 2 border and 3 padding on each side.
        assert_eq!(images_of(&out), [Rect::new(15.0, 15.0, 90.0, 50.0)]);
        assert!(
            !out.items.iter().any(|i| matches!(i, Item::PushClip { .. })),
            "an image inside its box needs no clip"
        );
    }

    #[test]
    fn cover_overflows_the_content_box_and_is_clipped_to_it() {
        let (store, id) = store();
        let out = list(
            &image_tree(ObjectFit::Cover, 0.0),
            &store,
            &bound(id, (2.0, 2.0)),
        );
        assert_eq!(images_of(&out), [Rect::new(15.0, -5.0, 90.0, 90.0)]);
        let kinds: Vec<_> = out
            .items
            .iter()
            .filter(|i| {
                matches!(
                    i,
                    Item::PushClip { .. } | Item::Image { .. } | Item::PopClip
                )
            })
            .collect();
        assert!(
            matches!(kinds[0], Item::PushClip { rect, .. } if *rect == Rect::new(15.0, 15.0, 90.0, 50.0))
        );
        assert!(matches!(kinds[1], Item::Image { .. }));
        assert!(matches!(kinds[2], Item::PopClip));
        assert!(out.is_balanced());
    }

    #[test]
    fn contain_centres_and_a_rounded_box_clips_even_when_it_fits() {
        let (store, id) = store();
        let contain = list(
            &image_tree(ObjectFit::Contain, 0.0),
            &store,
            &bound(id, (2.0, 2.0)),
        );
        assert_eq!(images_of(&contain), [Rect::new(35.0, 15.0, 50.0, 50.0)]);
        let rounded = list(
            &image_tree(ObjectFit::Fill, 8.0),
            &store,
            &bound(id, (2.0, 2.0)),
        );
        let clip = rounded.items.iter().find_map(|i| match i {
            Item::PushClip { radii, .. } => Some(*radii),
            _ => None,
        });
        // The inner radii: 8 less the border and the padding, floored at 0.
        assert_eq!(clip, Some([3.0; 8]));
    }

    #[test]
    fn an_unloaded_image_draws_nothing_and_a_broken_one_a_frame() {
        let (store, _) = store();
        let none = list(
            &image_tree(ObjectFit::Fill, 0.0),
            &store,
            &Bindings::default(),
        );
        assert!(images_of(&none).is_empty());
        assert!(!none.items.iter().any(|i| matches!(i, Item::Border { .. })));

        let broken = Bindings {
            broken: vec![NodeId(42)],
            ..Bindings::default()
        };
        let out = list(&image_tree(ObjectFit::Fill, 0.0), &store, &broken);
        let frames = out
            .items
            .iter()
            .filter(|i| matches!(i, Item::Border { .. }))
            .count();
        assert_eq!(frames, 1);
    }

    #[test]
    fn an_image_the_store_has_evicted_is_not_drawn() {
        let (store, id) = store();
        let gone = ImageId(id.0 + 100);
        let out = list(
            &image_tree(ObjectFit::Fill, 0.0),
            &store,
            &bound(gone, (2.0, 2.0)),
        );
        assert!(images_of(&out).is_empty());
    }

    fn background_tree(layer: BackgroundLayer) -> FragmentTree {
        let mut scene = Scene::new();
        scene.add(
            scene.root,
            Rect::new(10.0, 20.0, 100.0, 60.0),
            style(|s| s.background = vec![layer]),
        );
        scene.finish()
    }

    fn image_layer(
        size: BackgroundSize,
        repeat: RepeatKind,
        position: (LengthPercent, LengthPercent),
    ) -> BackgroundLayer {
        BackgroundLayer {
            content: BackgroundContent::Image(ImageKey("test:bg".into())),
            repeat: (repeat, repeat),
            position,
            size,
            ..layer(Color::TRANSPARENT)
        }
    }

    fn url_bindings(id: ImageId, size: (f32, f32)) -> Bindings {
        let mut bindings = Bindings::default();
        bindings.urls.insert(
            ImageKey("test:bg".into()),
            DrawableImage {
                id,
                natural: natural(size.0, size.1),
            },
        );
        bindings
    }

    const ORIGIN: (LengthPercent, LengthPercent) = (LengthPercent::px(0.0), LengthPercent::px(0.0));

    #[test]
    fn a_no_repeat_background_sits_at_its_position() {
        let (store, id) = store();
        let tree = background_tree(image_layer(
            BackgroundSize::Auto,
            RepeatKind::NoRepeat,
            (LengthPercent::fraction(1.0), LengthPercent::fraction(0.5)),
        ));
        let out = list(&tree, &store, &url_bindings(id, (20.0, 10.0)));
        // Right edge of a 100x60 box at (10, 20); vertically centred.
        assert_eq!(images_of(&out), [Rect::new(90.0, 45.0, 20.0, 10.0)]);
    }

    #[test]
    fn a_repeating_background_tiles_the_whole_box() {
        let (store, id) = store();
        let tree = background_tree(image_layer(
            BackgroundSize::Explicit(Some(LengthPercent::px(40.0)), Some(LengthPercent::px(30.0))),
            RepeatKind::Repeat,
            ORIGIN,
        ));
        let out = list(&tree, &store, &url_bindings(id, (2.0, 2.0)));
        let tiles = images_of(&out);
        // 3 columns (the last cut) by 2 rows.
        assert_eq!(tiles.len(), 6);
        assert_eq!(tiles[0], Rect::new(10.0, 20.0, 40.0, 30.0));
        assert!(
            out.items.iter().any(|i| matches!(i, Item::PushClip { .. })),
            "the edge tiles overflow"
        );
        assert!(out.is_balanced());
    }

    #[test]
    fn round_and_space_fit_whole_tiles() {
        let (store, id) = store();
        let explicit =
            BackgroundSize::Explicit(Some(LengthPercent::px(40.0)), Some(LengthPercent::px(60.0)));
        let round = list(
            &background_tree(image_layer(explicit, RepeatKind::Round, ORIGIN)),
            &store,
            &url_bindings(id, (2.0, 2.0)),
        );
        // 100 / 40 rounds to 3 tiles of 33.33.
        let tiles = images_of(&round);
        assert_eq!(tiles.len(), 3);
        assert!((tiles[0].w - 100.0 / 3.0).abs() < 0.01);
        let space = list(
            &background_tree(image_layer(explicit, RepeatKind::Space, ORIGIN)),
            &store,
            &url_bindings(id, (2.0, 2.0)),
        );
        // Two 40 px tiles with the 20 px left over between them.
        let tiles = images_of(&space);
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[1].x - tiles[0].x, 60.0);
    }

    #[test]
    fn cover_and_contain_scale_by_the_ratio() {
        let (store, id) = store();
        let at = |size| {
            let tree = background_tree(image_layer(size, RepeatKind::NoRepeat, ORIGIN));
            images_of(&list(&tree, &store, &url_bindings(id, (20.0, 20.0))))
        };
        assert_eq!(
            at(BackgroundSize::Contain),
            [Rect::new(10.0, 20.0, 60.0, 60.0)]
        );
        assert_eq!(
            at(BackgroundSize::Cover),
            [Rect::new(10.0, 20.0, 100.0, 100.0)]
        );
    }

    #[test]
    fn a_missing_background_image_and_an_absurd_tile_count_draw_nothing() {
        let (store, id) = store();
        let tree = background_tree(image_layer(
            BackgroundSize::Auto,
            RepeatKind::Repeat,
            ORIGIN,
        ));
        assert!(images_of(&list(&tree, &store, &Bindings::default())).is_empty());
        // A 1 px tile over a 100x60 box is 6000 tiles: one pattern fill, not
        // 6000 items.
        let tiny = list(&tree, &store, &url_bindings(id, (1.0, 1.0)));
        assert!(images_of(&tiny).is_empty());
        let patterns = tiny
            .items
            .iter()
            .filter(|i| matches!(i, Item::ImagePattern { .. }))
            .count();
        assert_eq!(patterns, 1);
    }

    #[test]
    fn display_list_pins_report_the_images_it_draws() {
        let (store, id) = store();
        let tree = background_tree(image_layer(
            BackgroundSize::Auto,
            RepeatKind::NoRepeat,
            ORIGIN,
        ));
        let out = list(&tree, &store, &url_bindings(id, (20.0, 10.0)));
        assert_eq!(out.image_ids(), [id]);
    }

    #[test]
    fn a_tile_below_the_float_step_or_half_a_pixel_draws_nothing_and_returns() {
        let (store, id) = store();
        // 0.002 px tiles over a box 130k px wide would never advance in f32.
        let mut scene = Scene::new();
        scene.add(
            scene.root,
            Rect::new(1_000_000.0, 0.0, 130_000.0, 60.0),
            style(|s| {
                s.background = vec![image_layer(
                    BackgroundSize::Explicit(
                        Some(LengthPercent::px(0.002)),
                        Some(LengthPercent::px(0.002)),
                    ),
                    RepeatKind::Repeat,
                    ORIGIN,
                )]
            }),
        );
        let out = list(&scene.finish(), &store, &url_bindings(id, (2.0, 2.0)));
        assert!(images_of(&out).is_empty());
        for repeat in [RepeatKind::Round, RepeatKind::Space] {
            let tree = background_tree(image_layer(
                BackgroundSize::Explicit(
                    Some(LengthPercent::px(0.01)),
                    Some(LengthPercent::px(0.01)),
                ),
                repeat,
                ORIGIN,
            ));
            assert!(images_of(&list(&tree, &store, &url_bindings(id, (2.0, 2.0)))).is_empty());
        }
    }

    #[test]
    fn round_scales_the_auto_axis_with_the_rounded_one() {
        let (store, id) = store();
        let layer = BackgroundLayer {
            repeat: (RepeatKind::Round, RepeatKind::NoRepeat),
            ..image_layer(
                BackgroundSize::Explicit(Some(LengthPercent::px(40.0)), None),
                RepeatKind::Repeat,
                ORIGIN,
            )
        };
        let out = list(
            &background_tree(layer),
            &store,
            &url_bindings(id, (20.0, 10.0)),
        );
        let tiles = images_of(&out);
        // 100 / 40 rounds to 3 tiles of 33.33; the height follows its ratio.
        assert_eq!(tiles.len(), 3);
        assert!((tiles[0].w - 100.0 / 3.0).abs() < 0.01);
        assert!(
            (tiles[0].h - tiles[0].w / 2.0).abs() < 0.01,
            "{:?}",
            tiles[0]
        );
    }

    #[test]
    fn a_stripe_repeated_down_a_tall_box_is_one_pattern_cut_to_its_column() {
        let (store, id) = store();
        let mut scene = Scene::new();
        scene.add(
            scene.root,
            Rect::new(10.0, 0.0, 100.0, 8000.0),
            style(|s| {
                s.background = vec![BackgroundLayer {
                    repeat: (RepeatKind::NoRepeat, RepeatKind::Repeat),
                    ..image_layer(
                        BackgroundSize::Explicit(
                            Some(LengthPercent::px(20.0)),
                            Some(LengthPercent::px(2.0)),
                        ),
                        RepeatKind::Repeat,
                        ORIGIN,
                    )
                }]
            }),
        );
        let out = list(&scene.finish(), &store, &url_bindings(id, (2.0, 2.0)));
        let patterns: Vec<_> = out
            .items
            .iter()
            .filter_map(|i| match i {
                Item::ImagePattern { area, tile, .. } => Some((*area, *tile)),
                _ => None,
            })
            .collect();
        // 4000 tiles tall, one 20 px column wide.
        assert_eq!(
            patterns,
            [(Rect::new(10.0, 0.0, 20.0, 8000.0), (20.0, 2.0))]
        );
        assert!(images_of(&out).is_empty());
        assert_eq!(out.image_ids(), [id]);
    }
}
