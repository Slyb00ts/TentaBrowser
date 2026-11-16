//! Laying out a table box: widths, cells, rows and fragments.

use smallvec::SmallVec;
use style::computed_values::border_collapse::T as BorderCollapse;
use style::computed_values::caption_side::T as CaptionSide;
use style::computed_values::empty_cells::T as EmptyCells;
use style::computed_values::table_layout::T as TableLayout;
use style::properties::ComputedValues;
use style::properties::longhands::box_sizing::computed_value::T as BoxSizing;
use style::values::computed::{Length, LengthPercentage, Margin, MaxSize, Size};
use style::values::generics::box_::{BaselineShift, BaselineShiftKeyword};
use style::values::specified::box_::DisplayInside;
use style::values::specified::text::TextAlignKeyword;

use crate::block::{child_overflow, fragment_flags, layout_block, relative_shift};
use crate::block_sizes::{frame_of, intrinsic_inline_contribution};
use crate::box_tree::{BoxFlags, BoxId, BoxTree, InlineItem, LayoutCx, TextSource};
use crate::constraint::{InlineEquation, own_direction};
use crate::fragment::{FragId, Fragment, FragmentBuilder, FragmentKind};
use crate::space::{AvailableSize, BlockResult, ConstraintSpace, MarginStrut};
use crate::table::columns::{
    Column, auto_columns, distribute, distribute_fixed, fixed_columns, track_sizes,
};
use crate::table::grid::{Section, TableGrid};
use crate::types::{Direction, Edges, PaintStyle, Rect};

/// Style facts of a table box that decide its algorithm.
struct TableStyle {
    collapsed: bool,
    spacing_h: f32,
    spacing_v: f32,
    fixed: bool,
}

impl TableStyle {
    fn of(style: &ComputedValues) -> Self {
        let table = style.get_inherited_table();
        let collapsed = table.border_collapse == BorderCollapse::Collapse;
        let (spacing_h, spacing_v) = if collapsed {
            (0.0, 0.0)
        } else {
            (
                table.border_spacing.0.width().px(),
                table.border_spacing.0.height().px(),
            )
        };
        let fixed = style.get_table().table_layout == TableLayout::Fixed
            && !matches!(style.get_position().width, Size::Auto);
        Self {
            collapsed,
            spacing_h,
            spacing_v,
            fixed,
        }
    }
}

/// Where a cell puts its content when the row is taller.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum CellAlign {
    Top,
    Middle,
    Bottom,
    Baseline,
}

fn cell_align(style: &ComputedValues) -> CellAlign {
    let b = style.get_box();
    match &b.baseline_shift {
        BaselineShift::Keyword(BaselineShiftKeyword::Top) => CellAlign::Top,
        BaselineShift::Keyword(BaselineShiftKeyword::Bottom) => CellAlign::Bottom,
        BaselineShift::Keyword(BaselineShiftKeyword::Center) => CellAlign::Middle,
        BaselineShift::Length(lp) if lp.to_length().is_some_and(|l| l.px() == 0.0) => {
            if b.alignment_baseline == style::values::computed::AlignmentBaseline::Middle {
                CellAlign::Middle
            } else {
                CellAlign::Baseline
            }
        }
        _ => CellAlign::Baseline,
    }
}

fn grid_of(tree: &BoxTree, id: BoxId) -> &TableGrid {
    static EMPTY: std::sync::OnceLock<TableGrid> = std::sync::OnceLock::new();
    tree.get(id)
        .table
        .as_deref()
        .unwrap_or_else(|| EMPTY.get_or_init(TableGrid::default))
}

fn columns_of(cx: &mut LayoutCx, tree: &BoxTree, id: BoxId, t: &TableStyle) -> Vec<Column> {
    let grid = grid_of(tree, id);
    if t.fixed {
        fixed_columns(cx, tree, grid)
    } else {
        auto_columns(cx, tree, grid, t.spacing_h)
    }
}

/// Border and padding of the table box; a collapsed table has no padding.
fn table_edges(tree: &BoxTree, id: BoxId, t: &TableStyle, basis: f32) -> (Edges, Edges) {
    let frame = frame_of(tree, id, basis);
    let padding = if t.collapsed {
        Edges::default()
    } else {
        frame.padding
    };
    (frame.border, padding)
}

