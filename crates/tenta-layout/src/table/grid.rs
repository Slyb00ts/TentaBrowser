//! The grid of a table: which cell sits in which row and column, and how far
//! it spans (HTML "forming a table", with CSS Tables 3 for the box kinds).

use std::ops::Range;

use style::values::specified::box_::DisplayInside;
use tenta_core::atom::atoms;
use tenta_dom::Document;

use crate::box_tree::{BoxFlags, BoxId, BoxKind, BoxTree};
use crate::types::TableKind;

/// Largest `colspan` and `span` (HTML clamps both to 1000).
const MAX_SPAN: usize = 1000;
/// Largest `rowspan` (HTML clamps it to 65534).
const MAX_ROWSPAN: usize = 65534;

/// A `col` or `colgroup` box and the columns it covers.
#[derive(Clone, Debug)]
pub(crate) struct ColumnBox {
    pub(crate) box_id: BoxId,
    pub(crate) columns: Range<usize>,
}

/// A row group in visual order.
#[derive(Clone, Debug)]
pub(crate) struct Section {
    pub(crate) box_id: BoxId,
    pub(crate) rows: Range<usize>,
}

/// A row of the grid.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub(crate) box_id: BoxId,
    /// The cells that start in this row, as indices into [`TableGrid::cells`].
    pub(crate) cells: Range<usize>,
}

/// A cell and the slots it covers.
#[derive(Clone, Debug)]
pub(crate) struct Cell {
    pub(crate) box_id: BoxId,
    pub(crate) row: usize,
    pub(crate) column: usize,
    pub(crate) colspan: usize,
    /// Rows covered, already cut at the end of the row group.
    pub(crate) rowspan: usize,
}

impl Cell {
    pub(crate) fn columns(&self) -> Range<usize> {
        self.column..self.column + self.colspan
    }
}

/// Everything layout needs to know about the structure of one table box.
#[derive(Clone, Debug, Default)]
pub(crate) struct TableGrid {
    pub(crate) columns: usize,
    pub(crate) captions: Vec<BoxId>,
    pub(crate) column_groups: Vec<ColumnBox>,
    pub(crate) column_boxes: Vec<ColumnBox>,
    pub(crate) sections: Vec<Section>,
    pub(crate) rows: Vec<Row>,
    pub(crate) cells: Vec<Cell>,
}

fn parse_span(doc: &Document, node: tenta_core::NodeId, name: &str, zero: usize) -> usize {
    if !node.is_some() {
        return 1;
    }
    let Some(value) = doc.attribute_str(node, name) else {
        return 1;
    };
    let digits: String = value
        .trim_start_matches([' ', '\t', '\n', '\r', '\x0c'])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    match digits.parse::<usize>() {
        Ok(0) => zero,
        Ok(n) => n,
        Err(_) if digits.is_empty() => 1,
        Err(_) => usize::MAX,
    }
}

fn is_html(doc: &Document, node: tenta_core::NodeId, tags: &[tenta_core::atom::Atom]) -> bool {
    node.is_some()
        && doc.namespace(node) == Some(atoms::NS_HTML)
        && doc.tag(node).is_some_and(|tag| tags.contains(&tag))
}

fn display_inside(tree: &BoxTree, id: BoxId) -> DisplayInside {
    tree.get(id).style.get_box().display.inside()
}

