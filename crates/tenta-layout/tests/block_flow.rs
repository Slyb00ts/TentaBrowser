//! Block flow against Chrome: widths, margin collapsing, box sizing, heights
//! and formatting-context boundaries on documents without text.
//!
//! The expectations in `block_flow/chrome_cases.rs` are `getBoundingClientRect`
//! values recorded from Chrome 153 for exactly the HTML next to them.

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

#[path = "block_flow/chrome_cases.rs"]
mod chrome_cases;
#[path = "block_flow/chrome_size_cases.rs"]
mod chrome_size_cases;
mod support;

use support::{Case, check_cases, check_cases_when_ready};

#[test]
fn rectangles_match_chrome() {
    check_cases(chrome_cases::CASES);
}

/// Bottom-margin collapse with `min-height`/`max-height`; needs the size
/// constraint step, so it reports itself skipped until that lands.
#[test]
fn size_constraints_decide_bottom_margin_escape() {
    check_cases_when_ready(chrome_size_cases::CASES);
}

mod behaviour {
    use std::sync::Arc;

    use style::properties::ComputedValues;
    use style::properties::style_structs::Font;
    use tenta_core::NodeId;
    use tenta_css::cascade::test_util::style_html;
    use tenta_css::cascade::{FontMetrics, FontMetricsData, Style, StyleEngine, StyleEnv};
    use tenta_dom::Document;
    use tenta_layout::{
        AvailableSize, BoxFlags, BoxId, BoxKind, BoxTree, ConstraintSpace, Edges, FontSystem,
        FragFlags, FragmentBuilder, FragmentKind, ImageInfo, IntrinsicSize, LayoutBox, LayoutCx,
        NoImages, PaintStyle, layout_block,
    };

    use super::support::{lay_out, with_layout_cx};

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

    #[test]
    fn hand_built_tree_stacks_framed_and_plain_boxes() {
        let style = initial_style();
        let mut tree = BoxTree::new(0);
        let mut root = LayoutBox::new(
            NodeId::NONE,
            BoxId::NONE,
            BoxKind::FlowRoot,
            style.clone(),
            Arc::new(PaintStyle::default()),
        );
        root.flags = BoxFlags::ESTABLISHES_BFC | BoxFlags::ANONYMOUS;
        let root = tree.push(root);
        let mut framed_paint = PaintStyle::default();
        framed_paint.border.widths = Edges::all(5.0);
        let framed = tree.push(LayoutBox::new(
            NodeId(2),
            root,
            BoxKind::Block,
            style.clone(),
            Arc::new(framed_paint),
        ));
        let plain = tree.push(LayoutBox::new(
            NodeId(3),
            root,
            BoxKind::Block,
            style.clone(),
            Arc::new(PaintStyle::default()),
        ));
        tree.get_mut(root).children.extend([framed, plain]);

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
        let space = ConstraintSpace::new(AvailableSize::Definite(800.0), AvailableSize::MaxContent);
        let mut out = FragmentBuilder::new();
        let result = layout_block(&mut cx, &tree, root, &space, &mut out);

        let root_frag = out.get(result.frag);
        assert_eq!(
            (root_frag.rect.w, root_frag.rect.h),
            (800.0, 10.0),
            "a framed empty box is 2 x 5 px tall and the plain box adds nothing"
        );
        assert!(root_frag.flags.contains(FragFlags::ESTABLISHES_BFC));
        let kids = &root_frag.children;
        assert_eq!(kids.len(), 2);
        let first = out.get(kids[0]);
        assert_eq!(
            (first.rect.y, first.rect.w, first.rect.h),
            (0.0, 800.0, 10.0)
        );
        assert_eq!(first.border, Edges::all(5.0));
        let second = out.get(kids[1]);
        assert_eq!((second.rect.y, second.rect.h), (10.0, 0.0));
        assert!(matches!(second.kind, FragmentKind::Box));
        assert!(
            !result.self_collapsing,
            "a BFC root never collapses through"
        );
    }

    #[test]
    fn layout_is_repeatable() {
        let html = r#"<!doctype html><body style="margin:0"><div style="margin:7px;padding:3px"><div style="height:11px;margin:5% 2px"></div><div style="width:40%;height:9px;margin:auto"></div></div>"#;
        let a = lay_out(html, (640.0, 480.0)).frags;
        let b = lay_out(html, (640.0, 480.0)).frags;
        assert_eq!(a.len(), b.len());
        for i in 0..a.len() as u32 {
            let (fa, fb) = (
                a.get(tenta_layout::FragId(i)),
                b.get(tenta_layout::FragId(i)),
            );
            assert_eq!(fa.rect, fb.rect);
            assert_eq!(fa.overflow, fb.overflow);
            assert_eq!(fa.flags, fb.flags);
            assert_eq!(fa.children, fb.children);
        }
        assert_eq!(a.scroll_extent, b.scroll_extent);
    }

