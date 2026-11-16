//! Column constraints and the distribution of the table's width over them
//! (CSS Tables 3 "Distributing width to columns", as Blink implements it).

use style::values::computed::{MaxSize, Size};

use crate::block_sizes::{frame_of, intrinsic_inline_sizes_of_block};
use crate::box_tree::{BoxId, BoxTree, LayoutCx};
use crate::table::grid::TableGrid;

/// What one column asks for. Sizes are border-box, so a column holds the
/// padding and borders of its cells.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Column {
    pub(crate) min: f32,
    pub(crate) max: f32,
    /// Percentage of the table's track space, 0 to 100.
    pub(crate) percent: Option<f32>,
    /// A cell or `col` of the column has a definite width.
    pub(crate) fixed: bool,
    /// Fixed layout only: the width the first row or a `col` gave.
    pub(crate) width: Option<f32>,
}

/// A cell or column width request.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum Request {
    Auto,
    Length(f32),
    Percent(f32),
}

/// The `width` of a box as a request. A length is a content-box size unless
/// the box is `border-box`; `edges` converts it.
pub(crate) fn width_request(tree: &BoxTree, id: BoxId, edges: f32) -> Request {
    let style = &tree.get(id).style;
    let pos = style.get_position();
    let Size::LengthPercentage(lp) = &pos.width else {
        return Request::Auto;
    };
    if let Some(p) = lp.0.to_percentage() {
        return Request::Percent(p.0 * 100.0);
    }
    match lp.0.to_length() {
        Some(l) => {
            let border_box = matches!(
                pos.box_sizing,
                style::properties::longhands::box_sizing::computed_value::T::BorderBox
            );
            let v = l.px().max(0.0);
            Request::Length(if border_box { v } else { v + edges })
        }
        None => Request::Auto,
    }
}

/// The border-box `(min, max, request)` of one cell.
pub(crate) fn cell_sizes(cx: &mut LayoutCx, tree: &BoxTree, cell: BoxId) -> (f32, f32, Request) {
    let (min, max) = intrinsic_inline_sizes_of_block(cx, tree, cell);
    let frame = frame_of(tree, cell, 0.0);
    let edges = frame.padding.horizontal() + frame.border.horizontal();
    let (mut min, mut max) = (min + edges, max + edges);
    let request = width_request(tree, cell, edges);
    let pos = tree.get(cell).style.get_position();
    let border_box = matches!(
        pos.box_sizing,
        style::properties::longhands::box_sizing::computed_value::T::BorderBox
    );
    let to_border_box = |v: f32| v + if border_box { 0.0 } else { edges };
    if let MaxSize::LengthPercentage(lp) = &pos.max_width
        && let Some(l) = lp.0.to_length()
    {
        // The limit wins over the content, even below its minimum.
        let limit = to_border_box(l.px());
        min = min.min(limit);
        max = max.min(limit);
    }
    if let Size::LengthPercentage(lp) = &pos.min_width
        && let Some(l) = lp.0.to_length()
    {
        let v = to_border_box(l.px());
        min = min.max(v);
        max = max.max(v);
    }
    // A definite `width` replaces the content's maximum; it never goes below
    // the minimum.
    if let Request::Length(w) = request {
        max = w.max(min);
    }
    (min, max.max(min), request)
}

/// Column constraints of an automatic table layout.
pub(crate) fn auto_columns(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    grid: &TableGrid,
    spacing: f32,
) -> Vec<Column> {
    let mut columns = vec![Column::default(); grid.columns];
    column_requests(tree, grid, &mut columns);
    let mut spanning: Vec<(usize, usize, f32, f32)> = Vec::new();
    for cell in &grid.cells {
        let (min, max, request) = cell_sizes(cx, tree, cell.box_id);
        if cell.colspan == 1 {
            let col = &mut columns[cell.column];
            col.min = col.min.max(min);
            col.max = col.max.max(max);
            match request {
                Request::Length(_) => col.fixed = true,
                Request::Percent(p) => col.percent = Some(col.percent.map_or(p, |q| q.max(p))),
                Request::Auto => {}
            }
        } else {
            let _ = request;
            spanning.push((cell.column, cell.colspan, min, max));
        }
    }
    for col in &mut columns {
        col.max = col.max.max(col.min);
    }
    // Narrow spans first, so a wide one finds the columns already sized.
    spanning.sort_by_key(|&(column, span, ..)| (span, column));
    for (column, span, min, max) in spanning {
        let range = column..column + span;
        let gaps = spacing * (span as f32 - 1.0);
        let slice = columns[range.clone()].to_vec();
        let widths = distribute((min - gaps).max(0.0), &slice);
        for (col, w) in columns[range.clone()].iter_mut().zip(widths) {
            col.min = col.min.max(w);
            col.max = col.max.max(col.min);
        }
        let slice = columns[range.clone()].to_vec();
        let widths = distribute((max - gaps).max(0.0), &slice);
        for (col, w) in columns[range.clone()].iter_mut().zip(widths) {
            col.max = col.max.max(w).max(col.min);
        }
    }
    columns
}

