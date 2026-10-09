//! Inline layout through the real cascade and box tree.
//!
//! The geometry expectations were measured in headless Chrome on the same
//! bundled fonts (declared with `@font-face`), then written down as numbers;
//! no browser output is stored.

use tenta_core::NodeId;
use tenta_css::cascade::test_util::style_html;
use tenta_css::cascade::{StyleEngine, StyleEnv};
use tenta_dom::Document;

use crate::box_tree::{BoxTree, InlineFormattingContext, LayoutCx, NoImages};
use crate::font::FontSystem;
use crate::fragment::{FragId, FragmentBuilder, FragmentKind};
use crate::space::{AvailableSize, ConstraintSpace, InlineResult, NoFloats};
use crate::types::Rect;
use crate::{intrinsic_inline_content_sizes, layout_inline};

const PAGE_STYLE: &str = "<style>
    #t { font: 16px 'Liberation Sans'; }
    </style>";

/// One piece of text on a line, in the coordinates of the context.
#[derive(Clone, Debug, PartialEq)]
struct Piece {
    text: String,
    rect: Rect,
}

#[derive(Clone, Debug)]
struct LineInfo {
    rect: Rect,
    pieces: Vec<Piece>,
}

struct Laid {
    doc: Document,
    out: FragmentBuilder,
    result: InlineResult,
    engine: StyleEngine,
    tree: BoxTree,
    fonts: FontSystem,
}

fn page(body: &str) -> String {
    format!("<!doctype html><meta charset=utf-8>{PAGE_STYLE}{body}")
}

fn setup(html: &str) -> (Document, StyleEngine, BoxTree, FontSystem) {
    let (doc, engine) = style_html(html, StyleEnv::default());
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
    (doc, engine, tree, fonts)
}

fn ifc_of<'a>(
    doc: &Document,
    tree: &'a BoxTree,
    id: &str,
) -> (&'a InlineFormattingContext, crate::box_tree::BoxId) {
    let node = doc.get_element_by_id(id).expect("element");
    let box_id = tree.boxes_of(node)[0];
    (
        tree.get(box_id)
            .ifc
            .as_deref()
            .expect("inline formatting context"),
        box_id,
    )
}

/// Lays out the inline content of `#t` in `body` at `width`.
fn lay(body: &str, width: f32) -> Laid {
    lay_with(
        body,
        ConstraintSpace::new(AvailableSize::Definite(width), AvailableSize::MaxContent),
        false,
    )
}

fn lay_with(body: &str, space: ConstraintSpace, quirks: bool) -> Laid {
    let (doc, engine, tree, mut fonts) = setup(&page(body));
    let (ifc, _) = ifc_of(&doc, &tree, "t");
    let mut out = FragmentBuilder::new();
    let result = {
        let mut cx = LayoutCx {
            doc: &doc,
            styles: &engine,
            fonts: &mut fonts,
            images: &NoImages,
            viewport: (800.0, 600.0),
            quirks,
        };
        layout_inline(&mut cx, &tree, ifc, &space, &mut NoFloats, &mut out)
    };
    Laid {
        doc,
        out,
        result,
        engine,
        tree,
        fonts,
    }
}

impl Laid {
    fn text_of(&self, node: NodeId, range: &std::ops::Range<u32>) -> String {
        self.doc
            .text_data(node)
            .and_then(|d| d.get(range.start as usize..range.end as usize))
            .unwrap_or("?")
            .to_owned()
    }

    fn collect(&self, id: FragId, origin: (f32, f32), pieces: &mut Vec<Piece>) {
        let frag = self.out.get(id);
        let at = (origin.0 + frag.rect.x, origin.1 + frag.rect.y);
        if let FragmentKind::Text(t) = &frag.kind {
            pieces.push(Piece {
                text: self.text_of(frag.node, &t.text_range),
                rect: Rect::new(at.0, at.1, frag.rect.w, frag.rect.h),
            });
        }
        for &child in &frag.children {
            self.collect(child, at, pieces);
        }
    }

    fn lines(&self) -> Vec<LineInfo> {
        self.result
            .lines
            .iter()
            .map(|&id| {
                let line = self.out.get(id);
                let mut pieces = Vec::new();
                for &child in &line.children {
                    self.collect(child, (line.rect.x, line.rect.y), &mut pieces);
                }
                LineInfo {
                    rect: line.rect,
                    pieces,
                }
            })
            .collect()
    }

