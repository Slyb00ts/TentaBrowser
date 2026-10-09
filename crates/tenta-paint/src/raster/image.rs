//! Drawing stored images and linear gradients.
//!
//! An image is one `tiny-skia` pattern shader over the stored pixmap. A smooth
//! draw that shrinks the image by two or more first averages it down a mip
//! chain, kept for the length of the rasterisation, so that bilinear sampling
//! sees at most a 2x reduction and does not alias. Boxes drawn under an
//! axis-aligned transform snap to whole device pixels like every other box.

use std::rc::Rc;

use tenta_layout::{Color, Rect};
use tiny_skia::{
    FillRule, FilterQuality, GradientStop, LinearGradient, Paint, Pattern, Point, SpreadMode,
    Transform,
};

use super::geometry::{clamp_radii, device_bounds, push_rounded_rect, scale_radii};
use super::{Painter, solid_color};
use crate::dl::Radii;
use crate::image::{ImageId, ImageQuality};

/// Levels of a mip chain are cut off here; 2^12 is a 4096x reduction.
const MAX_LEVELS: u32 = 12;

/// One reduced copy of an image.
pub(super) struct Mip {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Mip {
    /// Half the size, rounding up, every texel the mean of the up to four it
    /// covers. Premultiplied values average linearly.
    fn halved(width: u32, height: u32, rgba: &[u8]) -> Mip {
        let new_w = width.div_ceil(2).max(1);
        let new_h = height.div_ceil(2).max(1);
        let mut out = vec![0u8; new_w as usize * new_h as usize * 4];
        let stride = width as usize * 4;
        for y in 0..new_h as usize {
            let y0 = (y * 2).min(height as usize - 1);
            let y1 = (y * 2 + 1).min(height as usize - 1);
            for x in 0..new_w as usize {
                let x0 = (x * 2).min(width as usize - 1);
                let x1 = (x * 2 + 1).min(width as usize - 1);
                for c in 0..4 {
                    let sum = u32::from(rgba[y0 * stride + x0 * 4 + c])
                        + u32::from(rgba[y0 * stride + x1 * 4 + c])
                        + u32::from(rgba[y1 * stride + x0 * 4 + c])
                        + u32::from(rgba[y1 * stride + x1 * 4 + c]);
                    out[(y * new_w as usize + x) * 4 + c] = ((sum + 2) / 4) as u8;
                }
            }
        }
        Mip {
            width: new_w,
            height: new_h,
            rgba: out,
        }
    }
}

impl Painter<'_> {
    /// The reduced copy of `id` at `level`, 1 or more (level 0 is the stored
    /// image itself and is never copied).
    fn mip(&mut self, id: ImageId, level: u32) -> Option<Rc<Mip>> {
        if let Some(mip) = self.mips.get(&(id, level)) {
            return Some(Rc::clone(mip));
        }
        let mip = if level == 1 {
            let image = self.images.get(id)?;
            Mip::halved(image.width, image.height, &image.rgba)
        } else {
            let above = self.mip(id, level - 1)?;
            Mip::halved(above.width, above.height, &above.rgba)
        };
        let mip = Rc::new(mip);
        self.mips.insert((id, level), Rc::clone(&mip));
        Some(mip)
    }