    #[test]
    fn fragments_carry_formatting_context_and_paint_flags() {
        let laid = lay_out(
            r#"<!doctype html><body>
            <div id=plain style="height:5px"></div>
            <div id=clip style="overflow:hidden;height:5px"></div>
            <div id=root style="display:flow-root;height:5px"></div>
            <div id=flex style="display:flex"><div id=item style="height:5px"></div></div>
            <div id=grid style="display:grid"><div id=cell style="height:5px"></div></div>"#,
            (800.0, 600.0),
        );
        let flags = |id: &str| laid.frags.get(laid.frag(id)).flags;
        assert!(flags("plain").is_empty());
        assert!(flags("clip").contains(FragFlags::CLIPS_OVERFLOW | FragFlags::ESTABLISHES_BFC));
        assert!(flags("root").contains(FragFlags::ESTABLISHES_BFC));
        assert!(!flags("root").contains(FragFlags::CLIPS_OVERFLOW));
        // A flex container really lays its items out, so they paint atomically;
        // a grid container is still laid out as a block, so its items do not.
        assert!(flags("item").contains(FragFlags::PAINT_ATOMIC));
        assert!(!flags("flex").contains(FragFlags::PAINT_ATOMIC));
        assert!(!flags("cell").contains(FragFlags::PAINT_ATOMIC));
    }

    #[test]
    fn root_is_a_stacking_context_and_body_feeds_the_canvas() {
        let laid = lay_out(
            "<!doctype html><body><div id=d style=height:5px></div>",
            (800.0, 600.0),
        );
        let root = laid.frags.get(laid.frags.root);
        assert!(
            root.flags
                .contains(FragFlags::STACKING_CONTEXT | FragFlags::ESTABLISHES_BFC)
        );
        let body = laid.doc.body().expect("the parser inserts a body");
        let body_frag = laid.frags.get(laid.frags.fragments_of(body)[0]);
        assert!(body_frag.flags.contains(FragFlags::CANVAS_SOURCE));
        let marked = (0..laid.frags.len() as u32)
            .filter(|&i| {
                laid.frags
                    .get(tenta_layout::FragId(i))
                    .flags
                    .contains(FragFlags::CANVAS_SOURCE)
            })
            .count();
        assert_eq!(marked, 1);
    }

    #[test]
    fn body_overflow_applies_to_the_viewport_not_the_body() {
        let laid = lay_out(
            r#"<!doctype html><body style="overflow:hidden"><div style="width:3000px;height:2000px"></div>"#,
            (800.0, 600.0),
        );
        let body = laid.doc.body().expect("body");
        let frag = laid.frags.get(laid.frags.fragments_of(body)[0]);
        assert!(!frag.flags.contains(FragFlags::CLIPS_OVERFLOW));
        let html_clipped = lay_out(
            r#"<!doctype html><html style="overflow:hidden"><body style="overflow:hidden"><div style="height:50px"></div>"#,
            (800.0, 600.0),
        );
        let body = html_clipped.doc.body().expect("body");
        let frag = html_clipped
            .frags
            .get(html_clipped.frags.fragments_of(body)[0]);
        assert!(frag.flags.contains(FragFlags::CLIPS_OVERFLOW));
    }

    #[test]
    fn fragments_record_the_used_box_model() {
        let laid = lay_out(
            r#"<!doctype html><body style="margin:0"><div id=p style="width:400px"><div id=a style="margin:10px 20px 30px auto;padding:1px 2px 3px 4px;border:5px solid;height:10px"></div></div>"#,
            (800.0, 600.0),
        );
        let a = laid.frags.get(laid.frag("a"));
        assert_eq!(a.border, Edges::all(5.0));
        assert_eq!(
            a.padding,
            Edges {
                top: 1.0,
                right: 2.0,
                bottom: 3.0,
                left: 4.0
            }
        );
        assert_eq!(
            a.margin,
            Edges {
                top: 10.0,
                right: 20.0,
                bottom: 30.0,
                left: 0.0
            }
        );
        assert_eq!(a.rect.w, 380.0);
    }

    #[test]
    fn scroll_extent_covers_overflow_except_clipped_content() {
        let overflowing = lay_out(
            r#"<!doctype html><body style="margin:0"><div style="height:10px;width:1200px"></div><div style="height:2000px"></div>"#,
            (800.0, 600.0),
        );
        let extent = overflowing.frags.scroll_extent;
        assert_eq!(
            (extent.x, extent.y, extent.w, extent.h),
            (0.0, 0.0, 1200.0, 2010.0)
        );

        let clipped = lay_out(
            r#"<!doctype html><body style="margin:0"><div style="overflow:hidden;height:10px;width:100px"><div style="height:5000px;width:5000px"></div></div>"#,
            (800.0, 600.0),
        );
        let extent = clipped.frags.scroll_extent;
        assert_eq!((extent.w, extent.h), (800.0, 600.0));
    }