    /// The text on each line, pieces joined.
    fn line_texts(&self) -> Vec<String> {
        self.lines()
            .iter()
            .map(|l| l.pieces.iter().map(|p| p.text.as_str()).collect::<String>())
            .collect()
    }

    fn heights(&self) -> Vec<f32> {
        self.lines().iter().map(|l| l.rect.h).collect()
    }

    fn intrinsic(&mut self) -> (f32, f32) {
        let (ifc, _) = ifc_of(&self.doc, &self.tree, "t");
        let mut cx = LayoutCx {
            doc: &self.doc,
            styles: &self.engine,
            fonts: &mut self.fonts,
            images: &NoImages,
            viewport: (800.0, 600.0),
            quirks: false,
        };
        intrinsic_inline_content_sizes(&mut cx, &self.tree, ifc)
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.02
}

macro_rules! assert_close {
    ($a:expr, $b:expr) => {
        assert!(close($a, $b), "{} != {}", $a, $b)
    };
    ($a:expr, $b:expr, $($msg:tt)+) => {
        assert!(close($a, $b), "{} != {}: {}", $a, $b, format!($($msg)+))
    };
}

const LOREM: &str = "The quick brown fox jumps over the lazy dog and keeps running far away from the old farm house";

impl Laid {
    /// Border boxes, paddings and borders of the `tag` boxes, in the
    /// coordinates of the context, in line order.
    fn boxes(&self, tag: &str) -> Vec<(Rect, crate::types::Edges, crate::types::Edges)> {
        let mut found = Vec::new();
        let mut stack: Vec<(FragId, (f32, f32))> = self
            .result
            .lines
            .iter()
            .rev()
            .map(|&l| (l, (0.0, 0.0)))
            .collect();
        while let Some((id, origin)) = stack.pop() {
            let f = self.out.get(id);
            let at = (origin.0 + f.rect.x, origin.1 + f.rect.y);
            if matches!(f.kind, FragmentKind::Box)
                && f.node.is_some()
                && self.doc.tag(f.node).map(|t| t.to_string()).as_deref() == Some(tag)
            {
                found.push((
                    Rect::new(at.0, at.1, f.rect.w, f.rect.h),
                    f.padding,
                    f.border,
                ));
            }
            stack.extend(f.children.iter().rev().map(|&c| (c, at)));
        }
        found
    }

