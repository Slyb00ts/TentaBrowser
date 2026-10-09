//! Border painting after Blink's `BoxBorderPainter`.
//!
//! Uniform solid and double borders are filled as rings. Everything else is
//! painted side by side in Blink's order (translucent after opaque, dashed
//! before solid, top-bottom-right-left), so that a side painted first may
//! cover the corner and its neighbour overdraws the corner's half with a
//! mitre. Rounded boxes cannot overdraw, so each side is cut to the diagonal
//! cone through its corners and shaped by the ring of the border.
//!
//! Known gaps: dashes and dots run along the straight edge and are trimmed by
//! the rounded outline instead of following the arcs, translucent sides get
//! mitres instead of Blink's opacity layers, and an anti-aliased mitre between
//! two different colours leaves the background showing through the seam on a
//! rounded box.

use tenta_layout::{BorderStyle, Color, Edges, Rect};
use tiny_skia::{FillRule, Path, Transform};

use super::dash::{Mark, dashed_marks, dotted_marks};
use super::geometry::{
    IRect, Poly, clamp_radii, device_bounds, inset_radii, is_rounded, push_polygon,
    push_rounded_rect, rect_corners, scale_radii,
};
use super::{Painter, solid_paint};
use crate::dl::Radii;

const TOP: usize = 0;
const RIGHT: usize = 1;
const BOTTOM: usize = 2;
const LEFT: usize = 3;

/// Luminance below which Blink treats a colour as "very dark" for inset and
/// outset shading, and above which a light edge keeps the colour unchanged.
const VERY_DARK_LUMINANCE: f32 = 0.014_443_844;
const VERY_LIGHT_LUMINANCE: f32 = 0.830_77;

/// How two neighbouring sides meet at a corner.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Miter {
    /// The side runs across the whole corner square.
    None,
    /// A diagonal cut, anti-aliased, for sides of different colours.
    Soft,
    /// A diagonal cut without anti-aliasing, so same-coloured sides leave no
    /// seam where they meet.
    Hard,
}

#[derive(Copy, Clone, Debug)]
struct Side {
    /// Width in drawing pixels; zero for `none` and `hidden`.
    width: f32,
    /// The style after Blink's fallbacks (thin double and groove are solid).
    style: BorderStyle,
    color: Color,
    visible: bool,
}

/// A stripe of one side, as distances from the outer edge.
#[derive(Copy, Clone, Debug)]
struct Band {
    from: f32,
    to: f32,
    color: Color,
}

/// A border box in drawing space: whole device pixels under an axis-aligned
/// transform, CSS pixels under a rotation.
struct Geometry {
    transform: Transform,
    snapped: bool,
    outer: Rect,
    inner: Rect,
    radii: Radii,
    sides: [Side; 4],
    rounded: bool,
    /// Device pixels the border can touch.
    area: IRect,
}

impl Geometry {
    fn width(&self, side: usize) -> f32 {
        self.sides[side].width
    }

    fn widths(&self) -> [f32; 4] {
        [
            self.width(TOP),
            self.width(RIGHT),
            self.width(BOTTOM),
            self.width(LEFT),
        ]
    }
}

impl Painter {
    /// Draws a border around `rect`, the border box.
    pub(super) fn draw_border(
        &mut self,
        rect: Rect,
        widths: Edges,
        colors: [Color; 4],
        styles: [BorderStyle; 4],
        radii: Radii,
    ) {
        let Some(g) = self.border_geometry(rect, widths, colors, styles, radii) else {
            return;
        };
        if self.clip.bounds().intersect(g.area).is_empty() || !g.sides.iter().any(|s| s.visible) {
            return;
        }
        if self.fill_uniform_border(&g) {
            return;
        }
        if g.rounded {
            self.border_rounded(&g);
        } else {
            self.border_straight(&g);
        }
    }

    fn border_geometry(
        &self,
        rect: Rect,
        widths: Edges,
        colors: [Color; 4],
        styles: [BorderStyle; 4],
        radii: Radii,
    ) -> Option<Geometry> {
        let raw = [widths.top, widths.right, widths.bottom, widths.left];
        let (outer, used, radii, transform, snapped) = match self.axis {
            Some(axis) => {
                let outer = axis.rect(rect);
                let scales = [
                    axis.scale_y(),
                    axis.scale_x(),
                    axis.scale_y(),
                    axis.scale_x(),
                ];
                let used: [f32; 4] = std::array::from_fn(|i| used_width(raw[i], scales[i], true));
                (
                    outer.to_rect(),
                    used,
                    scale_radii(radii, axis.scale_x(), axis.scale_y()),
                    Transform::identity(),
                    true,
                )
            }
            None => (
                rect,
                raw.map(|w| used_width(w, 1.0, false)),
                radii,
                self.transform,
                false,
            ),
        };
        if outer.w <= 0.0 || outer.h <= 0.0 {
            return None;
        }
        let mut used = used;
        for (width, style) in used.iter_mut().zip(styles) {
            if matches!(style, BorderStyle::None | BorderStyle::Hidden) {
                *width = 0.0;
            }
        }
        // A side cannot be wider than the box it sits on.
        used[TOP] = used[TOP].min(outer.h);
        used[BOTTOM] = used[BOTTOM].min(outer.h);
        used[LEFT] = used[LEFT].min(outer.w);
        used[RIGHT] = used[RIGHT].min(outer.w);

        let sides = std::array::from_fn(|i| Side {
            width: used[i],
            style: effective_style(styles[i], used[i]),
            color: colors[i],
            visible: used[i] > 0.0
                && !matches!(styles[i], BorderStyle::None | BorderStyle::Hidden)
                && !colors[i].is_transparent(),
        });
        let radii = clamp_radii(outer, radii);
        let inner = Rect::new(
            outer.x + used[LEFT],
            outer.y + used[TOP],
            (outer.w - used[LEFT] - used[RIGHT]).max(0.0),
            (outer.h - used[TOP] - used[BOTTOM]).max(0.0),
        );
        Some(Geometry {
            transform,
            snapped,
            outer,
            inner,
            radii,
            sides,
            rounded: is_rounded(&radii),
            area: device_bounds(outer, transform, 1.0),
        })
    }

