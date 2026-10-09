//! Box-tree structure and paint-style extraction, through the real cascade.

use std::fmt::Write as _;

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_css::cascade::test_util::{FixedRatioMetrics, style_html};
use tenta_css::cascade::{StyleDiff, StyleEngine, StyleEnv};
use tenta_dom::{Document, Mutation};

use crate::box_tree::{
    BoxFlags, BoxId, BoxKind, BoxTree, InlineItem, LayoutCx, NoImages, TextSource,
};
use crate::font::FontSystem;
use crate::types::{
    BackgroundContent, BackgroundSize, BlendMode, BorderStyle, BoxArea, Color, ControlKind, Edges,
    FilterOp, ImageRendering, LengthPercent, LineStyle, MarkerKind, MarkerPaint, ObjectFit,
    OverflowKind, PaintStyle, PositionKind, Rect, RepeatKind, ReplacedKind, TableKind,
};

struct Built {
    doc: Document,
    engine: StyleEngine,
    tree: BoxTree,
}

fn build_with(html: &str, env: StyleEnv) -> Built {
    let (doc, engine) = style_html(html, env);
    let mut fonts = FontSystem::new_embedded();
    let cx = LayoutCx {
        doc: &doc,
        styles: &engine,
        fonts: &mut fonts,
        images: &NoImages,
        viewport: (800.0, 600.0),
        quirks: false,
    };
    let tree = BoxTree::build(&cx);
    Built { doc, engine, tree }
}

fn build(html: &str) -> Built {
    build_with(html, StyleEnv::default())
}

impl Built {
    fn element(&self, id: &str) -> NodeId {
        self.doc
            .get_element_by_id(id)
            .expect("element with that id")
    }

    fn principal(&self, id: &str) -> BoxId {
        self.tree.boxes_of(self.element(id))[0]
    }

    /// The dump of the subtree rooted at the principal box of `#id`.
    fn dump_of(&self, id: &str) -> String {
        let mut out = String::new();
        self.dump_box(self.principal(id), 0, &mut out);
        out
    }

    fn label(&self, id: BoxId) -> String {
        let b = self.tree.get(id);
        let kind = match &b.kind {
            BoxKind::Block => "block".to_owned(),
            BoxKind::FlowRoot => "flow-root".to_owned(),
            BoxKind::ListItem { marker } => format!("list-item(marker={})", marker.is_some()),
            BoxKind::AnonymousBlock => "anon".to_owned(),
            BoxKind::InlineBlock => "inline-block".to_owned(),
            BoxKind::Inline => "inline".to_owned(),
            BoxKind::Replaced(k) => format!("replaced:{k:?}"),
            BoxKind::Marker(k) => format!("marker:{k:?}"),
            BoxKind::Flex => "flex".to_owned(),
            BoxKind::Grid => "grid".to_owned(),
            BoxKind::Table(k) => format!("table:{k:?}"),
        };
        match b.node.is_some().then(|| self.doc.tag(b.node)).flatten() {
            Some(tag) => format!("{kind}<{tag}>"),
            None => kind,
        }
    }

    fn text(&self, source: &TextSource) -> String {
        match source {
            TextSource::Node { node, range } => self.doc.text_data(*node).unwrap()
                [range.start as usize..range.end as usize]
                .replace('\n', "\\n"),
            TextSource::Owned(text) => format!("~{text}"),
        }
    }

    fn dump_box(&self, id: BoxId, depth: usize, out: &mut String) {
        let b = self.tree.get(id);
        let pad = "  ".repeat(depth);
        let _ = writeln!(out, "{pad}{}", self.label(id));
        if let BoxKind::ListItem { marker: Some(m) } = b.kind {
            let _ = writeln!(out, "{pad}  marker:");
            self.dump_box(m, depth + 2, out);
        }
        if let Some(ifc) = &b.ifc {
            let mut line = String::new();
            for item in &ifc.items {
                match item {
                    InlineItem::Text { source, .. } => {
                        let _ = write!(line, " \"{}\"", self.text(source));
                    }
                    InlineItem::OpenInline { box_id, .. } => {
                        let _ = write!(line, " <{}", self.label(*box_id));
                    }
                    InlineItem::CloseInline { .. } => line.push_str(" >"),
                    InlineItem::Atomic { box_id } => {
                        let _ = write!(line, " [{}]", self.label(*box_id));
                    }
                    InlineItem::ForcedBreak { .. } => line.push_str(" br"),
                    InlineItem::SoftBreak => line.push_str(" wbr"),
                    InlineItem::Float { box_id } => {
                        let _ = write!(line, " float[{}]", self.label(*box_id));
                    }
                    InlineItem::AbsPos { box_id } => {
                        let _ = write!(line, " abspos[{}]", self.label(*box_id));
                    }
                }
            }
            let _ = writeln!(out, "{pad}  items:{line}");
            for item in &ifc.items {
                if let InlineItem::Atomic { box_id }
                | InlineItem::Float { box_id }
                | InlineItem::AbsPos { box_id } = item
                {
                    self.dump_box(*box_id, depth + 2, out);
                }
            }
        }
        for &child in &b.children {
            self.dump_box(child, depth + 1, out);
        }
    }
}

impl Built {
    fn paint(&self, id: &str) -> &PaintStyle {
        &self.tree.get(self.principal(id)).paint
    }

    fn root_dump(&self) -> String {
        let mut out = String::new();
        self.dump_box(self.tree.root, 0, &mut out);
        out
    }

    fn rebuild(&mut self) -> BoxTree {
        let mut fonts = FontSystem::new_embedded();
        let cx = LayoutCx {
            doc: &self.doc,
            styles: &self.engine,
            fonts: &mut fonts,
            images: &NoImages,
            viewport: (800.0, 600.0),
            quirks: false,
        };
        BoxTree::build(&cx)
    }
}

// ---------------------------------------------------------------- structure

#[test]
fn inline_only_container_owns_one_formatting_context() {
    let b = build("<body><p id=a>Hello <b>big</b> world</p>");
    assert_eq!(
        b.dump_of("a"),
        "block<p>\n  items: \"Hello \" <inline<b> \"big\" > \" world\"\n"
    );
}