    /// Draws `src` (or the part `src_rect` of it) into `dest`.
    pub(super) fn draw_image(
        &mut self,
        dest: Rect,
        src: ImageId,
        src_rect: Option<Rect>,
        quality: ImageQuality,
    ) {
        let Some(image) = self.images.get(src) else {
            self.stats.skip("Image", 1);
            return;
        };
        if self.clip.bounds().is_empty() || dest.w <= 0.0 || dest.h <= 0.0 {
            return;
        }
        let (full_w, full_h) = (image.width as f32, image.height as f32);
        let part = src_rect
            .filter(|r| r.w > 0.0 && r.h > 0.0)
            .unwrap_or_else(|| Rect::new(0.0, 0.0, full_w, full_h));

        let positive = self.transform.sx > 0.0 && self.transform.sy > 0.0;
        let snapped = self.axis.filter(|_| positive).map(|axis| axis.rect(dest));
        // The drawn size in device pixels decides how far to reduce.
        let (device_w, device_h) = match snapped {
            Some(s) => (s.width() as f32, s.height() as f32),
            None => {
                let t = self.transform;
                (
                    dest.w * (t.sx * t.sx + t.ky * t.ky).sqrt(),
                    dest.h * (t.kx * t.kx + t.sy * t.sy).sqrt(),
                )
            }
        };
        let mut level = 0;
        if quality == ImageQuality::Smooth {
            while level < MAX_LEVELS
                && part.w / (2u32 << level) as f32 >= device_w
                && part.h / (2u32 << level) as f32 >= device_h
                && device_w >= 1.0
                && device_h >= 1.0
            {
                level += 1;
            }
        }
        // Level 0 reads the stored pixels in place.
        let reduced = if level == 0 {
            None
        } else {
            self.mip(src, level)
        };
        let (width, height, bytes): (u32, u32, &[u8]) = match &reduced {
            Some(mip) => (mip.width, mip.height, &mip.rgba),
            None if level == 0 => (image.width, image.height, &image.rgba),
            None => return,
        };
        let Some(pixmap) = tiny_skia::PixmapRef::from_bytes(bytes, width, height) else {
            return;
        };
        let factor = (1u32 << level) as f32;
        let part = Rect::new(
            part.x / factor,
            part.y / factor,
            part.w / factor,
            part.h / factor,
        );
        let filter = match quality {
            ImageQuality::Smooth => FilterQuality::Bilinear,
            ImageQuality::Pixelated => FilterQuality::Nearest,
        };

        match snapped {
            Some(device) => {
                let area = device.intersect(self.clip.bounds());
                let Some(rect) = area.to_skia() else {
                    return;
                };
                let (sx, sy) = (
                    device.width() as f32 / part.w,
                    device.height() as f32 / part.h,
                );
                let map = Transform::from_row(
                    sx,
                    0.0,
                    0.0,
                    sy,
                    device.x0 as f32 - part.x * sx,
                    device.y0 as f32 - part.y * sy,
                );
                let paint = Paint {
                    shader: Pattern::new(pixmap, SpreadMode::Pad, filter, 1.0, map),
                    anti_alias: false,
                    ..Paint::default()
                };
                self.target
                    .fill_rect(rect, &paint, Transform::identity(), self.clip.shape());
            }
            None => {
                let area = device_bounds(dest, self.transform, 1.0);
                if self.clip.bounds().intersect(area).is_empty() {
                    return;
                }
                let Some(rect) = tiny_skia::Rect::from_xywh(dest.x, dest.y, dest.w, dest.h) else {
                    return;
                };
                let (sx, sy) = (dest.w / part.w, dest.h / part.h);
                let map = Transform::from_row(
                    sx,
                    0.0,
                    0.0,
                    sy,
                    dest.x - part.x * sx,
                    dest.y - part.y * sy,
                );
                let paint = Paint {
                    shader: Pattern::new(pixmap, SpreadMode::Pad, filter, 1.0, map),
                    anti_alias: true,
                    ..Paint::default()
                };
                self.target
                    .fill_rect(rect, &paint, self.transform, self.clip.mask_for(area));
            }
        }
    }