/// `col` widths, as if they were a cell of that column.
fn column_requests(tree: &BoxTree, grid: &TableGrid, columns: &mut [Column]) {
    for col in &grid.column_boxes {
        let box_id = col.box_id;
        let request = width_request(tree, box_id, 0.0);
        for index in col.columns.clone() {
            let Some(column) = columns.get_mut(index) else {
                break;
            };
            match request {
                Request::Length(w) => {
                    column.fixed = true;
                    column.max = column.max.max(w);
                }
                Request::Percent(p) => column.percent = Some(p),
                Request::Auto => {}
            }
        }
    }
    for group in &grid.column_groups {
        // A group with columns of its own leaves the widths to them.
        if grid.column_boxes.iter().any(|c| c.box_id == group.box_id) {
            let request = width_request(tree, group.box_id, 0.0);
            if let Request::Length(w) = request {
                for index in group.columns.clone() {
                    if let Some(column) = columns.get_mut(index) {
                        column.fixed = true;
                        column.max = column.max.max(w);
                    }
                }
            }
        }
    }
}

/// Column constraints of `table-layout: fixed`: only `col` elements and the
/// first row say anything.
pub(crate) fn fixed_columns(cx: &mut LayoutCx, tree: &BoxTree, grid: &TableGrid) -> Vec<Column> {
    let mut columns = vec![Column::default(); grid.columns];
    for col in &grid.column_boxes {
        let request = width_request(tree, col.box_id, 0.0);
        for index in col.columns.clone() {
            let Some(column) = columns.get_mut(index) else {
                break;
            };
            match request {
                Request::Length(w) => column.width = Some(w),
                Request::Percent(p) => column.percent = Some(p),
                Request::Auto => {}
            }
        }
    }
    if let Some(row) = grid.rows.first() {
        for cell in &grid.cells[row.cells.clone()] {
            let frame = frame_of(tree, cell.box_id, 0.0);
            let edges = frame.padding.horizontal() + frame.border.horizontal();
            let request = width_request(tree, cell.box_id, edges);
            let span = cell.colspan as f32;
            for index in cell.columns() {
                let column = &mut columns[index];
                if column.width.is_some() || column.percent.is_some() {
                    continue;
                }
                match request {
                    Request::Length(w) => column.width = Some(w / span),
                    Request::Percent(p) => column.percent = Some(p / span),
                    Request::Auto => {}
                }
            }
        }
    }
    let _ = cx;
    for column in &mut columns {
        if let Some(w) = column.width {
            column.min = w;
            column.max = w;
            column.fixed = true;
        }
    }
    columns
}

/// The border-box `(min, max)` of the grid's tracks: the columns alone, with
/// no spacing. Percentage columns push the maximum up until they would get
/// their share.
pub(crate) fn track_sizes(columns: &[Column]) -> (f32, f32) {
    let min: f32 = columns.iter().map(|c| c.min).sum();
    let mut max: f32 = columns.iter().map(|c| c.max).sum();
    let percents = effective_percents(columns);
    let percent_sum: f32 = percents.iter().flatten().sum();
    if percent_sum > 0.0 {
        let others: f32 = columns
            .iter()
            .filter(|c| c.percent.is_none())
            .map(|c| c.max)
            .sum();
        let mut needed = columns
            .iter()
            .zip(&percents)
            .filter_map(|(c, p)| p.filter(|&p| p > 0.0).map(|p| c.max * 100.0 / p))
            .fold(0.0_f32, f32::max);
        if percent_sum < 100.0 {
            needed = needed.max(others * 100.0 / (100.0 - percent_sum));
        }
        max = max.max(needed);
    }
    (min, max.max(min))
}

/// The percentages the columns get: they are honoured in order until the
/// table's 100 per cent are used up.
fn effective_percents(columns: &[Column]) -> Vec<Option<f32>> {
    let mut left = 100.0_f32;
    columns
        .iter()
        .map(|c| {
            c.percent.map(|p| {
                let used = p.clamp(0.0, left);
                left -= used;
                used
            })
        })
        .collect()
}

