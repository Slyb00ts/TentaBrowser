//! CSS table layout (CSS 2.1 section 17, CSS Tables 3 where Blink follows it).
//!
//! The box tree builder normalises the boxes and stores a [`grid::TableGrid`]
//! on each table box (`boxes/builder/table.rs`); collapsed borders are
//! resolved there too (`collapse.rs`). Layout works on that grid: column
//! constraints and widths (`columns.rs`), cells laid out at their final
//! widths, row heights, then the fragments (`layout.rs`).

pub(crate) mod collapse;
mod columns;
pub(crate) mod grid;
mod layout;

pub(crate) use layout::{border_box_sizes, contribution, intrinsic_inline_sizes, layout_table};
