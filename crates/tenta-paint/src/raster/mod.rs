//! CPU rasterisation of display lists with tiny-skia.

mod border;
mod clip;
mod dash;
mod geometry;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::rc::Rc;

use tenta_layout::{Color, Rect};
use tiny_skia::{FillRule, Paint, Path, PathBuilder, PixmapPaint, Transform};

use crate::PaintError;
use crate::dl::{DisplayList, Item, Radii};
use crate::glyph;
use crate::image::ImageStore;
use clip::{ClipState, Scratch};
use geometry::{
    Axis, IRect, clamp_radii, device_bounds, is_rounded, push_rounded_rect, scale_radii,
};

/// A premultiplied RGBA8 raster.
#[derive(Clone, Debug)]
pub struct Pixmap(tiny_skia::Pixmap);

impl Pixmap {
    /// A transparent pixmap; `None` when a dimension is zero or the size
    /// overflows.
    pub fn new(width: u32, height: u32) -> Option<Pixmap> {
        tiny_skia::Pixmap::new(width, height).map(Pixmap)
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.0.width()
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.0.height()
    }

    /// Premultiplied RGBA8 pixels, row-major.
    pub fn data(&self) -> &[u8] {
        self.0.data()
    }
}

/// How and where to rasterise.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RasterOptions {
    /// Output width in device pixels.
    pub width: u32,
    /// Output height in device pixels.
    pub height: u32,
    /// Device pixels per CSS pixel.
    pub scale: f32,
    /// Top-left of the output in CSS pixels.
    pub origin: (f32, f32),
    /// Background; `None` leaves the pixmap transparent.
    pub background: Option<Color>,
}

/// What a rasterisation left undrawn. Rasterising never fails on an item it
/// cannot draw yet; callers and the corpus harness read this instead.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RasterStats {
    /// Skipped items by kind, in name order. Glyphs the glyph renderer could
    /// not draw are counted under `"Glyph"`.
    pub skipped: BTreeMap<&'static str, usize>,
}

impl RasterStats {
    /// True when every item was drawn.
    pub fn is_complete(&self) -> bool {
        self.skipped.is_empty()
    }

    /// Items skipped over all kinds.
    pub fn skipped_total(&self) -> usize {
        self.skipped.values().sum()
    }

    fn skip(&mut self, kind: &'static str, count: usize) {
        if count > 0 {
            *self.skipped.entry(kind).or_default() += count;
        }
    }
}

/// The result of [`rasterize`].
#[derive(Clone, Debug)]
pub struct RasterOutput {
    /// The drawn pixels.
    pub pixmap: Pixmap,
    /// What was not drawn.
    pub stats: RasterStats,
}

/// Rasterises `list` into a new pixmap.
///
/// Items the rasteriser cannot draw yet are skipped and reported in the
/// returned stats, identically in every build profile. Images are not drawn
/// yet, so the store is accepted only to keep the signature stable.
pub fn rasterize(
    list: &DisplayList,
    opts: &RasterOptions,
    _images: &ImageStore,
) -> Result<RasterOutput, PaintError> {
    let mut painter = Painter::new(opts)?;
    for (&feature, &count) in &list.unsupported {
        painter.stats.skip(feature, count);
    }
    let mut index = 0;
    while index < list.items.len() {
        index = painter.draw(&list.items, index);
    }
    Ok(painter.finish())
}

/// State saved by a `Push` and restored by its `Pop`. Pushes nest; a `Pop`
/// that does not match the innermost push is ignored, and pushes left open at
/// the end of the list are closed.
enum Frame {
    Clip(Rc<ClipState>),
    Transform(Transform),
    /// An opacity group; `None` when it needs no layer (fully opaque or
    /// invisible).
    Opacity(Option<Layer>),
}

/// An opacity group's pixels. The group is drawn into a pixmap that covers
/// only its content, and composited onto `parent` when it is popped.
struct Layer {
    parent: tiny_skia::Pixmap,
    transform: Transform,
    clip: Rc<ClipState>,
    /// The layer's top-left in the parent's pixels.
    origin: (i32, i32),
    opacity: f32,
}

/// What an item contributes to the bounds of an opacity group.
enum Footprint {
    Nothing,
    Area(IRect),
    /// Not known without drawing, so the group falls back to its clip.
    Unknown,
}

/// Rasteriser state.
struct Painter {
    target: tiny_skia::Pixmap,
    /// Maps CSS pixels to pixels of `target`.
    transform: Transform,
    /// `transform` when it only scales and translates.
    axis: Option<Axis>,
    clip: Rc<ClipState>,
    frames: Vec<Frame>,
    /// Scratch masks for shaping a single draw; see `border`.
    rings: [Scratch; 3],
    side: Scratch,
    /// Recycled builder, so that each path does not allocate its own buffers.
    path: PathBuilder,
    stats: RasterStats,
}

