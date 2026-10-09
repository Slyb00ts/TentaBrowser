//! Proves the layout contract is sufficient: a stand-in block algorithm and a
//! stand-in inline algorithm, written against nothing but the public
//! signatures, lay out a hand-built box tree into a fragment tree.

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

use std::sync::Arc;

use smallvec::SmallVec;
use style::properties::ComputedValues;
use style::properties::style_structs::Font;
use tenta_core::NodeId;
use tenta_css::cascade::{FontMetrics, FontMetricsData, Style, StyleEngine, StyleEnv};
use tenta_dom::Document;
use tenta_layout::{
    AvailableSize, BlockResult, BoxId, BoxKind, BoxTree, ConstraintSpace, FloatContext, FontSystem,
    FragFlags, Fragment, FragmentBuilder, FragmentKind, FragmentTree, InlineFormattingContext,
    InlineItem, InlineResult, LayoutBox, LayoutCx, MarginStrut, NoFloats, NoImages, PaintStyle,
    Rect, TextSource, layout, layout_block, layout_inline,
};

type BlockFn =
    fn(&mut LayoutCx, &BoxTree, BoxId, &ConstraintSpace, &mut FragmentBuilder) -> BlockResult;
type InlineFn = fn(
    &mut LayoutCx,
    &BoxTree,
    &InlineFormattingContext,
    &ConstraintSpace,
    &mut dyn FloatContext,
    &mut FragmentBuilder,
) -> InlineResult;

/// The shipped entry points must keep the signatures the stand-ins implement.
#[test]
fn entry_points_have_the_contract_signatures() {
    let _: BlockFn = layout_block;
    let _: InlineFn = layout_inline;
    let _: fn(&mut LayoutCx, &BoxTree) -> FragmentTree = layout;
    let _: fn(&mut LayoutCx, &BoxTree, &InlineFormattingContext) -> (f32, f32) =
        tenta_layout::intrinsic_inline_content_sizes;
    let _: fn(&mut LayoutCx, &BoxTree, BoxId) -> (f32, f32) =
        tenta_layout::intrinsic_inline_sizes_of_block;
}

fn stub_inline(
    _cx: &mut LayoutCx,
    _tree: &BoxTree,
    _ifc: &InlineFormattingContext,
    space: &ConstraintSpace,
    _floats: &mut dyn FloatContext,
    out: &mut FragmentBuilder,
) -> InlineResult {
    let width = match space.inline_size {
        AvailableSize::Definite(w) => w,
        _ => 0.0,
    };
    let line = out.push(Fragment::new(
        NodeId::NONE,
        FragmentKind::Line,
        Arc::new(PaintStyle::default()),
        Rect::new(0.0, 0.0, width, 0.0),
    ));
    InlineResult {
        lines: SmallVec::from_slice(&[line]),
        block_size: 0.0,
        first_baseline: None,
        last_baseline: None,
        overflow: Rect::default(),
        inline_extent: 0.0,
    }
}

fn stub_block(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
    out: &mut FragmentBuilder,
) -> BlockResult {
    let b = tree.get(id);
    let mut kids: SmallVec<[_; 4]> = SmallVec::new();
    for &child in &b.children {
        kids.push(stub_block(cx, tree, child, space, out).frag);
    }
    if let Some(ifc) = &b.ifc {
        let mut floats = NoFloats;
        let inline: InlineFn = stub_inline;
        kids.extend(inline(cx, tree, ifc, space, &mut floats, out).lines);
    }
    let width = match space.inline_size {
        AvailableSize::Definite(w) => w,
        _ => 0.0,
    };
    let mut frag = Fragment::new(
        b.node,
        FragmentKind::Box,
        b.paint.clone(),
        Rect::new(0.0, 0.0, width, 0.0),
    );
    frag.flags = FragFlags::ESTABLISHES_BFC;
    let id = out.push(frag);
    out.set_children(id, kids);
    BlockResult {
        frag: id,
        margin_before: MarginStrut::default(),
        margin_after: MarginStrut::default(),
        first_baseline: None,
        last_baseline: None,
        floats_out: Vec::new(),
        exclusions: Vec::new(),
        self_collapsing: false,
    }
}

struct FixedMetrics;

impl FontMetrics for FixedMetrics {
    fn metrics(&self, _: &str, _: u16, _: bool, size_px: f32) -> FontMetricsData {
        FontMetricsData {
            ascent: size_px * 0.9,
            ..FontMetricsData::default()
        }
    }
}

fn initial_style() -> Style {
    ComputedValues::initial_values_with_font_override(Font::initial_values())
}

fn layout_box(node: NodeId, parent: BoxId, kind: BoxKind, style: &Style) -> LayoutBox {
    LayoutBox::new(
        node,
        parent,
        kind,
        style.clone(),
        Arc::new(PaintStyle::default()),
    )
}

#[test]
fn stand_in_algorithms_lay_out_a_hand_built_tree() {
    let style = initial_style();
    let mut tree = BoxTree::new(7);
    let root = tree.push(layout_box(NodeId(1), BoxId::NONE, BoxKind::Block, &style));
    let mut para = layout_box(NodeId(2), root, BoxKind::Block, &style);
    para.ifc = Some(Box::new(InlineFormattingContext {
        items: vec![InlineItem::Text {
            source: TextSource::Node {
                node: NodeId(3),
                range: 0..5,
            },
            style_box: BoxId(1),
        }],
        root: BoxId(1),
    }));
    let para = tree.push(para);
    tree.get_mut(root).children.push(para);

    assert_eq!(tree.root, root);
    assert_eq!(tree.generation, 7);
    assert_eq!(tree.boxes_of(NodeId(2)), &[para]);
    assert!(tree.boxes_of(NodeId(40)).is_empty());

    let doc = Document::new();
    let styles = StyleEngine::new(StyleEnv::default(), Arc::new(FixedMetrics));
    let mut fonts = FontSystem::new_embedded();
    let mut cx = LayoutCx {
        doc: &doc,
        styles: &styles,
        fonts: &mut fonts,
        images: &NoImages,
        viewport: (800.0, 600.0),
        quirks: false,
    };

    let mut out = FragmentBuilder::new();
    let space = ConstraintSpace::new(AvailableSize::Definite(800.0), AvailableSize::MaxContent);
    let block: BlockFn = stub_block;
    let result = block(&mut cx, &tree, root, &space, &mut out);
    let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
    let frags = out.finish(result.frag, viewport, viewport);

    assert_eq!(frags.len(), 3, "root box, paragraph box and one line");
    let root_frag = frags.get(frags.root);
    assert_eq!(root_frag.rect, Rect::new(0.0, 0.0, 800.0, 0.0));
    assert_eq!(root_frag.children.len(), 1);
    let para_frag = frags.fragments_of(NodeId(2));
    assert_eq!(para_frag.len(), 1);
    let line = frags.get(para_frag[0]).children[0];
    assert!(matches!(frags.get(line).kind, FragmentKind::Line));
    assert_eq!(
        frags.absolute_border_box(line),
        Rect::new(0.0, 0.0, 800.0, 0.0)
    );
}