    #[test]
    fn size_dependent_paint_style_is_resolved_for_each_border_box() {
        let laid = lay_out(
            r#"<!doctype html><body style="margin:0"><div id=a style="width:100px;height:20px;transform:translate(50%,0)"></div>"#,
            (800.0, 600.0),
        );
        let paint = &laid.frags.get(laid.frag("a")).paint;
        let transform = paint.transform.expect("the transform is kept");
        assert_eq!(transform.e, 50.0);
    }

    #[test]
    fn block_result_reports_the_margins_that_adjoin_each_edge() {
        with_layout_cx(
            r#"<!doctype html><body style="margin:0"><div id=p style="margin:10px 0 15px"><div id=c style="margin:30px 0 5px;height:20px"></div></div><div id=e style="margin:4px 0 9px"></div>"#,
            (800.0, 600.0),
            |cx, tree, doc| {
                let mut space =
                    ConstraintSpace::new(AvailableSize::Definite(800.0), AvailableSize::MaxContent);
                space.percentage_base = (Some(800.0), None);
                let box_of = |id: &str| tree.boxes_of(doc.get_element_by_id(id).unwrap())[0];
                let mut out = FragmentBuilder::new();

                let p = layout_block(cx, tree, box_of("p"), &space, &mut out);
                assert_eq!(
                    (p.margin_before.positive, p.margin_before.negative),
                    (30.0, 0.0)
                );
                assert_eq!(
                    (p.margin_after.positive, p.margin_after.negative),
                    (15.0, 0.0)
                );
                assert!(!p.self_collapsing);
                assert_eq!(out.get(p.frag).rect.h, 20.0);

                let e = layout_block(cx, tree, box_of("e"), &space, &mut out);
                assert!(e.self_collapsing);
                assert_eq!(
                    (e.margin_before.positive, e.margin_after.positive),
                    (4.0, 9.0)
                );
            },
        );
    }

    #[test]
    fn decided_sizes_are_taken_as_given() {
        with_layout_cx(
            r#"<!doctype html><body style="margin:0"><div id=a style="margin:5px;padding:10px;border:2px solid"></div>"#,
            (800.0, 600.0),
            |cx, tree, doc| {
                let id = tree.boxes_of(doc.get_element_by_id("a").unwrap())[0];
                let mut space = ConstraintSpace::new(
                    AvailableSize::Definite(300.0),
                    AvailableSize::Definite(90.0),
                );
                space.fixed_inline = true;
                space.fixed_block = true;
                let mut out = FragmentBuilder::new();
                let result = layout_block(cx, tree, id, &space, &mut out);
                let frag = out.get(result.frag);
                assert_eq!((frag.rect.w, frag.rect.h), (300.0, 90.0));
                assert_eq!(frag.rect.x, 5.0);
            },
        );
    }

    struct Pictures;

    impl ImageInfo for Pictures {
        fn intrinsic(&self, _: NodeId) -> Option<IntrinsicSize> {
            Some(IntrinsicSize {
                width: Some(120.0),
                height: Some(60.0),
                ratio: None,
            })
        }
    }

    #[test]
    fn block_level_images_use_natural_size_and_ratio() {
        let html = r#"<!doctype html><body style="margin:0">
            <img id=natural style="display:block">
            <img id=wide style="display:block;width:60px">
            <img id=tall style="display:block;height:15px">
            <img id=both style="display:block;width:10px;height:50px">"#;
        let (doc, engine) = style_html(html, StyleEnv::default());
        let mut fonts = FontSystem::new_embedded();
        let mut cx = LayoutCx {
            doc: &doc,
            styles: &engine,
            fonts: &mut fonts,
            images: &Pictures,
            viewport: (800.0, 600.0),
            quirks: false,
        };
        let tree = BoxTree::build(&cx);
        let frags = tenta_layout::layout(&mut cx, &tree);
        let size = |id: &str| {
            let frag = frags.fragments_of(doc.get_element_by_id(id).unwrap())[0];
            let rect = frags.get(frag).rect;
            (rect.w, rect.h)
        };
        assert_eq!(size("natural"), (120.0, 60.0));
        assert_eq!(size("wide"), (60.0, 30.0));
        assert_eq!(size("tall"), (30.0, 15.0));
        assert_eq!(size("both"), (10.0, 50.0));
    }
}
