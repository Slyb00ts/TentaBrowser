//! Text, decoration and `text-shadow` items.
//!
//! Decoration lines follow Blink's geometry, measured against Chrome on
//! Liberation Sans: whole-pixel rows, an automatic thickness of a tenth of
//! the font size, and positions taken from the first run's ascent. Skip-ink
//! is not implemented: underlines run unbroken through descenders, and each
//! underlined fragment that asked for skip-ink is counted as unsupported.

use std::collections::BTreeMap;

use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::raw::TableProvider;
use skrifa::raw::types::F2Dot14;
use tenta_layout::{
    Color, DecorationPaint, Fragment, FragmentKind, GlyphRun, LineStyle, Rect, TextFragment,
    UnderlinePosition,
};

use crate::dl::Item;

/// The thinnest decoration line, in CSS pixels.
const MIN_THICKNESS: f32 = 1.0;

/// Pushes the text, decoration and text-shadow items of a text fragment;
/// does nothing for other fragment kinds.
///
/// Shadows come first, the last-listed shadow lowest, then the text itself.
/// A shadow is the same text and the same lines in the shadow's colour,
/// displaced by its offset. Blurred shadows are not drawn: there is no blur
/// item, and a sharp copy would misrepresent them.
pub(super) fn emit(frag: &Fragment, border_box: Rect, items: &mut Vec<Item>) {
    let FragmentKind::Text(text) = &frag.kind else {
        return;
    };
    let Some(first) = text.runs.first() else {
        return;
    };
    let style = &frag.paint;
    let baseline = border_box.y + text.baseline;
    let lines = Lines::new(&style.text_decoration, first, border_box, baseline);
    let decoration_color = style.text_decoration.color;
    for shadow in style.text_shadow.iter().rev().filter(|s| s.blur <= 0.0) {
        let (dx, dy) = shadow.offset;
        paint(
            items,
            text,
            (border_box.x + dx, baseline + dy),
            shadow.color,
            &lines.displaced(dx, dy),
            shadow.color,
        );
    }
    paint(
        items,
        text,
        (border_box.x, baseline),
        style.color,
        &lines,
        decoration_color,
    );
}

/// Underlines and overlines under the glyphs, then the glyphs, then the
/// line-through over them, as Blink paints them.
fn paint(
    items: &mut Vec<Item>,
    text: &TextFragment,
    origin: (f32, f32),
    color: Color,
    lines: &Lines,
    decoration_color: Color,
) {
    let line = |rect: Rect| Item::Decoration {
        rect,
        color: decoration_color,
        style: lines.style,
    };
    if !decoration_color.is_transparent() {
        items.extend(lines.behind.iter().copied().map(line));
    }
    if !color.is_transparent() {
        items.extend(
            text.runs
                .iter()
                .filter(|run| !run.glyphs.is_empty())
                .map(|run| Item::Text {
                    origin,
                    run: run.clone(),
                    color,
                }),
        );
    }
    if let Some(rect) = lines.through.filter(|_| !decoration_color.is_transparent()) {
        items.push(line(rect));
    }
}

/// The boxes of a fragment's decoration lines.
struct Lines {
    style: LineStyle,
    /// Underline and overline, which sit beneath the glyphs.
    behind: Vec<Rect>,
    /// Line-through, which sits over the glyphs.
    through: Option<Rect>,
}