impl Painter {
    fn new(opts: &RasterOptions) -> Result<Self, PaintError> {
        let mut target =
            tiny_skia::Pixmap::new(opts.width, opts.height).ok_or(PaintError::InvalidSize {
                width: opts.width,
                height: opts.height,
            })?;
        if let Some(color) = opts.background.and_then(solid_color) {
            target.fill(color);
        }
        let transform = Transform::from_scale(opts.scale, opts.scale)
            .post_translate(-opts.origin.0 * opts.scale, -opts.origin.1 * opts.scale);
        let canvas = IRect::from_size(opts.width, opts.height);
        Ok(Self {
            target,
            transform,
            axis: Axis::of(transform),
            clip: ClipState::open(canvas),
            frames: Vec::new(),
            rings: std::array::from_fn(|_| Scratch::new()),
            side: Scratch::new(),
            path: PathBuilder::new(),
            stats: RasterStats::default(),
        })
    }

    /// Draws `items[index]` and returns the index of the next item to draw.
    fn draw(&mut self, items: &[Item], index: usize) -> usize {
        match &items[index] {
            Item::FillRect { rect, color } => self.fill_rect(*rect, *color),
            Item::FillRRect { rect, radii, color } => self.fill_rrect(*rect, *radii, *color),
            Item::Border {
                rect,
                widths,
                colors,
                styles,
                radii,
            } => self.draw_border(*rect, *widths, *colors, *styles, *radii),
            Item::BoxShadow { .. } => self.stats.skip("BoxShadow", 1),
            Item::LinearGradient { .. } => self.stats.skip("LinearGradient", 1),
            Item::Image { .. } => self.stats.skip("Image", 1),
            Item::Text { origin, run, color } => {
                let clip = Rc::clone(&self.clip);
                if clip.bounds().is_empty() {
                    return index + 1;
                }
                let canvas = IRect::from_size(self.target.width(), self.target.height());
                let skipped = glyph::draw_text(
                    &mut self.target,
                    self.transform,
                    *origin,
                    run,
                    *color,
                    clip.mask_for(canvas),
                );
                self.stats.skip("Glyph", skipped);
            }
            Item::Decoration { rect, color, style } => {
                let clip = Rc::clone(&self.clip);
                if clip.bounds().is_empty() {
                    return index + 1;
                }
                let canvas = IRect::from_size(self.target.width(), self.target.height());
                let skipped = glyph::draw_decoration(
                    &mut self.target,
                    self.transform,
                    *rect,
                    *color,
                    *style,
                    clip.mask_for(canvas),
                );
                self.stats.skip("Decoration", skipped);
            }
            Item::PushClip { rect, radii } => self.push_clip(*rect, *radii),
            Item::PopClip => {
                if matches!(self.frames.last(), Some(Frame::Clip(_))) {
                    self.pop_frame();
                }
            }
            Item::PushOpacity(opacity) => return self.push_opacity(items, index, *opacity),
            Item::PopOpacity => {
                if matches!(self.frames.last(), Some(Frame::Opacity(_))) {
                    self.pop_frame();
                }
            }
            Item::PushTransform(matrix) => self.push_transform(*matrix),
            Item::PopTransform => {
                if matches!(self.frames.last(), Some(Frame::Transform(_))) {
                    self.pop_frame();
                }
            }
        }
        index + 1
    }

    fn finish(mut self) -> RasterOutput {
        while !self.frames.is_empty() {
            self.pop_frame();
        }
        RasterOutput {
            pixmap: Pixmap(self.target),
            stats: self.stats,
        }
    }

    fn set_transform(&mut self, transform: Transform) {
        self.transform = transform;
        self.axis = Axis::of(transform);
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        let Some(axis) = self.axis else {
            let area = device_bounds(rect, self.transform, 1.0);
            self.fill_rect_exact(rect, color, area);
            return;
        };
        let snapped = axis.rect(rect).intersect(self.clip.bounds());
        if snapped.is_empty() {
            return;
        }
        let (Some(paint), Some(skia)) = (solid_paint(color, false), snapped.to_skia()) else {
            return;
        };
        self.target
            .fill_rect(skia, &paint, Transform::identity(), self.clip.shape());
    }

    /// Fills a rectangle under a transform that is not axis-aligned.
    fn fill_rect_exact(&mut self, rect: Rect, color: Color, area: IRect) {
        let (Some(paint), Some(skia)) = (
            solid_paint(color, true),
            tiny_skia::Rect::from_xywh(rect.x, rect.y, rect.w, rect.h),
        ) else {
            return;
        };
        if self.clip.bounds().intersect(area).is_empty() {
            return;
        }
        self.target
            .fill_rect(skia, &paint, self.transform, self.clip.mask_for(area));
    }