    fn texts(&self) -> Vec<(String, Rect)> {
        self.lines()
            .into_iter()
            .flat_map(|l| l.pieces.into_iter().map(|p| (p.text, p.rect)))
            .collect()
    }
}

fn div(style: &str, inner: &str) -> String {
    format!("<div id=t style=\"{style}\">{inner}</div>")
}

#[test]
fn a_paragraph_breaks_where_chrome_breaks_it() {
    let laid = lay(&div("", LOREM), 200.0);
    let texts = laid.line_texts();
    assert_eq!(texts[0], "The quick brown fox jumps ");
    assert_eq!(texts[1], "over the lazy dog and ");
    assert_eq!(texts[2], "keeps running far away ");
    let widths: Vec<f32> = laid.lines().iter().map(|l| l.pieces[0].rect.w).collect();
    assert_close!(widths[0], 190.3125);
    assert_close!(widths[1], 153.0);
    assert_eq!(laid.heights()[..3], [18.0, 18.0, 18.0]);
}

#[test]
fn line_counts_at_several_widths() {
    for (width, lines, text0) in [
        (137.0, 6, "The quick brown "),
        (90.0, 9, "The quick "),
        (345.0, 3, "The quick brown fox jumps over the lazy dog "),
    ] {
        let laid = lay(&div("", LOREM), width);
        assert_eq!(laid.result.lines.len(), lines, "width {width}");
        assert_eq!(laid.line_texts()[0], text0);
        assert_close!(laid.result.block_size, 18.0 * lines as f32);
        assert!(laid.result.inline_extent <= width + 0.01);
    }
}

#[test]
fn line_height_values() {
    for (style, each) in [
        ("line-height:30px", 30.0),
        ("line-height:1.5", 24.0),
        ("line-height:150%", 24.0),
        ("line-height:10px", 10.0),
        ("line-height:1.3", 20.796875),
        ("", 18.0),
    ] {
        let laid = lay(&div(style, LOREM), 200.0);
        assert_eq!(laid.result.lines.len(), 4, "{style}");
        for h in laid.heights() {
            assert_close!(h, each, "{style}");
        }
    }
}

#[test]
fn half_leading_puts_the_baseline_inside_the_line() {
    let laid = lay(&div("line-height:30px", "x"), 200.0);
    // Ascent 14 and descent 3 in a 30px line: 6 above, 7 below.
    assert_close!(laid.result.first_baseline.unwrap(), 20.0);
}

#[test]
fn forced_breaks_make_lines_and_a_trailing_one_makes_none() {
    let laid = lay(&div("", "one<br>two<br><br>three<br>"), 200.0);
    assert_eq!(laid.line_texts(), ["one", "two", "", "three"]);
    assert_close!(laid.result.block_size, 72.0);
    let only = lay(&div("", "<br>"), 200.0);
    assert_eq!(only.result.lines.len(), 1);
    assert_close!(only.result.block_size, 18.0);
}

#[test]
fn white_space_modes() {
    let pre = lay(
        &div(
            "white-space:pre",
            "line one\n  two  spaces\n\n\tTab\tcols\nlast",
        ),
        200.0,
    );
    assert_eq!(pre.result.lines.len(), 5);
    assert_eq!(pre.line_texts()[1], "  two  spaces");
    let tabs: Vec<(f32, f32)> = pre.lines()[3]
        .pieces
        .iter()
        .map(|p| (p.rect.x, p.rect.w))
        .collect();
    assert_close!(tabs[0].1, 35.5625);
    assert_close!(tabs[1].0, 35.5625);
    assert_close!(tabs[2].0 + tabs[2].1, 71.125);

    let wrap = lay(
        &div("white-space:pre-wrap", "aaaa bbbb      cccc dd"),
        100.0,
    );
    assert_eq!(wrap.line_texts(), ["aaaa bbbb      ", "cccc dd"]);
    // The spaces hang past the box and still count as text.
    assert_close!(wrap.lines()[0].pieces[0].rect.w, 102.3125);

    let line = lay(
        &div(
            "white-space:pre-line",
            "a   b   \n   c d e f g h i j k l m n o\n\n x",
        ),
        120.0,
    );
    // Texts are shown by their source bytes; the collapsed width is 22.25.
    assert_close!(line.lines()[0].pieces[0].rect.w, 22.25);
    assert_eq!(
        line.line_texts(),
        ["a   b ", "c d e f g h i j k l ", "m n o", "", "x"]
    );

    let spaces = lay(&div("white-space:break-spaces", "aaa      bbb  ccc"), 100.0);
    assert_eq!(spaces.line_texts(), ["aaa      bbb  ", "ccc"]);

    let nowrap = lay(
        &div("white-space:nowrap", "alpha beta gamma delta epsilon"),
        100.0,
    );
    assert_eq!(nowrap.result.lines.len(), 1);
    assert_close!(nowrap.result.inline_extent, 226.828_13);
}

#[test]
fn normal_white_space_collapses_across_inline_boundaries() {
    let laid = lay(&div("", "  a   b \n c\t\td   <b> e </b>  f   "), 300.0);
    let plain = lay(&div("", "a b c d <b>e</b> f"), 300.0);
    assert_eq!(laid.result.lines.len(), 1);
    assert_close!(laid.result.inline_extent, plain.result.inline_extent);
    // Collapsed runs map to the whole source run.
    assert_eq!(laid.line_texts()[0], "a   b \n c\t\td e f ");
}

#[test]
fn text_align_moves_lines_inside_the_box() {
    let text = "alpha beta gamma delta epsilon zeta";
    let center = lay(&div("text-align:center", text), 150.0);
    assert_close!(center.lines()[0].pieces[0].rect.x, 8.734375);
    assert_close!(center.lines()[1].pieces[0].rect.x, 12.734375);
    let right = lay(&div("text-align:right", text), 150.0);
    assert_close!(right.lines()[0].pieces[0].rect.x, 17.46875);
    let justify = lay(&div("text-align:justify", text), 150.0);
    let first = &justify.lines()[0].pieces[0].rect;
    assert_close!(first.x + first.w, 150.0);
    assert_close!(justify.lines()[1].pieces[0].rect.x, 0.0);
    let last = lay(
        &div("text-align:justify;text-align-last:center", text),
        150.0,
    );
    assert_close!(last.lines()[1].pieces[0].rect.x, 12.734375);
}

#[test]
fn moz_center_centres_inline_content() {
    let legacy = lay(&div("text-align:-webkit-center", "alpha beta"), 150.0);
    let plain = lay(&div("text-align:center", "alpha beta"), 150.0);
    assert_eq!(legacy.texts(), plain.texts());
}

#[test]
fn text_indent_narrows_the_first_line() {
    let laid = lay(
        &div("text-indent:30px", "alpha beta gamma delta epsilon zeta"),
        150.0,
    );
    assert_eq!(laid.line_texts()[0], "alpha beta ");
    assert_close!(laid.lines()[0].pieces[0].rect.x, 30.0);
    assert_close!(laid.lines()[1].pieces[0].rect.x, 0.0);
}

#[test]
fn inline_box_edges_belong_to_the_first_and_last_piece() {
    let laid = lay(
        &div(
            "",
            "xx <span style=\"margin:0 15px;padding:1px 6px;border:1px solid\">alpha beta gamma delta epsilon</span> yy zz",
        ),
        120.0,
    );
    let spans = laid.boxes("span");
    assert!(spans.len() >= 2);
    let (first, last) = (&spans[0], &spans[spans.len() - 1]);
    assert_close!(first.1.left, 6.0);
    assert_close!(first.1.right, 0.0);
    assert_close!(first.2.left, 1.0);
    assert_close!(last.1.left, 0.0);
    assert_close!(last.1.right, 6.0);
    assert_close!(last.2.right, 1.0);
    // The content area is the font's ascent and descent plus padding and border.
    assert_close!(first.0.h, 17.0 + 2.0 + 2.0);
    for middle in &spans[1..spans.len() - 1] {
        assert_eq!((middle.1.left, middle.1.right), (0.0, 0.0));
    }
}

#[test]
fn larger_inline_text_grows_the_line() {
    let laid = lay(
        &div(
            "",
            "small <span style=\"font-size:32px\">BIG text</span> small again <span style=\"font-size:8px\">tiny</span> end",
        ),
        300.0,
    );
    assert_eq!(laid.result.lines.len(), 1);
    assert_close!(laid.result.block_size, 37.0);
    assert_close!(laid.result.first_baseline.unwrap(), 29.0);
    let tall = lay(
        &div(
            "",
            "a<span style=\"font-size:40px;line-height:1\">B</span>c<span style=\"font-size:10px\">d</span>",
        ),
        300.0,
    );
    assert_close!(tall.result.block_size, 40.0);
    let spaced = lay(
        &div(
            "",
            "a<span style=\"line-height:40px\">B</span>c<span style=\"line-height:2\">d</span>",
        ),
        300.0,
    );
    assert_close!(spaced.result.block_size, 40.0);
}

#[test]
fn a_line_with_nothing_in_it_is_not_a_line() {
    let vertical = lay(&div("", "<span style=\"padding:5px 0\"></span>"), 200.0);
    assert!(vertical.result.lines.is_empty());
    assert_eq!(vertical.result.block_size, 0.0);
    assert_eq!(vertical.result.first_baseline, None);
    let horizontal = lay(&div("", "<span style=\"padding:0 10px\"></span>"), 200.0);
    assert_eq!(horizontal.result.lines.len(), 1);
    assert_close!(horizontal.result.block_size, 18.0);
    assert_close!(horizontal.boxes("span")[0].0.w, 20.0);
}

#[test]
fn words_that_do_not_fit_overflow_or_break_as_asked() {
    let long = "short verylongwordthatdoesnotfitatall tail";
    assert_eq!(
        lay(&div("", long), 100.0).line_texts(),
        ["short ", "verylongwordthatdoesnotfitatall ", "tail"]
    );
    let broken = lay(&div("overflow-wrap:break-word", long), 100.0);
    assert!(broken.result.lines.len() > 3);
    assert!(broken.result.inline_extent <= 100.01);
    let all = lay(&div("word-break:break-all", long), 100.0);
    assert!(all.lines().iter().all(|l| l.pieces[0].rect.w <= 100.01));
}

#[test]
fn intrinsic_sizes_agree_with_layout() {
    let mut laid = lay(&div("", LOREM), 200.0);
    let (min, max) = laid.intrinsic();
    let at_max = lay_with(
        &div("", LOREM),
        ConstraintSpace::new(AvailableSize::MaxContent, AvailableSize::MaxContent),
        false,
    );
    let at_min = lay_with(
        &div("", LOREM),
        ConstraintSpace::new(AvailableSize::MinContent, AvailableSize::MaxContent),
        false,
    );
    assert_eq!(at_max.result.lines.len(), 1);
    assert_close!(max, at_max.result.inline_extent);
    assert_close!(min, at_min.result.inline_extent);
    let widest = LOREM
        .split(' ')
        .map(|word| lay(&div("", word), 500.0).result.inline_extent)
        .fold(0.0_f32, f32::max);
    assert_close!(min, widest);
}

#[test]
fn inline_padding_counts_in_intrinsic_sizes() {
    let mut plain = lay(&div("", "a cd"), 200.0);
    let mut padded = lay(
        &div("", "a <span style=\"padding:0 10px\">cd</span>"),
        200.0,
    );
    let (pmin, pmax) = plain.intrinsic();
    let (smin, smax) = padded.intrinsic();
    assert_close!(smax - pmax, 20.0);
    assert_close!(smin - pmin, 20.0);
}

#[test]
fn text_decoration_reaches_text_without_stacking_effects() {
    let laid = lay(&div("", "plain <u style=\"opacity:.5\">under</u>"), 200.0);
    let mut underlined = 0;
    for id in 0..laid.out.len() as u32 {
        let f = laid.out.get(FragId(id));
        if matches!(f.kind, FragmentKind::Text(_)) {
            assert_eq!(f.paint.opacity, 1.0);
            if f.paint.text_decoration.underline {
                underlined += 1;
            }
        }
    }
    assert_eq!(underlined, 1);
}

#[test]
fn text_transform_keeps_source_ranges() {
    let laid = lay(&div("text-transform:uppercase", "straße  x"), 200.0);
    let plain = lay(&div("", "STRASSE X"), 200.0);
    assert_close!(laid.result.inline_extent, plain.result.inline_extent);
    assert_eq!(laid.line_texts()[0], "straße  x");
}

#[test]
fn right_to_left_blocks_start_at_the_right_edge() {
    let laid = lay(
        &div("direction:rtl", "alpha beta gamma delta epsilon"),
        150.0,
    );
    for line in laid.lines() {
        let last = line.pieces.last().unwrap();
        assert!(last.rect.x + last.rect.w <= 150.01);
        assert!(last.rect.x + last.rect.w > 150.0 - 10.0);
    }
    let hebrew = lay(&div("direction:rtl", "שלום עולם"), 300.0);
    let rect = hebrew.lines()[0].pieces[0].rect;
    assert_close!(rect.x + rect.w, 300.0);
}

#[test]
fn layout_is_deterministic() {
    let html = div(
        "text-align:justify",
        "a b <b>c d e</b> f g <i>h i j k l m n o p</i> x y z",
    );
    let a = lay(&html, 120.0);
    let b = lay(&html, 120.0);
    assert_eq!(a.result, b.result);
    assert_eq!(a.texts(), b.texts());
}

#[test]
fn deeply_nested_inline_boxes_do_not_overflow_the_stack() {
    let open = "<span>".repeat(3000);
    let close = "</span>".repeat(3000);
    let laid = lay(&div("", &format!("{open}deep text{close}")), 200.0);
    assert_eq!(laid.result.lines.len(), 1);
}

#[test]
fn capitalize_continues_a_word_across_inline_boxes() {
    let laid = lay(
        &div(
            "text-transform:capitalize",
            "hello <b>f</b>oo wor<i>ld</i> x",
        ),
        300.0,
    );
    let plain = lay(&div("", "Hello <b>F</b>oo Wor<i>ld</i> X"), 300.0);
    assert_close!(laid.result.inline_extent, plain.result.inline_extent);
    // "foo" and "world" were not capitalised a second time.
    let lower = lay(&div("", "Hello <b>F</b>OO WOR<i>LD</i> X"), 300.0);
    assert!(laid.result.inline_extent < lower.result.inline_extent);
}

#[test]
fn text_transform_changes_the_glyphs_not_the_source_range() {
    let laid = lay(&div("text-transform:uppercase", "straße  x"), 200.0);
    let plain = lay(&div("", "STRASSE X"), 200.0);
    let glyphs = |l: &Laid| -> usize {
        (0..l.out.len() as u32)
            .filter_map(|i| match &l.out.get(FragId(i)).kind {
                FragmentKind::Text(t) => Some(t.runs.iter().map(|r| r.glyphs.len()).sum::<usize>()),
                _ => None,
            })
            .sum()
    };
    assert_eq!(glyphs(&laid), glyphs(&plain));
    assert_eq!(laid.line_texts(), ["straße  x"]);
}

#[test]
fn nowrap_inside_a_wrapping_parent_stays_together() {
    let laid = lay(
        &div(
            "",
            "aaa <span style=\"white-space:nowrap\">bbbb cccc dddd</span> eee fff",
        ),
        100.0,
    );
    let texts = laid.texts();
    assert_eq!(texts.len(), 3);
    assert_close!(texts[0].1.w, 26.703125);
    assert_close!(texts[1].1.w, 112.078125);
    assert_close!(texts[1].1.y, 18.0);
    assert_close!(texts[2].1.w, 43.90625);
    assert_close!(laid.result.block_size, 54.0);
}

#[test]
fn justification_widens_every_space_by_the_same_amount() {
    let glyph_xs = |laid: &Laid| -> Vec<f32> {
        (0..laid.out.len() as u32)
            .filter_map(|i| match &laid.out.get(FragId(i)).kind {
                FragmentKind::Text(t) => Some(
                    t.runs
                        .iter()
                        .flat_map(|r| r.glyphs.iter().map(|g| g.x))
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .next()
            .unwrap()
    };
    let text = "alpha beta gamma delta";
    let plain = lay(&div("", &format!("{text} zz")), 150.0);
    let justified = lay(&div("text-align:justify", &format!("{text} zz")), 150.0);
    let first = &plain.lines()[0].pieces[0].rect;
    let spaces = plain.line_texts()[0].trim_end().matches(' ').count();
    let gap = (150.0 - first.w) / spaces as f32;
    assert!(gap > 0.0 && spaces >= 2);
    let shifts: Vec<f32> = glyph_xs(&justified)
        .iter()
        .zip(glyph_xs(&plain))
        .map(|(j, p)| j - p)
        .collect();
    let mut distinct: Vec<f32> = Vec::new();
    for s in &shifts {
        if !distinct.iter().any(|d| (d - s).abs() < 0.01) {
            distinct.push(*s);
        }
    }
    assert_eq!(distinct.len(), spaces + 1, "{distinct:?}");
    for (i, d) in distinct.iter().enumerate() {
        assert_close!(*d, gap * i as f32);
    }
}

#[test]
fn break_word_splits_only_words_that_overflow() {
    let long = "aaa bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb ccc";
    let laid = lay(&div("overflow-wrap:break-word", long), 100.0);
    assert_eq!(laid.line_texts()[0], "aaa ");
    assert!(laid.result.lines.len() >= 5);
    for line in laid.lines() {
        let w: f32 = line.pieces.iter().map(|p| p.rect.w).sum();
        assert!(w <= 100.01, "{w}");
    }
    assert_eq!(laid.line_texts().concat(), long);
    // The same word inside a nested inline box breaks the same way.
    let nested = lay(
        &div(
            "overflow-wrap:break-word",
            "xx ver<b>ylongwordthatdoesnotfitatall</b>andmore yy",
        ),
        100.0,
    );
    for line in nested.lines() {
        assert!(line.pieces.iter().map(|p| p.rect.w).sum::<f32>() <= 100.01);
    }
    assert_eq!(
        nested.line_texts().concat(),
        "xx verylongwordthatdoesnotfitatallandmore yy"
    );
}

#[test]
fn a_huge_unbroken_string_is_not_quadratic() {
    let token = "x".repeat(80_000);
    for style in [
        "overflow-wrap:break-word",
        "overflow-wrap:anywhere",
        "word-break:break-all",
        "",
    ] {
        let start = std::time::Instant::now();
        let laid = lay(&div(style, &token), 200.0);
        let took = start.elapsed();
        let expected = if style.is_empty() { 1 } else { 3200 };
        assert_eq!(laid.result.lines.len(), expected, "{style}");
        // Quadratic scans of this string take minutes; the linear ones a few
        // seconds even in a debug build on a loaded machine.
        assert!(took.as_secs() < 40, "{style}: {took:?}");
    }
}

#[test]
fn a_soft_hyphen_shows_a_hyphen_only_where_the_line_breaks() {
    let laid = lay(
        &div(
            "",
            "Averyveryverylong&shy;wordthat&shy;breaks here and there",
        ),
        100.0,
    );
    let texts = laid.texts();
    // 131.3125 of text plus the 5.328 px hyphen.
    assert_close!(texts[0].1.w, 131.3125 + 5.328125);
    assert_close!(texts[1].1.w, 61.375 + 5.328125);
    let unbroken = lay(&div("", "Averyveryverylong&shy;wordthat"), 300.0);
    let single = lay(&div("", "Averyveryverylongwordthat"), 300.0);
    assert_close!(unbroken.result.inline_extent, single.result.inline_extent);
}

#[test]
fn a_closing_edge_after_a_trailing_space_does_not_move_the_span() {
    let laid = lay(
        &div(
            "",
            "aaa bbb <span style=\"padding-right:20px\">ccc </span>ddd eee",
        ),
        90.0,
    );
    assert_eq!(laid.result.lines.len(), 2);
    let span = &laid.boxes("span")[0];
    assert_close!(span.0.x, 62.28125);
    assert_close!(span.0.w, 44.0);
    assert_close!(span.0.y, 0.0);
}

#[test]
fn text_paint_is_shared_across_layouts_of_a_box() {
    let (doc, engine, tree, mut fonts) = setup(&page(&div("", "<b>one</b> two")));
    let (ifc, _) = ifc_of(&doc, &tree, "t");
    let space = ConstraintSpace::new(AvailableSize::Definite(300.0), AvailableSize::MaxContent);
    let mut paints = Vec::new();
    for _ in 0..2 {
        let mut out = FragmentBuilder::new();
        let mut cx = LayoutCx {
            doc: &doc,
            styles: &engine,
            fonts: &mut fonts,
            images: &NoImages,
            viewport: (800.0, 600.0),
            quirks: false,
        };
        layout_inline(&mut cx, &tree, ifc, &space, &mut NoFloats, &mut out);
        for i in 0..out.len() as u32 {
            if matches!(out.get(FragId(i)).kind, FragmentKind::Text(_)) {
                paints.push(out.get(FragId(i)).paint.clone());
            }
        }
    }
    assert_eq!(paints.len(), 4);
    assert!(std::sync::Arc::ptr_eq(&paints[0], &paints[2]));
    assert!(std::sync::Arc::ptr_eq(&paints[1], &paints[3]));
    assert!(!std::sync::Arc::ptr_eq(&paints[0], &paints[1]));
}

#[test]
fn the_chunk_cut_keeps_the_wrap_opportunity_after_a_space() {
    const CAP: usize = 24 * 1024;
    // The space is the last byte under the cap; the cut falls after it.
    let words = format!("{} {} tail", "w".repeat(CAP - 1), "z".repeat(10));
    // The space is the first byte over the cap; there is none before it.
    let over = format!("{} {} tail", "w".repeat(CAP), "z".repeat(10));
    for text in [words, over] {
        let laid = lay(&div("", &text), 120.0);
        let lines = laid.line_texts();
        assert_eq!(lines.concat(), text);
        assert!(
            lines[0].starts_with('w') && lines[0].ends_with(' '),
            "{}",
            &lines[0][lines[0].len() - 3..]
        );
        assert!(lines[1].starts_with('z'), "{:?}", &lines[1][..5]);
        // Nothing was glued or lost: the same words laid out in one piece
        // break in the same places.
        let short = lay(
            &div("", &format!("{} {} tail", "w".repeat(30), "z".repeat(10))),
            120.0,
        );
        assert_eq!(&lines[1..], &short.line_texts()[1..]);
    }
}