impl Lines {
    /// Sizes and places the lines for a fragment whose first run is in `run`'s
    /// font. A fragment mixing fonts (a fallback face for one character) is
    /// decorated with the first run's metrics, so the line stays straight.
    ///
    /// Rows are whole CSS pixels, as Chrome draws them. The automatic
    /// thickness is the font size over ten, floored (the font's own `post`
    /// thickness gives 2 rows at 16 px where Chrome draws 1). Blink places
    /// the automatic underline a gap below the baseline, half the unrounded
    /// thickness rounded up and at least a pixel. The overline sits a
    /// thickness above the rounded ascent, and the line-through is centred a
    /// third of the rounded ascent above the baseline.
    fn new(decoration: &DecorationPaint, run: &GlyphRun, border_box: Rect, baseline: f32) -> Self {
        let mut lines = Lines {
            style: decoration.style,
            behind: Vec::new(),
            through: None,
        };
        if border_box.w <= 0.0
            || !(decoration.underline || decoration.overline || decoration.line_through)
        {
            return lines;
        }
        let size = run.size;
        let metrics = FontMetrics::of(run);
        let ascent = metrics.ascent.round();
        let exact = decoration
            .thickness
            .map_or(size / 10.0, |t| t.max(MIN_THICKNESS));
        let thickness = match decoration.thickness {
            Some(_) => exact.round(),
            None => exact.floor().max(MIN_THICKNESS),
        };
        // Double and wavy lines are drawn by splitting one band into thirds.
        let band = match decoration.style {
            LineStyle::Double | LineStyle::Wavy => 3.0,
            LineStyle::Solid | LineStyle::Dotted | LineStyle::Dashed => 1.0,
        };
        let line = |top: f32| Rect::new(border_box.x, top.round(), border_box.w, thickness * band);
        if decoration.underline {
            let below = match decoration.underline_position {
                UnderlinePosition::Auto => (exact / 2.0).ceil().max(1.0),
                UnderlinePosition::FromFont => metrics.underline_depth,
                UnderlinePosition::Under => metrics.descent.round(),
            };
            let offset = decoration.underline_offset.map_or(0.0, |o| o.resolve(size));
            lines.behind.push(line(baseline + below + offset));
        }
        if decoration.overline {
            lines.behind.push(line(baseline - ascent - thickness));
        }
        if decoration.line_through {
            lines.through = Some(line(baseline - ascent / 3.0 - thickness / 2.0));
        }
        lines
    }

    fn displaced(&self, dx: f32, dy: f32) -> Lines {
        Lines {
            style: self.style,
            behind: self.behind.iter().map(|r| r.translated(dx, dy)).collect(),
            through: self.through.map(|r| r.translated(dx, dy)),
        }
    }
}

/// The font numbers decoration needs, in CSS pixels at the run's size.
#[derive(Copy, Clone, Debug, PartialEq)]
struct FontMetrics {
    ascent: f32,
    /// Descent as a positive distance below the baseline.
    descent: f32,
    /// Distance below the baseline of the `post` underline position.
    underline_depth: f32,
}

impl FontMetrics {
    /// Reads the metrics of the run's face; a face that does not parse or a
    /// value that is missing falls back to proportions of the size.
    fn of(run: &GlyphRun) -> Self {
        let size = run.size;
        let fallback = FontMetrics {
            ascent: size * 0.8,
            descent: size * 0.2,
            underline_depth: size / 10.0,
        };
        let usable = |v: f32| v.is_finite() && v > 0.0;
        let Some(read) = Self::read(run, size) else {
            return fallback;
        };
        FontMetrics {
            ascent: if usable(read.ascent) {
                read.ascent
            } else {
                fallback.ascent
            },
            descent: if usable(read.descent) {
                read.descent
            } else {
                fallback.descent
            },
            underline_depth: if read.underline_depth.is_finite() && read.underline_depth > 0.0 {
                read.underline_depth
            } else {
                fallback.underline_depth
            },
        }
    }

    fn read(run: &GlyphRun, size: f32) -> Option<Self> {
        let font = skrifa::FontRef::from_index(run.font.data.bytes(), run.font.index).ok()?;
        let coords: Vec<NormalizedCoord> = run
            .font
            .coords
            .iter()
            .map(|&c| F2Dot14::from_bits(c))
            .collect();
        let metrics = font.metrics(Size::new(size), LocationRef::new(&coords));
        let scale = size / f32::from(font.head().ok()?.units_per_em());
        let position = font.post().ok()?.underline_position().to_i16();
        Some(FontMetrics {
            ascent: metrics.ascent,
            descent: -metrics.descent,
            underline_depth: -f32::from(position) * scale,
        })
    }
}