fn spacing_total(grid: &TableGrid, spacing: f32) -> f32 {
    if grid.columns == 0 {
        0.0
    } else {
        spacing * (grid.columns as f32 + 1.0)
    }
}

/// Border-box `(min-content, max-content)` widths of the table box: the
/// tracks, the spacing, the box's own border and padding, and the
/// min-content width of its captions.
pub(crate) fn border_box_sizes(cx: &mut LayoutCx, tree: &BoxTree, id: BoxId) -> (f32, f32) {
    let t = TableStyle::of(&tree.get(id).style);
    let grid = grid_of(tree, id);
    let columns = columns_of(cx, tree, id, &t);
    sizes_with(cx, tree, id, &t, grid, &columns)
}

fn sizes_with(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    t: &TableStyle,
    grid: &TableGrid,
    columns: &[Column],
) -> (f32, f32) {
    let (border, padding) = table_edges(tree, id, t, 0.0);
    let edges = border.horizontal() + padding.horizontal();
    let (track_min, track_max) = track_sizes(columns);
    let spacing = spacing_total(grid, t.spacing_h);
    let mut min = track_min + spacing + edges;
    let mut max = track_max + spacing + edges;
    for &caption in &grid.captions {
        let (caption_min, _) = intrinsic_inline_contribution(cx, tree, caption);
        let margin = frame_of(tree, caption, 0.0).margin;
        let wanted = caption_min + margin.horizontal();
        min = min.max(wanted);
        max = max.max(wanted);
    }
    (min, max.max(min))
}

/// Content-box `(min, max)` of the table box, for the callers that size boxes
/// from their content.
pub(crate) fn intrinsic_inline_sizes(cx: &mut LayoutCx, tree: &BoxTree, id: BoxId) -> (f32, f32) {
    let t = TableStyle::of(&tree.get(id).style);
    let (border, padding) = table_edges(tree, id, &t, 0.0);
    let edges = border.horizontal() + padding.horizontal();
    let (min, max) = border_box_sizes(cx, tree, id);
    ((min - edges).max(0.0), (max - edges).max(0.0))
}

fn length_of(lp: &LengthPercentage, basis: Option<f32>) -> Option<f32> {
    if lp.has_percentage() {
        basis.map(|b| lp.resolve(Length::new(b)).px())
    } else {
        lp.to_length().map(Length::px)
    }
}

/// The border-box width the `width` of table `id` asks for, when it is a
/// length (or a percentage of a known `basis`).
fn specified_width(tree: &BoxTree, id: BoxId, edges: f32, basis: Option<f32>) -> Option<f32> {
    let pos = tree.get(id).style.get_position();
    let Size::LengthPercentage(lp) = &pos.width else {
        return None;
    };
    let v = length_of(&lp.0, basis)?.max(0.0);
    Some(if matches!(pos.box_sizing, BoxSizing::BorderBox) {
        v
    } else {
        v + edges
    })
}

/// `(min-width, max-width)` of the table as border-box lengths.
fn width_limits(tree: &BoxTree, id: BoxId, edges: f32, basis: Option<f32>) -> (f32, f32) {
    let pos = tree.get(id).style.get_position();
    let border_box = matches!(pos.box_sizing, BoxSizing::BorderBox);
    let convert = |v: f32| if border_box { v } else { v + edges };
    let min = match &pos.min_width {
        Size::LengthPercentage(lp) => length_of(&lp.0, basis).map_or(0.0, |v| convert(v.max(0.0))),
        _ => 0.0,
    };
    let max = match &pos.max_width {
        MaxSize::LengthPercentage(lp) => {
            length_of(&lp.0, basis).map_or(f32::INFINITY, |v| convert(v.max(0.0)))
        }
        _ => f32::INFINITY,
    };
    (min, max)
}

