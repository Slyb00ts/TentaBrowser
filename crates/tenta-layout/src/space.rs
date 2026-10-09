//! Constraint spaces and the results of block and inline layout.
//!
//! Block layout builds a [`ConstraintSpace`] for each child and receives a
//! [`BlockResult`]; inline layout receives the space of its container and
//! returns an [`InlineResult`]. Floats cross the block/inline boundary through
//! [`FloatContext`].

use smallvec::SmallVec;

use crate::fragment::FragId;
use crate::types::{ClearSide, Direction, FloatSide, Rect, WritingMode};

/// The space offered along one axis.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum AvailableSize {
    /// A definite size in CSS pixels.
    Definite(f32),
    /// Size under a min-content constraint.
    MinContent,
    /// Size under a max-content constraint.
    MaxContent,
}

/// What a parent tells a child about the space it may occupy.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct ConstraintSpace {
    /// Inline-axis space; for horizontal text the width of the containing
    /// block's content box.
    pub inline_size: AvailableSize,
    /// Block-axis space.
    pub block_size: AvailableSize,
    /// `(width, height)` percentages resolve against; `None` is indefinite.
    pub percentage_base: (Option<f32>, Option<f32>),
    /// The inline size is already decided (flex items, tables, abspos).
    pub fixed_inline: bool,
    /// The block size is already decided.
    pub fixed_block: bool,
    /// Position of this content box inside its block formatting context;
    /// floats use it to translate exclusions.
    pub bfc_offset: (f32, f32),
    /// Inline base direction of the containing block.
    pub direction: Direction,
    /// Writing mode of the containing block.
    pub writing_mode: WritingMode,
}

impl ConstraintSpace {
    /// A horizontal left-to-right space with the given sizes, indefinite
    /// percentage bases, no fixed axes and a zero BFC offset.
    pub fn new(inline_size: AvailableSize, block_size: AvailableSize) -> Self {
        Self {
            inline_size,
            block_size,
            percentage_base: (None, None),
            fixed_inline: false,
            fixed_block: false,
            bfc_offset: (0.0, 0.0),
            direction: Direction::Ltr,
            writing_mode: WritingMode::HorizontalTb,
        }
    }
}

/// The accumulated margins of a vertical margin-collapsing chain: the largest
/// positive and the most negative adjoining margin.
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct MarginStrut {
    /// Largest adjoining positive margin (at least zero).
    pub positive: f32,
    /// Most negative adjoining margin (at most zero).
    pub negative: f32,
}

impl MarginStrut {
    /// Adds an adjoining margin.
    pub fn append(&mut self, margin: f32) {
        if margin >= 0.0 {
            self.positive = self.positive.max(margin);
        } else {
            self.negative = self.negative.min(margin);
        }
    }

    /// The collapsed margin: CSS 2.1 section 8.3.1 sums the largest positive
    /// and the most negative margin.
    pub fn collapsed(&self) -> f32 {
        self.positive + self.negative
    }

    /// Merges another strut that adjoins this one.
    pub fn merge(&mut self, other: &MarginStrut) {
        self.positive = self.positive.max(other.positive);
        self.negative = self.negative.min(other.negative);
    }
}

/// Free inline range at a block position: how much of each content-box edge
/// floats occupy.
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct Band {
    /// Distance from the left edge that is occupied.
    pub left: f32,
    /// Distance from the right edge that is occupied.
    pub right: f32,
}

/// A float's margin box as an exclusion area.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct FloatExclusion {
    /// Side the float sits on.
    pub side: FloatSide,
    /// Area in the producing block's content-box coordinates.
    pub rect: Rect,
}

/// Where floats sit, as seen by inline layout. Block layout supplies the real
/// implementation; [`NoFloats`] serves callers without floats.
pub trait FloatContext {
    /// The occupied inline range for `block_size` starting at `block_offset`.
    fn band(&self, block_offset: f32, block_size: f32) -> Band;