impl TableGrid {
    /// Reads the grid of table box `table`, whose children are already
    /// normalised (captions, column groups, columns, row groups of rows of
    /// cells).
    pub(crate) fn build(tree: &BoxTree, doc: &Document, table: BoxId) -> TableGrid {
        let mut grid = TableGrid::default();
        let mut sections: Vec<BoxId> = Vec::new();
        let mut declared = 0usize;
        for &child in &tree.get(table).children {
            let child_box = tree.get(child);
            if child_box.flags.contains(BoxFlags::OUT_OF_FLOW) {
                continue;
            }
            let BoxKind::Table(kind) = child_box.kind else {
                continue;
            };
            match kind {
                TableKind::Caption => grid.captions.push(child),
                TableKind::ColumnGroup => {
                    let start = declared;
                    let cols: Vec<BoxId> = child_box
                        .children
                        .iter()
                        .copied()
                        .filter(|&c| tree.get(c).kind == BoxKind::Table(TableKind::Column))
                        .collect();
                    if cols.is_empty() {
                        let span = parse_span(doc, child_box.node, "span", 1)
                            .clamp(1, MAX_SPAN)
                            .min(MAX_SPAN.saturating_sub(declared));
                        grid.column_boxes.push(ColumnBox {
                            box_id: child,
                            columns: declared..declared + span,
                        });
                        declared += span;
                    } else {
                        for col in cols {
                            let node = tree.get(col).node;
                            let span = parse_span(doc, node, "span", 1)
                                .clamp(1, MAX_SPAN)
                                .min(MAX_SPAN.saturating_sub(declared));
                            grid.column_boxes.push(ColumnBox {
                                box_id: col,
                                columns: declared..declared + span,
                            });
                            declared += span;
                        }
                    }
                    grid.column_groups.push(ColumnBox {
                        box_id: child,
                        columns: start..declared,
                    });
                }
                TableKind::Column => {
                    let span = parse_span(doc, child_box.node, "span", 1)
                        .clamp(1, MAX_SPAN)
                        .min(MAX_SPAN.saturating_sub(declared));
                    grid.column_boxes.push(ColumnBox {
                        box_id: child,
                        columns: declared..declared + span,
                    });
                    declared += span;
                }
                TableKind::RowGroup => sections.push(child),
                TableKind::Row | TableKind::Cell | TableKind::Table => {}
            }
        }

        // The first header group goes first and the first footer group last.
        let header = sections
            .iter()
            .position(|&s| display_inside(tree, s) == DisplayInside::TableHeaderGroup);
        let footer = sections
            .iter()
            .position(|&s| display_inside(tree, s) == DisplayInside::TableFooterGroup);
        let mut ordered: Vec<BoxId> = Vec::with_capacity(sections.len());
        ordered.extend(header.map(|i| sections[i]));
        ordered.extend(
            sections
                .iter()
                .enumerate()
                .filter(|(i, _)| Some(*i) != header && Some(*i) != footer)
                .map(|(_, &s)| s),
        );
        ordered.extend(footer.map(|i| sections[i]));

        let mut columns = declared;
        for section in ordered {
            grid.add_section(tree, doc, section, &mut columns);
        }
        grid.columns = columns;
        grid
    }

    fn add_section(&mut self, tree: &BoxTree, doc: &Document, section: BoxId, columns: &mut usize) {
        let first_row = self.rows.len();
        // Rows still to be covered, per column, by cells from earlier rows.
        let mut pending: Vec<usize> = Vec::new();
        let mut spanning: Vec<usize> = Vec::new();
        for &row_id in &tree.get(section).children {
            let row_box = tree.get(row_id);
            if row_box.flags.contains(BoxFlags::OUT_OF_FLOW)
                || row_box.kind != BoxKind::Table(TableKind::Row)
            {
                continue;
            }
            let row = self.rows.len();
            let first_cell = self.cells.len();
            let mut x = 0usize;
            for &cell_id in &row_box.children {
                let cell_box = tree.get(cell_id);
                if cell_box.flags.contains(BoxFlags::OUT_OF_FLOW)
                    || cell_box.kind != BoxKind::Table(TableKind::Cell)
                {
                    continue;
                }
                let node = cell_box.node;
                let html_cell = is_html(doc, node, &[atoms::TD, atoms::TH]);
                let (colspan, rowspan) = if html_cell {
                    (
                        parse_span(doc, node, "colspan", 1).clamp(1, MAX_SPAN),
                        parse_span(doc, node, "rowspan", 0).min(MAX_ROWSPAN),
                    )
                } else {
                    (1, 1)
                };
                while pending.get(x).is_some_and(|&p| p > 0) {
                    x += 1;
                }
                if x >= MAX_SPAN {
                    continue;
                }
                let colspan = colspan.min(MAX_SPAN - x);
                if pending.len() < x + colspan {
                    pending.resize(x + colspan, 0);
                }
                // `0` means "to the end of the row group"; the real count is
                // fixed once the group is complete.
                let reach = if rowspan == 0 { MAX_ROWSPAN } else { rowspan };
                for slot in &mut pending[x..x + colspan] {
                    *slot = reach;
                }
                spanning.push(self.cells.len());
                self.cells.push(Cell {
                    box_id: cell_id,
                    row,
                    column: x,
                    colspan,
                    rowspan: reach,
                });
                x += colspan;
            }
            *columns = (*columns).max(pending.len());
            for slot in &mut pending {
                *slot = slot.saturating_sub(1);
            }
            self.rows.push(Row {
                box_id: row_id,
                cells: first_cell..self.cells.len(),
            });
        }
        let rows = first_row..self.rows.len();
        for index in spanning {
            let cell = &mut self.cells[index];
            cell.rowspan = cell.rowspan.min(rows.end - cell.row).max(1);
        }
        self.sections.push(Section {
            box_id: section,
            rows,
        });
    }

    /// The rows a cell covers.
    pub(crate) fn cell_rows(&self, cell: &Cell) -> Range<usize> {
        cell.row..cell.row + cell.rowspan
    }
}