/// Border-box `(min, max)` contribution of the table to its container's
/// intrinsic width, margins excluded.
pub(crate) fn contribution(cx: &mut LayoutCx, tree: &BoxTree, id: BoxId) -> (f32, f32) {
    let t = TableStyle::of(&tree.get(id).style);
    let (border, padding) = table_edges(tree, id, &t, 0.0);
    let edges = border.horizontal() + padding.horizontal();
    let (min, max) = border_box_sizes(cx, tree, id);
    let pos = tree.get(id).style.get_position();
    let (mut lo, mut hi) = match &pos.width {
        Size::MinContent => (min, min),
        Size::MaxContent => (max, max),
        _ => match specified_width(tree, id, edges, None) {
            Some(w) => (w.max(min), w.max(min)),
            None => (min, max),
        },
    };
    let (min_width, max_width) = width_limits(tree, id, edges, None);
    lo = lo.min(max_width).max(min_width).max(min);
    hi = hi.min(max_width).max(min_width).max(min);
    (lo, hi)
}

fn margin_of(margin: &Margin, basis: f32) -> Option<f32> {
    match margin {
        Margin::Auto => None,
        Margin::LengthPercentage(lp) => Some(length_of(lp, Some(basis)).unwrap_or(0.0)),
        _ => Some(0.0),
    }
}

/// The `height` of a box as a border-box length, when it is one.
fn specified_height(style: &ComputedValues, edges: f32, basis: Option<f32>) -> Option<f32> {
    let pos = style.get_position();
    let Size::LengthPercentage(lp) = &pos.height else {
        return None;
    };
    let v = length_of(&lp.0, basis)?.max(0.0);
    Some(if matches!(pos.box_sizing, BoxSizing::BorderBox) {
        v
    } else {
        v + edges
    })
}

