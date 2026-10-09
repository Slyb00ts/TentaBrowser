//! Font measurements read from the bundled files' tables.

use skrifa::instance::{LocationRef, Size};
use skrifa::metrics::{GlyphMetrics, Metrics};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tenta_css::cascade::{FontMetrics, FontMetricsData};

use super::{ResolvedFont, VerticalMetrics};

/// The water ideograph, whose advance is the `ic` unit.
const WATER_IDEOGRAPH: char = '\u{6C34}';

/// A valid pixel size, or `None` for the zero, negative and non-finite sizes a
/// font cannot be scaled to.
fn scaled(size_px: f32) -> Option<Size> {
    (size_px.is_finite() && size_px > 0.0).then(|| Size::new(size_px))
}

fn advance(font: &FontRef<'_>, size: Size, ch: char) -> Option<f32> {
    let glyph = font.charmap().map(ch)?;
    GlyphMetrics::new(font, size, LocationRef::default()).advance_width(glyph)
}

/// The top of a glyph's outline, used when OS/2 does not carry the height.
fn glyph_top(font: &FontRef<'_>, size: Size, ch: char) -> Option<f32> {
    let glyph: GlyphId = font.charmap().map(ch)?;
    let bounds = GlyphMetrics::new(font, size, LocationRef::default()).bounds(glyph)?;
    Some(bounds.y_max)
}

fn positive(value: Option<f32>) -> Option<f32> {
    value.filter(|v| *v > 0.0)
}

impl ResolvedFont {
    fn font_ref(&self) -> Option<FontRef<'static>> {
        FontRef::new(self.bytes()).ok()
    }

    /// Ascent, descent and line gap for `line-height: normal` at `size_px`.
    pub fn vertical_metrics(&self, size_px: f32) -> VerticalMetrics {
        let (Some(font), Some(size)) = (self.font_ref(), scaled(size_px)) else {
            return VerticalMetrics::default();
        };
        let metrics = Metrics::new(&font, size, LocationRef::default());
        VerticalMetrics {
            ascent: metrics.ascent.round(),
            descent: (-metrics.descent).round(),
            line_gap: metrics.leading,
        }
    }

    /// The measurements Stylo needs for `ex`, `ch`, `cap`, `ic` and
    /// `font-size-adjust`, in CSS pixels.
    pub fn metrics_data(&self, size_px: f32) -> FontMetricsData {
        let (Some(font), Some(size)) = (self.font_ref(), scaled(size_px)) else {
            return FontMetricsData::default();
        };
        let metrics = Metrics::new(&font, size, LocationRef::default());
        FontMetricsData {
            x_height: positive(metrics.x_height).or_else(|| positive(glyph_top(&font, size, 'x'))),
            zero_advance: advance(&font, size, '0'),
            cap_height: positive(metrics.cap_height)
                .or_else(|| positive(glyph_top(&font, size, 'H'))),
            ic_width: advance(&font, size, WATER_IDEOGRAPH),
            ascent: metrics.ascent,
            // The bundled fonts have no MATH table, so Stylo's default scale
            // factors apply.
            script_percent_scale_down: None,
            script_script_percent_scale_down: None,
        }
    }
}

/// Answers Stylo's font queries from the bundled files. It holds no state, so
/// it is shared freely across threads.
#[derive(Debug)]
pub(super) struct EmbeddedMetrics;

impl FontMetrics for EmbeddedMetrics {
    fn metrics(
        &self,
        family_list: &str,
        weight: u16,
        italic: bool,
        size_px: f32,
    ) -> FontMetricsData {
        ResolvedFont::resolve(family_list, weight, italic).metrics_data(size_px)
    }
}
