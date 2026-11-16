//! Collapsed borders (CSS 2.1 section 17.6.2): each grid line gets one
//! winning border, and the two cells beside it paint half of it each.
//!
//! The result is stored in the boxes' paint styles: a cell's border widths are
//! half of the winners around it, the table's are half of the outermost ones,
//! and rows, row groups and columns paint no border of their own.

use std::sync::Arc;

use crate::box_tree::{BoxId, BoxTree};
use crate::table::grid::TableGrid;
use crate::types::{BorderStyle, Color, PaintStyle};

/// Grids bigger than this many slots keep their own borders: the edge tables
/// would cost more than the answer is worth.
const MAX_SLOTS: usize = 1 << 21;

/// One side of a box as a candidate for a grid line.
#[derive(Copy, Clone, Debug)]
struct Border {
    style: BorderStyle,
    width: f32,
    color: Color,
    /// Cell 5, row 4, row group 3, column 2, column group 1, table 0.
    origin: u8,
}

impl Border {
    const NONE: Border = Border {
        style: BorderStyle::None,
        width: 0.0,
        color: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        },
        origin: 0,
    };

    fn of(tree: &BoxTree, id: BoxId, side: usize, origin: u8) -> Border {
        let border = &tree.get(id).paint.border;
        let width = [
            border.widths.top,
            border.widths.right,
            border.widths.bottom,
            border.widths.left,
        ][side];
        Border {
            style: border.styles[side],
            width,
            color: border.colors[side],
            origin,
        }
    }

    fn rank(style: BorderStyle) -> u8 {
        match style {
            BorderStyle::Double => 7,
            BorderStyle::Solid => 6,
            BorderStyle::Dashed => 5,
            BorderStyle::Dotted => 4,
            BorderStyle::Ridge => 3,
            BorderStyle::Outset => 2,
            BorderStyle::Groove => 1,
            BorderStyle::Inset => 0,
            BorderStyle::None | BorderStyle::Hidden => 0,
        }
    }

    /// Whether `self` beats `other` (CSS 2.1 17.6.2.1: hidden wins, then the
    /// wider border, then the more prominent style, then the closer origin).
    fn beats(&self, other: &Border) -> bool {
        if self.style == BorderStyle::Hidden {
            return other.style != BorderStyle::Hidden || self.origin > other.origin;
        }
        if other.style == BorderStyle::Hidden {
            return false;
        }
        let shown = |b: &Border| !matches!(b.style, BorderStyle::None) && b.width > 0.0;
        match (shown(self), shown(other)) {
            (false, _) => false,
            (true, false) => true,
            (true, true) => {
                if self.width != other.width {
                    return self.width > other.width;
                }
                if self.style != other.style {
                    return Self::rank(self.style) > Self::rank(other.style);
                }
                self.origin > other.origin
            }
        }
    }

    fn merge(slot: &mut Border, candidate: Border) {
        if candidate.beats(slot) {
            *slot = candidate;
        }
    }

    fn drawn_width(&self) -> f32 {
        if matches!(self.style, BorderStyle::None | BorderStyle::Hidden) {
            0.0
        } else {
            self.width
        }
    }
}

