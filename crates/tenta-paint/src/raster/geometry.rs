//! Device-pixel geometry: integer rectangles, snapping of boxes to the pixel
//! grid, small convex polygons and rounded-rectangle paths.

use tenta_layout::{Edges, Rect};
use tiny_skia::{PathBuilder, Transform};

use crate::dl::Radii;

/// Control-point distance of a cubic approximating a quarter circle, as a
/// fraction of the radius.
const KAPPA: f32 = 0.552_284_8;

/// Corner radii as `(x, y)` pairs in top-left, top-right, bottom-right,
/// bottom-left order.
type Corners = [(f32, f32); 4];

/// A half-open rectangle of device pixels.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct IRect {
    pub(super) x0: i32,
    pub(super) y0: i32,
    pub(super) x1: i32,
    pub(super) y1: i32,
}

impl IRect {
    pub(super) const EMPTY: IRect = IRect::new(0, 0, 0, 0);

    pub(super) const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub(super) fn from_size(width: u32, height: u32) -> Self {
        Self::new(
            0,
            0,
            i32::try_from(width).unwrap_or(i32::MAX),
            i32::try_from(height).unwrap_or(i32::MAX),
        )
    }

    pub(super) fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub(super) fn width(&self) -> i32 {
        self.x1 - self.x0
    }

    pub(super) fn height(&self) -> i32 {
        self.y1 - self.y0
    }

    /// The overlap, which is empty when the rectangles are disjoint.
    pub(super) fn intersect(&self, other: IRect) -> IRect {
        let overlap = IRect::new(
            self.x0.max(other.x0),
            self.y0.max(other.y0),
            self.x1.min(other.x1),
            self.y1.min(other.y1),
        );
        if overlap.is_empty() {
            IRect::EMPTY
        } else {
            overlap
        }
    }

    /// The smallest rectangle holding both; an empty operand is ignored.
    pub(super) fn union(&self, other: IRect) -> IRect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return *self;
        }
        IRect::new(
            self.x0.min(other.x0),
            self.y0.min(other.y0),
            self.x1.max(other.x1),
            self.y1.max(other.y1),
        )
    }

    /// True when `other` lies wholly inside; an empty `other` always does.
    pub(super) fn contains(&self, other: IRect) -> bool {
        other.is_empty()
            || (self.x0 <= other.x0
                && self.y0 <= other.y0
                && self.x1 >= other.x1
                && self.y1 >= other.y1)
    }

    pub(super) fn to_rect(self) -> Rect {
        Rect::new(
            self.x0 as f32,
            self.y0 as f32,
            self.width() as f32,
            self.height() as f32,
        )
    }

    /// The tiny-skia rectangle, or `None` when empty.
    pub(super) fn to_skia(self) -> Option<tiny_skia::Rect> {
        tiny_skia::Rect::from_ltrb(
            self.x0 as f32,
            self.y0 as f32,
            self.x1 as f32,
            self.y1 as f32,
        )
    }
}

/// The smallest pixel rectangle holding `rect` after `transform`, grown by
/// `outset` device pixels for anti-aliasing and strokes.
pub(super) fn device_bounds(rect: Rect, transform: Transform, outset: f32) -> IRect {
    let corners = [
        (rect.x, rect.y),
        (rect.right(), rect.y),
        (rect.right(), rect.bottom()),
        (rect.x, rect.bottom()),
    ];
    let mut min = (f32::INFINITY, f32::INFINITY);
    let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for (x, y) in corners {
        let dx = transform.sx * x + transform.kx * y + transform.tx;
        let dy = transform.ky * x + transform.sy * y + transform.ty;
        min = (min.0.min(dx), min.1.min(dy));
        max = (max.0.max(dx), max.1.max(dy));
    }
    if !(min.0.is_finite() && min.1.is_finite() && max.0.is_finite() && max.1.is_finite()) {
        return IRect::EMPTY;
    }
    IRect::new(
        (min.0 - outset).floor() as i32,
        (min.1 - outset).floor() as i32,
        (max.0 + outset).ceil() as i32,
        (max.1 + outset).ceil() as i32,
    )
}