    /// Fills `area` with `src` repeated from `origin` in `tile`-sized tiles
    /// (one pattern shader, whatever the number of tiles).
    pub(super) fn draw_image_pattern(
        &mut self,
        area: Rect,
        origin: (f32, f32),
        tile: (f32, f32),
        src: ImageId,
        quality: ImageQuality,
    ) {
        let Some(image) = self.images.get(src) else {
            self.stats.skip("Image", 1);
            return;
        };
        if self.clip.bounds().is_empty()
            || area.w <= 0.0
            || area.h <= 0.0
            || !(tile.0 > 0.0 && tile.1 > 0.0)
        {
            return;
        }
        let positive = self.transform.sx > 0.0 && self.transform.sy > 0.0;
        let snapped = self.axis.filter(|_| positive).map(|axis| axis.rect(area));
        let (scale_x, scale_y) = match self.axis.filter(|_| positive) {
            Some(axis) => (axis.scale_x(), axis.scale_y()),
            None => {
                let t = self.transform;
                (
                    (t.sx * t.sx + t.ky * t.ky).sqrt(),
                    (t.kx * t.kx + t.sy * t.sy).sqrt(),
                )
            }
        };
        let (device_w, device_h) = (tile.0 * scale_x, tile.1 * scale_y);
        let (full_w, full_h) = (image.width as f32, image.height as f32);
        let mut level = 0;
        if quality == ImageQuality::Smooth {
            while level < MAX_LEVELS
                && full_w / (2u32 << level) as f32 >= device_w
                && full_h / (2u32 << level) as f32 >= device_h
                && device_w >= 1.0
                && device_h >= 1.0
            {
                level += 1;
            }
        }
        let reduced = if level == 0 {
            None
        } else {
            self.mip(src, level)
        };
        let (width, height, bytes): (u32, u32, &[u8]) = match &reduced {
            Some(mip) => (mip.width, mip.height, &mip.rgba),
            None if level == 0 => (image.width, image.height, &image.rgba),
            None => return,
        };
        let Some(pixmap) = tiny_skia::PixmapRef::from_bytes(bytes, width, height) else {
            return;
        };
        let filter = match quality {
            ImageQuality::Smooth => FilterQuality::Bilinear,
            ImageQuality::Pixelated => FilterQuality::Nearest,
        };
        // Pixels of the (reduced) image per tile pixel, and where a tile starts.
        let (sx, sy) = (tile.0 / width as f32, tile.1 / height as f32);
        match snapped {
            Some(device) => {
                let region = device.intersect(self.clip.bounds());
                let Some(rect) = region.to_skia() else {
                    return;
                };
                let map = Transform::from_row(
                    sx * scale_x,
                    0.0,
                    0.0,
                    sy * scale_y,
                    self.transform.sx * origin.0 + self.transform.tx,
                    self.transform.sy * origin.1 + self.transform.ty,
                );
                let paint = Paint {
                    shader: Pattern::new(pixmap, SpreadMode::Repeat, filter, 1.0, map),
                    anti_alias: false,
                    ..Paint::default()
                };
                self.target
                    .fill_rect(rect, &paint, Transform::identity(), self.clip.shape());
            }
            None => {
                let bounds = device_bounds(area, self.transform, 1.0);
                if self.clip.bounds().intersect(bounds).is_empty() {
                    return;
                }
                let Some(rect) = tiny_skia::Rect::from_xywh(area.x, area.y, area.w, area.h) else {
                    return;
                };
                let map = Transform::from_row(sx, 0.0, 0.0, sy, origin.0, origin.1);
                let paint = Paint {
                    shader: Pattern::new(pixmap, SpreadMode::Repeat, filter, 1.0, map),
                    anti_alias: true,
                    ..Paint::default()
                };
                self.target
                    .fill_rect(rect, &paint, self.transform, self.clip.mask_for(bounds));
            }
        }
    }

