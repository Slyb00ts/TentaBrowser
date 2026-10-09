//! Proves paint can consume a fragment tree built through the layout contract
//! alone: a stand-in display-list builder walks a hand-built tree, with no
//! Stylo anywhere.

use std::sync::Arc;

use tenta_core::NodeId;
use tenta_layout::{FragFlags, FragId, Fragment, FragmentBuilder, FragmentKind, FragmentTree};
use tenta_paint::{
    BackgroundContent, BackgroundLayer, BackgroundSize, BoxArea, Color, DisplayList, Item,
    LengthPercent, PaintContext, PaintStyle, Rect, RepeatKind, build,
};

type BuildFn = fn(&FragmentTree, &PaintContext) -> DisplayList;

fn background(color: Color) -> BackgroundLayer {
    BackgroundLayer {
        content: BackgroundContent::Color(color),
        repeat: (RepeatKind::Repeat, RepeatKind::Repeat),
        position: (LengthPercent::default(), LengthPercent::default()),
        size: BackgroundSize::Auto,
        origin: BoxArea::BorderBox,
        clip: BoxArea::BorderBox,
    }
}

fn stub_build(frags: &FragmentTree, _cx: &PaintContext) -> DisplayList {
    fn walk(frags: &FragmentTree, id: FragId, items: &mut Vec<Item>) {
        let frag = frags.get(id);
        let rect = frags.absolute_border_box(id);
        let clips = frag.flags.contains(FragFlags::CLIPS_OVERFLOW);
        for layer in &frag.paint.background {
            if let BackgroundContent::Color(color) = layer.content {
                items.push(Item::FillRect { rect, color });
            }
        }
        if clips {
            items.push(Item::PushClip {
                rect,
                radii: [0.0; 8],
            });
        }
        for &child in &frag.children {
            walk(frags, child, items);
        }
        if clips {
            items.push(Item::PopClip);
        }
    }
    let mut items = Vec::new();
    walk(frags, frags.root, &mut items);
    DisplayList {
        items,
        bounds: frags.scroll_extent,
        ..DisplayList::default()
    }
}

#[test]
fn build_has_the_contract_signature() {
    let _: BuildFn = build;
}

#[test]
fn a_hand_built_fragment_tree_becomes_a_balanced_display_list() {
    let red = Color::from_rgba8(255, 0, 0, 255);
    let paint = Arc::new(PaintStyle {
        background: vec![background(red)],
        ..PaintStyle::default()
    });

    let mut out = FragmentBuilder::new();
    let mut root = Fragment::new(
        NodeId(1),
        FragmentKind::Box,
        Arc::new(PaintStyle::default()),
        Rect::new(0.0, 0.0, 200.0, 100.0),
    );
    root.flags = FragFlags::CLIPS_OVERFLOW;
    let root = out.push(root);
    let child = out.push(Fragment::new(
        NodeId(2),
        FragmentKind::Box,
        paint,
        Rect::new(10.0, 20.0, 50.0, 30.0),
    ));
    out.add_child(root, child);
    let extent = Rect::new(0.0, 0.0, 200.0, 100.0);
    let tree = out.finish(root, extent, extent);

    let list = (stub_build as BuildFn)(
        &tree,
        &PaintContext {
            images: &tenta_paint::ImageStore::new(),
        },
    );
    assert_eq!(
        list.items,
        vec![
            Item::PushClip {
                rect: extent,
                radii: [0.0; 8]
            },
            Item::FillRect {
                rect: Rect::new(10.0, 20.0, 50.0, 30.0),
                color: red
            },
            Item::PopClip,
        ]
    );
    assert!(list.is_balanced());
    assert_eq!(list.bounds, extent);
}