    /// Fills a border whose four sides share a colour and a style that needs
    /// no mitres (solid, or double) as rings. Returns whether it did.
    fn fill_uniform_border(&mut self, g: &Geometry) -> bool {
        let first = g.sides[TOP];
        let uniform = g.sides.iter().all(|s| {
            s.visible && s.color == first.color && s.style == first.style && s.width > 0.0
        });
        if !uniform {
            return false;
        }
        let widths = g.widths();
        match first.style {
            BorderStyle::Solid => self.fill_ring(g, [0.0; 4], widths, first.color),
            BorderStyle::Double => {
                let thirds = widths.map(|w| third(w, g.snapped));
                self.fill_ring(g, [0.0; 4], thirds, first.color);
                let inner_from: [f32; 4] = std::array::from_fn(|i| widths[i] - thirds[i]);
                self.fill_ring(g, inner_from, widths, first.color);
            }
            _ => return false,
        }
        true
    }

    /// Fills the area between the border box inset by `from` and by `to`.
    fn fill_ring(&mut self, g: &Geometry, from: [f32; 4], to: [f32; 4], color: Color) {
        let outer = inset_rect(g.outer, from);
        let inner = inset_rect(g.outer, to);
        if outer.w <= 0.0 || outer.h <= 0.0 {
            return;
        }
        if g.snapped && !g.rounded {
            // Four rectangles that do not overlap, so a translucent colour is
            // not blended twice, and a pixel-aligned fill needs no anti-aliasing.
            let Some(paint) = solid_paint(color, false) else {
                return;
            };
            let (right, bottom) = (outer.right(), outer.bottom());
            let strips = [
                Rect::new(outer.x, outer.y, outer.w, inner.y - outer.y),
                Rect::new(outer.x, inner.bottom(), outer.w, bottom - inner.bottom()),
                Rect::new(outer.x, inner.y, inner.x - outer.x, inner.h),
                Rect::new(inner.right(), inner.y, right - inner.right(), inner.h),
            ];
            for strip in strips {
                self.fill_pixel_rect(strip, &paint);
            }
            return;
        }
        let Some(paint) = solid_paint(color, true) else {
            return;
        };
        let outer_radii = clamp_radii(outer, inset_radii(g.radii, edges(from)));
        let inner_radii = clamp_radii(inner, inset_radii(g.radii, edges(to)));
        let Some(path) = self.build_path(|pb| {
            push_rounded_rect(pb, outer, outer_radii);
            if inner.w > 0.0 && inner.h > 0.0 {
                push_rounded_rect(pb, inner, inner_radii);
            }
        }) else {
            return;
        };
        self.target.fill_path(
            &path,
            &paint,
            FillRule::EvenOdd,
            g.transform,
            self.clip.mask_for(g.area),
        );
        self.recycle(path);
    }

    /// Fills a rectangle of whole device pixels, clipped without a mask.
    fn fill_pixel_rect(&mut self, rect: Rect, paint: &tiny_skia::Paint<'_>) {
        let pixels = IRect::new(
            rect.x.round() as i32,
            rect.y.round() as i32,
            rect.right().round() as i32,
            rect.bottom().round() as i32,
        )
        .intersect(self.clip.bounds());
        if let Some(skia) = pixels.to_skia().filter(|_| !pixels.is_empty()) {
            self.target
                .fill_rect(skia, paint, Transform::identity(), self.clip.shape());
        }
    }

    /// Paints the sides of a rectangular box in Blink's order.
    fn border_straight(&mut self, g: &Geometry) {
        let mut completed: [bool; 4] = std::array::from_fn(|i| !g.sides[i].visible);
        for side in paint_order(g) {
            if !g.sides[side].visible {
                continue;
            }
            let miters = side_miters(g, side, &completed);
            if matches!(
                g.sides[side].style,
                BorderStyle::Dashed | BorderStyle::Dotted
            ) {
                self.pattern_side_straight(g, side, miters);
            } else {
                for band in side_bands(side, &g.sides[side], g.snapped)
                    .into_iter()
                    .flatten()
                {
                    self.fill_band(g, side, band, miters);
                }
            }
            completed[side] = true;
        }
    }