fn is_empty_cell(cx: &LayoutCx, tree: &BoxTree, id: BoxId) -> bool {
    let b = tree.get(id);
    if !b.children.is_empty() {
        return false;
    }
    let Some(ifc) = &b.ifc else { return true };
    ifc.items.iter().all(|item| match item {
        InlineItem::Text {
            source: TextSource::Node { node, range },
            ..
        } => cx
            .doc
            .text_data(*node)
            .and_then(|t| t.get(range.start as usize..range.end as usize))
            .is_some_and(|t| {
                t.bytes()
                    .all(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
            }),
        InlineItem::Text {
            source: TextSource::Owned(text),
            ..
        } => text.trim().is_empty(),
        InlineItem::OpenInline { .. } | InlineItem::CloseInline { .. } => true,
        _ => false,
    })
}

/// A paint style with the box's decoration (background, borders, shadows)
/// removed.
fn without_decoration(paint: &PaintStyle) -> PaintStyle {
    let mut p = paint.clone();
    p.background.clear();
    p.box_shadow.clear();
    p.border = Default::default();
    p
}

/// Splits the table's paint style between the wrapper (everything that
/// applies to the box with its captions) and the grid box (its decoration).
fn split_paint(paint: &PaintStyle) -> (PaintStyle, PaintStyle) {
    let wrapper = without_decoration(paint);
    let grid = PaintStyle {
        background: paint.background.clone(),
        border: paint.border.clone(),
        box_shadow: paint.box_shadow.clone(),
        visibility_hidden: paint.visibility_hidden,
        pointer_events: paint.pointer_events,
        color: paint.color,
        ..PaintStyle::default()
    };
    (wrapper, grid)
}

/// What laying out one cell at its final width produced.
struct CellOut {
    frag: FragId,
    /// Natural border-box height.
    height: f32,
    /// First baseline from the border-box top.
    baseline: Option<f32>,
    /// Minimum border-box height from the cell's `height`.
    min_height: f32,
    align: CellAlign,
    /// The inline offset `position: relative` gives the cell.
    shift: (f32, f32),
}

/// Lays out table box `id` inside `space`; same contract as `layout_block`.
pub(crate) fn layout_table(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
    out: &mut FragmentBuilder,
) -> BlockResult {
    let b = tree.get(id);
    let t = TableStyle::of(&b.style);
    let grid = grid_of(tree, id);
    let direction = own_direction(&b.style);
    let definite = match space.inline_size {
        AvailableSize::Definite(w) => Some(w),
        _ => None,
    };
    let basis = space.percentage_base.0.or(definite);
    let (border, padding) = table_edges(tree, id, &t, basis.unwrap_or(0.0));
    let edges_h = border.horizontal() + padding.horizontal();
    let edges_v = border.vertical() + padding.vertical();

    let columns = columns_of(cx, tree, id, &t);
    let (min_bb, max_bb) = sizes_with(cx, tree, id, &t, grid, &columns);

    // Used border-box width and horizontal margins.
    let style = &b.style;
    let margin_style = style.get_margin();
    let margin_basis = basis.unwrap_or(0.0);
    let margin_left = margin_of(&margin_style.margin_left, margin_basis);
    let margin_right = margin_of(&margin_style.margin_right, margin_basis);
    let (used, margin_l, margin_r) = if space.fixed_inline {
        let w = definite.unwrap_or(max_bb);
        (w, margin_left.unwrap_or(0.0), margin_right.unwrap_or(0.0))
    } else {
        let pos = style.get_position();
        let available = match space.inline_size {
            AvailableSize::Definite(w) => w,
            AvailableSize::MinContent => 0.0,
            AvailableSize::MaxContent => f32::INFINITY,
        };
        let fit = (available - margin_left.unwrap_or(0.0) - margin_right.unwrap_or(0.0))
            .max(min_bb)
            .min(max_bb);
        let wanted = match &pos.width {
            Size::MinContent => min_bb,
            Size::MaxContent => max_bb,
            Size::FitContent => fit,
            _ => specified_width(tree, id, edges_h, basis).map_or(fit, |w| w.max(min_bb)),
        };
        let (min_width, max_width) = width_limits(tree, id, edges_h, basis);
        let w = wanted.min(max_width).max(min_width).max(min_bb);
        let equation = InlineEquation {
            available: definite.unwrap_or(w),
            edges: 0.0,
            margin_left,
            margin_right,
            direction: space.direction,
        };
        let (w, mut l, mut r) = equation.solve(Some(w));
        // Legacy alignment (`<center>`, `align`): with no auto margin, a
        // parent whose `text-align` is `-webkit-center` or `-webkit-right`
        // places the margin box itself.
        if let (Some(ml), Some(mr), Some(room), false) = (
            margin_left,
            margin_right,
            definite,
            b.flags
                .intersects(BoxFlags::INLINE_LEVEL | BoxFlags::OUT_OF_FLOW),
        ) && b.parent != BoxId::NONE
        {
            let free = (room - w - ml - mr).max(0.0);
            match tree.get(b.parent).style.get_inherited_text().text_align {
                TextAlignKeyword::MozCenter => {
                    l = ml + free / 2.0;
                    r = mr + free / 2.0;
                }
                TextAlignKeyword::MozRight => {
                    l = ml + free;
                    r = mr;
                }
                _ => {}
            }
        }
        (w, l, r)
    };

    // Column widths.
    let inner = (used - edges_h).max(0.0);
    let track_space = (inner - spacing_total(grid, t.spacing_h)).max(0.0);
    let widths = if t.fixed {
        distribute_fixed(track_space, &columns)
    } else {
        distribute(track_space, &columns)
    };
    let origin_x = border.left + padding.left + t.spacing_h;
    let mut column_x = Vec::with_capacity(widths.len());
    let mut x = 0.0;
    for &w in &widths {
        column_x.push(x);
        x += w + t.spacing_h;
    }
    let track_total = if widths.is_empty() {
        0.0
    } else {
        x - t.spacing_h
    };
    // Position of column `i` from the left edge of the table's content.
    let col_left = |i: usize| -> f32 {
        let rel = column_x[i];
        if direction == Direction::Rtl {
            track_total - rel - widths[i]
        } else {
            rel
        }
    };
    let span_width = |first: usize, count: usize| -> f32 {
        widths[first..first + count].iter().sum::<f32>() + t.spacing_h * (count as f32 - 1.0)
    };

    // Captions.
    let caption_space = ConstraintSpace {
        inline_size: AvailableSize::Definite(used),
        block_size: AvailableSize::MaxContent,
        percentage_base: (Some(used), None),
        fixed_inline: false,
        fixed_block: false,
        content_block_size: false,
        bfc_offset: (0.0, 0.0),
        direction,
        writing_mode: space.writing_mode,
    };
    let mut top_captions: Vec<(FragId, f32, f32)> = Vec::new();
    let mut bottom_captions: Vec<(FragId, f32, f32)> = Vec::new();
    for &caption in &grid.captions {
        let result = layout_block(cx, tree, caption, &caption_space, out);
        let side = tree.get(caption).style.get_inherited_table().caption_side;
        let entry = (
            result.frag,
            result.margin_before.collapsed(),
            result.margin_after.collapsed(),
        );
        if side == CaptionSide::Bottom {
            bottom_captions.push(entry);
        } else {
            top_captions.push(entry);
        }
    }

    // Cells at their final widths.
    let table_height_basis = space.percentage_base.1;
    let mut cells: Vec<CellOut> = Vec::with_capacity(grid.cells.len());
    for cell in &grid.cells {
        let w = span_width(cell.column, cell.colspan);
        let cell_space = ConstraintSpace {
            inline_size: AvailableSize::Definite(w),
            block_size: AvailableSize::MaxContent,
            percentage_base: (Some(w), None),
            fixed_inline: true,
            fixed_block: false,
            content_block_size: true,
            bfc_offset: (0.0, 0.0),
            direction,
            writing_mode: space.writing_mode,
        };
        let result = layout_block(cx, tree, cell.box_id, &cell_space, out);
        let frag = out.get_mut(result.frag);
        // Table cells have no margins.
        let shift = (frag.rect.x - frag.margin.left, frag.rect.y);
        frag.margin = Edges::default();
        frag.rect.x = 0.0;
        frag.rect.y = 0.0;
        let height = frag.rect.h;
        let vertical = frag.padding.vertical() + frag.border.vertical();
        let min_height =
            specified_height(&tree.get(cell.box_id).style, vertical, None).unwrap_or(0.0);
        cells.push(CellOut {
            frag: result.frag,
            height,
            baseline: result.first_baseline,
            min_height,
            align: cell_align(&tree.get(cell.box_id).style),
            shift,
        });
    }

    // Row heights.
    let nrows = grid.rows.len();
    let mut row_height = vec![0.0_f32; nrows];
    let mut row_ascent: Vec<Option<f32>> = vec![None; nrows];
    let fixed_row: Vec<Option<f32>> = grid
        .rows
        .iter()
        .map(|row| specified_height(&tree.get(row.box_id).style, 0.0, None))
        .collect();
    for (r, row) in grid.rows.iter().enumerate() {
        let mut ascent = 0.0_f32;
        let mut descent = 0.0_f32;
        let mut other = fixed_row[r].unwrap_or(0.0);
        let mut has_baseline = false;
        for index in row.cells.clone() {
            if grid.cells[index].rowspan != 1 {
                continue;
            }
            let c = &cells[index];
            let h = c.height.max(c.min_height);
            if c.align == CellAlign::Baseline {
                let base = c.baseline.unwrap_or(c.height).min(h);
                has_baseline = true;
                ascent = ascent.max(base);
                descent = descent.max(h - base);
            } else {
                other = other.max(h);
            }
        }
        if has_baseline {
            row_ascent[r] = Some(ascent);
        }
        row_height[r] = other.max(ascent + descent);
    }
    let mut spanning: Vec<usize> = (0..grid.cells.len())
        .filter(|&i| grid.cells[i].rowspan > 1)
        .collect();
    spanning.sort_by_key(|&i| (grid.cells[i].rowspan, grid.cells[i].row));
    for index in spanning {
        let cell = &grid.cells[index];
        let rows = grid.cell_rows(cell);
        let gaps = t.spacing_v * (cell.rowspan as f32 - 1.0);
        let have: f32 = row_height[rows.clone()].iter().sum::<f32>() + gaps;
        let need = cells[index].height.max(cells[index].min_height);
        if need > have {
            distribute_rows(&mut row_height[rows], need - have);
        }
    }

    // The table's own height.
    let spacing_rows = if nrows == 0 {
        0.0
    } else {
        t.spacing_v * (nrows as f32 + 1.0)
    };
    let natural_grid = row_height.iter().sum::<f32>() + spacing_rows + edges_v;
    let pos = style.get_position();
    let asked = specified_height(style, edges_v, table_height_basis);
    let min_asked = match &pos.min_height {
        Size::LengthPercentage(lp) => length_of(&lp.0, table_height_basis).map(|v| {
            if matches!(pos.box_sizing, BoxSizing::BorderBox) {
                v
            } else {
                v + edges_v
            }
        }),
        _ => None,
    };
    let wanted_total = asked.into_iter().chain(min_asked).fold(0.0_f32, f32::max);
    let grid_height = if wanted_total > natural_grid && nrows > 0 {
        let extra = wanted_total - natural_grid;
        grow_rows(tree, grid, &fixed_row, &mut row_height, extra);
        row_height.iter().sum::<f32>() + spacing_rows + edges_v
    } else {
        natural_grid
    };

    // Vertical positions, grid-relative.
    let mut section_frags: Vec<FragId> = Vec::with_capacity(grid.sections.len());
    let section_width = if grid.columns == 0 {
        inner
    } else {
        track_total
    };
    let mut y = border.top + padding.top + t.spacing_v;
    let mut row_top = vec![0.0_f32; nrows];
    let mut first_baseline: Option<f32> = None;
    let mut section_extent: Option<(f32, f32)> = None;
    for section in &grid.sections {
        let section_top = y;
        let mut row_frags: Vec<FragId> = Vec::with_capacity(section.rows.len());
        for r in section.rows.clone() {
            row_top[r] = y;
            let row = &grid.rows[r];
            let mut kids: SmallVec<[FragId; 4]> = SmallVec::new();
            for index in row.cells.clone() {
                let cell = &grid.cells[index];
                let c = &cells[index];
                let rows = grid.cell_rows(cell);
                let span_h = row_height[rows.clone()].iter().sum::<f32>()
                    + t.spacing_v * (cell.rowspan as f32 - 1.0);
                let extra = (span_h - c.height).max(0.0);
                let dy = match c.align {
                    CellAlign::Top => 0.0,
                    CellAlign::Middle => extra / 2.0,
                    CellAlign::Bottom => extra,
                    CellAlign::Baseline => {
                        if cell.rowspan == 1 {
                            match (row_ascent[r], c.baseline) {
                                (Some(a), Some(base)) => (a - base).clamp(0.0, extra),
                                (Some(a), None) => (a - c.height).clamp(0.0, extra),
                                _ => 0.0,
                            }
                        } else {
                            0.0
                        }
                    }
                };
                if dy != 0.0 {
                    let children: SmallVec<[FragId; 4]> = out.get(c.frag).children.clone();
                    for kid in children {
                        out.get_mut(kid).rect.y += dy;
                    }
                }
                let hide = !t.collapsed
                    && tree
                        .get(cell.box_id)
                        .style
                        .get_inherited_table()
                        .empty_cells
                        == EmptyCells::Hide
                    && is_empty_cell(cx, tree, cell.box_id);
                let left = col_left(cell.column) + c.shift.0;
                let fragment = out.get_mut(c.frag);
                // A row with no baseline-aligned cell sits on the bottom of
                // its first cell's content box.
                if r == 0 && index == row.cells.start && row_ascent[0].is_none() {
                    let below = fragment.padding.bottom + fragment.border.bottom;
                    first_baseline = Some(y + span_h.max(c.height) - below);
                }
                fragment.rect = Rect::new(left, c.shift.1, fragment.rect.w, span_h.max(c.height));
                if hide {
                    fragment.paint = std::sync::Arc::new(without_decoration(&fragment.paint));
                }
                let kids_of_cell = fragment.children.clone();
                let border_box = Rect::new(0.0, 0.0, fragment.rect.w, fragment.rect.h);
                let overflow = child_overflow(out, &kids_of_cell, border_box);
                out.get_mut(c.frag).overflow = overflow;
                kids.push(c.frag);
            }
            let row_rect = Rect::new(0.0, y - section_top, section_width, row_height[r]);
            let mut frag = Fragment::new(
                tree.get(row.box_id).node,
                FragmentKind::Box,
                tree.paint_style_for(row.box_id, row_rect),
                row_rect,
            );
            frag.flags = fragment_flags(cx, tree, row.box_id);
            frag.overflow = child_overflow(out, &kids, Rect::new(0.0, 0.0, row_rect.w, row_rect.h));
            frag.children = kids;
            row_frags.push(out.push(frag));
            y += row_height[r] + t.spacing_v;
        }
        let section_h = if section.rows.is_empty() {
            0.0
        } else {
            y - t.spacing_v - section_top
        };
        let section_rect = Rect::new(origin_x, section_top, section_width, section_h);
        let mut frag = Fragment::new(
            tree.get(section.box_id).node,
            FragmentKind::Box,
            tree.paint_style_for(section.box_id, section_rect),
            section_rect,
        );
        frag.flags = fragment_flags(cx, tree, section.box_id);
        let kids: SmallVec<[FragId; 4]> = row_frags.into_iter().collect();
        frag.overflow = child_overflow(
            out,
            &kids,
            Rect::new(0.0, 0.0, section_rect.w, section_rect.h),
        );
        frag.children = kids;
        section_frags.push(out.push(frag));
        if !section.rows.is_empty() {
            let extent = section_extent.get_or_insert((section_top, section_top + section_h));
            extent.1 = section_top + section_h;
        }
    }
    if let (false, Some(a)) = (grid.rows.is_empty(), row_ascent.first().copied().flatten()) {
        first_baseline = Some(row_top[0] + a);
    } else if first_baseline.is_none() && !grid.rows.is_empty() {
        first_baseline = Some(row_top[0] + row_height[0]);
    }

    // Column and column-group boxes.
    let mut column_frags: Vec<FragId> = Vec::new();
    if let Some((top, bottom)) = section_extent {
        for list in [&grid.column_groups, &grid.column_boxes] {
            for col in list {
                let first = col.columns.start.min(widths.len().saturating_sub(1));
                let last = col.columns.end.min(widths.len());
                if widths.is_empty() || last <= first {
                    continue;
                }
                let left = (first..last).map(&col_left).fold(f32::INFINITY, f32::min);
                let right = (first..last)
                    .map(|i| col_left(i) + widths[i])
                    .fold(f32::NEG_INFINITY, f32::max);
                let rect = Rect::new(origin_x + left, top, right - left, bottom - top);
                let mut frag = Fragment::new(
                    tree.get(col.box_id).node,
                    FragmentKind::Box,
                    tree.paint_style_for(col.box_id, rect),
                    rect,
                );
                frag.flags = fragment_flags(cx, tree, col.box_id);
                column_frags.push(out.push(frag));
            }
        }
    }

    let top_height: f32 = top_captions
        .iter()
        .map(|&(frag, before, after)| out.get(frag).rect.h + before + after)
        .sum();
    let bottom_height: f32 = bottom_captions
        .iter()
        .map(|&(frag, before, after)| out.get(frag).rect.h + before + after)
        .sum();
    let total_height = top_height + grid_height + bottom_height;
    let has_captions = !top_captions.is_empty() || !bottom_captions.is_empty();
    let (dx, dy) = relative_shift(tree, id, space);

    let border_box = Rect::new(0.0, 0.0, used, total_height);
    let table_paint = tree.paint_style_for(id, border_box);
    let mut kids: SmallVec<[FragId; 4]> = SmallVec::new();
    let frag = if has_captions {
        let (wrapper_paint, grid_paint) = split_paint(&table_paint);
        let grid_rect = Rect::new(0.0, top_height, used, grid_height);
        let mut grid_kids: SmallVec<[FragId; 4]> = SmallVec::new();
        grid_kids.extend(column_frags.iter().copied());
        grid_kids.extend(section_frags.iter().copied());
        let mut grid_frag = Fragment::new(
            tenta_core::NodeId::NONE,
            FragmentKind::Box,
            std::sync::Arc::new(grid_paint),
            grid_rect,
        );
        grid_frag.border = border;
        grid_frag.padding = padding;
        grid_frag.overflow =
            child_overflow(out, &grid_kids, Rect::new(0.0, 0.0, used, grid_height));
        grid_frag.children = grid_kids;
        let grid_id = out.push(grid_frag);

        let mut cursor = 0.0;
        for &(frag, before, after) in &top_captions {
            cursor += before;
            let f = out.get_mut(frag);
            f.rect.y += cursor;
            cursor += f.rect.h + after;
            kids.push(frag);
        }
        kids.push(grid_id);
        let mut cursor = top_height + grid_height;
        for &(frag, before, after) in &bottom_captions {
            cursor += before;
            let f = out.get_mut(frag);
            f.rect.y += cursor;
            cursor += f.rect.h + after;
            kids.push(frag);
        }
        let mut wrapper = Fragment::new(
            b.node,
            FragmentKind::Box,
            std::sync::Arc::new(wrapper_paint),
            Rect::new(margin_l + dx, dy, used, total_height),
        );
        wrapper.margin = Edges {
            top: 0.0,
            right: margin_r,
            bottom: 0.0,
            left: margin_l,
        };
        wrapper.flags = fragment_flags(cx, tree, id);
        wrapper.overflow = child_overflow(out, &kids, border_box);
        wrapper.children = kids;
        out.push(wrapper)
    } else {
        kids.extend(column_frags.iter().copied());
        kids.extend(section_frags.iter().copied());
        let mut fragment = Fragment::new(
            b.node,
            FragmentKind::Box,
            table_paint,
            Rect::new(margin_l + dx, dy, used, total_height),
        );
        fragment.border = border;
        fragment.padding = padding;
        fragment.margin = Edges {
            top: 0.0,
            right: margin_r,
            bottom: 0.0,
            left: margin_l,
        };
        fragment.flags = fragment_flags(cx, tree, id);
        fragment.overflow = child_overflow(out, &kids, border_box);
        fragment.children = kids;
        out.push(fragment)
    };
    let top_margin = margin_of(&margin_style.margin_top, margin_basis).unwrap_or(0.0);
    let bottom_margin = margin_of(&margin_style.margin_bottom, margin_basis).unwrap_or(0.0);
    {
        let f = out.get_mut(frag);
        f.margin.top = top_margin;
        f.margin.bottom = bottom_margin;
    }
    let mut margin_before = MarginStrut::default();
    margin_before.append(top_margin);
    let mut margin_after = MarginStrut::default();
    margin_after.append(bottom_margin);
    let baseline = first_baseline.map(|b| b + top_height);
    // Only an inline-table sits on its first row's baseline; a block table
    // gives its container no last baseline.
    let atomic = b.flags.contains(BoxFlags::INLINE_LEVEL);
    BlockResult {
        frag,
        margin_before,
        margin_after,
        first_baseline: baseline,
        last_baseline: baseline.filter(|_| atomic),
        floats_out: Vec::new(),
        exclusions: Vec::new(),
        self_collapsing: false,
    }
}

/// Hands the surplus of a table taller than its rows to the rows: body rows
/// only (header and footer groups keep their height) and, among those, the
/// rows without a `height` of their own.
fn grow_rows(
    tree: &BoxTree,
    grid: &TableGrid,
    fixed_row: &[Option<f32>],
    heights: &mut [f32],
    extra: f32,
) {
    let is_body = |section: &Section| {
        !matches!(
            tree.get(section.box_id).style.get_box().display.inside(),
            DisplayInside::TableHeaderGroup | DisplayInside::TableFooterGroup
        )
    };
    let mut eligible: Vec<usize> = grid
        .sections
        .iter()
        .filter(|s| is_body(s))
        .flat_map(|s| s.rows.clone())
        .collect();
    if eligible.is_empty() {
        eligible = (0..heights.len()).collect();
    }
    let auto: Vec<usize> = eligible
        .iter()
        .copied()
        .filter(|&r| fixed_row[r].is_none())
        .collect();
    let targets = if auto.is_empty() { eligible } else { auto };
    let total: f32 = targets.iter().map(|&r| heights[r]).sum();
    let n = targets.len() as f32;
    for &r in &targets {
        heights[r] += if total > 0.0 {
            extra * heights[r] / total
        } else {
            extra / n
        };
    }
}

/// Adds `extra` to `heights` in proportion to what they have (equally when
/// all are empty).
fn distribute_rows(heights: &mut [f32], extra: f32) {
    let total: f32 = heights.iter().sum();
    let n = heights.len() as f32;
    for h in heights.iter_mut() {
        *h += if total > 0.0 {
            extra * *h / total
        } else {
            extra / n
        };
    }
}