    /// Places a float of `size` (width, height) on `side` no higher than
    /// `at_least`; returns its `(inline, block)` offset.
    fn place_float(&mut self, side: FloatSide, size: (f32, f32), at_least: f32) -> (f32, f32);

    /// Block offset below the floats `clear` names.
    fn clear_offset(&self, clear: ClearSide) -> f32;
}

/// A [`FloatContext`] with no floats: every band is free and a float lands
/// at the inline start of the requested block offset.
#[derive(Copy, Clone, Default, Debug)]
pub struct NoFloats;

impl FloatContext for NoFloats {
    fn band(&self, _block_offset: f32, _block_size: f32) -> Band {
        Band::default()
    }

    fn place_float(&mut self, _side: FloatSide, _size: (f32, f32), at_least: f32) -> (f32, f32) {
        (0.0, at_least)
    }

    fn clear_offset(&self, _clear: ClearSide) -> f32 {
        0.0
    }
}

/// What inline layout returns for one inline formatting context.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineResult {
    /// Line fragments; `rect.y` is relative to the context's content box.
    pub lines: SmallVec<[FragId; 8]>,
    /// Total height of the lines.
    pub block_size: f32,
    /// Baseline of the first line, from the content-box top.
    pub first_baseline: Option<f32>,
    /// Baseline of the last line, from the content-box top.
    pub last_baseline: Option<f32>,
    /// Scrollable overflow of the lines.
    pub overflow: Rect,
    /// Width of the widest line, for shrink-to-fit sizing.
    pub inline_extent: f32,
}

/// What block layout returns for one block box.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockResult {
    /// The block's fragment.
    pub frag: FragId,
    /// Margins adjoining the block's top edge, for the parent to collapse.
    pub margin_before: MarginStrut,
    /// Margins adjoining the block's bottom edge.
    pub margin_after: MarginStrut,
    /// First baseline from the border-box top.
    pub first_baseline: Option<f32>,
    /// Last baseline from the border-box top.
    pub last_baseline: Option<f32>,
    /// Floats extending past this block's bottom edge, so a following sibling
    /// can wrap around them without reopening this type.
    pub floats_out: Vec<FloatExclusion>,
    /// Exclusion areas this block's formatting context exposes to its parent.
    pub exclusions: Vec<FloatExclusion>,
    /// The block has no content, height, padding or border, so its top and
    /// bottom margins collapse through it into one adjoining margin.
    pub self_collapsing: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margin_strut_collapses_per_css_2_1() {
        let mut strut = MarginStrut::default();
        for m in [20.0, 10.0, -5.0, -8.0] {
            strut.append(m);
        }
        assert_eq!(strut.positive, 20.0);
        assert_eq!(strut.negative, -8.0);
        assert_eq!(strut.collapsed(), 12.0);

        let mut only_negative = MarginStrut::default();
        only_negative.append(-3.0);
        only_negative.append(-7.0);
        assert_eq!(only_negative.collapsed(), -7.0);
    }

    #[test]
    fn margin_strut_merge_keeps_extremes() {
        let mut a = MarginStrut {
            positive: 4.0,
            negative: -1.0,
        };
        a.merge(&MarginStrut {
            positive: 2.0,
            negative: -6.0,
        });
        assert_eq!(a.collapsed(), -2.0);
    }

    #[test]
    fn constraint_space_defaults_are_horizontal_ltr() {
        let space = ConstraintSpace::new(AvailableSize::Definite(800.0), AvailableSize::MaxContent);
        assert_eq!(space.direction, Direction::Ltr);
        assert_eq!(space.writing_mode, WritingMode::HorizontalTb);
        assert_eq!(space.percentage_base, (None, None));
        assert!(!space.fixed_inline && !space.fixed_block);
    }

    #[test]
    fn no_floats_leaves_every_band_free() {
        let mut floats = NoFloats;
        assert_eq!(floats.band(0.0, 100.0), Band::default());
        assert_eq!(
            floats.place_float(FloatSide::Left, (10.0, 10.0), 7.0),
            (0.0, 7.0)
        );
        assert_eq!(floats.clear_offset(ClearSide::Both), 0.0);
    }
}
