//! Glyph outlines from skrifa, built into tiny-skia paths.
//!
//! Outlines are unhinted: hinting would snap stems to the pixel grid and
//! defeat the quarter-pixel subpixel positions, and unhinted output does not
//! depend on a hinting interpreter version, which keeps the bytes stable
//! across platforms.

use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::types::F2Dot14;
use skrifa::{GlyphId, MetadataProvider};
use tenta_layout::FontRef;
use tiny_skia::{Path, PathBuilder};

/// A glyph the outline source cannot provide: a colour glyph, an id the font
/// lacks or an outline that fails to decode.
#[derive(Debug)]
pub(super) struct NoOutline;

/// One font face parsed for outline extraction.
pub(super) struct Outliner<'a> {
    font: skrifa::FontRef<'a>,
    coords: Vec<NormalizedCoord>,
}

impl<'a> Outliner<'a> {
    pub(super) fn new(font: &'a FontRef) -> Option<Self> {
        let parsed = skrifa::FontRef::from_index(font.data.bytes(), font.index).ok()?;
        let coords = font.coords.iter().map(|c| F2Dot14::from_bits(*c)).collect();
        Some(Self {
            font: parsed,
            coords,
        })
    }

    /// The outline of `glyph` at `size` pixels per em, y pointing down with
    /// the origin on the baseline, sheared by `skew` (the tangent of the slant,
    /// positive leaning right). `Ok(None)` is a glyph that draws nothing,
    /// such as a space.
    pub(super) fn path(&self, glyph: u32, size: f32, skew: f32) -> Result<Option<Path>, NoOutline> {
        let id = GlyphId::new(glyph);
        if self.font.color_glyphs().get(id).is_some() {
            return Err(NoOutline);
        }
        let outline = self.font.outline_glyphs().get(id).ok_or(NoOutline)?;
        let mut pen = PathPen {
            builder: PathBuilder::new(),
            skew,
        };
        let settings = DrawSettings::unhinted(Size::new(size), LocationRef::new(&self.coords));
        outline.draw(settings, &mut pen).map_err(|_| NoOutline)?;
        Ok(pen.builder.finish())
    }
}

struct PathPen {
    builder: PathBuilder,
    skew: f32,
}

impl PathPen {
    /// Font units are y-up; the canvas is y-down, and a slant moves points
    /// right in proportion to their height above the baseline.
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        (x + self.skew * y, -y)
    }
}

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.map(x, y);
        self.builder.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.map(x, y);
        self.builder.line_to(x, y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (cx0, cy0) = self.map(cx0, cy0);
        let (x, y) = self.map(x, y);
        self.builder.quad_to(cx0, cy0, x, y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (cx0, cy0) = self.map(cx0, cy0);
        let (cx1, cy1) = self.map(cx1, cy1);
        let (x, y) = self.map(x, y);
        self.builder.cubic_to(cx0, cy0, cx1, cy1, x, y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
}