#[test]
fn text_items_cover_the_whole_node_data() {
    let b = build("<body><p id=a>Hello <b>big</b></p>");
    let ifc = b.tree.get(b.principal("a")).ifc.as_ref().unwrap();
    let ranges: Vec<_> = ifc
        .items
        .iter()
        .filter_map(|i| match i {
            InlineItem::Text {
                source: TextSource::Node { node, range },
                ..
            } => Some((b.doc.text_data(*node).unwrap().len() as u32, range.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(ranges, [(6, 0..6), (3, 0..3)]);
    assert_eq!(ifc.root, b.principal("a"));
}

#[test]
fn inline_text_is_styled_by_its_inline_box() {
    let b = build("<body><p id=a>x<b id=b>y</b></p>");
    let ifc = b.tree.get(b.principal("a")).ifc.as_ref().unwrap();
    let style_boxes: Vec<_> = ifc
        .items
        .iter()
        .filter_map(|i| match i {
            InlineItem::Text { style_box, .. } => Some(*style_box),
            _ => None,
        })
        .collect();
    assert_eq!(style_boxes, [b.principal("a"), b.principal("b")]);
    assert_eq!(b.tree.get(b.principal("b")).parent, b.principal("a"));
}

#[test]
fn mixed_children_get_anonymous_block_wrappers() {
    let b = build("<body><div id=a>x<p>y</p>z<span>s<div>blk</div>t</span></div>");
    assert_eq!(
        b.dump_of("a"),
        "block<div>
  anon
    items: \"x\"
  block<p>
    items: \"y\"
  anon
    items: \"z\" <inline<span> \"s\" >
  block<div>
    items: \"blk\"
  anon
    items: <inline<span> \"t\" >
"
    );
}

#[test]
fn anonymous_wrappers_inherit_but_do_not_repeat_box_properties() {
    let b = build(
        "<body><div id=a style='color:#f00;padding:9px;background:#00f;border:3px solid'>x<p>y</p></div>",
    );
    let wrapper = b.tree.get(b.principal("a")).children[0];
    let w = b.tree.get(wrapper);
    assert_eq!(w.kind, BoxKind::AnonymousBlock);
    assert!(w.flags.contains(BoxFlags::ANONYMOUS));
    assert_eq!(w.node, NodeId::NONE);
    assert_eq!(w.parent, b.principal("a"));
    assert_eq!(w.paint.color, Color::from_rgba8(255, 0, 0, 255));
    assert!(w.paint.background.is_empty());
    assert_eq!(w.paint.border.widths, Edges::default());
    let padding = w.style.get_padding();
    assert_eq!(padding.padding_top.0.to_length().unwrap().px(), 0.0);
}

#[test]
fn collapsible_white_space_between_blocks_is_dropped() {
    let b = build("<body><div id=a>\n  <p>one</p>\n  <p>two</p>\n</div>");
    assert_eq!(
        b.dump_of("a"),
        "block<div>
  block<p>
    items: \"one\"
  block<p>
    items: \"two\"
"
    );
}

#[test]
fn preserved_white_space_between_blocks_is_kept() {
    let b = build("<body><div id=a style='white-space:pre'><p>one</p> <p>two</p></div>");
    assert!(b.dump_of("a").contains("anon\n    items: \" \""));
}

#[test]
fn white_space_only_container_keeps_its_text() {
    let b = build("<body><div id=a> </div>");
    assert_eq!(b.dump_of("a"), "block<div>\n  items: \" \"\n");
}

#[test]
fn display_none_subtrees_generate_no_boxes() {
    let b = build("<body><div id=a>x<div id=n style='display:none'><p id=m>y</p></div>z</div>");
    assert!(b.tree.boxes_of(b.element("n")).is_empty());
    assert!(b.tree.boxes_of(b.element("m")).is_empty());
    assert_eq!(b.dump_of("a"), "block<div>\n  items: \"x\" \"z\"\n");
}

#[test]
fn head_and_script_elements_generate_no_boxes() {
    let b = build("<title>t</title><body id=a>hi<script>1</script>");
    assert_eq!(b.dump_of("a"), "block<body>\n  items: \"hi\"\n");
    assert!(b.tree.boxes_of(b.doc.head().unwrap()).is_empty());
}

#[test]
fn display_contents_children_are_promoted_and_still_styled() {
    let b = build(
        "<body><div id=a><section id=c style='display:contents;color:#0f0'>t<p>u</p></section></div>",
    );
    assert!(b.tree.boxes_of(b.element("c")).is_empty());
    assert_eq!(
        b.dump_of("a"),
        "block<div>
  anon
    items: <inline \"t\" >
  block<p>
    items: \"u\"
"
    );
    // The text keeps the colour of the element that generated no box.
    let wrapper = b.tree.get(b.principal("a")).children[0];
    let ifc = b.tree.get(wrapper).ifc.as_ref().unwrap();
    let InlineItem::Text { style_box, .. } = &ifc.items[1] else {
        panic!("text item expected");
    };
    assert_eq!(
        b.tree.get(*style_box).paint.color,
        Color::from_rgba8(0, 255, 0, 255)
    );
}

#[test]
fn display_contents_with_only_blocks_adds_no_wrapper() {
    let b =
        build("<body><div id=a><section style='display:contents'><p>u</p><p>v</p></section></div>");
    assert_eq!(
        b.dump_of("a"),
        "block<div>
  block<p>
    items: \"u\"
  block<p>
    items: \"v\"
"
    );
}

#[test]
fn inline_box_is_split_around_a_block_and_reopened() {
    let b =
        build("<body><div id=a><span id=s style='border:1px solid'>a<div>b</div>c</span></div>");
    let wrappers = &b.tree.get(b.principal("a")).children;
    assert_eq!(wrappers.len(), 3);
    let items_of = |id: BoxId| b.tree.get(id).ifc.as_ref().unwrap().items.clone();
    let s = b.principal("s");
    let first = items_of(wrappers[0]);
    let last = items_of(wrappers[2]);
    // The first piece carries the start edge only, the last piece the end edge only.
    assert_eq!(
        first[0],
        InlineItem::OpenInline {
            box_id: s,
            resumes: false
        }
    );
    assert_eq!(
        first.last().unwrap(),
        &InlineItem::CloseInline {
            box_id: s,
            interrupted: true
        }
    );
    assert_eq!(
        last[0],
        InlineItem::OpenInline {
            box_id: s,
            resumes: true
        }
    );
    assert_eq!(
        last.last().unwrap(),
        &InlineItem::CloseInline {
            box_id: s,
            interrupted: false
        }
    );
    assert_eq!(b.tree.get(s).parent, wrappers[0]);
}

#[test]
fn an_interrupted_inline_box_ending_at_the_block_keeps_an_end_piece() {
    let b = build("<body><div id=a><span id=s style='border:1px solid'>a<div>b</div></span></div>");
    let wrappers = &b.tree.get(b.principal("a")).children;
    assert_eq!(
        wrappers.len(),
        3,
        "text, block, and an empty piece that holds the end edge"
    );
    let s = b.principal("s");
    assert_eq!(
        b.tree.get(wrappers[2]).ifc.as_ref().unwrap().items,
        [
            InlineItem::OpenInline {
                box_id: s,
                resumes: true
            },
            InlineItem::CloseInline {
                box_id: s,
                interrupted: false
            },
        ]
    );
}

#[test]
fn an_interrupted_inline_box_without_edges_leaves_no_empty_wrapper() {
    let b = build("<body><div id=a><span>a<div>b</div></span></div>");
    assert_eq!(b.tree.get(b.principal("a")).children.len(), 2);
}

#[test]
fn empty_inline_boxes_with_edges_survive_between_blocks() {
    let b = build("<body><div id=a><p>a</p><span style='padding:0 4px'></span><p>b</p></div>");
    assert_eq!(b.tree.get(b.principal("a")).children.len(), 3);
    let plain = build("<body><div id=a><p>a</p><span></span><p>b</p></div>");
    assert_eq!(plain.tree.get(plain.principal("a")).children.len(), 2);
}

#[test]
fn floats_do_not_split_an_inline_run() {
    let b = build("<body><div id=a>foo<img id=f style='float:left'>bar<p>x</p></div>");
    assert_eq!(
        b.dump_of("a"),
        "block<div>
  anon
    items: \"foo\" float[replaced:Image<img>] \"bar\"
      replaced:Image<img>
  block<p>
    items: \"x\"
"
    );
}

#[test]
fn floats_in_a_dropped_white_space_run_become_direct_children() {
    let b = build("<body><div id=a><p>a</p> <span id=f style='float:left'></span> <p>b</p></div>");
    let kids = &b.tree.get(b.principal("a")).children;
    assert_eq!(kids.len(), 3);
    assert_eq!(kids[1], b.principal("f"));
    assert_eq!(b.tree.get(kids[1]).parent, b.principal("a"));
}

#[test]
fn list_item_has_an_outside_marker_box() {
    let b = build("<body><ul><li id=a>one</ul>");
    let li = b.tree.get(b.principal("a"));
    let BoxKind::ListItem { marker: Some(m) } = li.kind else {
        panic!("list item with a marker expected, got {:?}", li.kind);
    };
    let marker = b.tree.get(m);
    assert_eq!(marker.kind, BoxKind::Marker(MarkerKind::Disc));
    assert_eq!(marker.parent, b.principal("a"));
    assert!(marker.flags.contains(BoxFlags::GENERATED));
    assert!(marker.ifc.is_none());
    assert_eq!(b.tree.boxes_of(b.element("a")), [b.principal("a"), m]);
    assert_eq!(
        li.paint.list_marker,
        Some(MarkerPaint {
            kind: MarkerKind::Disc,
            inside: false
        })
    );
    assert_eq!(li.ifc.as_ref().unwrap().items.len(), 1);
}

#[test]
fn ordered_list_markers_count() {
    let b = build(
        "<body><ol id=o start=3><li>a<li value=10>b<li>c</ol><ol reversed><li>x<li>y</ol><ol type=a id=p><li>q</ol>",
    );
    let texts: Vec<String> = b
        .doc
        .get_elements_by_tag_name("li")
        .into_iter()
        .map(|li| {
            let BoxKind::ListItem { marker: Some(m) } = b.tree.get(b.tree.boxes_of(li)[0]).kind
            else {
                panic!("marker expected");
            };
            let ifc = b.tree.get(m).ifc.as_ref().expect("text marker");
            let InlineItem::Text { source, .. } = &ifc.items[0] else {
                panic!("text expected");
            };
            b.text(source)
        })
        .collect();
    assert_eq!(texts, ["~3. ", "~10. ", "~11. ", "~2. ", "~1. ", "~a. "]);
}

#[test]
fn inside_marker_is_inline_content_of_the_item() {
    let b = build("<body><ul style='list-style-position:inside'><li id=a>one</ul>");
    assert_eq!(
        b.dump_of("a"),
        "list-item(marker=false)<li>
  items: [marker:Disc<li>] \"one\"
    marker:Disc<li>
"
    );
    assert!(
        b.tree
            .get(b.principal("a"))
            .paint
            .list_marker
            .unwrap()
            .inside
    );
}

#[test]
fn list_style_none_has_no_marker() {
    let b = build("<body><ul style='list-style:none'><li id=a>one</ul>");
    assert_eq!(b.tree.boxes_of(b.element("a")).len(), 1);
    assert_eq!(
        b.tree.get(b.principal("a")).kind,
        BoxKind::ListItem { marker: None }
    );
}

#[test]
fn closed_details_shows_only_its_summary() {
    let b = build("<body><details id=d><summary id=s>sum</summary><p id=p>body</p></details>");
    assert!(!b.tree.boxes_of(b.element("s")).is_empty());
    assert!(b.tree.boxes_of(b.element("p")).is_empty());
    // The UA sheet puts the disclosure triangle inside the summary's line.
    assert_eq!(
        b.dump_of("s"),
        "list-item(marker=false)<summary>
  items: [marker:DisclosureClosed<summary>] \"sum\"
    marker:DisclosureClosed<summary>
"
    );
}

#[test]
fn inline_block_is_an_atomic_item_with_its_own_content() {
    let b = build("<body><p id=a>x<span id=ib style='display:inline-block'>in</span>y</p>");
    assert_eq!(
        b.dump_of("a"),
        "block<p>
  items: \"x\" [inline-block<span>] \"y\"
    inline-block<span>
      items: \"in\"
"
    );
    let ib = b.tree.get(b.principal("ib"));
    assert!(
        ib.flags
            .contains(BoxFlags::ESTABLISHES_BFC | BoxFlags::INLINE_LEVEL)
    );
    assert_eq!(ib.parent, b.principal("a"));
    assert!(b.tree.is_paint_atomic(b.principal("ib")));
    assert!(!b.tree.is_paint_atomic(b.principal("a")));
}

#[test]
fn line_breaks_and_word_break_opportunities_become_items() {
    let b = build("<body><p id=a>a<br>b<wbr>c</p>");
    assert_eq!(
        b.dump_of("a"),
        "block<p>\n  items: \"a\" br \"b\" wbr \"c\"\n"
    );
}

#[test]
fn floats_and_positioned_boxes_leave_placeholders_in_inline_content() {
    let b = build(
        "<body><div id=a>x<span id=f style='float:left'>f</span><span id=p style='position:absolute'>p</span></div>",
    );
    let items = &b.tree.get(b.principal("a")).ifc.as_ref().unwrap().items;
    assert!(matches!(items[1], InlineItem::Float { box_id } if box_id == b.principal("f")));
    assert!(matches!(items[2], InlineItem::AbsPos { box_id } if box_id == b.principal("p")));
    let f = b.tree.get(b.principal("f"));
    assert!(
        f.flags
            .contains(BoxFlags::OUT_OF_FLOW | BoxFlags::FLOAT | BoxFlags::ESTABLISHES_BFC)
    );
    assert_eq!(f.kind, BoxKind::FlowRoot);
    assert_eq!(f.parent, b.principal("a"));
    let p = b.tree.get(b.principal("p"));
    assert!(p.flags.contains(BoxFlags::OUT_OF_FLOW | BoxFlags::ABSPOS));
    assert!(!p.flags.contains(BoxFlags::FLOAT));
}

#[test]
fn out_of_flow_boxes_between_blocks_are_direct_children() {
    let b = build("<body><div id=a><p>a</p><div id=f style='float:right'>f</div><p>b</p></div>");
    let kids = &b.tree.get(b.principal("a")).children;
    assert_eq!(kids.len(), 3);
    assert_eq!(kids[1], b.principal("f"));
    assert_eq!(b.tree.get(kids[1]).parent, b.principal("a"));
}

#[test]
fn flex_grid_and_table_are_recognised_and_built_as_blocks() {
    let b = build(
        "<body><div id=f style='display:flex'>a<p>b</p></div><div id=g style='display:grid'><p>c</p></div>\
         <table id=t><tr id=r><td id=c>x</table><span id=i style='display:inline-flex'>y</span>",
    );
    assert_eq!(b.tree.get(b.principal("f")).kind, BoxKind::Flex);
    assert_eq!(b.tree.get(b.principal("g")).kind, BoxKind::Grid);
    assert_eq!(
        b.tree.get(b.principal("t")).kind,
        BoxKind::Table(TableKind::Table)
    );
    assert_eq!(
        b.tree.get(b.principal("r")).kind,
        BoxKind::Table(TableKind::Row)
    );
    assert_eq!(
        b.tree.get(b.principal("c")).kind,
        BoxKind::Table(TableKind::Cell)
    );
    // Children are built exactly as for a block container, so layout can treat them as blocks.
    assert_eq!(
        b.dump_of("f"),
        "flex<div>
  anon
    items: \"a\"
  flow-root<p>
    items: \"b\"
"
    );
    assert!(
        b.tree
            .get(b.principal("f"))
            .flags
            .contains(BoxFlags::ESTABLISHES_BFC)
    );
    let i = b.tree.get(b.principal("i"));
    assert_eq!(i.kind, BoxKind::Flex);
    assert!(i.flags.contains(BoxFlags::INLINE_LEVEL));
    assert!(b.tree.is_paint_atomic(b.principal("i")));
}

#[test]
fn flex_and_grid_children_are_paint_atomic() {
    let b = build("<body><div style='display:flex'><p id=a>x</p></div><p id=n>y</p>");
    assert!(b.tree.is_paint_atomic(b.principal("a")));
    assert!(!b.tree.is_paint_atomic(b.principal("n")));
}

#[test]
fn replaced_elements_are_atomic_inlines_or_blocks() {
    let b = build(
        "<body><p id=a>x<img id=i src=a.png width=10 height=5>y<canvas id=c></canvas></p>\
         <img id=blk style='display:block' src=b.png>",
    );
    assert_eq!(
        b.dump_of("a"),
        "block<p>
  items: \"x\" [replaced:Image<img>] \"y\" [replaced:Canvas<canvas>]
    replaced:Image<img>
    replaced:Canvas<canvas>
"
    );
    let img = b.tree.get(b.principal("i"));
    assert!(
        img.flags
            .contains(BoxFlags::REPLACED | BoxFlags::INLINE_LEVEL)
    );
    let blk = b.tree.get(b.principal("blk"));
    assert!(blk.flags.contains(BoxFlags::REPLACED));
    assert!(!blk.flags.contains(BoxFlags::INLINE_LEVEL));
    assert_eq!(
        blk.parent,
        b.doc.body().map(|n| b.tree.boxes_of(n)[0]).unwrap()
    );
}

#[test]
fn form_controls_are_replaced_boxes_with_their_text() {
    let b = build(
        "<body><p id=a><input id=t value=hi><input id=pw type=password value=abc><input id=cb type=checkbox>\
         <input id=s type=submit><input type=hidden id=h value=z>\
         <textarea id=ta>area</textarea><select id=sel><option>one<option selected>two</select></p>",
    );
    let text_of = |id: &str| {
        let ifc = b.tree.get(b.principal(id)).ifc.as_ref()?;
        let InlineItem::Text { source, .. } = &ifc.items[0] else {
            return None;
        };
        Some(b.text(source))
    };
    let kind = |id: &str| b.tree.get(b.principal(id)).kind.clone();
    assert_eq!(
        kind("t"),
        BoxKind::Replaced(ReplacedKind::Control(ControlKind::TextInput))
    );
    assert_eq!(text_of("t").as_deref(), Some("~hi"));
    assert_eq!(text_of("pw").as_deref(), Some("~\u{2022}\u{2022}\u{2022}"));
    assert_eq!(
        kind("cb"),
        BoxKind::Replaced(ReplacedKind::Control(ControlKind::Checkbox))
    );
    assert_eq!(text_of("cb"), None);
    assert_eq!(text_of("s").as_deref(), Some("~Submit"));
    assert!(b.tree.boxes_of(b.element("h")).is_empty());
    assert_eq!(text_of("ta").as_deref(), Some("~area"));
    assert_eq!(text_of("sel").as_deref(), Some("~two"));
    assert!(
        b.tree
            .boxes_of(b.doc.get_elements_by_tag_name("option")[0])
            .is_empty()
    );
}

#[test]
fn dirty_form_values_win_over_the_attribute() {
    let (mut doc, engine) = style_html("<body><input id=t value=old>", StyleEnv::default());
    let input = doc.get_element_by_id("t").unwrap();
    doc.set_dirty_value(input, "typed".to_owned());
    let mut fonts = FontSystem::new_embedded();
    let cx = LayoutCx {
        doc: &doc,
        styles: &engine,
        fonts: &mut fonts,
        images: &NoImages,
        viewport: (800.0, 600.0),
        quirks: false,
    };
    let tree = BoxTree::build(&cx);
    let ifc = tree.get(tree.boxes_of(input)[0]).ifc.as_ref().unwrap();
    assert!(matches!(
        &ifc.items[0],
        InlineItem::Text { source: TextSource::Owned(t), .. } if &**t == "typed"
    ));
}

#[test]
fn svg_content_is_one_replaced_box() {
    let b = build("<body><p id=a><svg id=v width=10 height=10><rect width=5 height=5/></svg></p>");
    assert_eq!(
        b.tree.get(b.principal("v")).kind,
        BoxKind::Replaced(ReplacedKind::Svg)
    );
    let rect = b.doc.get_elements_by_tag_name("rect");
    assert!(rect.iter().all(|&r| b.tree.boxes_of(r).is_empty()));
}

#[test]
fn root_box_is_the_html_element_establishing_a_bfc() {
    let b = build("<body id=a>x");
    let root = b.tree.get(b.tree.root);
    assert_eq!(root.node, b.doc.document_element().unwrap());
    assert_eq!(root.parent, BoxId::NONE);
    assert_eq!(root.kind, BoxKind::FlowRoot);
    assert!(root.flags.contains(BoxFlags::ESTABLISHES_BFC));
    assert_eq!(b.tree.generation, b.doc.mutation_generation());
    assert_eq!(b.tree.get(b.principal("a")).parent, b.tree.root);
}

#[test]
fn a_document_without_elements_still_has_a_root() {
    let doc = Document::new();
    let mut engine = StyleEngine::new(StyleEnv::default(), std::sync::Arc::new(FixedRatioMetrics));
    engine.flush(&doc);
    let mut fonts = FontSystem::new_embedded();
    let cx = LayoutCx {
        doc: &doc,
        styles: &engine,
        fonts: &mut fonts,
        images: &NoImages,
        viewport: (800.0, 600.0),
        quirks: false,
    };
    let tree = BoxTree::build(&cx);
    assert_eq!(tree.len(), 1);
    assert_eq!(tree.get(tree.root).kind, BoxKind::FlowRoot);
    assert!(tree.get(tree.root).flags.contains(BoxFlags::ANONYMOUS));
}

#[test]
fn canvas_source_is_the_body_of_an_html_root() {
    let b = build("<body id=a>x");
    assert_eq!(b.tree.canvas_source(&b.doc), Some(b.principal("a")));
}

#[test]
fn display_none_root_leaves_an_empty_anonymous_root() {
    let b = build("<html style='display:none'><body>x");
    assert_eq!(b.tree.len(), 1);
    assert!(b.tree.get(b.tree.root).flags.contains(BoxFlags::ANONYMOUS));
}

#[test]
fn text_decoration_propagates_to_descendant_boxes_but_not_atomic_ones() {
    let b = build(
        "<body><div id=a style='text-decoration:underline'><p id=p>x</p><span id=s>y</span><span id=ib style='display:inline-block'>z</span></div>",
    );
    assert!(b.paint("a").text_decoration.underline);
    assert!(b.paint("p").text_decoration.underline);
    assert!(b.paint("s").text_decoration.underline);
    assert!(!b.paint("ib").text_decoration.underline);
}

#[test]
fn deeply_nested_documents_build_without_recursion() {
    let depth = 2000;
    let html = format!("<body>{}x{}", "<div>".repeat(depth), "</div>".repeat(depth));
    let b = build(&html);
    assert!(b.tree.len() > depth);
}

#[test]
fn update_keeps_the_tree_for_none_and_rebuilds_for_a_change() {
    let mut b = build("<body id=a><p>one</p>");
    let before = b.root_dump();
    let mut fonts = FontSystem::new_embedded();
    let p = b.doc.create_html_element(atoms::P);
    let text = b.doc.create_text("two");
    b.doc
        .mutate(Mutation::Append {
            parent: p,
            child: text,
        })
        .unwrap();
    let body = b.element("a");
    b.doc
        .mutate(Mutation::Append {
            parent: body,
            child: p,
        })
        .unwrap();
    let diff = b.engine.flush(&b.doc);
    assert!(matches!(diff, StyleDiff::Full));

    let stale_generation = b.tree.generation;
    let mut tree = std::mem::replace(&mut b.tree, BoxTree::new(0));
    {
        let cx = LayoutCx {
            doc: &b.doc,
            styles: &b.engine,
            fonts: &mut fonts,
            images: &NoImages,
            viewport: (800.0, 600.0),
            quirks: false,
        };
        tree.update(&cx, &StyleDiff::None);
        assert_eq!(tree.generation, stale_generation, "None keeps the tree");
        tree.update(&cx, &diff);
        assert_eq!(tree.generation, b.doc.mutation_generation());
    }
    b.tree = tree;
    let fresh = b.rebuild();
    let updated = b.root_dump();
    b.tree = fresh;
    assert_eq!(updated, b.root_dump());
    assert_ne!(updated, before);
    assert!(updated.contains("\"two\""));
}

// ---------------------------------------------------------------- generated content

#[test]
fn before_and_after_content_become_owned_text_in_order() {
    let b =
        build("<style>#a::before{content:'x'} #a::after{content:'y'}</style><body><p id=a>mid</p>");
    assert_eq!(
        b.dump_of("a"),
        "block<p>
  items: <inline<p> \"~x\" > \"mid\" <inline<p> \"~y\" >
"
    );
    let p = b.element("a");
    let boxes = b.tree.boxes_of(p);
    assert_eq!(boxes.len(), 3, "principal, ::before, ::after");
    assert!(
        b.tree
            .get(boxes[1])
            .flags
            .contains(BoxFlags::GENERATED | BoxFlags::INLINE_LEVEL)
    );
    assert_eq!(b.tree.get(boxes[1]).parent, boxes[0]);
}

#[test]
fn content_none_and_display_none_generate_nothing() {
    let b = build(
        "<style>#a::before{content:none} #a::after{content:'x';display:none}</style><body><p id=a>mid</p>",
    );
    assert_eq!(b.dump_of("a"), "block<p>\n  items: \"mid\"\n");
}

#[test]
fn generated_content_resolves_attr_list_counters_and_quotes() {
    let b = build(
        "<style>li::before{content:counter(list-item) ') '} a::after{content:' (' attr(href) ')'}\
         q::before{content:open-quote} q::after{content:close-quote}</style>\
         <body><ol><li id=i1>A<li id=i2>B</ol><a id=l href=/x>go</a><q id=q>hi</q>",
    );
    assert!(b.dump_of("i1").contains("\"~1) \""), "{}", b.dump_of("i1"));
    assert!(b.dump_of("i2").contains("\"~2) \""), "{}", b.dump_of("i2"));
    let q = b.root_dump();
    assert!(q.contains("\"~ (/x)\""), "{q}");
    assert!(
        q.contains("\"~\u{201c}\"") && q.contains("\"~\u{201d}\""),
        "{q}"
    );
}

#[test]
fn nested_lists_keep_independent_counters() {
    let b = build(
        "<style>li::before{content:counters(list-item,'.') ' '}</style>\
         <body><ol><li id=a>x<ol><li id=b>y<li id=c>z</ol><li id=d>w</ol>",
    );
    for (id, want) in [("a", "1"), ("b", "1.1"), ("c", "1.2"), ("d", "2")] {
        let dump = b.dump_of(id);
        assert!(dump.contains(&format!("\"~{want} \"")), "{id}: {dump}");
    }
}

#[test]
fn author_marker_style_is_used_when_present() {
    let b = build("<style>li::marker{color:#f00}</style><body><ul><li id=a>x</ul>");
    let BoxKind::ListItem { marker: Some(m) } = b.tree.get(b.principal("a")).kind else {
        panic!("marker expected");
    };
    assert_eq!(b.tree.get(m).paint.color, Color::from_rgba8(255, 0, 0, 255));
}

#[test]
fn flex_and_grid_items_and_contained_boxes_establish_formatting_contexts() {
    let b = build(
        "<body><div style='display:flex'>text<p id=a>x</p><span id=sp>y</span></div>\
         <div style='display:grid'><p id=g>z</p></div>\
         <div id=c style='contain:layout'>c</div><div id=pt style='contain:paint'>p</div>\
         <div id=st style='contain:strict'>s</div><div id=sz style='contain:size'>z</div>\
         <fieldset id=fs>f</fieldset><p id=n>n</p>",
    );
    let bfc = |id: &str| {
        b.tree
            .get(b.principal(id))
            .flags
            .contains(BoxFlags::ESTABLISHES_BFC)
    };
    assert!(bfc("a") && bfc("sp") && bfc("g"), "flex and grid items");
    assert!(
        bfc("c") && bfc("pt") && bfc("st"),
        "layout and paint containment"
    );
    assert!(
        !bfc("sz") && !bfc("n"),
        "size containment alone and plain blocks do not"
    );
    assert!(bfc("fs"));
    // Text directly in a flex container is wrapped in an anonymous item.
    let flex = b.doc.get_elements_by_tag_name("div")[0];
    let wrapper = b.tree.get(b.tree.boxes_of(flex)[0]).children[0];
    assert!(
        b.tree
            .get(wrapper)
            .flags
            .contains(BoxFlags::ANONYMOUS | BoxFlags::ESTABLISHES_BFC)
    );
}

#[test]
fn inline_boxes_ignore_containment() {
    let b = build("<body><p><span id=s style='contain:paint'>x</span></p>");
    assert!(
        !b.tree
            .get(b.principal("s"))
            .flags
            .contains(BoxFlags::ESTABLISHES_BFC)
    );
}

#[test]
fn nested_decorations_merge_into_one_decoration() {
    // PaintStyle holds one decoration, so an inner line replaces the style and
    // colour of an outer one while the line kinds accumulate.
    let b = build(
        "<body><div id=a style='text-decoration:underline #f00'><span id=s style='text-decoration:line-through #00f'>x</span></div>",
    );
    let d = b.paint("s").text_decoration;
    assert!(d.underline && d.line_through);
    assert_eq!(d.color, Color::from_rgba8(0, 0, 255, 255));
    assert_eq!(
        b.paint("a").text_decoration.color,
        Color::from_rgba8(255, 0, 0, 255)
    );
}

#[test]
fn markers_are_resolved_for_every_list_item_not_by_sheet_text() {
    // A class that merely contains the word does not change anything...
    let b =
        build("<style>.map-marker{color:#f00}</style><body><ul><li id=a class=map-marker>x</ul>");
    let BoxKind::ListItem { marker: Some(m) } = b.tree.get(b.principal("a")).kind else {
        panic!("marker expected");
    };
    assert_eq!(
        b.tree.get(m).paint.color,
        Color::from_rgba8(255, 0, 0, 255),
        "inherited from the item"
    );
    // ...and the user-agent ::marker rule applies without any author rule.
    let b = build("<style>li{text-transform:uppercase}</style><body><ul><li id=a>x</ul>");
    let BoxKind::ListItem { marker: Some(m) } = b.tree.get(b.principal("a")).kind else {
        panic!("marker expected");
    };
    assert!(
        b.tree
            .get(m)
            .style
            .get_inherited_text()
            .text_transform
            .is_none()
    );
}

#[test]
fn marker_content_replaces_the_marker_even_with_list_style_none() {
    let b = build(
        "<style>li::marker{content:'>> '} .none{list-style:none} .off::marker{content:none}</style>\
         <body><ul><li id=a>x<li id=b class=none>y<li id=c class=off>z</ul>",
    );
    for id in ["a", "b"] {
        let BoxKind::ListItem { marker: Some(m) } = b.tree.get(b.principal(id)).kind else {
            panic!("{id}: marker expected");
        };
        let ifc = b.tree.get(m).ifc.as_ref().unwrap();
        let InlineItem::Text { source, .. } = &ifc.items[0] else {
            panic!("text expected");
        };
        assert_eq!(b.text(source), "~>> ", "{id}");
    }
    let c = b.tree.get(b.principal("c"));
    assert_eq!(c.kind, BoxKind::ListItem { marker: None });
    assert_eq!(b.tree.boxes_of(b.element("c")).len(), 1);
}

fn marker_resolves(html: &str) -> usize {
    let (doc, engine) = style_html(html, StyleEnv::default());
    let mut fonts = FontSystem::new_embedded();
    let cx = LayoutCx {
        doc: &doc,
        styles: &engine,
        fonts: &mut fonts,
        images: &NoImages,
        viewport: (800.0, 600.0),
        quirks: false,
    };
    BoxTree::build_counting_marker_resolves(&cx).1
}

#[test]
fn pages_without_author_marker_rules_resolve_the_default_marker_once() {
    let items = "<li>x".repeat(6);
    assert_eq!(marker_resolves(&format!("<body><ul>{items}</ul>")), 1);
    assert_eq!(
        marker_resolves(&format!(
            "<style>li{{color:#f00}}</style><body><ul>{items}</ul>"
        )),
        1,
        "author rules for other selectors do not disable the cache"
    );
}

#[test]
fn author_marker_rules_apply_per_element() {
    let html = "<style>li.x::marker{color:#f00}</style><body><ul><li id=a class=x>1<li id=b>2<li id=c class=x>3</ul>";
    assert_eq!(marker_resolves(html), 3);
    let b = build(html);
    let colour = |id: &str| {
        let BoxKind::ListItem { marker: Some(m) } = b.tree.get(b.principal(id)).kind else {
            panic!("marker expected");
        };
        b.tree.get(m).paint.color
    };
    let red = Color::from_rgba8(255, 0, 0, 255);
    assert_eq!((colour("a"), colour("c")), (red, red));
    assert_ne!(colour("b"), red);
}

#[test]
fn clearfix_pseudo_with_empty_content_generates_a_box() {
    let b = build(
        "<style>#a::after{content:'';display:block;clear:both}</style><body><div id=a>mid</div>",
    );
    assert_eq!(
        b.dump_of("a"),
        "block<div>
  anon
    items: \"mid\"
  block<div>
"
    );
}

// ---------------------------------------------------------------- paint style

fn styled(css: &str) -> Built {
    build(&format!("<body><div id=a style=\"{css}\">x</div>"))
}

#[test]
fn paint_colors_opacity_visibility_and_pointer_events() {
    let b = styled("color:rgb(255 0 0 / 50%);opacity:.25;visibility:hidden;pointer-events:none");
    let p = b.paint("a");
    assert_eq!(
        p.color,
        Color {
            r: 0.5,
            g: 0.0,
            b: 0.0,
            a: 0.5
        }
    );
    assert_eq!(p.opacity, 0.25);
    assert!(p.visibility_hidden);
    assert!(!p.pointer_events);
    let d = PaintStyle::default();
    let plain = build("<body><p id=a>x</p>");
    assert_eq!(plain.paint("a").opacity, d.opacity);
    assert!(plain.paint("a").pointer_events);
    assert!(!plain.paint("a").visibility_hidden);
}

#[test]
fn paint_background_layers_and_clip() {
    let b = styled(
        "background:url('http://example.test/i.png') no-repeat 10px 50% / 20px auto padding-box content-box, linear-gradient(to right, #f00, #00f) , #0f0",
    );
    let layers = &b.paint("a").background;
    assert_eq!(layers.len(), 3);
    let BackgroundContent::Image(key) = &layers[0].content else {
        panic!("image layer first");
    };
    assert_eq!(&*key.0, "http://example.test/i.png");
    assert_eq!(
        layers[0].repeat,
        (RepeatKind::NoRepeat, RepeatKind::NoRepeat)
    );
    assert_eq!(layers[0].position.0, LengthPercent::px(10.0));
    assert_eq!(layers[0].position.1, LengthPercent::fraction(0.5));
    assert_eq!(
        layers[0].size,
        BackgroundSize::Explicit(Some(LengthPercent::px(20.0)), None)
    );
    assert_eq!(layers[0].origin, BoxArea::PaddingBox);
    assert_eq!(layers[0].clip, BoxArea::ContentBox);
    let BackgroundContent::Gradient(g) = &layers[1].content else {
        panic!("gradient layer second");
    };
    assert_eq!(g.angle_deg, 90.0);
    assert_eq!(g.stops.len(), 2);
    assert_eq!(g.stops[0].position, LengthPercent::fraction(0.0));
    assert_eq!(g.stops[1].position, LengthPercent::fraction(1.0));
    assert_eq!(g.stops[1].color, Color::from_rgba8(0, 0, 255, 255));
    assert_eq!(
        layers[2].content,
        BackgroundContent::Color(Color::from_rgba8(0, 255, 0, 255))
    );
}

#[test]
fn paint_gradient_stops_without_positions_are_spread_evenly() {
    let b = styled("background:linear-gradient(0deg,#f00,#0f0,#00f,#fff)");
    let BackgroundContent::Gradient(g) = &b.paint("a").background[0].content else {
        panic!("gradient expected");
    };
    let fractions: Vec<f32> = g.stops.iter().map(|s| s.position.fraction).collect();
    assert!((fractions[1] - 1.0 / 3.0).abs() < 1e-5);
    assert!((fractions[2] - 2.0 / 3.0).abs() < 1e-5);
    assert!(!g.repeating);
}

#[test]
fn paint_borders_use_resolved_widths_colors_and_radii() {
    let b = styled(
        "border:3px solid #f00;border-bottom:5px dashed #00f;border-top-style:none;border-radius:4px 50% / 8px 25%",
    );
    let border = &b.paint("a").border;
    assert_eq!(
        border.widths,
        Edges {
            top: 0.0,
            right: 3.0,
            bottom: 5.0,
            left: 3.0
        }
    );
    assert_eq!(
        border.styles,
        [
            BorderStyle::None,
            BorderStyle::Solid,
            BorderStyle::Dashed,
            BorderStyle::Solid
        ]
    );
    assert_eq!(border.colors[2], Color::from_rgba8(0, 0, 255, 255));
    assert_eq!(border.radii[0], LengthPercent::px(4.0));
    assert_eq!(border.radii[1], LengthPercent::px(8.0));
    assert_eq!(border.radii[2], LengthPercent::fraction(0.5));
    assert_eq!(border.radii[3], LengthPercent::fraction(0.25));
}

#[test]
fn paint_border_color_defaults_to_current_color() {
    let b = styled("color:#0f0;border:2px solid");
    assert_eq!(
        b.paint("a").border.colors,
        [Color::from_rgba8(0, 255, 0, 255); 4]
    );
}

#[test]
fn paint_outline_shadows_and_text_decoration() {
    let b = styled(
        "outline:2px dotted #f00;outline-offset:3px;box-shadow:1px 2px 3px 4px #000, inset 0 0 5px #fff;text-shadow:1px 1px 2px #123;text-decoration:underline line-through wavy #f0f;",
    );
    let p = b.paint("a");
    let o = p.outline.expect("outline");
    assert_eq!(
        (o.width, o.style, o.offset),
        (2.0, BorderStyle::Dotted, 3.0)
    );
    assert_eq!(p.box_shadow.len(), 2);
    assert_eq!(p.box_shadow[0].offset, (1.0, 2.0));
    assert_eq!((p.box_shadow[0].blur, p.box_shadow[0].spread), (3.0, 4.0));
    assert!(!p.box_shadow[0].inset && p.box_shadow[1].inset);
    assert_eq!(p.text_shadow.len(), 1);
    assert_eq!(p.text_shadow[0].blur, 2.0);
    let d = p.text_decoration;
    assert!(d.underline && d.line_through && !d.overline);
    assert_eq!(d.style, LineStyle::Wavy);
    assert_eq!(d.color, Color::from_rgba8(255, 0, 255, 255));
}

#[test]
fn paint_underline_offset_and_position() {
    let b =
        styled("text-decoration:underline;text-underline-offset:3px;text-underline-position:under");
    let d = b.paint("a").text_decoration;
    assert_eq!(d.underline_offset, Some(LengthPercent::px(3.0)));
    assert_eq!(d.underline_position, crate::types::UnderlinePosition::Under);
    assert!(d.skip_ink);
    let plain = styled("text-decoration:underline")
        .paint("a")
        .text_decoration;
    assert_eq!(plain.underline_offset, None);
    assert_eq!(
        plain.underline_position,
        crate::types::UnderlinePosition::Auto
    );
}

#[test]
fn paint_overflow_position_and_z_index() {
    let b = styled("overflow-x:hidden;overflow-y:auto;position:relative;z-index:-3");
    let p = b.paint("a");
    assert_eq!(
        (p.overflow_x, p.overflow_y),
        (OverflowKind::Hidden, OverflowKind::Auto)
    );
    assert_eq!(p.position, PositionKind::Relative);
    assert_eq!(p.z_index, Some(-3));
    assert_eq!(build("<body><p id=a>").paint("a").z_index, None);
    let fixed = styled("position:fixed");
    assert_eq!(fixed.paint("a").position, PositionKind::Fixed);
}

#[test]
fn paint_transform_origin_filter_blend_and_isolation() {
    let b = styled(
        "transform:translate(10px,20px) scale(2);transform-origin:10% 4px;filter:blur(2px) grayscale(.5);mix-blend-mode:multiply;isolation:isolate",
    );
    let p = b.paint("a");
    let t = p.transform.expect("transform");
    assert_eq!(t.apply(1.0, 1.0), (12.0, 22.0));
    assert_eq!(
        p.transform_origin,
        (LengthPercent::fraction(0.1), LengthPercent::px(4.0))
    );
    assert_eq!(p.filter, [FilterOp::Blur(2.0), FilterOp::Grayscale(0.5)]);
    assert_eq!(p.blend, BlendMode::Multiply);
    assert!(p.isolate);
    assert_eq!(build("<body><p id=a>").paint("a").transform, None);
}

#[test]
fn percentage_translate_resolves_against_the_border_box() {
    let b = styled("transform:translate(-50%,-50%) rotate(90deg)");
    let id = b.principal("a");
    assert!(
        b.tree
            .get(id)
            .flags
            .contains(BoxFlags::PAINT_SIZE_DEPENDENT)
    );
    let p = b.tree.paint_style_for(id, Rect::new(0.0, 0.0, 100.0, 40.0));
    let t = p.transform.expect("transform");
    // The translation applies after the rotation: (1, 0) turns into (0, 1),
    // then moves by (-50, -20).
    let (x, y) = t.apply(1.0, 0.0);
    assert!(
        (x + 50.0).abs() < 1e-3 && (y + 19.0).abs() < 1e-3,
        "{x} {y}"
    );
    let other = b.tree.paint_style_for(id, Rect::new(0.0, 0.0, 200.0, 40.0));
    assert!((other.transform.unwrap().e + 100.0).abs() < 1e-3);
}

#[test]
fn individual_transform_properties_compose_in_css_order() {
    let b = styled("translate:10px 20%;rotate:90deg;scale:2;transform:translateX(5px)");
    let id = b.principal("a");
    assert!(
        b.tree
            .get(id)
            .flags
            .contains(BoxFlags::PAINT_SIZE_DEPENDENT)
    );
    let p = b.tree.paint_style_for(id, Rect::new(0.0, 0.0, 100.0, 50.0));
    // translate(10, 10) rotate(90) scale(2) translateX(5): (0,0) -> (5,0) -> (10,0) -> (0,10) -> (10,20).
    let (x, y) = p.transform.unwrap().apply(0.0, 0.0);
    assert!(
        (x - 10.0).abs() < 1e-3 && (y - 20.0).abs() < 1e-3,
        "{x} {y}"
    );
}

#[test]
fn length_only_transforms_do_not_depend_on_the_box() {
    let b = styled("transform:translate(3px,4px)");
    let id = b.principal("a");
    assert!(
        !b.tree
            .get(id)
            .flags
            .contains(BoxFlags::PAINT_SIZE_DEPENDENT)
    );
    assert!(std::sync::Arc::ptr_eq(
        &b.tree.paint_style_for(id, Rect::new(0.0, 0.0, 9.0, 9.0)),
        &b.tree.get(id).paint
    ));
}

#[test]
fn corner_gradients_aim_by_the_box_shape() {
    let b = styled("background:linear-gradient(to top right,#f00,#00f)");
    let id = b.principal("a");
    assert!(
        b.tree
            .get(id)
            .flags
            .contains(BoxFlags::PAINT_SIZE_DEPENDENT)
    );
    let angle = |w: f32, h: f32| {
        let p = b.tree.paint_style_for(id, Rect::new(0.0, 0.0, w, h));
        let BackgroundContent::Gradient(g) = &p.background[0].content else {
            panic!("gradient expected");
        };
        g.angle_deg
    };
    assert!((angle(100.0, 100.0) - 45.0).abs() < 1e-3);
    assert!((angle(200.0, 100.0) - 26.565).abs() < 1e-2);
    let d = styled("background:linear-gradient(to bottom left,#f00,#00f)");
    let p = d
        .tree
        .paint_style_for(d.principal("a"), Rect::new(0.0, 0.0, 100.0, 200.0));
    let BackgroundContent::Gradient(g) = &p.background[0].content else {
        panic!("gradient expected");
    };
    assert!((g.angle_deg - 243.435).abs() < 1e-2, "{}", g.angle_deg);
}

#[test]
fn unsupported_paint_features_are_flagged_not_silent() {
    let flagged = |css: &str| {
        let b = styled(css);
        b.tree
            .get(b.principal("a"))
            .flags
            .contains(BoxFlags::PAINT_SKIPPED)
    };
    assert!(flagged("background:radial-gradient(#f00,#00f)"));
    assert!(flagged("background:conic-gradient(#f00,#00f)"));
    assert!(flagged("background:linear-gradient(#f00,40%,#00f)"));
    assert!(flagged("rotate:1 0 0 30deg"));
    assert!(!flagged(
        "background:linear-gradient(#f00,#00f);transform:rotate(3deg)"
    ));
}

#[test]
fn paint_object_fit_position_and_image_rendering() {
    let b = styled("object-fit:cover;object-position:left 25%;image-rendering:pixelated");
    let p = b.paint("a");
    assert_eq!(p.object_fit, ObjectFit::Cover);
    assert_eq!(
        p.object_position,
        (LengthPercent::fraction(0.0), LengthPercent::fraction(0.25))
    );
    assert_eq!(p.image_rendering, ImageRendering::Pixelated);
}

#[test]
fn paint_calc_lengths_keep_their_percentage_part() {
    let b = styled("border-radius:calc(10px + 20%)");
    let r = b.paint("a").border.radii[0];
    assert!(
        (r.px - 10.0).abs() < 1e-3 && (r.fraction - 0.2).abs() < 1e-3,
        "{r:?}"
    );
}

// ---------------------------------------------------------------- scale

#[test]
#[ignore = "timing run; use --release --run-ignored only"]
fn five_thousand_element_page_builds() {
    let mut html = String::from("<body>");
    for i in 0..714 {
        let _ = write!(
            html,
            "<div class=r{}><p>Item <b>{i}</b> <span>text</span> more</p><ul><li>a<li>b</ul></div>",
            i % 7
        );
    }
    let (doc, engine) = style_html(&html, StyleEnv::default());
    let mut fonts = FontSystem::new_embedded();
    let mut samples = Vec::new();
    let mut boxes = 0;
    for _ in 0..30 {
        let cx = LayoutCx {
            doc: &doc,
            styles: &engine,
            fonts: &mut fonts,
            images: &NoImages,
            viewport: (800.0, 600.0),
            quirks: false,
        };
        let start = std::time::Instant::now();
        let tree = BoxTree::build(&cx);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        boxes = tree.len();
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "elements={} boxes={boxes} runs={} p50={:.3}ms p95={:.3}ms",
        doc.len(),
        samples.len(),
        samples[samples.len() / 2],
        samples[samples.len() * 95 / 100]
    );
}
