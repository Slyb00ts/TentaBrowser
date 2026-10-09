//! The display list: a flat, ordered list of draw items.
//!
//! The builder produces it from a fragment tree; the rasteriser consumes it.
//! Clips, opacity groups and transforms are balanced `Push`/`Pop` pairs.

use std::collections::BTreeMap;
use std::sync::Arc;

use tenta_layout::{BorderStyle, Color, Edges, GlyphRun, LineStyle, Rect};

use crate::image::{ImageId, ImageQuality};

/// Corner radii as `(x, y)` pairs in top-left, top-right, bottom-right,
/// bottom-left order, in CSS pixels.
pub type Radii = [f32; 8];

/// One draw or state-change command.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// Fills a rectangle.
    FillRect {
        /// Area.
        rect: Rect,
        /// Colour.
        color: Color,
    },
    /// Fills a rounded rectangle.
    FillRRect {
        /// Area.
        rect: Rect,
        /// Corner radii.
        radii: Radii,
        /// Colour.
        color: Color,
    },
    /// Draws a border.
    Border {
        /// Border box.
        rect: Rect,
        /// Widths per side.
        widths: Edges,
        /// Colours in top, right, bottom, left order.
        colors: [Color; 4],
        /// Styles in top, right, bottom, left order.
        styles: [BorderStyle; 4],
        /// Corner radii.
        radii: Radii,
    },
    /// Draws a box shadow.
    BoxShadow {
        /// Box the shadow belongs to.
        rect: Rect,
        /// Corner radii of the box.
        radii: Radii,
        /// Shadow offset.
        offset: (f32, f32),
        /// Blur radius.
        blur: f32,
        /// Spread radius.
        spread: f32,
        /// Colour.
        color: Color,
        /// `inset` shadow.
        inset: bool,
    },
    /// Fills a rectangle with a linear gradient.
    LinearGradient {
        /// Area.
        rect: Rect,
        /// Corner radii.
        radii: Radii,
        /// Direction in degrees, `0` pointing up, clockwise.
        angle: f32,
        /// `(offset in 0..=1, colour)` stops.
        stops: Vec<(f32, Color)>,
        /// Repeating gradient.
        repeat: bool,
    },
    /// Draws an image.
    Image {
        /// Destination.
        dest: Rect,
        /// Image in the store.
        src: ImageId,
        /// Source sub-rectangle in image pixels; `None` is the whole image.
        src_rect: Option<Rect>,
        /// Resampling quality.
        quality: ImageQuality,
    },
    /// Fills `area` with an image repeated from `origin` in tiles of `tile`
    /// size, in both directions: many more tiles than one item each could
    /// carry. A background that repeats along one axis only is cut to one tile
    /// along the other.
    ImagePattern {
        /// Area filled.
        area: Rect,
        /// Top-left of a tile that the repetition is anchored to.
        origin: (f32, f32),
        /// Size of one tile.
        tile: (f32, f32),
        /// Image in the store.
        src: ImageId,
        /// Resampling quality.
        quality: ImageQuality,
    },
    /// Draws a glyph run.
    Text {
        /// Origin of the run's glyph coordinates.
        origin: (f32, f32),
        /// Glyphs.
        run: Arc<GlyphRun>,
        /// Colour.
        color: Color,
    },
    /// Draws a text-decoration line.
    Decoration {
        /// Line box.
        rect: Rect,
        /// Colour.
        color: Color,
        /// Line style.
        style: LineStyle,
    },
    /// Starts clipping to a rounded rectangle.
    PushClip {
        /// Clip area.
        rect: Rect,
        /// Corner radii.
        radii: Radii,
    },
    /// Ends the innermost clip.
    PopClip,
    /// Starts a group composited at this opacity.
    PushOpacity(f32),
    /// Ends the innermost opacity group.
    PopOpacity,
    /// Starts applying a 2D transform `[a, b, c, d, e, f]`.
    PushTransform([f32; 6]),
    /// Ends the innermost transform.
    PopTransform,
}

/// An ordered list of items and the area they cover.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayList {
    /// Items in paint order.
    pub items: Vec<Item>,
    /// Union of everything the items can touch.
    pub bounds: Rect,
    /// Style features the builder could not express as items, by name and
    /// number of fragments; the rasteriser reports them with what it skips.
    pub unsupported: BTreeMap<&'static str, usize>,
    /// Style features the builder draws in a simpler form than specified, by
    /// name and number of fragments; the rasteriser reports them as
    /// approximated.
    pub approximated: BTreeMap<&'static str, usize>,
}

impl DisplayList {
    /// True when every `Push*` has its `Pop*` and none is popped early.
    pub fn is_balanced(&self) -> bool {
        let (mut clips, mut groups, mut transforms) = (0_i32, 0_i32, 0_i32);
        for item in &self.items {
            let counter = match item {
                Item::PushClip { .. } => (&mut clips, 1),
                Item::PopClip => (&mut clips, -1),
                Item::PushOpacity(_) => (&mut groups, 1),
                Item::PopOpacity => (&mut groups, -1),
                Item::PushTransform(_) => (&mut transforms, 1),
                Item::PopTransform => (&mut transforms, -1),
                _ => continue,
            };
            *counter.0 += counter.1;
            if *counter.0 < 0 {
                return false;
            }
        }
        clips == 0 && groups == 0 && transforms == 0
    }

    /// The distinct images the list draws, for pinning them in the store while
    /// the list is alive.
    pub fn image_ids(&self) -> Vec<ImageId> {
        let mut ids: Vec<ImageId> = self
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Image { src, .. } | Item::ImagePattern { src, .. } => Some(*src),
                _ => None,
            })
            .collect();
        ids.sort_by_key(|id| id.0);
        ids.dedup();
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: Vec<Item>) -> DisplayList {
        DisplayList {
            items,
            bounds: Rect::default(),
            ..DisplayList::default()
        }
    }

    #[test]
    fn balanced_lists_pair_every_push() {
        let clip = Item::PushClip {
            rect: Rect::new(0.0, 0.0, 1.0, 1.0),
            radii: [0.0; 8],
        };
        assert!(list(vec![]).is_balanced());
        assert!(
            list(vec![
                clip.clone(),
                Item::PushOpacity(0.5),
                Item::PopOpacity,
                Item::PopClip
            ])
            .is_balanced()
        );
        assert!(!list(vec![clip.clone()]).is_balanced());
        assert!(!list(vec![Item::PopTransform, Item::PushTransform([0.0; 6])]).is_balanced());
    }
}
