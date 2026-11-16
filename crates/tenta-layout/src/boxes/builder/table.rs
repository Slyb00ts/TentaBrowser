//! Anonymous table boxes (CSS 2.1 section 17.2.1) and the grid of each table.
//!
//! A table, row group or row collects its children like any container, then
//! this module settles them: every child that is not what the parent expects
//! goes into an anonymous box of the next kind down (cells wrap loose content,
//! rows wrap loose cells, row groups wrap loose rows), and table parts found
//! under a parent that is not a table part get an anonymous table. Wrappers
//! are finished through [`Builder::finish`], so each one normalises its own
//! children the same way.

use std::sync::Arc;

use smallvec::SmallVec;
use style::computed_values::border_collapse::T as BorderCollapse;
use tenta_core::NodeId;
use tenta_css::cascade::Style;

use super::{Builder, Entry, Frame, Level};
use crate::box_tree::{BoxFlags, BoxId, BoxKind, InlineItem, LayoutBox};
use crate::boxes::paint::paint_style;
use crate::table::collapse;
use crate::table::grid::TableGrid;
use crate::types::{DecorationPaint, TableKind};

impl Builder<'_, '_> {
    fn table_kind(&self, id: BoxId) -> Option<TableKind> {
        match self.tree.get(id).kind {
            BoxKind::Table(kind) => Some(kind),
            _ => None,
        }
    }

    /// Finishes a table, row group, row, column or column group.
    pub(super) fn finish_table_part(&mut self, frame: Frame, kind: TableKind) -> BoxId {
        let Frame {
            box_id: container,
            style,
            decoration,
            entries,
            ..
        } = frame;
        let mut children: SmallVec<[BoxId; 4]> = SmallVec::new();
        match kind {
            // A column has no content.
            TableKind::Column | TableKind::Caption | TableKind::Cell => {}
            TableKind::ColumnGroup => {
                for entry in entries {
                    if let Entry::Block(id) = entry
                        && self.table_kind(id) == Some(TableKind::Column)
                    {
                        self.tree.get_mut(id).parent = container;
                        children.push(id);
                    }
                }
            }
            TableKind::Row => {
                self.settle(
                    entries,
                    container,
                    &style,
                    &decoration,
                    &mut children,
                    &[TableKind::Cell],
                    TableKind::Cell,
                );
            }
            TableKind::RowGroup => {
                self.settle(
                    entries,
                    container,
                    &style,
                    &decoration,
                    &mut children,
                    &[TableKind::Row],
                    TableKind::Row,
                );
            }
            TableKind::Table => {
                self.settle(
                    entries,
                    container,
                    &style,
                    &decoration,
                    &mut children,
                    &[
                        TableKind::Caption,
                        TableKind::ColumnGroup,
                        TableKind::Column,
                        TableKind::RowGroup,
                    ],
                    TableKind::RowGroup,
                );
            }
        }
        self.tree.get_mut(container).children = children;
        if kind == TableKind::Table {
            let grid = TableGrid::build(&self.tree, self.cx.doc, container);
            if style.get_inherited_table().border_collapse == BorderCollapse::Collapse {
                collapse::resolve(&mut self.tree, &grid, container);
            }
            self.tree.get_mut(container).table = Some(Box::new(grid));
        }
        container
    }

    /// Moves the children that are `allowed` into `children` and wraps each
    /// run of the others in an anonymous box of kind `wrapper`.
    #[allow(clippy::too_many_arguments)]
    fn settle(
        &mut self,
        entries: Vec<Entry>,
        container: BoxId,
        style: &Style,
        decoration: &DecorationPaint,
        children: &mut SmallVec<[BoxId; 4]>,
        allowed: &[TableKind],
        wrapper: TableKind,
    ) {
        let mut group: Vec<Entry> = Vec::new();
        for entry in entries {
            match entry {
                Entry::Block(id)
                    if self.table_kind(id).is_some_and(|k| allowed.contains(&k))
                        && !self.tree.get(id).flags.contains(BoxFlags::OUT_OF_FLOW) =>
                {
                    self.wrap_group(wrapper, &mut group, container, style, decoration, children);
                    self.tree.get_mut(id).parent = container;
                    children.push(id);
                }
                other => group.push(other),
            }
        }
        self.wrap_group(wrapper, &mut group, container, style, decoration, children);
    }