    /// Fills one stripe of a side of a rectangular box.
    fn fill_band(&mut self, g: &Geometry, side: usize, band: Band, miters: [Miter; 2]) {
        let poly = band_polygon(g, side, band.from, band.to, miters);
        if miters == [Miter::None; 2] && g.snapped {
            let Some(paint) = solid_paint(band.color, false) else {
                return;
            };
            let b = poly_bounds(&poly);
            self.fill_pixel_rect(b, &paint);
            return;
        }
        for (part, anti_alias) in self
            .mitre_parts(g, side, poly, miters)
            .into_iter()
            .flatten()
        {
            self.fill_polygon(g, &part, band.color, anti_alias);
        }
    }

    /// Splits a side's polygon at its middle when its two ends need different
    /// anti-aliasing, so that each end gets its own mode.
    fn mitre_parts(
        &self,
        g: &Geometry,
        side: usize,
        poly: Poly,
        miters: [Miter; 2],
    ) -> [Option<(Poly, bool)>; 2] {
        let mode = |m: Miter| match m {
            Miter::None => None,
            Miter::Soft => Some(true),
            Miter::Hard => Some(false),
        };
        match (mode(miters[0]), mode(miters[1])) {
            (Some(a), Some(b)) if a != b => {
                let frame = side_frame(g.outer, side);
                let (start, end) = (frame.start, frame.end);
                let mut mid = ((start.0 + end.0) / 2.0, (start.1 + end.1) / 2.0);
                if g.snapped {
                    mid = (mid.0.floor(), mid.1.floor());
                }
                [
                    Some((poly.clipped(mid, frame.inward, start), a)),
                    Some((poly.clipped(mid, frame.inward, end), b)),
                ]
            }
            (a, b) => [Some((poly, a.or(b).unwrap_or(true))), None],
        }
    }