    fn fill_rrect(&mut self, rect: Rect, radii: Radii, color: Color) {
        let Some(paint) = solid_paint(color, true) else {
            return;
        };
        let (rect, radii, transform, area) = match self.axis {
            Some(axis) => {
                let snapped = axis.rect(rect);
                let radii = scale_radii(radii, axis.scale_x(), axis.scale_y());
                (
                    snapped.to_rect(),
                    radii,
                    Transform::identity(),
                    snapped.intersect(self.clip.bounds()),
                )
            }
            None => (
                rect,
                radii,
                self.transform,
                device_bounds(rect, self.transform, 1.0),
            ),
        };
        if rect.w <= 0.0 || rect.h <= 0.0 || self.clip.bounds().intersect(area).is_empty() {
            return;
        }
        let radii = clamp_radii(rect, radii);
        let Some(path) = self.build_path(|pb| push_rounded_rect(pb, rect, radii)) else {
            return;
        };
        self.target.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            transform,
            self.clip.mask_for(area),
        );
        self.recycle(path);
    }

    fn push_clip(&mut self, rect: Rect, radii: Radii) {
        let next = self.clip_for(rect, radii);
        let previous = std::mem::replace(&mut self.clip, next);
        self.frames.push(Frame::Clip(previous));
    }

    /// The active clip narrowed to a rounded rectangle. A pixel-aligned
    /// rectangle needs no mask; rounded corners and rotations do.
    fn clip_for(&mut self, rect: Rect, radii: Radii) -> Rc<ClipState> {
        let (rect, radii, transform, area) = match self.axis {
            Some(axis) => {
                let snapped = axis.rect(rect);
                let radii = scale_radii(radii, axis.scale_x(), axis.scale_y());
                (
                    snapped.to_rect(),
                    clamp_radii(snapped.to_rect(), radii),
                    Transform::identity(),
                    snapped,
                )
            }
            None => (
                rect,
                clamp_radii(rect, radii),
                self.transform,
                device_bounds(rect, self.transform, 1.0),
            ),
        };
        if area.is_empty() {
            return self.clip.restrict_to_rect(IRect::EMPTY);
        }
        if self.axis.is_some() && !is_rounded(&radii) {
            return self.clip.restrict_to_rect(area);
        }
        let Some(path) = self.build_path(|pb| push_rounded_rect(pb, rect, radii)) else {
            return self.clip.restrict_to_rect(IRect::EMPTY);
        };
        let next = self.clip.restrict_to_path(&path, transform, area);
        self.recycle(path);
        next
    }

    fn push_transform(&mut self, [a, b, c, d, e, f]: [f32; 6]) {
        self.frames.push(Frame::Transform(self.transform));
        self.set_transform(
            self.transform
                .pre_concat(Transform::from_row(a, b, c, d, e, f)),
        );
    }

    /// Starts an opacity group and returns the next item to draw. A group that
    /// draws nothing visible is skipped up to its `PopOpacity`.
    fn push_opacity(&mut self, items: &[Item], index: usize, opacity: f32) -> usize {
        let opacity = if opacity.is_nan() {
            1.0
        } else {
            opacity.clamp(0.0, 1.0)
        };
        if opacity >= 1.0 {
            self.frames.push(Frame::Opacity(None));
            return index + 1;
        }
        let (end, footprint) = group_extent(items, index, self.transform);
        let region = match footprint {
            Footprint::Nothing => IRect::EMPTY,
            Footprint::Area(area) => area.intersect(self.clip.bounds()),
            Footprint::Unknown => self.clip.bounds(),
        };
        let layer = (opacity > 0.0 && !region.is_empty())
            .then(|| tiny_skia::Pixmap::new(region.width() as u32, region.height() as u32))
            .flatten();
        let Some(pixmap) = layer else {
            self.frames.push(Frame::Opacity(None));
            return end;
        };
        let layer = Layer {
            parent: std::mem::replace(&mut self.target, pixmap),
            transform: self.transform,
            clip: Rc::clone(&self.clip),
            origin: (region.x0, region.y0),
            opacity,
        };
        self.clip = self.clip.cropped(region);
        self.set_transform(
            self.transform
                .post_translate(-(region.x0 as f32), -(region.y0 as f32)),
        );
        self.frames.push(Frame::Opacity(Some(layer)));
        index + 1
    }

    /// Undoes the innermost `Push`.
    fn pop_frame(&mut self) {
        match self.frames.pop() {
            Some(Frame::Clip(previous)) => self.clip = previous,
            Some(Frame::Transform(previous)) => self.set_transform(previous),
            Some(Frame::Opacity(Some(layer))) => {
                let content = std::mem::replace(&mut self.target, layer.parent);
                self.set_transform(layer.transform);
                self.clip = layer.clip;
                // The content was clipped while it was drawn, so the composite
                // takes no mask: applying the clip twice would square its edges.
                let paint = PixmapPaint {
                    opacity: layer.opacity,
                    ..PixmapPaint::default()
                };
                self.target.draw_pixmap(
                    layer.origin.0,
                    layer.origin.1,
                    content.as_ref(),
                    &paint,
                    Transform::identity(),
                    None,
                );
            }
            Some(Frame::Opacity(None)) | None => {}
        }
    }

    fn build_path(&mut self, build: impl FnOnce(&mut PathBuilder)) -> Option<Path> {
        let mut builder = std::mem::take(&mut self.path);
        build(&mut builder);
        builder.finish()
    }

    fn recycle(&mut self, path: Path) {
        self.path = path.clear();
    }
}