/// A transform that only scales and translates. Boxes drawn through one are
/// snapped to whole device pixels, so fractional layout never paints soft
/// fringes. Rotations and skews keep their geometry exact, since snapping in
/// local space would not land on device pixels.
#[derive(Copy, Clone, Debug)]
pub(super) struct Axis {
    sx: f32,
    sy: f32,
    tx: f32,
    ty: f32,
}

impl Axis {
    pub(super) fn of(transform: Transform) -> Option<Axis> {
        let usable = |v: f32| v != 0.0 && v.is_finite();
        (transform.kx == 0.0
            && transform.ky == 0.0
            && usable(transform.sx)
            && usable(transform.sy)
            && transform.tx.is_finite()
            && transform.ty.is_finite())
        .then_some(Axis {
            sx: transform.sx,
            sy: transform.sy,
            tx: transform.tx,
            ty: transform.ty,
        })
    }

    /// Device pixels per local unit, horizontally.
    pub(super) fn scale_x(&self) -> f32 {
        self.sx.abs()
    }

    /// Device pixels per local unit, vertically.
    pub(super) fn scale_y(&self) -> f32 {
        self.sy.abs()
    }

    /// Chrome's box snapping: each edge rounds to the nearest pixel, so the
    /// size is the difference of the rounded edges rather than a rounded size.
    pub(super) fn rect(&self, rect: Rect) -> IRect {
        // A negative size is not a box; a mirrored axis still has a positive one.
        if rect.w.is_nan() || rect.h.is_nan() || rect.w <= 0.0 || rect.h <= 0.0 {
            return IRect::EMPTY;
        }
        let (x0, x1) = snap_span(self.sx * rect.x + self.tx, self.sx * rect.right() + self.tx);
        let (y0, y1) = snap_span(
            self.sy * rect.y + self.ty,
            self.sy * rect.bottom() + self.ty,
        );
        IRect::new(x0, y0, x1, y1)
    }
}

/// Rounds both ends half-up and orders them, since a mirrored axis swaps them.
fn snap_span(a: f32, b: f32) -> (i32, i32) {
    let round = |v: f32| (v + 0.5).floor() as i32;
    let (a, b) = (round(a), round(b));
    (a.min(b), a.max(b))
}

/// Scales the radii so that no side is shorter than the sum of the two radii
/// meeting on it, as CSS requires: every radius shrinks by the same factor.
pub(super) fn clamp_radii(rect: Rect, radii: Radii) -> Radii {
    let c = corners(radii);
    let sides = [
        (rect.w, c[0].0 + c[1].0),
        (rect.w, c[3].0 + c[2].0),
        (rect.h, c[0].1 + c[3].1),
        (rect.h, c[1].1 + c[2].1),
    ];
    let factor = sides
        .into_iter()
        .filter(|&(_, sum)| sum > 0.0)
        .map(|(side, sum)| side.max(0.0) / sum)
        .fold(1.0_f32, f32::min);
    radii_of(c.map(|(rx, ry)| (rx * factor, ry * factor)))
}

/// The radii multiplied per axis, for moving them into device pixels.
pub(super) fn scale_radii(radii: Radii, sx: f32, sy: f32) -> Radii {
    let mut scaled = radii;
    for pair in scaled.as_chunks_mut::<2>().0 {
        pair[0] *= sx;
        pair[1] *= sy;
    }
    scaled
}

/// The radii of the edge inset by `by`. A border shrinks each corner by the
/// widths of the two sides it sits against, never below zero.
pub(super) fn inset_radii(radii: Radii, by: Edges) -> Radii {
    let shrink = |(rx, ry): (f32, f32), dx: f32, dy: f32| ((rx - dx).max(0.0), (ry - dy).max(0.0));
    let c = corners(radii);
    radii_of([
        shrink(c[0], by.left, by.top),
        shrink(c[1], by.right, by.top),
        shrink(c[2], by.right, by.bottom),
        shrink(c[3], by.left, by.bottom),
    ])
}