    fn fill_polygon(&mut self, g: &Geometry, poly: &Poly, color: Color, anti_alias: bool) {
        let Some(paint) = solid_paint(color, anti_alias) else {
            return;
        };
        let area = device_bounds(poly_bounds(poly), g.transform, 1.0);
        let Some(path) = self.build_path(|pb| push_polygon(pb, poly)) else {
            return;
        };
        self.target.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            g.transform,
            self.clip.mask_for(area),
        );
        self.recycle(path);
    }

    /// Draws a dashed or dotted side of a rectangular box, cut to its mitres.
    fn pattern_side_straight(&mut self, g: &Geometry, side: usize, miters: [Miter; 2]) {
        let Some(marks) = self.pattern_path(g, side) else {
            return;
        };
        let Some(paint) = solid_paint(g.sides[side].color, true) else {
            return;
        };
        if miters == [Miter::None; 2] {
            self.target.fill_path(
                &marks,
                &paint,
                FillRule::Winding,
                g.transform,
                self.clip.mask_for(g.area),
            );
        } else {
            let poly = band_polygon(g, side, 0.0, g.width(side), miters);
            let canvas = IRect::from_size(self.target.width(), self.target.height());
            let area = device_bounds(poly_bounds(&poly), g.transform, 1.0).intersect(canvas);
            self.side.reset(canvas);
            for (part, anti_alias) in self
                .mitre_parts(g, side, poly, miters)
                .into_iter()
                .flatten()
            {
                if let Some(path) = self.build_path(|pb| push_polygon(pb, &part)) {
                    self.side
                        .add_path(&path, FillRule::Winding, anti_alias, g.transform, area);
                    self.recycle(path);
                }
            }
            if let Some(clip) = self.clip.mask_for(area) {
                self.side.intersect(clip, area);
            }
            self.target.fill_path(
                &marks,
                &paint,
                FillRule::Winding,
                g.transform,
                self.side.mask(),
            );
        }
        self.recycle(marks);
    }

    /// The dashes or dots of a side as one path, laid along its full length.
    fn pattern_path(&mut self, g: &Geometry, side: usize) -> Option<Path> {
        let width = g.width(side);
        let strip = side_strip(g.outer, side, width);
        let horizontal = side == TOP || side == BOTTOM;
        let length = if horizontal { strip.w } else { strip.h };
        let style = g.sides[side].style;
        self.build_path(|pb| {
            let mut emit = |mark: Mark| match mark {
                Mark::Span(a, b) if b > a => {
                    let span = if horizontal {
                        Rect::new(strip.x + a, strip.y, b - a, strip.h)
                    } else {
                        Rect::new(strip.x, strip.y + a, strip.w, b - a)
                    };
                    if let Some(rect) = tiny_skia::Rect::from_xywh(span.x, span.y, span.w, span.h) {
                        pb.push_rect(rect);
                    }
                }
                Mark::Span(..) => {}
                Mark::Dot(centre) => {
                    let (cx, cy) = if horizontal {
                        (strip.x + centre, strip.y + strip.h / 2.0)
                    } else {
                        (strip.x + strip.w / 2.0, strip.y + centre)
                    };
                    pb.push_circle(cx, cy, width / 2.0);
                }
            };
            if style == BorderStyle::Dashed {
                dashed_marks(length, width, &mut emit);
            } else {
                dotted_marks(length, width, &mut emit);
            }
        })
    }

    /// Paints a rounded box side by side in Blink's order. A side's stripe is
    /// its reach (the strip along the edge with the corner arcs) cut by the
    /// diagonal at each mitred end and shaped by the ring of its band, so
    /// that a side painted first can cover a corner for its neighbour to
    /// overdraw.
    fn border_rounded(&mut self, g: &Geometry) {
        let bands: [[Option<Band>; 2]; 4] =
            std::array::from_fn(|i| side_bands(i, &g.sides[i], g.snapped));
        let patterned = |i: usize| {
            g.sides[i].visible
                && matches!(g.sides[i].style, BorderStyle::Dashed | BorderStyle::Dotted)
        };
        // The insets of each ring's two edges: one ring per band index, and a
        // whole-border ring for dashes and dots.
        let ring_insets: [Option<RingInsets>; 3] = std::array::from_fn(|index| {
            if index == 2 {
                return (0..4)
                    .any(patterned)
                    .then(|| RingInsets([0.0; 4], g.widths()));
            }
            bands.iter().any(|b| b[index].is_some()).then(|| {
                RingInsets(
                    std::array::from_fn(|i| bands[i][index].map_or(g.width(i), |b| b.from)),
                    std::array::from_fn(|i| bands[i][index].map_or(g.width(i), |b| b.to)),
                )
            })
        });
        let mut ready = [false; 3];
        let mut completed: [bool; 4] = std::array::from_fn(|i| !g.sides[i].visible);
        for side in paint_order(g) {
            if !g.sides[side].visible {
                continue;
            }
            let miters = side_miters(g, side, &completed);
            let region = side_region(g, side, miters);
            if patterned(side) {
                if self.ensure_ring(g, 2, &ring_insets, &mut ready) {
                    self.pattern_side_rounded(g, side, region, miters);
                }
            } else {
                for (index, band) in bands[side].into_iter().enumerate() {
                    let Some(band) = band else { continue };
                    if self.ensure_ring(g, index, &ring_insets, &mut ready) {
                        self.fill_region(g, side, band.color, region, miters, index);
                    }
                }
            }
            completed[side] = true;
        }
    }

    /// Builds ring `index` the first time a side needs it. False when the
    /// ring holds nothing.
    fn ensure_ring(
        &mut self,
        g: &Geometry,
        index: usize,
        insets: &[Option<RingInsets>; 3],
        ready: &mut [bool; 3],
    ) -> bool {
        if ready[index] {
            return true;
        }
        let Some(RingInsets(from, to)) = insets[index] else {
            return false;
        };
        ready[index] = self.prepare_ring(g, index, from, to);
        ready[index]
    }

    /// Builds the ring between the border box inset by `from` and by `to`,
    /// within the clip, in scratch mask `index`. False when nothing remains.
    fn prepare_ring(&mut self, g: &Geometry, index: usize, from: [f32; 4], to: [f32; 4]) -> bool {
        let outer = inset_rect(g.outer, from);
        let inner = inset_rect(g.outer, to);
        if outer.w <= 0.0 || outer.h <= 0.0 {
            return false;
        }
        let outer_radii = clamp_radii(outer, inset_radii(g.radii, edges(from)));
        let inner_radii = clamp_radii(inner, inset_radii(g.radii, edges(to)));
        let Some(path) = self.build_path(|pb| {
            push_rounded_rect(pb, outer, outer_radii);
            if inner.w > 0.0 && inner.h > 0.0 {
                push_rounded_rect(pb, inner, inner_radii);
            }
        }) else {
            return false;
        };
        let canvas = IRect::from_size(self.target.width(), self.target.height());
        let area = g.area.intersect(canvas);
        let ring = &mut self.rings[index];
        ring.reset(canvas);
        ring.add_path(&path, FillRule::EvenOdd, true, g.transform, area);
        if let Some(clip) = self.clip.mask_for(area) {
            ring.intersect(clip, area);
        }
        self.recycle(path);
        true
    }

    /// Builds `region`, with each end anti-aliased as its mitre asks,
    /// intersected with ring `ring` in the `side` scratch mask.
    fn prepare_side_mask(
        &mut self,
        g: &Geometry,
        side: usize,
        region: Poly,
        miters: [Miter; 2],
        ring: usize,
    ) -> bool {
        let canvas = IRect::from_size(self.target.width(), self.target.height());
        let area = g.area.intersect(canvas);
        self.side.reset(canvas);
        for (part, anti_alias) in self
            .mitre_parts(g, side, region, miters)
            .into_iter()
            .flatten()
        {
            if let Some(path) = self.build_path(|pb| push_polygon(pb, &part)) {
                self.side
                    .add_path(&path, FillRule::Winding, anti_alias, g.transform, area);
                self.recycle(path);
            }
        }
        let Some(ring) = self.rings[ring].mask() else {
            return false;
        };
        self.side.intersect(ring, area);
        true
    }

    fn fill_region(
        &mut self,
        g: &Geometry,
        side: usize,
        color: Color,
        region: Poly,
        miters: [Miter; 2],
        ring: usize,
    ) {
        let Some(paint) = solid_paint(color, true) else {
            return;
        };
        if !self.prepare_side_mask(g, side, region, miters, ring) {
            return;
        }
        let reach = side_reach(g, side);
        if let (Some(rect), Some(mask)) = (
            tiny_skia::Rect::from_xywh(reach.x, reach.y, reach.w, reach.h),
            self.side.mask(),
        ) {
            self.target.fill_rect(rect, &paint, g.transform, Some(mask));
        }
    }

    fn pattern_side_rounded(
        &mut self,
        g: &Geometry,
        side: usize,
        region: Poly,
        miters: [Miter; 2],
    ) {
        let Some(paint) = solid_paint(g.sides[side].color, true) else {
            return;
        };
        let Some(marks) = self.pattern_path(g, side) else {
            return;
        };
        if self.prepare_side_mask(g, side, region, miters, 2) {
            self.target.fill_path(
                &marks,
                &paint,
                FillRule::Winding,
                g.transform,
                self.side.mask(),
            );
        }
        self.recycle(marks);
    }
}

