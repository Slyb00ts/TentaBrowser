//! Shaping with parley and extraction of lines and glyph runs.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parley::fontique::{FontStyle, FontWeight};
use parley::setting::Tag;
use parley::style::{
    FontFamily, FontFamilyName, FontFeature, FontFeatures, Language, TextStyle as ParleyStyle,
    TextWrapMode,
};
use parley::{
    BreakReason, CHROMIUM_LINE_BREAK_OVERRIDE, Cluster, IndentOptions, Layout, Line,
    LineBreakContext,
};

use super::cache::{self, Brush};
use super::shaped::{
    ClusterBox, IntrinsicWidths, LineEnd, ShapedLine, ShapedLines, ShapedRun, font_ref,
};
use super::style::{LineBreak, OverflowWrap, Paragraph, TextSpan, TextStyle, WordBreak};
use crate::font::{FontSystem, VerticalMetrics};
use crate::types::{Direction, Glyph, GlyphRun};

/// How a paragraph is broken into lines.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub(crate) struct ShapeOptions {
    /// The width lines may fill; `None` lets only segment breaks end a line.
    pub(crate) max_width: Option<f32>,
    /// `text-indent` for the first line. It narrows that line; glyph positions
    /// are still measured from the line's start edge, so the caller offsets
    /// them.
    pub(crate) first_line_indent: f32,
}

/// Blink's synthetic italic shear, `tan` of about 14 degrees rounded to a
/// quarter, which is what fontique's fake italic stands for.
const SYNTHETIC_ITALIC_SKEW: f32 = 0.25;

/// A strong character of the paragraph's direction, put in front of the text
/// because parley takes the base direction from the first strong character
/// while CSS fixes it. It is a zero-width format character and is dropped from
/// the output.
fn direction_mark(direction: Direction) -> &'static str {
    match direction {
        Direction::Ltr => "\u{200E}",
        Direction::Rtl => "\u{200F}",
    }
}

/// Whether `ch` ends a line by itself.
fn is_segment_break(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\r' | '\u{B}' | '\u{C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// How the line break opportunities inside one span are decided.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum BreakMode {
    /// Unicode rules with Chromium's tailoring.
    Default,
    /// `word-break: break-all`. The Chromium table suppresses opportunities
    /// between letters that break-all exists to allow, so only its rule for the
    /// position after spaces is kept.
    BreakAll,
    /// `line-break: anywhere`: an opportunity after every character unit.
    Anywhere,
}

impl BreakMode {
    fn of(style: &TextStyle) -> Self {
        if style.line_break == LineBreak::Anywhere {
            Self::Anywhere
        } else if style.word_break == WordBreak::BreakAll {
            Self::BreakAll
        } else {
            Self::Default
        }
    }
}

/// Marks that attach to the previous character and so never start a line.
fn attaches_to_previous(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            u32::from(ch),
            0x0300..=0x036F | 0x200B..=0x200F | 0xFE00..=0xFE0F
        )
}

/// The break override for one paragraph. Parley calls it once per pair of
/// adjacent characters, in text order and without offsets, so the closure counts
/// bytes to find the span a pair belongs to (the span of the character after the
/// opportunity) and applies that span's mode only there.
///
/// Two rules hold everywhere. A segment break is never an opportunity of its
/// own: parley takes a flagged opportunity in front of a newline for the break
/// and then never ends the line at the newline, which the Chromium table would
/// cause after every space. And nothing may separate the direction mark from the
/// first character, or the first line would be empty.
fn span_override(
    modes: Vec<(usize, BreakMode)>,
    mark: char,
) -> impl Fn(LineBreakContext) -> Option<bool> + Send + Sync {
    let after_offset = AtomicUsize::new(0);
    move |context| {
        let at = after_offset.fetch_add(context.after.len_utf8(), Ordering::Relaxed);
        if context.before == mark || is_segment_break(context.after) {
            return Some(false);
        }
        let index = modes
            .partition_point(|(end, _)| *end <= at)
            .min(modes.len().saturating_sub(1));
        match modes
            .get(index)
            .map_or(BreakMode::Default, |(_, mode)| *mode)
        {
            BreakMode::Default => CHROMIUM_LINE_BREAK_OVERRIDE(context),
            BreakMode::BreakAll => (context.before == ' ' && context.after != ' ').then_some(true),
            BreakMode::Anywhere if attaches_to_previous(context.after) => {
                CHROMIUM_LINE_BREAK_OVERRIDE(context)
            }
            BreakMode::Anywhere => Some(true),
        }
    }
}