/// Resolves the collapsed borders of table `table` and rewrites the paint
/// styles of the table and its parts.
pub(crate) fn resolve(tree: &mut BoxTree, grid: &TableGrid, table: BoxId) {
    let (nrows, ncols) = (grid.rows.len(), grid.columns);
    if nrows == 0 || ncols == 0 || (nrows + 1).saturating_mul(ncols + 1) > MAX_SLOTS {
        return;
    }
    // Vertical lines: `vertical[row * (ncols + 1) + column]`; horizontal
    // lines: `horizontal[row * ncols + column]` for row in 0..=nrows.
    let mut vertical = vec![Border::NONE; nrows * (ncols + 1)];
    let mut horizontal = vec![Border::NONE; (nrows + 1) * ncols];
    const TOP: usize = 0;
    const RIGHT: usize = 1;
    const BOTTOM: usize = 2;
    const LEFT: usize = 3;

    let v = |row: usize, column: usize| row * (ncols + 1) + column;
    let h = |row: usize, column: usize| row * ncols + column;

    // Table: its four sides on the outer lines.
    for row in 0..nrows {
        Border::merge(&mut vertical[v(row, 0)], Border::of(tree, table, LEFT, 0));
        Border::merge(
            &mut vertical[v(row, ncols)],
            Border::of(tree, table, RIGHT, 0),
        );
    }
    for column in 0..ncols {
        Border::merge(
            &mut horizontal[h(0, column)],
            Border::of(tree, table, TOP, 0),
        );
        Border::merge(
            &mut horizontal[h(nrows, column)],
            Border::of(tree, table, BOTTOM, 0),
        );
    }
    // Column groups and columns: their left and right sides on every row.
    for (list, origin) in [(&grid.column_groups, 1u8), (&grid.column_boxes, 2u8)] {
        for col in list {
            let left = Border::of(tree, col.box_id, LEFT, origin);
            let right = Border::of(tree, col.box_id, RIGHT, origin);
            let end = col.columns.end.min(ncols);
            if col.columns.start >= ncols {
                continue;
            }
            for row in 0..nrows {
                Border::merge(&mut vertical[v(row, col.columns.start)], left);
                Border::merge(&mut vertical[v(row, end)], right);
            }
        }
    }
    // Row groups and rows.
    for section in &grid.sections {
        if section.rows.is_empty() {
            continue;
        }
        let top = Border::of(tree, section.box_id, TOP, 3);
        let bottom = Border::of(tree, section.box_id, BOTTOM, 3);
        let left = Border::of(tree, section.box_id, LEFT, 3);
        let right = Border::of(tree, section.box_id, RIGHT, 3);
        for column in 0..ncols {
            Border::merge(&mut horizontal[h(section.rows.start, column)], top);
            Border::merge(&mut horizontal[h(section.rows.end, column)], bottom);
        }
        for row in section.rows.clone() {
            Border::merge(&mut vertical[v(row, 0)], left);
            Border::merge(&mut vertical[v(row, ncols)], right);
        }
    }
    for (index, row) in grid.rows.iter().enumerate() {
        let top = Border::of(tree, row.box_id, TOP, 4);
        let bottom = Border::of(tree, row.box_id, BOTTOM, 4);
        for column in 0..ncols {
            Border::merge(&mut horizontal[h(index, column)], top);
            Border::merge(&mut horizontal[h(index + 1, column)], bottom);
        }
        Border::merge(
            &mut vertical[v(index, 0)],
            Border::of(tree, row.box_id, LEFT, 4),
        );
        Border::merge(
            &mut vertical[v(index, ncols)],
            Border::of(tree, row.box_id, RIGHT, 4),
        );
    }
    // Cells.
    for cell in &grid.cells {
        let rows = grid.cell_rows(cell);
        let columns = cell.columns();
        let top = Border::of(tree, cell.box_id, TOP, 5);
        let bottom = Border::of(tree, cell.box_id, BOTTOM, 5);
        let left = Border::of(tree, cell.box_id, LEFT, 5);
        let right = Border::of(tree, cell.box_id, RIGHT, 5);
        for column in columns.clone() {
            Border::merge(&mut horizontal[h(rows.start, column)], top);
            Border::merge(&mut horizontal[h(rows.end, column)], bottom);
        }
        for row in rows {
            Border::merge(&mut vertical[v(row, columns.start)], left);
            Border::merge(&mut vertical[v(row, columns.end)], right);
        }
    }

    // Cells keep half of the line on each side, taken at their first slot.
    for cell in &grid.cells {
        let rows = grid.cell_rows(cell);
        let columns = cell.columns();
        let sides = [
            horizontal[h(rows.start, columns.start)],
            vertical[v(rows.start, columns.end)],
            horizontal[h(rows.end, columns.start)],
            vertical[v(rows.start, columns.start)],
        ];
        set_borders(tree, cell.box_id, &sides, 0.5);
    }
    // The table keeps half of the widest outer segment on each side.
    let widest = |segments: &mut dyn Iterator<Item = Border>| {
        let mut best = Border::NONE;
        for segment in segments {
            if segment.drawn_width() > best.drawn_width() {
                best = segment;
            }
        }
        best
    };
    let sides = [
        widest(&mut (0..ncols).map(|c| horizontal[h(0, c)])),
        widest(&mut (0..nrows).map(|r| vertical[v(r, ncols)])),
        widest(&mut (0..ncols).map(|c| horizontal[h(nrows, c)])),
        widest(&mut (0..nrows).map(|r| vertical[v(r, 0)])),
    ];
    set_borders(tree, table, &sides, 0.5);

    // Everything else paints nothing: the lines belong to the cells.
    let silent: Vec<BoxId> = grid
        .rows
        .iter()
        .map(|r| r.box_id)
        .chain(grid.sections.iter().map(|s| s.box_id))
        .chain(grid.column_boxes.iter().map(|c| c.box_id))
        .chain(grid.column_groups.iter().map(|c| c.box_id))
        .collect();
    for id in silent {
        set_borders(tree, id, &[Border::NONE; 4], 0.0);
    }
}

fn set_borders(tree: &mut BoxTree, id: BoxId, sides: &[Border; 4], scale: f32) {
    let mut paint: PaintStyle = (*tree.get(id).paint).clone();
    let border = &mut paint.border;
    let width = |b: &Border| b.drawn_width() * scale;
    border.widths.top = width(&sides[0]);
    border.widths.right = width(&sides[1]);
    border.widths.bottom = width(&sides[2]);
    border.widths.left = width(&sides[3]);
    for (i, side) in sides.iter().enumerate() {
        border.colors[i] = side.color;
        border.styles[i] = if side.drawn_width() > 0.0 {
            side.style
        } else {
            BorderStyle::None
        };
    }
    tree.get_mut(id).paint = Arc::new(paint);
}