/// The insets from the border box of a ring's outer and inner edge.
#[derive(Copy, Clone)]
struct RingInsets([f32; 4], [f32; 4]);

/// The order Blink paints sides in: translucent after opaque, then dashed
/// before solid, then top, bottom, right, left.
fn paint_order(g: &Geometry) -> [usize; 4] {
    let mut order = [TOP, RIGHT, BOTTOM, LEFT];
    order.sort_by(|&a, &b| {
        let (sa, sb) = (&g.sides[a], &g.sides[b]);
        sa.color
            .a
            .total_cmp(&sb.color.a)
            .then(style_priority(sa.style).cmp(&style_priority(sb.style)))
            .then(side_priority(a).cmp(&side_priority(b)))
    });
    order
}

/// How `side` meets its neighbours at its start and end corner.
fn side_miters(g: &Geometry, side: usize, completed: &[bool; 4]) -> [Miter; 2] {
    [
        miter_at(g, side, (side + 3) % 4, completed),
        miter_at(g, side, (side + 1) % 4, completed),
    ]
}

/// The width a computed border width paints with: floored to whole device
/// pixels with one pixel as the minimum, as Chrome does, when snapping.
fn used_width(raw: f32, scale: f32, snapped: bool) -> f32 {
    if raw.is_nan() || raw <= 0.0 {
        return 0.0;
    }
    if snapped {
        (raw * scale).floor().max(1.0)
    } else {
        raw
    }
}

/// Thin double borders and thin groove and ridge borders fall back to solid,
/// as in Blink, because their stripes would be below a pixel.
fn effective_style(style: BorderStyle, width: f32) -> BorderStyle {
    match style {
        BorderStyle::Double if width < 3.0 => BorderStyle::Solid,
        BorderStyle::Groove | BorderStyle::Ridge if width <= 1.0 => BorderStyle::Solid,
        other => other,
    }
}

fn style_priority(style: BorderStyle) -> u8 {
    match style {
        BorderStyle::None | BorderStyle::Hidden => 0,
        BorderStyle::Dotted | BorderStyle::Dashed | BorderStyle::Double => 1,
        BorderStyle::Inset | BorderStyle::Groove | BorderStyle::Outset | BorderStyle::Ridge => 2,
        BorderStyle::Solid => 3,
    }
}

/// Sides of one style are painted non-adjacent first, which minimises the
/// number of mitres that need clipping.
fn side_priority(side: usize) -> u8 {
    match side {
        TOP => 0,
        BOTTOM => 1,
        RIGHT => 2,
        _ => 3,
    }
}

/// True for styles whose paint covers the side's whole area, so that a side
/// drawn later can overdraw a corner left to it.
fn fills_area(style: BorderStyle) -> bool {
    !matches!(
        style,
        BorderStyle::Dotted | BorderStyle::Dashed | BorderStyle::Double
    )
}

/// The three-dimensional styles shade the two sides of a corner differently
/// unless the corner is between top and left or between bottom and right.
fn unmatched_at_corner(style: BorderStyle, side: usize, adjacent: usize) -> bool {
    matches!(
        style,
        BorderStyle::Inset | BorderStyle::Groove | BorderStyle::Ridge | BorderStyle::Outset
    ) && matches!(
        (side.min(adjacent), side.max(adjacent)),
        (TOP, RIGHT) | (BOTTOM, LEFT)
    )
}