fn parley_style(
    style: &TextStyle,
    families: &[&'static str],
    weight: f32,
    span_index: usize,
    ignore_break_word: bool,
) -> ParleyStyle<'static, 'static, Brush> {
    let overflow_wrap = match style.overflow_wrap {
        OverflowWrap::Normal => parley::OverflowWrap::Normal,
        OverflowWrap::Anywhere => parley::OverflowWrap::Anywhere,
        OverflowWrap::BreakWord if ignore_break_word => parley::OverflowWrap::Normal,
        OverflowWrap::BreakWord => parley::OverflowWrap::BreakWord,
    };
    let size = if style.size.is_finite() && style.size > 0.0 {
        style.size
    } else {
        0.0
    };
    ParleyStyle {
        font_family: FontFamily::List(Cow::Owned(
            families
                .iter()
                .map(|name| FontFamilyName::Named(Cow::Borrowed(*name)))
                .collect(),
        )),
        font_size: size,
        font_weight: FontWeight::new(weight),
        font_style: if style.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
        font_features: FontFeatures::List(Cow::Owned(
            style
                .features
                .iter()
                .map(|feature| FontFeature::new(Tag::from_bytes(feature.tag), feature.value))
                .collect(),
        )),
        locale: style
            .lang
            .as_deref()
            .and_then(|lang| Language::parse(lang).ok()),
        brush: u32::try_from(span_index).unwrap_or(u32::MAX).to_le_bytes(),
        word_spacing: style.word_spacing,
        letter_spacing: style.letter_spacing,
        word_break: match style.word_break {
            WordBreak::Normal => parley::WordBreak::Normal,
            WordBreak::BreakAll => parley::WordBreak::BreakAll,
            WordBreak::KeepAll => parley::WordBreak::KeepAll,
        },
        overflow_wrap,
        text_wrap_mode: if style.white_space.wraps() {
            TextWrapMode::Wrap
        } else {
            TextWrapMode::NoWrap
        },
        ..ParleyStyle::default()
    }
}

/// Shapes `paragraph` into an unbroken parley layout.
fn shape(
    fonts: &mut FontSystem,
    paragraph: &Paragraph<'_>,
    ignore_break_word: bool,
) -> Layout<Brush> {
    let mark = direction_mark(paragraph.direction);
    let text = format!("{mark}{}", paragraph.text);
    let resolved: Vec<(Vec<&'static str>, f32)> = paragraph
        .spans
        .iter()
        .map(|span| {
            let style = span.style;
            let font = fonts.resolve(&style.family_list, style.weight, style.italic);
            (fonts.family_stack(&style.family_list), font.parley_weight())
        })
        .collect();
    let line_break_override = span_override(
        paragraph
            .spans
            .iter()
            .map(|span| (span.range.end, BreakMode::of(span.style)))
            .collect(),
        mark.chars().next().unwrap_or_default(),
    );

    let (font_cx, layout_cx) = fonts.parley_contexts();
    let mut builder = layout_cx.style_run_builder(font_cx, &text, 1.0, false);
    builder.reserve(paragraph.spans.len() + 1, paragraph.spans.len() + 1);
    // The mark gets the first span's style without spacing, so that it stays
    // zero-width and does not narrow the first line.
    let unspaced = TextStyle {
        letter_spacing: 0.0,
        word_spacing: 0.0,
        ..paragraph.spans[0].style.clone()
    };
    let (families, weight) = &resolved[0];
    let mark_style = builder.push_style(parley_style(
        &unspaced,
        families,
        *weight,
        0,
        ignore_break_word,
    ));
    builder.push_style_run(mark_style, 0..mark.len());
    for (index, (span, (families, weight))) in paragraph.spans.iter().zip(&resolved).enumerate() {
        let style = parley_style(span.style, families, *weight, index, ignore_break_word);
        let id = builder.push_style(style);
        builder.push_style_run(
            id,
            mark.len() + span.range.start..mark.len() + span.range.end,
        );
    }
    builder.set_line_break_override(Some(&line_break_override));
    builder.build(&text)
}

fn break_lines(layout: &mut Layout<Brush>, options: ShapeOptions) {
    layout.set_text_indent(options.first_line_indent, IndentOptions::default());
    layout.break_all_lines(options.max_width.map(|width| width.max(0.0)));
}

/// The index of the span containing `byte`, or the last span past the end.
fn span_at(spans: &[TextSpan<'_>], byte: usize) -> usize {
    spans
        .partition_point(|span| span.range.end <= byte)
        .min(spans.len().saturating_sub(1))
}

/// The width of the spaces that hang off the logical end of a line, given the
/// line's clusters in any order as (byte range, advance). A segment break that
/// ends the line is not part of the width, but the spaces before it hang. Spaces
/// hang only where the line wraps: a `pre` line keeps them in its content.
///
/// This does not use parley's own figure because that one is zero on
/// right-to-left lines.
fn hanging_width(
    paragraph: &Paragraph<'_>,
    line_end: usize,
    clusters: impl IntoIterator<Item = (Range<usize>, f32)>,
) -> f32 {
    let last = span_at(paragraph.spans, line_end.saturating_sub(1));
    if !paragraph.spans[last].style.white_space.wraps() {
        return 0.0;
    }
    let mut clusters: Vec<_> = clusters.into_iter().collect();
    clusters.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let text_of = |range: &Range<usize>| &paragraph.text[range.clone()];
    clusters
        .iter()
        .skip_while(|(range, _)| text_of(range).chars().all(is_segment_break))
        .take_while(|(range, _)| text_of(range).chars().all(|ch| ch == ' '))
        .map(|(_, advance)| advance)
        .sum()
}

/// A line's content width and the width of the spaces that hang off its end,
/// read from parley's line for measuring.
fn line_widths(line: &Line<'_, Brush>, paragraph: &Paragraph<'_>, prefix: usize) -> (f32, f32) {
    let mut clusters = Vec::new();
    for run in line.runs() {
        for cluster in run.clusters() {
            let range = cluster.text_range();
            if range.end > prefix {
                clusters.push((
                    range.start.saturating_sub(prefix)..range.end - prefix,
                    cluster.advance(),
                ));
            }
        }
    }
    let end = line.text_range().end.saturating_sub(prefix);
    let hanging = hanging_width(paragraph, end, clusters);
    (line.metrics().advance - hanging, hanging)
}

fn span_index(cluster: &Cluster<'_, Brush>) -> usize {
    u32::from_le_bytes(cluster.first_style().brush) as usize
}

/// What the glyph runs cut from one parley run have in common.
struct RunFace {
    font: crate::types::FontRef,
    size: f32,
    synthetic_bold: bool,
    skew: f32,
    rtl: bool,
    metrics: VerticalMetrics,
}

/// A shaped run being filled cluster by cluster.
struct OpenRun {
    span: usize,
    x: f32,
    cursor: f32,
    glyphs: Vec<Glyph>,
    clusters: Vec<ClusterBox>,
    text: Range<usize>,
}

impl OpenRun {
    fn new(span: usize, x: f32, first: Range<usize>) -> Self {
        Self {
            span,
            x,
            cursor: 0.0,
            glyphs: Vec::new(),
            clusters: Vec::new(),
            text: first,
        }
    }

    fn push(&mut self, cluster: &Cluster<'_, Brush>, range: Range<usize>) {
        self.clusters.push(ClusterBox {
            text_range: range.clone(),
            x: self.cursor,
            advance: cluster.advance(),
        });
        self.text = self.text.start.min(range.start)..self.text.end.max(range.end);
        for glyph in cluster.glyphs() {
            self.glyphs.push(Glyph {
                id: glyph.id,
                x: self.cursor + glyph.x,
                y: glyph.y,
            });
            self.cursor += glyph.advance;
        }
    }

    fn finish(self, face: &RunFace) -> ShapedRun {
        ShapedRun {
            span: self.span,
            text_range: self.text,
            x: self.x,
            advance: self.cursor,
            rtl: face.rtl,
            glyphs: Arc::new(GlyphRun {
                font: face.font.clone(),
                size: face.size,
                glyphs: self.glyphs,
                synthetic_bold: face.synthetic_bold,
                skew: face.skew,
            }),
            metrics: face.metrics,
            clusters: self.clusters,
        }
    }
}

fn extract(layout: &Layout<Brush>, paragraph: &Paragraph<'_>, prefix: usize) -> ShapedLines {
    let mut lines = Vec::with_capacity(layout.len());
    for line in layout.lines() {
        let mut runs = Vec::new();
        let mut x = 0.0;
        for run in line.runs() {
            let synthesis = run.synthesis();
            let run_metrics = run.metrics();
            let face = RunFace {
                font: font_ref(run.font(), run.normalized_coords()),
                size: run.font_size(),
                synthetic_bold: synthesis.embolden(),
                skew: if synthesis.skew().is_some() {
                    SYNTHETIC_ITALIC_SKEW
                } else {
                    0.0
                },
                rtl: run.is_rtl(),
                metrics: VerticalMetrics {
                    ascent: run_metrics.ascent.round(),
                    descent: run_metrics.descent.round(),
                    line_gap: run_metrics.leading,
                },
            };
            let mut open: Option<OpenRun> = None;
            for cluster in run.visual_clusters() {
                let range = cluster.text_range();
                if range.end <= prefix {
                    continue;
                }
                let range = range.start.saturating_sub(prefix)..range.end - prefix;
                let span = span_index(&cluster);
                if open.as_ref().is_some_and(|open| open.span != span)
                    && let Some(done) = open.take()
                {
                    let done = done.finish(&face);
                    x += done.advance;
                    runs.push(done);
                }
                open.get_or_insert_with(|| OpenRun::new(span, x, range.clone()))
                    .push(&cluster, range);
            }
            if let Some(done) = open {
                let done = done.finish(&face);
                x += done.advance;
                runs.push(done);
            }
        }
        let metrics = runs
            .iter()
            .fold(VerticalMetrics::default(), |acc, run| VerticalMetrics {
                ascent: acc.ascent.max(run.metrics.ascent),
                descent: acc.descent.max(run.metrics.descent),
                line_gap: acc.line_gap.max(run.metrics.line_gap),
            });
        let text_range = line.text_range();
        let text_range =
            text_range.start.saturating_sub(prefix)..text_range.end.saturating_sub(prefix);
        let hanging = hanging_width(
            paragraph,
            text_range.end,
            runs.iter().flat_map(|run| {
                run.clusters
                    .iter()
                    .map(|cluster| (cluster.text_range.clone(), cluster.advance))
            }),
        );
        lines.push(ShapedLine {
            text_range,
            runs,
            advance: x - hanging,
            hanging,
            content_x: if paragraph.direction == Direction::Rtl {
                hanging
            } else {
                0.0
            },
            metrics,
            end: match line.break_reason() {
                BreakReason::None => LineEnd::Last,
                BreakReason::Regular => LineEnd::Soft,
                BreakReason::Explicit => LineEnd::Forced,
                BreakReason::Emergency => LineEnd::Overflow,
            },
        });
    }
    // A segment break at the very end of the text does not open another line.
    if lines.len() > 1 && lines.last().is_some_and(|line| line.text_range.is_empty()) {
        lines.pop();
        if let Some(last) = lines.last_mut() {
            last.end = LineEnd::Last;
        }
    }
    let width = lines
        .iter()
        .fold(0.0_f32, |widest, line| widest.max(line.advance));
    ShapedLines { lines, width }
}

/// Shapes `paragraph` and breaks it into lines.
///
/// The text must already be white-space processed: collapsing, trimming and
/// `text-transform` happen before, and every segment break (`\n`) left in the
/// text ends a line. Shaping is cached per paragraph, so asking again with a
/// different width only breaks lines again.
pub(crate) fn shape_and_break(
    fonts: &mut FontSystem,
    paragraph: &Paragraph<'_>,
    options: ShapeOptions,
) -> ShapedLines {
    if paragraph.text.is_empty() {
        return ShapedLines::default();
    }
    paragraph.assert_tiled();
    let prefix = direction_mark(paragraph.direction).len();
    cache::with_layout(
        paragraph,
        false,
        || shape(fonts, paragraph, false),
        |layout| {
            break_lines(layout, options);
            extract(layout, paragraph, prefix)
        },
    )
}

/// The min-content and max-content widths of `paragraph`.
///
/// Min-content is the widest piece when every soft wrap opportunity is taken,
/// `overflow-wrap: break-word` excluded; max-content is the widest line when
/// only segment breaks end lines. Spaces that would hang do not count.
pub(crate) fn measure(fonts: &mut FontSystem, paragraph: &Paragraph<'_>) -> IntrinsicWidths {
    if paragraph.text.is_empty() {
        return IntrinsicWidths::default();
    }
    paragraph.assert_tiled();
    let prefix = direction_mark(paragraph.direction).len();
    let widest = |layout: &Layout<Brush>| {
        layout
            .lines()
            .map(|line| line_widths(&line, paragraph, prefix).0)
            .fold(0.0_f32, f32::max)
    };
    let max = cache::with_layout(
        paragraph,
        false,
        || shape(fonts, paragraph, false),
        |layout| {
            break_lines(layout, ShapeOptions::default());
            widest(layout)
        },
    );
    let has_break_word = paragraph
        .spans
        .iter()
        .any(|span| span.style.overflow_wrap == OverflowWrap::BreakWord);
    let min = cache::with_layout(
        paragraph,
        has_break_word,
        || shape(fonts, paragraph, has_break_word),
        |layout| {
            break_lines(
                layout,
                ShapeOptions {
                    max_width: Some(0.0),
                    first_line_indent: 0.0,
                },
            );
            widest(layout)
        },
    );
    IntrinsicWidths { min, max }
}
