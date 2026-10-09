//! Tier 3: box tree, block/inline/flex/grid/table/positioned layout, text and form controls.
//!
//! The pipeline is box tree ([`BoxTree`]) to fragment tree ([`FragmentTree`]).
//! A fragment carries a [`PaintStyle`] whose definition is Stylo-free, so paint
//! code and hand-built fragment trees never name `ComputedValues`. The *crate*
//! still depends on Stylo (the box tree stores full computed styles), so
//! `tenta-paint`, which depends on this crate for its value types, links Stylo
//! transitively. Dependency direction is enforced by `cargo xtask lint-deps`
//! (`docs/spec/02-architecture.md` §2).

mod block;
mod block_sizes;
mod box_tree;
mod boxes;
mod constraint;
mod container;
#[cfg(feature = "embedded-fonts")]
pub mod embedded_fonts;
mod flex;
mod float;
mod font;
mod fragment;
mod grid;
mod inline;
mod layout;
mod list;
mod quirks;
mod relative;
mod space;
mod style_taffy;
mod taffy_tree;
mod text;
pub mod types;

pub use block::layout_block;
pub use block_sizes::intrinsic_inline_sizes_of_block;
pub use box_tree::{
    BoxFlags, BoxId, BoxKind, BoxTree, ImageInfo, InlineFormattingContext, InlineItem, LayoutBox,
    LayoutCx, NoImages, TextSource,
};
pub use font::FontSystem;
#[cfg(feature = "embedded-fonts")]
pub use font::{ResolvedFont, VerticalMetrics};
pub use fragment::{
    FragFlags, FragId, Fragment, FragmentBuilder, FragmentKind, FragmentTree, TextFragment,
};
pub use inline::{intrinsic_inline_content_sizes, layout_inline};
pub use layout::layout;
pub use space::{
    AvailableSize, Band, BlockResult, ConstraintSpace, FloatContext, FloatExclusion, InlineResult,
    MarginStrut, NoFloats,
};
pub use types::*;