fn colors_match_at_corner(g: &Geometry, side: usize, adjacent: usize) -> bool {
    let (a, b) = (&g.sides[side], &g.sides[adjacent]);
    b.visible && a.color == b.color && !unmatched_at_corner(a.style, side, adjacent)
}

fn styles_require_miter(a: BorderStyle, b: BorderStyle, side: usize, adjacent: usize) -> bool {
    if a == BorderStyle::Double
        || b == BorderStyle::Double
        || matches!(b, BorderStyle::Groove | BorderStyle::Ridge)
    {
        return true;
    }
    let patterned = |s| matches!(s, BorderStyle::Dotted | BorderStyle::Dashed);
    patterned(a) != patterned(b) || a != b || unmatched_at_corner(a, side, adjacent)
}

/// How `side` meets `adjacent` on a rectangular box, given which sides are
/// already painted.
fn miter_at(g: &Geometry, side: usize, adjacent: usize, completed: &[bool; 4]) -> Miter {
    let (a, b) = (&g.sides[side], &g.sides[adjacent]);
    if b.width <= 0.0 {
        return Miter::None;
    }
    // Overdrawing a translucent corner would blend it twice, so translucent
    // sides always get a mitre.
    let translucent = a.color.a < 1.0 || b.color.a < 1.0;
    if !translucent && !completed[adjacent] && fills_area(b.style) {
        return Miter::None;
    }
    if !colors_match_at_corner(g, side, adjacent) {
        return Miter::Soft;
    }
    if translucent || styles_require_miter(a.style, b.style, side, adjacent) {
        return Miter::Hard;
    }
    Miter::None
}

/// Start and end corner of `side` (clockwise), the unit direction along it
/// and the inward normal.
fn side_frame(outer: Rect, side: usize) -> SideFrame {
    let c = rect_corners(outer);
    let along = match side {
        TOP => (1.0, 0.0),
        RIGHT => (0.0, 1.0),
        BOTTOM => (-1.0, 0.0),
        _ => (0.0, -1.0),
    };
    SideFrame {
        start: c[side],
        end: c[(side + 1) % 4],
        along,
        inward: (-along.1, along.0),
    }
}

/// The corners of a side, clockwise, and its directions as unit vectors.
struct SideFrame {
    start: (f32, f32),
    end: (f32, f32),
    along: (f32, f32),
    inward: (f32, f32),
}

/// The stripe of `side` between depths `from` and `to`. A mitred end follows
/// the diagonal of the corner, so the depth of the stripe's corner along the
/// side is the depth scaled by the ratio of the neighbour's width to this
/// side's; an unmitred end stays on the box edge.
fn band_polygon(g: &Geometry, side: usize, from: f32, to: f32, miters: [Miter; 2]) -> Poly {
    let SideFrame {
        start,
        end,
        along: u,
        inward: n,
    } = side_frame(g.outer, side);
    let width = g.width(side).max(f32::MIN_POSITIVE);
    let neighbours = [g.width((side + 3) % 4), g.width((side + 1) % 4)];
    let point = |corner: (f32, f32), toward: f32, depth: f32, miter: Miter, neighbour: f32| {
        let shift = if miter == Miter::None {
            0.0
        } else {
            toward * depth * neighbour / width
        };
        (
            corner.0 + n.0 * depth + u.0 * shift,
            corner.1 + n.1 * depth + u.1 * shift,
        )
    };
    Poly::new(&[
        point(start, 1.0, from, miters[0], neighbours[0]),
        point(end, -1.0, from, miters[1], neighbours[1]),
        point(end, -1.0, to, miters[1], neighbours[1]),
        point(start, 1.0, to, miters[0], neighbours[0]),
    ])
}

/// The region of a rounded box a side may paint: its reach, cut by the
/// diagonal through the corner at each mitred end. The diagonals run from the
/// outer corner to the inner rectangle's corner, so they also cover the inner
/// corner squares that a rounded inner edge leaves in the frame.
fn side_region(g: &Geometry, side: usize, miters: [Miter; 2]) -> Poly {
    let outer = rect_corners(g.outer);
    let inner = rect_corners(g.inner);
    let next = (side + 1) % 4;
    let mid = (
        (outer[side].0 + outer[next].0) / 2.0,
        (outer[side].1 + outer[next].1) / 2.0,
    );
    let mut region = Poly::new(&rect_corners(side_reach(g, side)));
    for (corner, miter) in [(side, miters[0]), (next, miters[1])] {
        if miter == Miter::None {
            continue;
        }
        let dir = (
            inner[corner].0 - outer[corner].0,
            inner[corner].1 - outer[corner].1,
        );
        region = region.clipped(outer[corner], dir, mid);
    }
    region
}