pub(super) fn is_rounded(radii: &Radii) -> bool {
    radii.iter().any(|&r| r > 0.0)
}

/// Appends the closed contour of a rounded rectangle to `pb`. Radii must
/// already be clamped; contours added to one builder are filled together by
/// the caller's fill rule.
pub(super) fn push_rounded_rect(pb: &mut PathBuilder, rect: Rect, radii: Radii) {
    let [tl, tr, br, bl] = corners(radii);
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
    let arc = |pb: &mut PathBuilder, from: (f32, f32), c1: (f32, f32), c2: (f32, f32), to| {
        if from == to {
            return;
        }
        pb.cubic_to(c1.0, c1.1, c2.0, c2.1, to.0, to.1);
    };
    pb.move_to(x0 + tl.0, y0);
    pb.line_to(x1 - tr.0, y0);
    arc(
        pb,
        (x1 - tr.0, y0),
        (x1 - tr.0 + KAPPA * tr.0, y0),
        (x1, y0 + tr.1 - KAPPA * tr.1),
        (x1, y0 + tr.1),
    );
    pb.line_to(x1, y1 - br.1);
    arc(
        pb,
        (x1, y1 - br.1),
        (x1, y1 - br.1 + KAPPA * br.1),
        (x1 - br.0 + KAPPA * br.0, y1),
        (x1 - br.0, y1),
    );
    pb.line_to(x0 + bl.0, y1);
    arc(
        pb,
        (x0 + bl.0, y1),
        (x0 + bl.0 - KAPPA * bl.0, y1),
        (x0, y1 - bl.1 + KAPPA * bl.1),
        (x0, y1 - bl.1),
    );
    pb.line_to(x0, y0 + tl.1);
    arc(
        pb,
        (x0, y0 + tl.1),
        (x0, y0 + tl.1 - KAPPA * tl.1),
        (x0 + tl.0 - KAPPA * tl.0, y0),
        (x0 + tl.0, y0),
    );
    pb.close();
}

/// A convex polygon of at most eight vertices, kept inline so that clipping a
/// border side to its mitre allocates nothing.
#[derive(Copy, Clone, Debug)]
pub(super) struct Poly {
    points: [(f32, f32); Poly::CAPACITY],
    len: usize,
}

impl Poly {
    const CAPACITY: usize = 8;

    /// A polygon through `points`; extra points beyond the capacity are dropped.
    pub(super) fn new(points: &[(f32, f32)]) -> Poly {
        let mut poly = Poly {
            points: [(0.0, 0.0); Poly::CAPACITY],
            len: 0,
        };
        for &point in points.iter().take(Poly::CAPACITY) {
            poly.points[poly.len] = point;
            poly.len += 1;
        }
        poly
    }

    pub(super) fn points(&self) -> &[(f32, f32)] {
        &self.points[..self.len]
    }

    /// The part on the `keep` side of the line through `origin` with direction
    /// `dir`. A degenerate line or a `keep` point on it constrains nothing.
    pub(super) fn clipped(&self, origin: (f32, f32), dir: (f32, f32), keep: (f32, f32)) -> Poly {
        let signed = |p: (f32, f32)| dir.0 * (p.1 - origin.1) - dir.1 * (p.0 - origin.0);
        let sign = signed(keep);
        if (dir.0 == 0.0 && dir.1 == 0.0) || sign == 0.0 {
            return *self;
        }
        let side = |p| signed(p) * sign.signum();
        let mut out = Poly::new(&[]);
        let mut push = |p: (f32, f32)| {
            if out.len < Poly::CAPACITY {
                out.points[out.len] = p;
                out.len += 1;
            }
        };
        let points = self.points();
        for (i, &a) in points.iter().enumerate() {
            let b = points[(i + 1) % points.len()];
            let (sa, sb) = (side(a), side(b));
            if sa >= 0.0 {
                push(a);
            }
            // A vertex on the line is its own crossing and is already kept.
            if (sa >= 0.0) != (sb >= 0.0) && sa != 0.0 && sb != 0.0 {
                let s = sa / (sa - sb);
                push((a.0 + (b.0 - a.0) * s, a.1 + (b.1 - a.1) * s));
            }
        }
        out
    }
}

