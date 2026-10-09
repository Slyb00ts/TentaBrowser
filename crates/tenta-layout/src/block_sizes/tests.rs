//! Intrinsic sizes through the real cascade and box tree.

use tenta_css::cascade::StyleEnv;
use tenta_css::cascade::test_util::style_html;

use super::{intrinsic_inline_contribution, intrinsic_inline_sizes_of_block};
use crate::box_tree::{BoxId, BoxTree, LayoutCx, NoImages};
use crate::font::FontSystem;

struct Page {
    doc: tenta_dom::Document,
    engine: tenta_css::cascade::StyleEngine,
    fonts: FontSystem,
    tree: BoxTree,
}

fn page(body: &str) -> Page {
    let html = format!(
        "<!doctype html><style>html,body{{margin:0}} div{{display:block}} #t{{font:16px serif}}</style>{body}"
    );
    let (doc, engine) = style_html(&html, StyleEnv::default());
    let mut fonts = FontSystem::new_embedded();
    let tree = {
        let cx = LayoutCx {
            doc: &doc,
            styles: &engine,
            fonts: &mut fonts,
            images: &NoImages,
            viewport: (800.0, 600.0),
            quirks: false,
        };
        BoxTree::build(&cx)
    };
    Page {
        doc,
        engine,
        fonts,
        tree,
    }
}

impl Page {
    fn id(&self, element: &str) -> BoxId {
        let node = self.doc.get_element_by_id(element).expect("element exists");
        self.tree.boxes_of(node)[0]
    }

    fn with_cx<R>(&mut self, f: impl FnOnce(&mut LayoutCx, &BoxTree) -> R) -> R {
        let mut cx = LayoutCx {
            doc: &self.doc,
            styles: &self.engine,
            fonts: &mut self.fonts,
            images: &NoImages,
            viewport: (800.0, 600.0),
            quirks: false,
        };
        f(&mut cx, &self.tree)
    }

    fn content(&mut self, element: &str) -> (f32, f32) {
        let id = self.id(element);
        self.with_cx(|cx, tree| intrinsic_inline_sizes_of_block(cx, tree, id))
    }

    fn contribution(&mut self, element: &str) -> (f32, f32) {
        let id = self.id(element);
        self.with_cx(|cx, tree| intrinsic_inline_contribution(cx, tree, id))
    }
}

#[test]
fn content_sizes_leave_out_the_boxs_own_width_padding_and_border() {
    let mut p = page(
        "<div id=p style='width:500px;padding:10px;border:3px solid'>
           <div style='width:100px'></div><div style='width:60px'></div></div>",
    );
    assert_eq!(p.content("p"), (100.0, 100.0));
}

#[test]
fn a_contribution_is_border_box_and_honours_width_and_limits() {
    let mut p = page(
        "<div id=a style='width:100px;padding:0 5px;border:2px solid'></div>
         <div id=b style='box-sizing:border-box;width:100px;padding:0 5px'></div>
         <div id=c style='width:300px;max-width:120px'></div>
         <div id=d style='min-width:90px'></div>
         <div id=e style='width:50%'><div style='width:40px'></div></div>",
    );
    assert_eq!(p.contribution("a"), (114.0, 114.0));
    assert_eq!(p.contribution("b"), (100.0, 100.0));
    assert_eq!(p.contribution("c"), (120.0, 120.0));
    assert_eq!(p.contribution("d"), (90.0, 90.0));
    assert_eq!(p.contribution("e"), (40.0, 40.0));
}

#[test]
fn block_children_contribute_their_margins() {
    let mut p = page(
        "<div id=p><div style='width:100px;margin:0 7px 0 3px'></div>
           <div style='width:90px;margin:0 -10px 0 -20px'></div></div>",
    );
    assert_eq!(p.content("p"), (110.0, 110.0));
}

#[test]
fn inline_content_delegates_to_the_inline_formatting_context() {
    let mut p = page("<div id=t>aa bbbbbb c</div>");
    let (min, max) = p.content("t");
    assert!((min - 48.0).abs() < 0.02, "min-content {min}");
    assert!(max > min);
    let again = p.content("t");
    assert_eq!((min, max), again);
    let id = p.id("t");
    assert_eq!(p.tree.get(id).intrinsic_inline.get(), Some((min, max)));
}

#[test]
fn a_replaced_box_that_can_shrink_has_no_min_content_beyond_its_edges() {
    let mut p = page(
        "<canvas id=a style='display:block'></canvas>
         <canvas id=b style='display:block;max-width:100%;padding:0 4px'></canvas>
         <canvas id=c style='display:block;width:40px'></canvas>",
    );
    assert_eq!(p.contribution("a"), (300.0, 300.0));
    assert_eq!(p.contribution("b"), (8.0, 308.0));
    assert_eq!(p.contribution("c"), (40.0, 40.0));
}