/// Finds the `PopOpacity` that closes the group opened at `start` (the item
/// count when there is none) and the area its items can touch, in the pixels
/// of the surface that `transform` maps to.
fn group_extent(items: &[Item], start: usize, transform: Transform) -> (usize, Footprint) {
    let mut transforms = vec![transform];
    let mut depth = 0_usize;
    let mut total = Footprint::Nothing;
    for (offset, item) in items[start + 1..].iter().enumerate() {
        let current = *transforms.last().expect("the stack starts with one entry");
        match item {
            Item::PushOpacity(_) => depth += 1,
            Item::PopOpacity if depth == 0 => return (start + 1 + offset, total),
            Item::PopOpacity => depth -= 1,
            Item::PushTransform([a, b, c, d, e, f]) => {
                transforms.push(current.pre_concat(Transform::from_row(*a, *b, *c, *d, *e, *f)));
            }
            Item::PopTransform if transforms.len() > 1 => {
                transforms.pop();
            }
            _ => {
                total = match (total, footprint(item, current)) {
                    (Footprint::Unknown, _) | (_, Footprint::Unknown) => Footprint::Unknown,
                    (Footprint::Nothing, other) | (other, Footprint::Nothing) => other,
                    (Footprint::Area(a), Footprint::Area(b)) => Footprint::Area(a.union(b)),
                };
            }
        }
    }
    (items.len(), total)
}

/// The area one item can touch under `transform`. Anti-aliasing and dash
/// ends reach one pixel past a box, and a glyph can reach well past its
/// origin, so text is bounded by a generous multiple of its size.
fn footprint(item: &Item, transform: Transform) -> Footprint {
    match item {
        Item::FillRect { rect, .. } | Item::FillRRect { rect, .. } | Item::Border { rect, .. } => {
            Footprint::Area(device_bounds(*rect, transform, 1.0))
        }
        Item::Decoration { rect, .. } => Footprint::Area(device_bounds(*rect, transform, 2.0)),
        Item::Text { origin, run, .. } => {
            let mut xs = (f32::INFINITY, f32::NEG_INFINITY);
            let mut ys = (f32::INFINITY, f32::NEG_INFINITY);
            for glyph in &run.glyphs {
                xs = (xs.0.min(glyph.x), xs.1.max(glyph.x));
                ys = (ys.0.min(glyph.y), ys.1.max(glyph.y));
            }
            if run.glyphs.is_empty() {
                return Footprint::Nothing;
            }
            let reach = 2.0 * run.size.abs() + 2.0;
            let rect = Rect::new(
                origin.0 + xs.0 - reach,
                origin.1 + ys.0 - reach,
                xs.1 - xs.0 + 2.0 * reach,
                ys.1 - ys.0 + 2.0 * reach,
            );
            Footprint::Area(device_bounds(rect, transform, 1.0))
        }
        Item::BoxShadow { .. }
        | Item::LinearGradient { .. }
        | Item::Image { .. }
        | Item::PushClip { .. }
        | Item::PopClip
        | Item::PushOpacity(_)
        | Item::PopOpacity
        | Item::PushTransform(_)
        | Item::PopTransform => Footprint::Nothing,
    }
}

/// The straight-alpha colour tiny-skia paints with. Transparent colours paint
/// nothing and yield `None`.
fn solid_color(color: Color) -> Option<tiny_skia::Color> {
    if color.is_transparent() {
        return None;
    }
    let straight = |channel: f32| (channel / color.a).clamp(0.0, 1.0);
    tiny_skia::Color::from_rgba(
        straight(color.r),
        straight(color.g),
        straight(color.b),
        color.a.clamp(0.0, 1.0),
    )
}

fn solid_paint(color: Color, anti_alias: bool) -> Option<Paint<'static>> {
    let mut paint = Paint::default();
    paint.set_color(solid_color(color)?);
    paint.anti_alias = anti_alias;
    Some(paint)
}