/// Appends the closed contour of `poly` to `pb`.
pub(super) fn push_polygon(pb: &mut PathBuilder, poly: &Poly) {
    let Some((&(x, y), rest)) = poly.points().split_first() else {
        return;
    };
    pb.move_to(x, y);
    for &(x, y) in rest {
        pb.line_to(x, y);
    }
    pb.close();
}

/// The corners of `rect` in top-left, top-right, bottom-right, bottom-left order.
pub(super) fn rect_corners(rect: Rect) -> [(f32, f32); 4] {
    [
        (rect.x, rect.y),
        (rect.right(), rect.y),
        (rect.right(), rect.bottom()),
        (rect.x, rect.bottom()),
    ]
}

fn corners(radii: Radii) -> Corners {
    [
        (radii[0], radii[1]),
        (radii[2], radii[3]),
        (radii[4], radii[5]),
        (radii[6], radii[7]),
    ]
}

fn radii_of(c: Corners) -> Radii {
    [
        c[0].0, c[0].1, c[1].0, c[1].1, c[2].0, c[2].1, c[3].0, c[3].1,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_round_half_up_and_size_is_the_difference_of_edges() {
        let axis = Axis::of(Transform::identity()).unwrap();
        // 10.3 -> 10 and 10.3 + 4.3 = 14.6 -> 15: width 5, not round(4.3) = 4.
        let snapped = axis.rect(Rect::new(10.3, 0.5, 4.3, 1.0));
        assert_eq!((snapped.x0, snapped.x1), (10, 15));
        assert_eq!((snapped.y0, snapped.y1), (1, 2));
    }

    #[test]
    fn a_mirrored_axis_keeps_the_rectangle_ordered() {
        let axis = Axis::of(Transform::from_row(-1.0, 0.0, 0.0, 1.0, 10.0, 0.0)).unwrap();
        let snapped = axis.rect(Rect::new(2.0, 0.0, 3.0, 1.0));
        assert_eq!((snapped.x0, snapped.x1), (5, 8));
    }

    #[test]
    fn rotated_transforms_have_no_axis() {
        assert!(Axis::of(Transform::from_row(0.0, 1.0, -1.0, 0.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn overlapping_radii_shrink_together() {
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let clamped = clamp_radii(rect, [80.0, 80.0, 80.0, 80.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(clamped[0], 50.0);
        assert_eq!(clamped[1], 50.0);
        assert_eq!(clamped[2], 50.0);
        let untouched = clamp_radii(rect, [10.0; 8]);
        assert_eq!(untouched, [10.0; 8]);
    }

    #[test]
    fn inner_radii_lose_the_border_widths_they_sit_against() {
        let radii = [10.0, 10.0, 4.0, 4.0, 0.0, 0.0, 0.0, 0.0];
        let inner = inset_radii(radii, Edges::all(6.0));
        assert_eq!(inner[0], 4.0);
        assert_eq!(inner[2], 0.0);
    }

    #[test]
    fn clipping_a_square_by_a_diagonal_keeps_the_triangle() {
        let square = Poly::new(&[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]);
        let half = square.clipped((0.0, 0.0), (1.0, 1.0), (4.0, 0.0));
        assert_eq!(half.points().len(), 3);
        assert!(half.points().contains(&(4.0, 0.0)));
        assert!(!half.points().contains(&(0.0, 4.0)));
    }
}