    fn wrap_group(
        &mut self,
        kind: TableKind,
        group: &mut Vec<Entry>,
        container: BoxId,
        style: &Style,
        decoration: &DecorationPaint,
        children: &mut SmallVec<[BoxId; 4]>,
    ) {
        if group.is_empty() {
            return;
        }
        let entries = std::mem::take(group);
        if let Some(wrapper) = self.wrap_entries(kind, entries, container, style, decoration) {
            children.push(wrapper);
        }
    }

    /// Puts `entries` into a new anonymous box of `kind` under `parent`.
    /// Returns `None` when they hold nothing but collapsible white space.
    fn wrap_entries(
        &mut self,
        kind: TableKind,
        mut entries: Vec<Entry>,
        parent: BoxId,
        style: &Style,
        decoration: &DecorationPaint,
    ) -> Option<BoxId> {
        // A float is content: it needs a cell to sit in.
        let has_content = entries
            .iter()
            .any(|e| matches!(e, Entry::Block(_) | Entry::OutOfFlow(_, Level::Float)));
        if !has_content {
            let items: Vec<InlineItem> = entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Inline(item) => Some(item.clone()),
                    Entry::OutOfFlow(box_id, Level::Float) => {
                        Some(InlineItem::Float { box_id: *box_id })
                    }
                    Entry::OutOfFlow(box_id, _) => Some(InlineItem::AbsPos { box_id: *box_id }),
                    Entry::Block(_) => None,
                })
                .collect();
            if self.is_droppable(&items) {
                return None;
            }
        }
        let wrapper = self.anonymous_part(kind, style, decoration, parent);
        for entry in &mut entries {
            match entry {
                Entry::Block(id) => self.tree.get_mut(*id).parent = wrapper,
                Entry::Inline(InlineItem::Text { style_box, .. }) if *style_box == parent => {
                    *style_box = wrapper;
                }
                _ => {}
            }
        }
        let frame = Frame {
            box_id: wrapper,
            style: self.tree.get(wrapper).style.clone(),
            decoration: *decoration,
            level: Level::Block,
            entries,
            inlines: Vec::new(),
            opened: 0,
        };
        Some(self.finish(frame))
    }

    fn anonymous_part(
        &mut self,
        kind: TableKind,
        parent_style: &Style,
        decoration: &DecorationPaint,
        parent: BoxId,
    ) -> BoxId {
        let style = self.anon.derive(parent_style, true);
        let (paint, paint_flags) = paint_style(&style, decoration);
        let mut part = LayoutBox::new(
            NodeId::NONE,
            parent,
            BoxKind::Table(kind),
            style,
            Arc::new(paint),
        );
        part.flags = BoxFlags::ANONYMOUS | paint_flags;
        if matches!(kind, TableKind::Table | TableKind::Cell) {
            part.flags |= BoxFlags::ESTABLISHES_BFC;
        }
        self.tree.push(part)
    }

    /// Wraps the table parts among `children` that have a parent which is not
    /// a table part in anonymous tables, one per run of neighbours.
    pub(super) fn wrap_stray_parts(
        &mut self,
        children: &mut SmallVec<[BoxId; 4]>,
        container: BoxId,
        style: &Style,
        decoration: &DecorationPaint,
    ) {
        let is_part = |this: &Self, id: BoxId| {
            this.table_kind(id)
                .is_some_and(|kind| kind != TableKind::Table)
                && !this.tree.get(id).flags.contains(BoxFlags::OUT_OF_FLOW)
        };
        if !children.iter().any(|&id| is_part(self, id)) {
            return;
        }
        let old = std::mem::take(children);
        let mut group: Vec<Entry> = Vec::new();
        for id in old {
            if is_part(self, id) {
                group.push(Entry::Block(id));
            } else {
                self.wrap_group(
                    TableKind::Table,
                    &mut group,
                    container,
                    style,
                    decoration,
                    children,
                );
                children.push(id);
            }
        }
        self.wrap_group(
            TableKind::Table,
            &mut group,
            container,
            style,
            decoration,
            children,
        );
    }
}