/// The part of a rounded box a side can reach: its strip plus the corner
/// arcs, so filling it with the side's mask touches no more than needed.
fn side_reach(g: &Geometry, side: usize) -> Rect {
    let r = &g.radii;
    let (o, w) = (g.outer, g.width(side));
    let depth = |a: f32, b: f32| w.max(a).max(b);
    match side {
        TOP => Rect::new(o.x, o.y, o.w, depth(r[1], r[3]).min(o.h)),
        RIGHT => {
            let d = depth(r[2], r[4]).min(o.w);
            Rect::new(o.right() - d, o.y, d, o.h)
        }
        BOTTOM => {
            let d = depth(r[5], r[7]).min(o.h);
            Rect::new(o.x, o.bottom() - d, o.w, d)
        }
        _ => Rect::new(o.x, o.y, depth(r[0], r[6]).min(o.w), o.h),
    }
}

/// The strip a side occupies along the whole box edge, corners included.
fn side_strip(outer: Rect, side: usize, width: f32) -> Rect {
    match side {
        TOP => Rect::new(outer.x, outer.y, outer.w, width),
        RIGHT => Rect::new(outer.right() - width, outer.y, width, outer.h),
        BOTTOM => Rect::new(outer.x, outer.bottom() - width, outer.w, width),
        _ => Rect::new(outer.x, outer.y, width, outer.h),
    }
}

fn poly_bounds(poly: &Poly) -> Rect {
    let mut min = (f32::INFINITY, f32::INFINITY);
    let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for &(x, y) in poly.points() {
        min = (min.0.min(x), min.1.min(y));
        max = (max.0.max(x), max.1.max(y));
    }
    if poly.points().is_empty() {
        return Rect::default();
    }
    Rect::new(min.0, min.1, max.0 - min.0, max.1 - min.1)
}

/// `outer` inset by `by` per side, never below zero in size.
fn inset_rect(outer: Rect, by: [f32; 4]) -> Rect {
    Rect::new(
        outer.x + by[LEFT],
        outer.y + by[TOP],
        (outer.w - by[LEFT] - by[RIGHT]).max(0.0),
        (outer.h - by[TOP] - by[BOTTOM]).max(0.0),
    )
}

fn edges([top, right, bottom, left]: [f32; 4]) -> Edges {
    Edges {
        top,
        right,
        bottom,
        left,
    }
}

/// A third of a double border's width, rounded as Blink does: with whole
/// pixels the outer stripe is `(width + 1) / 3` and the gap takes the rest.
fn third(width: f32, snapped: bool) -> f32 {
    if snapped {
        ((width as i32 + 1) / 3) as f32
    } else {
        width / 3.0
    }
}

/// The stripes a side is made of. Dashed and dotted sides have none: they are
/// drawn as marks.
fn side_bands(side: usize, s: &Side, snapped: bool) -> [Option<Band>; 2] {
    if !s.visible {
        return [None, None];
    }
    let width = s.width;
    let whole = |color| {
        [
            Some(Band {
                from: 0.0,
                to: width,
                color,
            }),
            None,
        ]
    };
    match s.style {
        BorderStyle::Solid => whole(s.color),
        BorderStyle::Inset | BorderStyle::Outset => {
            whole(inset_outset_color(darkens(side, s.style), s.color))
        }
        BorderStyle::Double => {
            let t = third(width, snapped);
            [
                Some(Band {
                    from: 0.0,
                    to: t,
                    color: s.color,
                }),
                Some(Band {
                    from: width - t,
                    to: width,
                    color: s.color,
                }),
            ]
        }
        BorderStyle::Groove | BorderStyle::Ridge => {
            // The outer half is drawn as the first style and the inner half as
            // the second. The extra pixel of an odd width goes to the half
            // that comes first in reading order: the outer half of the top and
            // left sides and the inner half of the bottom and right sides.
            let (outer_style, inner_style) = if s.style == BorderStyle::Groove {
                (BorderStyle::Inset, BorderStyle::Outset)
            } else {
                (BorderStyle::Outset, BorderStyle::Inset)
            };
            let half = match (side == TOP || side == LEFT, snapped) {
                (true, true) => ((width as i32 + 1) / 2) as f32,
                (false, true) => (width as i32 / 2) as f32,
                (_, false) => width / 2.0,
            };
            [
                Some(Band {
                    from: 0.0,
                    to: half,
                    color: inset_outset_color(darkens(side, outer_style), s.color),
                }),
                Some(Band {
                    from: half,
                    to: width,
                    color: inset_outset_color(darkens(side, inner_style), s.color),
                }),
            ]
        }
        BorderStyle::Dashed | BorderStyle::Dotted | BorderStyle::None | BorderStyle::Hidden => {
            [None, None]
        }
    }
}

/// Inset darkens the top and left sides and outset the bottom and right.
fn darkens(side: usize, style: BorderStyle) -> bool {
    (side == TOP || side == LEFT) == (style == BorderStyle::Inset)
}

/// The colour of one edge of an inset, outset, groove or ridge border, as
/// Blink's `CalculateInsetOutsetColor` computes it with the current-colour
/// table default shipped: very dark colours are lightened so that both edges
/// stay visible, other colours darken on the shadow edge and lighten on the
/// lit edge unless they are already very light.
fn inset_outset_color(darken: bool, color: Color) -> Color {
    let luminance = relative_luminance(color);
    if luminance <= VERY_DARK_LUMINANCE {
        return if darken {
            light(color)
        } else {
            light(light(color))
        };
    }
    if darken {
        return dark(color);
    }
    if luminance > VERY_LIGHT_LUMINANCE {
        color
    } else {
        light(color)
    }
}

