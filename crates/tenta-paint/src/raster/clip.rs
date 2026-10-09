//! The active clip and the scratch masks used to shape single draws.
//!
//! A clip is a device-pixel rectangle plus, only when it is rounded or sits
//! under a rotation, a coverage mask. A pixel-aligned rectangle needs no mask:
//! rectangles are intersected with its bounds, and a mask for it is built
//! lazily and only for draws that cannot be cut geometrically (text and paths).

use std::cell::OnceCell;
use std::rc::Rc;

use tiny_skia::{FillRule, IntSize, Mask, Path, Transform};

use super::geometry::IRect;

/// A clip in the pixel space of the current target.
pub(super) struct ClipState {
    bounds: IRect,
    canvas: IRect,
    /// Coverage of the clip shape, zero outside `bounds`.
    mask: Option<Mask>,
    /// `bounds` as a mask, for a draw that needs one while `mask` is absent.
    bounds_mask: OnceCell<Mask>,
}

impl ClipState {
    /// No clipping beyond the canvas.
    pub(super) fn open(canvas: IRect) -> Rc<ClipState> {
        Rc::new(ClipState {
            bounds: canvas,
            canvas,
            mask: None,
            bounds_mask: OnceCell::new(),
        })
    }

    /// Pixels outside this rectangle are never touched.
    pub(super) fn bounds(&self) -> IRect {
        self.bounds
    }

    /// The coverage of a rounded or rotated clip. Rectangle draws apply the
    /// bounds themselves and need only this.
    pub(super) fn shape(&self) -> Option<&Mask> {
        self.mask.as_ref()
    }

    /// A mask that clips a draw confined to `area`, or `None` when `area`
    /// lies wholly inside an unshaped clip and needs none.
    pub(super) fn mask_for(&self, area: IRect) -> Option<&Mask> {
        if self.mask.is_some() {
            return self.mask.as_ref();
        }
        if self.bounds.contains(area.intersect(self.canvas)) {
            return None;
        }
        Some(self.bounds_mask.get_or_init(|| {
            let mut mask = new_mask(self.canvas);
            fill_region(&mut mask, self.bounds, 255);
            mask
        }))
    }

    /// This clip narrowed to the device rectangle `rect`.
    pub(super) fn restrict_to_rect(self: &Rc<Self>, rect: IRect) -> Rc<ClipState> {
        let bounds = self.bounds.intersect(rect);
        if bounds == self.bounds {
            return Rc::clone(self);
        }
        let mask = self.mask.as_ref().map(|parent| {
            let mut mask = parent.clone();
            zero_outside(&mut mask, bounds);
            mask
        });
        Rc::new(ClipState {
            bounds,
            canvas: self.canvas,
            mask,
            bounds_mask: OnceCell::new(),
        })
    }

    /// This clip narrowed to `path` drawn through `transform`, which lies
    /// within the device rectangle `area`.
    pub(super) fn restrict_to_path(
        self: &Rc<Self>,
        path: &Path,
        transform: Transform,
        area: IRect,
    ) -> Rc<ClipState> {
        let mask = match &self.mask {
            Some(parent) => {
                let mut mask = parent.clone();
                mask.intersect_path(path, FillRule::Winding, true, transform);
                mask
            }
            None => {
                let mut mask = new_mask(self.canvas);
                mask.fill_path(path, FillRule::Winding, true, transform);
                if self.bounds != self.canvas {
                    zero_outside(&mut mask, self.bounds);
                }
                mask
            }
        };
        Rc::new(ClipState {
            bounds: self.bounds.intersect(area),
            canvas: self.canvas,
            mask: Some(mask),
            bounds_mask: OnceCell::new(),
        })
    }

    /// The part of this clip inside `region`, re-based so that `region`'s
    /// top-left becomes the origin of a smaller canvas. `region` must lie
    /// within the bounds, so nothing of the clip remains but its mask.
    pub(super) fn cropped(&self, region: IRect) -> Rc<ClipState> {
        let canvas = IRect::new(0, 0, region.width(), region.height());
        let mask = self.mask.as_ref().map(|parent| {
            let width = region.width() as usize;
            let stride = parent.width() as usize;
            let mut data = Vec::with_capacity(width * region.height() as usize);
            for y in region.y0..region.y1 {
                let start = y as usize * stride + region.x0 as usize;
                data.extend_from_slice(&parent.data()[start..start + width]);
            }
            Mask::from_vec(
                data,
                IntSize::from_wh(region.width() as u32, region.height() as u32)
                    .expect("a layer region has a positive size"),
            )
            .expect("the data holds exactly width times height bytes")
        });
        Rc::new(ClipState {
            bounds: canvas,
            canvas,
            mask,
            bounds_mask: OnceCell::new(),
        })
    }
}