    /// Fills `rect` with a linear gradient. `stops` are offsets along the
    /// gradient line, whose length is `|w sin a| + |h cos a|`, and for a
    /// repeating gradient the first and last offsets bound one period.
    pub(super) fn draw_linear_gradient(
        &mut self,
        rect: Rect,
        radii: Radii,
        angle: f32,
        stops: &[(f32, Color)],
        repeat: bool,
    ) {
        if stops.is_empty() || rect.w <= 0.0 || rect.h <= 0.0 || self.clip.bounds().is_empty() {
            return;
        }
        // Geometry in the space the fill happens in: device pixels when the
        // box snaps, CSS pixels under the full transform otherwise.
        let (space, radii, fill_transform, area) = match self.axis {
            Some(axis) => {
                let snapped = axis.rect(rect);
                (
                    snapped.to_rect(),
                    scale_radii(radii, axis.scale_x(), axis.scale_y()),
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
        if space.w <= 0.0 || space.h <= 0.0 || self.clip.bounds().intersect(area).is_empty() {
            return;
        }
        let (sin, cos) = angle.to_radians().sin_cos();
        let line = (space.w * sin).abs() + (space.h * cos).abs();
        if line <= 1e-3 {
            return;
        }
        let (dx, dy) = (sin, -cos);
        let (cx, cy) = (space.x + space.w / 2.0, space.y + space.h / 2.0);
        let first = stops[0].0;
        let last = stops[stops.len() - 1].0;
        let (from, to) = if repeat && last - first > 1e-6 {
            (first, last)
        } else {
            (0.0, 1.0)
        };
        let point = |t: f32| {
            let along = (t - 0.5) * line;
            Point::from_xy(cx + dx * along, cy + dy * along)
        };
        let Some(skia_stops) = shader_stops(stops, from, to, repeat) else {
            return;
        };
        let spread = if repeat {
            SpreadMode::Repeat
        } else {
            SpreadMode::Pad
        };
        let Some(shader) = LinearGradient::new(
            point(from),
            point(to),
            skia_stops,
            spread,
            Transform::identity(),
        ) else {
            return;
        };
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        if radii.iter().any(|&r| r > 0.0) {
            let radii = clamp_radii(space, radii);
            let Some(path) = self.build_path(|pb| push_rounded_rect(pb, space, radii)) else {
                return;
            };
            self.target.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                fill_transform,
                self.clip.mask_for(area),
            );
            self.recycle(path);
            return;
        }
        if self.axis.is_some() {
            let Some(clipped) = area.to_skia() else {
                return;
            };
            self.target
                .fill_rect(clipped, &paint, Transform::identity(), self.clip.shape());
        } else {
            let Some(skia) = tiny_skia::Rect::from_xywh(space.x, space.y, space.w, space.h) else {
                return;
            };
            self.target
                .fill_rect(skia, &paint, fill_transform, self.clip.mask_for(area));
        }
    }
}

/// Extra stops put between two stops when either is not opaque, so that
/// tiny-skia's straight-alpha interpolation follows CSS Images 4's
/// premultiplied one closely.
const ALPHA_SUBDIVISIONS: usize = 8;

fn mix(a: Color, b: Color, t: f32) -> Color {
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Color {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    }
}

/// `tiny-skia` stops for offsets mapped from `from..to` onto `0..1`.
///
/// A gradient that does not repeat is clipped to `0..1` by interpolating the
/// colour where it enters and leaves, so stops outside the line change the
/// colours inside it as CSS says; a repeating one has its period at `0..1`
/// already. Colours are premultiplied, and a pair of stops with any
/// transparency gets intermediate stops.
fn shader_stops(
    stops: &[(f32, Color)],
    from: f32,
    to: f32,
    repeat: bool,
) -> Option<Vec<GradientStop>> {
    let span = to - from;
    let mut points: Vec<(f32, Color)> = Vec::with_capacity(stops.len());
    let mut last = f32::NEG_INFINITY;
    for &(offset, color) in stops {
        let t = ((offset - from) / span).max(last);
        last = t;
        points.push((t, color));
    }
    if !repeat {
        points = clip_unit(points);
    }
    let mut expanded: Vec<(f32, Color)> = Vec::with_capacity(points.len());
    for (index, &(t, color)) in points.iter().enumerate() {
        if let Some(&(t0, c0)) = expanded.last().filter(|_| index > 0)
            && (c0.a < 1.0 || color.a < 1.0)
            && t > t0
        {
            for step in 1..ALPHA_SUBDIVISIONS {
                let f = step as f32 / ALPHA_SUBDIVISIONS as f32;
                expanded.push((t0 + (t - t0) * f, mix(c0, color, f)));
            }
        }
        expanded.push((t, color));
    }
    expanded
        .into_iter()
        .map(|(t, c)| {
            let straight = if c.a > 0.0 {
                solid_color(c)?
            } else {
                tiny_skia::Color::from_rgba(0.0, 0.0, 0.0, 0.0)?
            };
            Some(GradientStop::new(t.clamp(0.0, 1.0), straight))
        })
        .collect()
}

/// Cuts a monotonic stop list to `0..=1`, adding the interpolated colour at an
/// end the list crosses.
fn clip_unit(points: Vec<(f32, Color)>) -> Vec<(f32, Color)> {
    // Beyond either end of the list the nearest colour continues (pad).
    let colour_at = |x: f32| {
        let Some(upper) = points.iter().position(|p| p.0 >= x) else {
            return points.last().map(|p| p.1);
        };
        if upper == 0 {
            return Some(points[0].1);
        }
        let ((t0, c0), (t1, c1)) = (points[upper - 1], points[upper]);
        Some(if t1 > t0 {
            mix(c0, c1, (x - t0) / (t1 - t0))
        } else {
            c1
        })
    };
    let (Some(start), Some(end)) = (colour_at(0.0), colour_at(1.0)) else {
        return Vec::new();
    };
    let mut out = vec![(0.0, start)];
    out.extend(points.iter().copied().filter(|p| p.0 > 0.0 && p.0 < 1.0));
    out.push((1.0, end));
    out
}
