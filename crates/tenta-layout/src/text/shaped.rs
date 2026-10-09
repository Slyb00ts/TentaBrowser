//! The result of shaping and line breaking a paragraph.

use std::ops::Range;
use std::sync::{Arc, OnceLock};

use parley::FontData;

use crate::embedded_fonts::EMBEDDED_FONTS;
use crate::font::VerticalMetrics;
use crate::types::{FontBlob, FontRef, GlyphRun};

/// Why a line ended.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum LineEnd {
    /// At a soft wrap opportunity: the next line continues the same paragraph.
    Soft,
    /// Inside a word, because the line overflowed and `overflow-wrap` or
    /// `word-break` allowed it.
    Overflow,
    /// At a segment break in the text (a preserved newline).
    Forced,
    /// The end of the text.
    Last,
}

/// The extent of one cluster along the baseline, for mapping text offsets to
/// positions (`Range.getClientRects`, caret and hit testing).
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ClusterBox {
    /// Byte range of the cluster in the paragraph text.
    pub(crate) text_range: Range<usize>,
    /// Distance from the run's left edge to the cluster's left edge. The
    /// clusters of `ShapedRun::clusters` are in visual order, so this grows with
    /// the index in right-to-left runs too (where the byte ranges descend).
    pub(crate) x: f32,
    /// Advance of the cluster. A ligature gives its whole advance to the first
    /// cluster and none to the continuations.
    pub(crate) advance: f32,
}

/// Glyphs of one span in one font on one line.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ShapedRun {
    /// Index of the span in `Paragraph::spans` whose style this run uses. A
    /// span that falls back to a second font yields several runs.
    pub(crate) span: usize,
    /// Byte range of the text the run shapes.
    pub(crate) text_range: Range<usize>,
    /// Distance from the line's left edge to the run's left edge.
    pub(crate) x: f32,
    /// Total advance of the run.
    pub(crate) advance: f32,
    /// Whether the run is right to left. Runs of a line are in visual order.
    pub(crate) rtl: bool,
    /// The glyphs. Their `x` is measured from the run's left edge, so a
    /// fragment that starts at `x` needs no adjustment. `y` is an offset from the
    /// baseline with the screen's orientation: positive moves the glyph down,
    /// negative up (a mark placed above a tall letter has a negative `y`). Shared
    /// with the display list.
    pub(crate) glyphs: Arc<GlyphRun>,
    /// Vertical metrics of the run's font at its size, rounded as for
    /// `line-height: normal`. Fallback fonts report their own.
    pub(crate) metrics: VerticalMetrics,
    /// The clusters of the run, in visual order.
    pub(crate) clusters: Vec<ClusterBox>,
}

/// One line box worth of shaped text.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ShapedLine {
    /// Byte range of the text on the line, including the hanging spaces and a
    /// terminating segment break.
    pub(crate) text_range: Range<usize>,
    /// Runs in visual order.
    pub(crate) runs: Vec<ShapedRun>,
    /// Width of the line's content. Spaces that hang past the end of a wrapping
    /// line are not part of it; they follow it in `hanging`.
    pub(crate) advance: f32,
    /// Width of the trailing spaces that hang.
    pub(crate) hanging: f32,
    /// Where the content starts, measured from the line's left edge. The runs
    /// are laid out left to right over `hanging + advance` in both directions;
    /// the hanging spaces are at the logical end, so they sit at the right in a
    /// left-to-right paragraph (`content_x` is 0) and at the left in a
    /// right-to-left one (`content_x` is `hanging`). To align a right-to-left
    /// line to the right edge of its box, put the line's left edge at
    /// `box_right - content_x - advance`; the spaces then overhang to the left.
    pub(crate) content_x: f32,
    /// The largest ascent, descent and line gap of the line's runs.
    pub(crate) metrics: VerticalMetrics,
    /// Why the line ended.
    pub(crate) end: LineEnd,
}

/// The lines of a paragraph.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct ShapedLines {
    /// The lines in order; empty only for empty text.
    pub(crate) lines: Vec<ShapedLine>,
    /// The widest line's `advance`.
    pub(crate) width: f32,
}

/// The widths a paragraph needs at least and wants at most, in pixels.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub(crate) struct IntrinsicWidths {
    /// Min-content: the widest unbreakable piece.
    pub(crate) min: f32,
    /// Max-content: the widest line when only forced breaks apply.
    pub(crate) max: f32,
}

/// One shared blob per bundled file, so every run in a font carries the same
/// `Arc` and `FontRef` identity is a pointer comparison.
fn embedded_blobs() -> &'static [Arc<FontBlob>] {
    static BLOBS: OnceLock<Vec<Arc<FontBlob>>> = OnceLock::new();
    BLOBS.get_or_init(|| {
        EMBEDDED_FONTS
            .iter()
            .map(|font| Arc::new(FontBlob::Static(font.bytes)))
            .collect()
    })
}

fn empty_coords() -> Arc<[i16]> {
    static EMPTY: OnceLock<Arc<[i16]>> = OnceLock::new();
    EMPTY.get_or_init(|| Arc::from([])).clone()
}

/// The paint-side handle of a face parley picked.
pub(super) fn font_ref(font: &FontData, coords: &[i16]) -> FontRef {
    let bytes = font.data.data();
    let data = EMBEDDED_FONTS
        .iter()
        .position(|embedded| {
            std::ptr::eq(embedded.bytes.as_ptr(), bytes.as_ptr())
                && embedded.bytes.len() == bytes.len()
        })
        .map_or_else(
            || Arc::new(FontBlob::Owned(bytes.into())),
            |index| embedded_blobs()[index].clone(),
        );
    FontRef {
        data,
        index: font.index,
        coords: if coords.is_empty() {
            empty_coords()
        } else {
            Arc::from(coords)
        },
    }
}