/// A reusable mask for shaping one draw, such as a border side clipped to its
/// mitre. Only the touched region is cleared between uses, so the cost follows
/// the size of the draw instead of the size of the canvas.
pub(super) struct Scratch {
    mask: Option<Mask>,
    dirty: IRect,
}

impl Scratch {
    pub(super) fn new() -> Self {
        Self {
            mask: None,
            dirty: IRect::EMPTY,
        }
    }

    /// Empties the mask, making it match `canvas` first when the target changed.
    pub(super) fn reset(&mut self, canvas: IRect) {
        match &mut self.mask {
            Some(mask)
                if mask.width() as i32 == canvas.width()
                    && mask.height() as i32 == canvas.height() =>
            {
                fill_region(mask, self.dirty, 0);
            }
            _ => self.mask = Some(new_mask(canvas)),
        }
        self.dirty = IRect::EMPTY;
    }

    /// Adds the coverage of `path` within `area`, which must hold its bounds.
    pub(super) fn add_path(
        &mut self,
        path: &Path,
        rule: FillRule,
        anti_alias: bool,
        transform: Transform,
        area: IRect,
    ) {
        let Some(mask) = &mut self.mask else { return };
        mask.fill_path(path, rule, anti_alias, transform);
        self.dirty = self.dirty.union(area);
    }

    /// Multiplies the mask by `other` over `area`.
    pub(super) fn intersect(&mut self, other: &Mask, area: IRect) {
        let Some(mask) = &mut self.mask else { return };
        let area = area.intersect(self.dirty);
        if area.is_empty() {
            return;
        }
        let stride = mask.width() as usize;
        let (x0, x1) = (area.x0 as usize, area.x1 as usize);
        for y in area.y0 as usize..area.y1 as usize {
            let row = y * stride;
            let ours = &mut mask.data_mut()[row + x0..row + x1];
            for (a, &b) in ours.iter_mut().zip(&other.data()[row + x0..row + x1]) {
                *a = mul_u8(*a, b);
            }
        }
    }

    pub(super) fn mask(&self) -> Option<&Mask> {
        self.mask.as_ref()
    }
}

/// `a * b / 255`, rounded, as tiny-skia multiplies coverage.
fn mul_u8(a: u8, b: u8) -> u8 {
    let product = u32::from(a) * u32::from(b) + 128;
    ((product + (product >> 8)) >> 8) as u8
}

fn new_mask(canvas: IRect) -> Mask {
    Mask::new(canvas.width() as u32, canvas.height() as u32).expect("a canvas has a positive size")
}

/// Sets `region` (clamped to the mask) to `value`.
fn fill_region(mask: &mut Mask, region: IRect, value: u8) {
    let region = region.intersect(IRect::new(0, 0, mask.width() as i32, mask.height() as i32));
    if region.is_empty() {
        return;
    }
    let stride = mask.width() as usize;
    for y in region.y0 as usize..region.y1 as usize {
        let row = y * stride;
        mask.data_mut()[row + region.x0 as usize..row + region.x1 as usize].fill(value);
    }
}

/// Zeroes everything outside `keep`.
fn zero_outside(mask: &mut Mask, keep: IRect) {
    let width = mask.width() as usize;
    let height = mask.height() as usize;
    let keep = keep.intersect(IRect::new(0, 0, width as i32, height as i32));
    if keep.is_empty() {
        mask.clear();
        return;
    }
    let data = mask.data_mut();
    data[..keep.y0 as usize * width].fill(0);
    data[keep.y1 as usize * width..].fill(0);
    for y in keep.y0 as usize..keep.y1 as usize {
        let row = y * width;
        data[row..row + keep.x0 as usize].fill(0);
        data[row + keep.x1 as usize..row + width].fill(0);
    }
}