/// Counts what `frag` asks of the text emitter that is not drawn: blurred
/// shadows and skip-ink on underlines.
pub(super) fn count_unsupported(frag: &Fragment, counts: &mut BTreeMap<&'static str, usize>) {
    let FragmentKind::Text(text) = &frag.kind else {
        return;
    };
    if frag.paint.visibility_hidden || text.runs.is_empty() {
        return;
    }
    let blurred = frag
        .paint
        .text_shadow
        .iter()
        .filter(|s| s.blur > 0.0 && !s.color.is_transparent())
        .count();
    if blurred > 0 {
        *counts.entry("TextShadowBlur").or_default() += blurred;
    }
    let decoration = &frag.paint.text_decoration;
    if decoration.underline && decoration.skip_ink && frag.rect.w > 0.0 {
        *counts.entry("TextDecorationSkipInk").or_default() += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tenta_core::NodeId;
    use tenta_layout::embedded_fonts::EMBEDDED_FONTS;
    use tenta_layout::{FontBlob, FontRef, Glyph, LengthPercent, PaintStyle, ShadowPaint};

    use super::*;

    const SANS: usize = 0;
    const DEJAVU: usize = 6;

    fn font(blob: FontBlob) -> FontRef {
        FontRef {
            data: Arc::new(blob),
            index: 0,
            coords: Arc::from([]),
        }
    }

    fn embedded(index: usize) -> FontRef {
        font(FontBlob::Static(EMBEDDED_FONTS[index].bytes))
    }

    fn run(font: FontRef, size: f32) -> Arc<GlyphRun> {
        Arc::new(GlyphRun {
            font,
            size,
            glyphs: vec![Glyph {
                id: 36,
                x: 0.0,
                y: 0.0,
            }],
            synthetic_bold: false,
            skew: 0.0,
        })
    }

    /// A text fragment whose baseline is `baseline` below the top of a border
    /// box at the origin.
    fn fragment_at(
        runs: Vec<Arc<GlyphRun>>,
        baseline: f32,
        edit: impl FnOnce(&mut PaintStyle),
    ) -> (Fragment, Rect) {
        let mut style = PaintStyle {
            color: Color::from_rgba8(10, 20, 30, 255),
            ..PaintStyle::default()
        };
        edit(&mut style);
        let text = TextFragment {
            text_range: 0..1,
            runs: runs.into_iter().collect(),
            baseline,
        };
        let rect = Rect::new(30.0, 40.0, 50.0, 18.0);
        let frag = Fragment::new(NodeId(2), FragmentKind::Text(text), Arc::new(style), rect);
        (frag, rect)
    }

    fn fragment(runs: Vec<Arc<GlyphRun>>, edit: impl FnOnce(&mut PaintStyle)) -> (Fragment, Rect) {
        fragment_at(runs, 15.0, edit)
    }

    fn emitted(frag: &Fragment, border_box: Rect) -> Vec<Item> {
        let mut items = Vec::new();
        emit(frag, border_box, &mut items);
        items
    }

    fn underlined(edit: impl FnOnce(&mut DecorationPaint)) -> impl FnOnce(&mut PaintStyle) {
        move |style| {
            style.text_decoration.underline = true;
            style.text_decoration.color = Color::from_rgba8(200, 0, 0, 255);
            edit(&mut style.text_decoration);
        }
    }

    fn decorations(items: &[Item]) -> Vec<(Rect, LineStyle)> {
        items
            .iter()
            .filter_map(|item| match item {
                Item::Decoration { rect, style, .. } => Some((*rect, *style)),
                _ => None,
            })
            .collect()
    }

    /// The single line a fragment decorated with `edit` gets, as
    /// `(top row, rows)`, for Liberation Sans at `size` with its baseline
    /// on row `baseline`.
    fn sole_line(size: f32, baseline: f32, edit: impl FnOnce(&mut PaintStyle)) -> (f32, f32) {
        let (mut frag, _) = fragment_at(vec![run(embedded(SANS), size)], 0.0, edit);
        let FragmentKind::Text(text) = &mut frag.kind else {
            unreachable!("built as text");
        };
        text.baseline = baseline;
        let bb = Rect::new(0.0, 0.0, 50.0, 100.0);
        let [(rect, _)] = decorations(&emitted(&frag, bb))[..] else {
            panic!("one line expected");
        };
        (rect.y, rect.h)
    }

    fn kinds(items: &[Item]) -> Vec<&'static str> {
        items
            .iter()
            .map(|item| match item {
                Item::Text { .. } => "text",
                Item::Decoration { .. } => "line",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn a_run_is_drawn_at_the_absolute_baseline_in_the_text_colour() {
        let glyphs = run(embedded(SANS), 16.0);
        let (frag, bb) = fragment(vec![glyphs.clone()], |_| {});
        assert_eq!(
            emitted(&frag, bb),
            vec![Item::Text {
                origin: (30.0, 55.0),
                run: glyphs,
                color: Color::from_rgba8(10, 20, 30, 255),
            }]
        );
    }

    #[test]
    fn every_run_becomes_an_item_and_empty_ones_do_not() {
        let empty = Arc::new(GlyphRun {
            glyphs: Vec::new(),
            ..(*run(embedded(SANS), 16.0)).clone()
        });
        let (frag, bb) = fragment(
            vec![run(embedded(SANS), 16.0), empty, run(embedded(SANS), 12.0)],
            |_| {},
        );
        assert_eq!(kinds(&emitted(&frag, bb)), ["text", "text"]);
    }

    #[test]
    fn other_fragment_kinds_and_runless_text_emit_nothing() {
        let (mut frag, bb) = fragment(vec![run(embedded(SANS), 16.0)], |_| {});
        frag.kind = FragmentKind::Box;
        assert!(emitted(&frag, bb).is_empty());
        let (frag, bb) = fragment(Vec::new(), underlined(|_| {}));
        assert!(emitted(&frag, bb).is_empty());
    }

    #[test]
    fn a_transparent_colour_draws_no_glyphs_but_keeps_the_lines() {
        let (frag, bb) = fragment(vec![run(embedded(SANS), 16.0)], |s| {
            s.color = Color::TRANSPARENT;
            underlined(|_| {})(s);
        });
        assert_eq!(kinds(&emitted(&frag, bb)), ["line"]);
    }

    #[test]
    fn underlines_match_the_rows_chrome_draws_for_liberation_sans() {
        // Measured in Chrome: (size, rows below the baseline to the top of the
        // line, rows of thickness).
        for (size, below, rows) in [
            (12.0, 1.0, 1.0),
            (16.0, 1.0, 1.0),
            (20.0, 1.0, 2.0),
            (32.0, 2.0, 3.0),
            (48.0, 3.0, 4.0),
        ] {
            assert_eq!(
                sole_line(size, 100.0, underlined(|_| {})),
                (100.0 + below, rows),
                "underline at {size}px"
            );
        }
    }

    #[test]
    fn the_overline_sits_a_thickness_above_the_ascent() {
        // Chrome at 16 px: baseline row 55, ascent 14, overline on row 40.
        let (top, rows) = sole_line(16.0, 55.0, |s| s.text_decoration.overline = true);
        assert_eq!((top, rows), (40.0, 1.0));
    }

    #[test]
    fn the_line_through_is_centred_a_third_of_the_ascent_above_the_baseline() {
        // Chrome rows: 12 px (baseline 54) row 50, 16 px (baseline 55) row 50,
        // 32 px (baseline 61) rows 50 to 52.
        for (size, baseline, top, rows) in [
            (12.0, 54.0, 50.0, 1.0),
            (16.0, 55.0, 50.0, 1.0),
            (32.0, 61.0, 50.0, 3.0),
        ] {
            assert_eq!(
                sole_line(size, baseline, |s| s.text_decoration.line_through = true),
                (top, rows),
                "line-through at {size}px"
            );
        }
    }

    #[test]
    fn lines_are_drawn_under_the_glyphs_except_the_strike() {
        let (frag, bb) = fragment(vec![run(embedded(SANS), 16.0)], |s| {
            s.text_decoration.underline = true;
            s.text_decoration.overline = true;
            s.text_decoration.line_through = true;
        });
        assert_eq!(kinds(&emitted(&frag, bb)), ["line", "line", "text", "line"]);
    }

    #[test]
    fn an_explicit_thickness_wins_and_is_at_least_one_pixel() {
        assert_eq!(
            sole_line(16.0, 100.0, underlined(|d| d.thickness = Some(3.0))),
            (102.0, 3.0)
        );
        assert_eq!(
            sole_line(16.0, 100.0, underlined(|d| d.thickness = Some(0.25))),
            (101.0, 1.0)
        );
    }

    #[test]
    fn an_explicit_thickness_grows_the_strike_around_its_centre() {
        let thin = sole_line(32.0, 100.0, |s| s.text_decoration.line_through = true);
        let thick = sole_line(32.0, 100.0, |s| {
            s.text_decoration.line_through = true;
            s.text_decoration.thickness = Some(5.0);
        });
        assert!((thin.0 + thin.1 / 2.0 - (thick.0 + thick.1 / 2.0)).abs() <= 0.5);
        assert_eq!(thick.1, 5.0);
    }

    #[test]
    fn underline_offset_and_position_move_the_line() {
        // 16 px: auto is one row below the baseline; 20% of the size is
        // 3.2 px more, and the row is rounded.
        let offset = LengthPercent {
            px: 2.0,
            fraction: 0.0,
        };
        assert_eq!(
            sole_line(
                16.0,
                100.0,
                underlined(|d| d.underline_offset = Some(offset))
            ),
            (103.0, 1.0)
        );
        let percent = LengthPercent::fraction(0.25);
        assert_eq!(
            sole_line(
                16.0,
                100.0,
                underlined(|d| d.underline_offset = Some(percent))
            ),
            (105.0, 1.0)
        );
        // `under` puts the line below the descent (3.4 px, rounded to 3).
        assert_eq!(
            sole_line(
                16.0,
                100.0,
                underlined(|d| d.underline_position = UnderlinePosition::Under)
            ),
            (103.0, 1.0)
        );
        // `from-font` follows `post`: 67 of 2048 units, 0.5 px, rounds to 1.
        assert_eq!(
            sole_line(
                16.0,
                100.0,
                underlined(|d| d.underline_position = UnderlinePosition::FromFont)
            ),
            (101.0, 1.0)
        );
    }

    #[test]
    fn double_and_wavy_lines_span_three_thicknesses() {
        for style in [LineStyle::Double, LineStyle::Wavy] {
            let (top, rows) = sole_line(20.0, 100.0, underlined(|d| d.style = style));
            assert_eq!((top, rows), (101.0, 6.0));
        }
    }

    #[test]
    fn the_first_run_decides_the_metrics_when_it_is_a_fallback_face() {
        // DejaVu's ascent at 16 px is 14.85, which rounds to 15 where Liberation
        // Sans rounds to 14, so the overline is a row higher.
        let (frag, bb) = fragment_at(
            vec![run(embedded(DEJAVU), 16.0), run(embedded(SANS), 16.0)],
            15.0,
            |s| s.text_decoration.overline = true,
        );
        let [(rect, _)] = decorations(&emitted(&frag, bb))[..] else {
            panic!("one line expected");
        };
        assert_eq!(rect.y, 55.0 - 15.0 - 1.0);
        let (frag, bb) = fragment_at(
            vec![run(embedded(SANS), 16.0), run(embedded(DEJAVU), 16.0)],
            15.0,
            |s| s.text_decoration.overline = true,
        );
        let [(rect, _)] = decorations(&emitted(&frag, bb))[..] else {
            panic!("one line expected");
        };
        assert_eq!(rect.y, 55.0 - 14.0 - 1.0);
    }

    fn shadow(dx: f32, dy: f32, blur: f32, r: u8) -> ShadowPaint {
        ShadowPaint {
            offset: (dx, dy),
            blur,
            spread: 0.0,
            color: Color::from_rgba8(r, 0, 0, 255),
            inset: false,
        }
    }

    #[test]
    fn text_shadows_paint_lowest_first_and_displace_text_and_lines() {
        let glyphs = run(embedded(SANS), 16.0);
        let (frag, bb) = fragment(vec![glyphs], |s| {
            underlined(|_| {})(s);
            s.text_shadow = vec![shadow(1.0, 2.0, 0.0, 1), shadow(3.0, 4.0, 0.0, 2)];
        });
        let items = emitted(&frag, bb);
        assert_eq!(
            kinds(&items),
            ["line", "text", "line", "text", "line", "text"]
        );
        let origins: Vec<_> = items
            .iter()
            .filter_map(|item| match item {
                Item::Text { origin, .. } => Some(*origin),
                _ => None,
            })
            .collect();
        assert_eq!(origins, [(33.0, 59.0), (31.0, 57.0), (30.0, 55.0)]);
        let lines = decorations(&items);
        assert_eq!((lines[0].0.x, lines[0].0.y), (33.0, 60.0));
        assert_eq!(lines[2].0.y, 56.0);
        // The shadow's lines take the shadow's colour, not the decoration's.
        let Item::Decoration { color, .. } = items[0] else {
            panic!("expected a line");
        };
        assert_eq!(color, Color::from_rgba8(2, 0, 0, 255));
    }

    #[test]
    fn blurred_shadows_are_skipped_and_counted() {
        let (frag, bb) = fragment(vec![run(embedded(SANS), 16.0)], |s| {
            s.text_shadow = vec![shadow(1.0, 1.0, 3.0, 1), shadow(2.0, 2.0, 0.0, 2)];
        });
        assert_eq!(kinds(&emitted(&frag, bb)), ["text", "text"]);
        let mut counts = BTreeMap::new();
        count_unsupported(&frag, &mut counts);
        assert_eq!(counts, BTreeMap::from([("TextShadowBlur", 1)]));
    }

    #[test]
    fn skip_ink_is_counted_once_per_underlined_fragment_that_asks_for_it() {
        fn count(edit: impl FnOnce(&mut PaintStyle)) -> BTreeMap<&'static str, usize> {
            let (frag, _) = fragment(vec![run(embedded(SANS), 16.0)], edit);
            let mut counts = BTreeMap::new();
            count_unsupported(&frag, &mut counts);
            counts
        }
        assert_eq!(
            count(underlined(|_| {})),
            BTreeMap::from([("TextDecorationSkipInk", 1)])
        );
        assert!(count(underlined(|d| d.skip_ink = false)).is_empty());
        assert!(count(|s| s.text_decoration.overline = true).is_empty());
        assert!(count(|s| s.visibility_hidden = true).is_empty());
    }

    #[test]
    fn a_face_that_does_not_parse_falls_back_to_proportions_of_the_size() {
        let broken = font(FontBlob::Owned(vec![0; 8].into_boxed_slice()));
        let (frag, bb) = fragment(vec![run(broken, 20.0)], |s| {
            s.text_decoration.overline = true;
        });
        let [(rect, _)] = decorations(&emitted(&frag, bb))[..] else {
            panic!("one line expected");
        };
        // Ascent 0.8 em = 16, thickness floor(2.0) = 2.
        assert_eq!((rect.y, rect.h), (55.0 - 16.0 - 2.0, 2.0));
    }

    #[test]
    fn a_zero_width_fragment_gets_no_lines() {
        let (frag, mut bb) = fragment(vec![run(embedded(SANS), 16.0)], underlined(|_| {}));
        bb.w = 0.0;
        assert_eq!(kinds(&emitted(&frag, bb)), ["text"]);
    }
}