/// Gives `target` pixels to `columns` (CSS Tables 3, 4.? "distributing width
/// to columns"): the widths move through four guesses (minimum, minimum with
/// percentages, with fixed widths, maximum) and, past the last, the surplus
/// goes to the automatic columns by their maximum.
pub(crate) fn distribute(target: f32, columns: &[Column]) -> Vec<f32> {
    let n = columns.len();
    if n == 0 {
        return Vec::new();
    }
    let percents = effective_percents(columns);
    let share = |i: usize| percents[i].map(|p| p / 100.0 * target);
    let min_guess: Vec<f32> = columns.iter().map(|c| c.min).collect();
    let percent_guess: Vec<f32> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| share(i).map_or(c.min, |s| s.max(c.min)))
        .collect();
    let specified_guess: Vec<f32> = columns
        .iter()
        .zip(&percent_guess)
        .map(|(c, &p)| {
            if c.percent.is_some() {
                p
            } else if c.fixed {
                c.min.max(c.max)
            } else {
                c.min
            }
        })
        .collect();
    let max_guess: Vec<f32> = columns
        .iter()
        .zip(&percent_guess)
        .map(|(c, &p)| {
            if c.percent.is_some() {
                p.max(c.max)
            } else {
                c.max.max(c.min)
            }
        })
        .collect();

    let sum = |v: &[f32]| v.iter().sum::<f32>();
    let ladder = [&min_guess, &percent_guess, &specified_guess, &max_guess];
    if target <= sum(&min_guess) {
        return min_guess;
    }
    for pair in ladder.windows(2) {
        let (low, high) = (pair[0], pair[1]);
        let (low_sum, high_sum) = (sum(low), sum(high));
        if target < high_sum {
            let t = (target - low_sum) / (high_sum - low_sum);
            return low
                .iter()
                .zip(high.iter())
                .map(|(&a, &b)| a + (b - a) * t)
                .collect();
        }
    }

    let excess = target - sum(&max_guess);
    let mut widths = max_guess;
    let auto: Vec<usize> = (0..n)
        .filter(|&i| columns[i].percent.is_none() && !columns[i].fixed)
        .collect();
    let fixed: Vec<usize> = (0..n)
        .filter(|&i| columns[i].percent.is_none() && columns[i].fixed)
        .collect();
    let percent: Vec<usize> = (0..n).filter(|&i| columns[i].percent.is_some()).collect();
    let (targets, weights): (Vec<usize>, Vec<f32>) = if !auto.is_empty() {
        let w = auto.iter().map(|&i| columns[i].max).collect();
        (auto, w)
    } else if !fixed.is_empty() {
        let w = fixed.iter().map(|&i| columns[i].max).collect();
        (fixed, w)
    } else if !percent.is_empty() {
        let w = percent
            .iter()
            .map(|&i| percents[i].unwrap_or(0.0))
            .collect();
        (percent, w)
    } else {
        ((0..n).collect(), vec![1.0; n])
    };
    let total: f32 = weights.iter().sum();
    for (k, &i) in targets.iter().enumerate() {
        let part = if total > 0.0 {
            weights[k] / total
        } else {
            1.0 / targets.len() as f32
        };
        widths[i] += excess * part;
    }
    widths
}

/// Widths of a fixed layout: percentages and given widths keep theirs and the
/// columns without one share what is left. With nothing left to share the
/// given widths grow in proportion.
pub(crate) fn distribute_fixed(target: f32, columns: &[Column]) -> Vec<f32> {
    let n = columns.len();
    let mut widths: Vec<f32> = columns
        .iter()
        .map(|c| match (c.width, c.percent) {
            (Some(w), _) => w,
            (None, Some(p)) => p / 100.0 * target,
            (None, None) => 0.0,
        })
        .collect();
    let auto = columns
        .iter()
        .filter(|c| c.width.is_none() && c.percent.is_none())
        .count();
    let given: f32 = widths.iter().sum();
    if auto > 0 {
        let share = ((target - given) / auto as f32).max(0.0);
        for (w, c) in widths.iter_mut().zip(columns) {
            if c.width.is_none() && c.percent.is_none() {
                *w = share;
            }
        }
    } else if given > 0.0 && given < target {
        let k = target / given;
        for w in &mut widths {
            *w *= k;
        }
    } else if given == 0.0 && n > 0 {
        widths.fill(target / n as f32);
    }
    widths
}