/// Blink's `Color::Light()`: the brightest channel gains a third, the rest
/// scale with it, and black becomes a fixed grey.
fn light(color: Color) -> Color {
    const LIGHTENED_BLACK: f32 = 84.0 / 255.0;
    let [r, g, b] = straight(color);
    let v = r.max(g).max(b);
    if v <= 0.0 {
        return from_straight([LIGHTENED_BLACK; 3], color.a);
    }
    let scale = (v + 0.33).min(1.0) / v;
    from_straight([r * scale, g * scale, b * scale], color.a)
}

/// Blink's `Color::Dark()`: the brightest channel loses a third, the rest
/// scale with it, and white becomes a fixed grey.
fn dark(color: Color) -> Color {
    const DARKENED_WHITE: f32 = 171.0 / 255.0;
    if color == Color::WHITE {
        return from_straight([DARKENED_WHITE; 3], 1.0);
    }
    let [r, g, b] = straight(color);
    let v = r.max(g).max(b);
    let scale = if v <= 0.0 {
        0.0
    } else {
        ((v - 0.33) / v).max(0.0)
    };
    from_straight([r * scale, g * scale, b * scale], color.a)
}

fn straight(color: Color) -> [f32; 3] {
    if color.a <= 0.0 {
        return [0.0; 3];
    }
    [color.r / color.a, color.g / color.a, color.b / color.a]
}

/// A colour from straight channels, quantised to 8 bits as Blink does.
fn from_straight(channels: [f32; 3], alpha: f32) -> Color {
    let q = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() / 255.0;
    Color {
        r: q(channels[0]) * alpha,
        g: q(channels[1]) * alpha,
        b: q(channels[2]) * alpha,
        a: alpha,
    }
}

/// WCAG relative luminance of the colour's straight sRGB channels.
fn relative_luminance(color: Color) -> f32 {
    let linear = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = straight(color);
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb8(color: Color) -> [u8; 3] {
        let [r, g, b] = straight(color).map(|c| (c * 255.0).round() as u8);
        [r, g, b]
    }

    #[test]
    fn shades_follow_blink_for_representative_colours() {
        let c = |r, g, b| Color::from_rgba8(r, g, b, 255);
        // Expected values come from evaluating Blink's Color::Dark/Light and
        // CalculateInsetOutsetColor on 8-bit channels.
        assert_eq!(rgb8(inset_outset_color(true, c(0, 0, 0))), [84, 84, 84]);
        assert_eq!(rgb8(inset_outset_color(false, c(0, 0, 0))), [168, 168, 168]);
        assert_eq!(
            rgb8(inset_outset_color(true, c(200, 200, 200))),
            [116, 116, 116]
        );
        assert_eq!(
            rgb8(inset_outset_color(false, c(200, 200, 200))),
            [255, 255, 255]
        );
        assert_eq!(
            rgb8(inset_outset_color(true, c(120, 160, 210))),
            [72, 96, 126]
        );
        assert_eq!(
            rgb8(inset_outset_color(false, c(120, 160, 210))),
            [146, 194, 255]
        );
        assert_eq!(
            rgb8(inset_outset_color(true, c(240, 240, 240))),
            [156, 156, 156]
        );
        assert_eq!(
            rgb8(inset_outset_color(false, c(240, 240, 240))),
            [240, 240, 240]
        );
        assert_eq!(rgb8(dark(Color::WHITE)), [171, 171, 171]);
    }

    #[test]
    fn thin_double_and_groove_fall_back_to_solid() {
        assert_eq!(
            effective_style(BorderStyle::Double, 2.0),
            BorderStyle::Solid
        );
        assert_eq!(
            effective_style(BorderStyle::Double, 3.0),
            BorderStyle::Double
        );
        assert_eq!(
            effective_style(BorderStyle::Groove, 1.0),
            BorderStyle::Solid
        );
        assert_eq!(effective_style(BorderStyle::Ridge, 2.0), BorderStyle::Ridge);
    }

    #[test]
    fn widths_floor_to_device_pixels_with_a_one_pixel_minimum() {
        assert_eq!(used_width(1.9, 1.0, true), 1.0);
        assert_eq!(used_width(0.2, 1.0, true), 1.0);
        assert_eq!(used_width(1.5, 2.0, true), 3.0);
        assert_eq!(used_width(0.0, 1.0, true), 0.0);
        assert_eq!(used_width(f32::NAN, 1.0, true), 0.0);
        assert_eq!(used_width(1.9, 1.0, false), 1.9);
    }

    #[test]
    fn double_thirds_use_blinks_integer_rounding() {
        let thirds: Vec<f32> = (3..=9).map(|w| third(w as f32, true)).collect();
        assert_eq!(thirds, [1.0, 1.0, 2.0, 2.0, 2.0, 3.0, 3.0]);
    }
}
