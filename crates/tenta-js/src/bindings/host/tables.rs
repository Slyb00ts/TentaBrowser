//! The table model's script members: `HTMLTableElement`,
//! `HTMLTableSectionElement`, `HTMLTableRowElement` and
//! `HTMLTableCellElement` (<https://html.spec.whatwg.org/multipage/tables.html>).
//!
//! Everything is read from the tree on each call. `rows`, `tBodies` and
//! `cells` are snapshot collections, like the form's `elements`.

use tenta_core::NodeId;
use tenta_core::atom::{Atom, atoms};
use tenta_dom::Mutation;
use tenta_html::QualName;

use super::{HostObject, encode_node, mutation_error};
use crate::bindings::{
    HTMLCollection, HTMLTableCaptionElement, HTMLTableCellElement, HTMLTableCellElementOps,
    HTMLTableElementOps, HTMLTableRowElement, HTMLTableRowElementOps, HTMLTableSectionElement,
    HTMLTableSectionElementOps, Handle, JsError,
};

fn index_error(index: i32) -> JsError {
    JsError::dom_exception(
        "IndexSizeError",
        &format!("The index provided ({index}) is outside the range of the collection."),
    )
}

impl HostObject {
    fn children_tagged(&self, of: NodeId, tags: &[Atom]) -> Vec<NodeId> {
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        doc.children(of)
            .filter(|child| doc.tag(*child).is_some_and(|tag| tags.contains(&tag)))
            .collect()
    }

    fn first_child_tagged(&self, tag: Atom) -> Option<NodeId> {
        let (_, id) = self.anchor();
        self.children_tagged(id, &[tag]).into_iter().next()
    }

    fn node_bits(&self, node: NodeId) -> u64 {
        let (doc, _) = self.anchor();
        encode_node(self.slot(), doc.borrow().node_ref(node))
    }

    /// A new HTML element of this host's document, not yet in the tree.
    fn new_html_element(&self, tag: Atom) -> Result<NodeId, JsError> {
        let (doc, _) = self.anchor();
        doc.borrow_mut()
            .create_element(
                QualName::new(atoms::NS_HTML, tag),
                Vec::new(),
                Default::default(),
            )
            .map_err(|error| mutation_error(&error))
    }

    fn apply(&self, mutation: Mutation) -> Result<(), JsError> {
        let (doc, _) = self.anchor();
        doc.borrow_mut()
            .mutate(mutation)
            .map_err(|error| mutation_error(&error))
    }

    /// Inserts `child` into `parent` before `before`, or at the end for
    /// `NodeId::NONE`.
    fn insert_child(&self, parent: NodeId, child: NodeId, before: NodeId) -> Result<(), JsError> {
        self.apply(if before == NodeId::NONE {
            Mutation::Append { parent, child }
        } else {
            Mutation::InsertBefore {
                parent,
                child,
                before,
            }
        })
    }

    fn remove_node(&self, child: NodeId) -> Result<(), JsError> {
        self.apply(Mutation::Remove { child })
    }

    /// The table's rows: the `<thead>`'s first, then those of the bodies and
    /// the table itself in tree order, then the `<tfoot>`'s.
    fn table_rows(&self) -> Vec<NodeId> {
        let (doc, table) = self.anchor();
        let doc = doc.borrow();
        let rows_of = |section: NodeId| {
            doc.children(section)
                .filter(|row| doc.tag(*row) == Some(atoms::TR))
                .collect::<Vec<_>>()
        };
        let (mut head, mut body, mut foot) = (Vec::new(), Vec::new(), Vec::new());
        for child in doc.children(table) {
            match doc.tag(child) {
                Some(tag) if tag == atoms::THEAD => head.extend(rows_of(child)),
                Some(tag) if tag == atoms::TFOOT => foot.extend(rows_of(child)),
                Some(tag) if tag == atoms::TBODY => body.extend(rows_of(child)),
                Some(tag) if tag == atoms::TR => body.push(child),
                _ => {}
            }
        }
        head.extend(body);
        head.extend(foot);
        head
    }

    /// The first child of the table of `tag`, made and inserted by `place`
    /// when there is none.
    fn table_part(
        &self,
        tag: Atom,
        place: impl FnOnce(&Self) -> NodeId,
    ) -> Result<NodeId, JsError> {
        if let Some(existing) = self.first_child_tagged(tag.clone()) {
            return Ok(existing);
        }
        let (_, table) = self.anchor();
        let before = place(self);
        let part = self.new_html_element(tag)?;
        self.insert_child(table, part, before)?;
        Ok(part)
    }

    fn delete_table_part(&self, tag: Atom) {
        if let Some(part) = self.first_child_tagged(tag) {
            let _ = self.remove_node(part);
        }
    }

    /// `insertRow()` of a section or a table whose rows are `rows`; `parent`
    /// is where a row goes when it is appended.
    fn insert_row_into(
        &self,
        rows: &[NodeId],
        index: i32,
        parent: NodeId,
    ) -> Result<Handle<HTMLTableRowElement>, JsError> {
        let length = i32::try_from(rows.len()).unwrap_or(i32::MAX);
        if index < -1 || index > length {
            return Err(index_error(index));
        }
        let row = self.new_html_element(atoms::TR)?;
        if index == -1 || index == length {
            self.insert_child(parent, row, NodeId::NONE)?;
        } else {
            let before = rows[index as usize];
            let (doc, _) = self.anchor();
            let before_parent = doc.borrow().parent(before);
            self.insert_child(before_parent, row, before)?;
        }
        Ok(Handle::from_bits(self.node_bits(row)))
    }

    fn delete_from(&self, items: &[NodeId], index: i32) -> Result<(), JsError> {
        let length = i32::try_from(items.len()).unwrap_or(i32::MAX);
        if index < -1 || index >= length {
            return Err(index_error(index));
        }
        let target = if index == -1 {
            items.last().copied()
        } else {
            Some(items[index as usize])
        };
        match target {
            Some(target) => self.remove_node(target),
            None => Ok(()),
        }
    }

    fn row_cells(&self) -> Vec<NodeId> {
        let (_, row) = self.anchor();
        self.children_tagged(row, &[atoms::TD, atoms::TH])
    }

    fn section_rows(&self) -> Vec<NodeId> {
        let (_, section) = self.anchor();
        self.children_tagged(section, &[atoms::TR])
    }

    /// A reflected `unsigned long` clamped to `[low, high]`, with `default`
    /// for a missing or unparsable value.
    fn reflect_clamped(&self, name: &str, default: u32, low: u32, high: u32) -> u32 {
        let (doc, id) = self.anchor();
        doc.borrow()
            .attribute_str(id, name)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .map_or(default, |value| value.clamp(low, high))
    }
}

impl HTMLTableElementOps for HostObject {
    fn caption(&self) -> Result<Option<Handle<HTMLTableCaptionElement>>, JsError> {
        Ok(self
            .first_child_tagged(atoms::CAPTION)
            .map(|caption| Handle::from_bits(self.node_bits(caption))))
    }
    fn create_caption(&self) -> Result<Handle<HTMLTableCaptionElement>, JsError> {
        let caption = self.table_part(atoms::CAPTION, |host| {
            let (doc, table) = host.anchor();
            doc.borrow().first_child(table)
        })?;
        Ok(Handle::from_bits(self.node_bits(caption)))
    }
    fn delete_caption(&self) {
        self.delete_table_part(atoms::CAPTION);
    }
    fn t_head(&self) -> Result<Option<Handle<HTMLTableSectionElement>>, JsError> {
        Ok(self
            .first_child_tagged(atoms::THEAD)
            .map(|head| Handle::from_bits(self.node_bits(head))))
    }
    fn create_t_head(&self) -> Result<Handle<HTMLTableSectionElement>, JsError> {
        // Before the first child that is not a caption or a colgroup.
        let head = self.table_part(atoms::THEAD, |host| {
            let (doc, table) = host.anchor();
            let doc = doc.borrow();
            doc.children(table)
                .find(|child| {
                    doc.tag(*child)
                        .is_none_or(|tag| tag != atoms::CAPTION && tag != atoms::COLGROUP)
                })
                .unwrap_or(NodeId::NONE)
        })?;
        Ok(Handle::from_bits(self.node_bits(head)))
    }
    fn delete_t_head(&self) {
        self.delete_table_part(atoms::THEAD);
    }
    fn t_foot(&self) -> Result<Option<Handle<HTMLTableSectionElement>>, JsError> {
        Ok(self
            .first_child_tagged(atoms::TFOOT)
            .map(|foot| Handle::from_bits(self.node_bits(foot))))
    }
    fn create_t_foot(&self) -> Result<Handle<HTMLTableSectionElement>, JsError> {
        let foot = self.table_part(atoms::TFOOT, |_| NodeId::NONE)?;
        Ok(Handle::from_bits(self.node_bits(foot)))
    }
    fn delete_t_foot(&self) {
        self.delete_table_part(atoms::TFOOT);
    }
    fn t_bodies(&self) -> Result<Handle<HTMLCollection>, JsError> {
        let (_, table) = self.anchor();
        self.collection_of(self.children_tagged(table, &[atoms::TBODY]))
    }
    fn create_t_body(&self) -> Result<Handle<HTMLTableSectionElement>, JsError> {
        let (doc, table) = self.anchor();
        // After the last body there is.
        let before = self
            .children_tagged(table, &[atoms::TBODY])
            .last()
            .map_or(NodeId::NONE, |last| doc.borrow().next_sibling(*last));
        let body = self.new_html_element(atoms::TBODY)?;
        self.insert_child(table, body, before)?;
        Ok(Handle::from_bits(self.node_bits(body)))
    }
    fn rows(&self) -> Result<Handle<HTMLCollection>, JsError> {
        self.collection_of(self.table_rows())
    }
    fn insert_row(&self, index: Option<i32>) -> Result<Handle<HTMLTableRowElement>, JsError> {
        let index = index.unwrap_or(-1);
        let rows = self.table_rows();
        let (doc, table) = self.anchor();
        let length = i32::try_from(rows.len()).unwrap_or(i32::MAX);
        if index < -1 || index > length {
            return Err(index_error(index));
        }
        let parent = if rows.is_empty() {
            match self.children_tagged(table, &[atoms::TBODY]).last() {
                Some(body) => *body,
                None => {
                    let body = self.new_html_element(atoms::TBODY)?;
                    self.insert_child(table, body, NodeId::NONE)?;
                    body
                }
            }
        } else {
            doc.borrow().parent(*rows.last().unwrap_or(&NodeId::NONE))
        };
        self.insert_row_into(&rows, index, parent)
    }
    fn delete_row(&self, index: i32) -> Result<(), JsError> {
        self.delete_from(&self.table_rows(), index)
    }
}

impl HTMLTableSectionElementOps for HostObject {
    fn rows(&self) -> Result<Handle<HTMLCollection>, JsError> {
        self.collection_of(self.section_rows())
    }
    fn insert_row(&self, index: Option<i32>) -> Result<Handle<HTMLTableRowElement>, JsError> {
        let (_, section) = self.anchor();
        self.insert_row_into(&self.section_rows(), index.unwrap_or(-1), section)
    }
    fn delete_row(&self, index: i32) -> Result<(), JsError> {
        self.delete_from(&self.section_rows(), index)
    }
}

impl HTMLTableRowElementOps for HostObject {
    fn row_index(&self) -> i32 {
        let (doc, row) = self.anchor();
        let table = {
            let doc = doc.borrow();
            let parent = doc.parent(row);
            match doc.tag(parent) {
                Some(tag) if tag == atoms::TABLE => Some(parent),
                Some(tag) if tag == atoms::THEAD || tag == atoms::TBODY || tag == atoms::TFOOT => {
                    let grandparent = doc.parent(parent);
                    (doc.tag(grandparent) == Some(atoms::TABLE)).then_some(grandparent)
                }
                _ => None,
            }
        };
        table
            .and_then(|table| {
                self.node_host(table)
                    .table_rows()
                    .iter()
                    .position(|candidate| *candidate == row)
            })
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1)
    }
    fn section_row_index(&self) -> i32 {
        let (doc, row) = self.anchor();
        let parent = doc.borrow().parent(row);
        if parent == NodeId::NONE {
            return -1;
        }
        let host = self.node_host(parent);
        let rows = if doc.borrow().tag(parent) == Some(atoms::TABLE) {
            host.table_rows()
        } else {
            host.section_rows()
        };
        rows.iter()
            .position(|candidate| *candidate == row)
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1)
    }
    fn cells(&self) -> Result<Handle<HTMLCollection>, JsError> {
        self.collection_of(self.row_cells())
    }
    fn insert_cell(&self, index: Option<i32>) -> Result<Handle<HTMLTableCellElement>, JsError> {
        let index = index.unwrap_or(-1);
        let cells = self.row_cells();
        let length = i32::try_from(cells.len()).unwrap_or(i32::MAX);
        if index < -1 || index > length {
            return Err(index_error(index));
        }
        let (_, row) = self.anchor();
        let cell = self.new_html_element(atoms::TD)?;
        let before = if index == -1 || index == length {
            NodeId::NONE
        } else {
            cells[index as usize]
        };
        self.insert_child(row, cell, before)?;
        Ok(Handle::from_bits(self.node_bits(cell)))
    }
    fn delete_cell(&self, index: i32) -> Result<(), JsError> {
        self.delete_from(&self.row_cells(), index)
    }
}

impl HTMLTableCellElementOps for HostObject {
    fn col_span(&self) -> u32 {
        self.reflect_clamped("colspan", 1, 1, 1000)
    }
    fn set_col_span(&self, value: u32) {
        self.reflect_attribute("colspan", &value.to_string());
    }
    fn row_span(&self) -> u32 {
        self.reflect_clamped("rowspan", 1, 0, 65_534)
    }
    fn set_row_span(&self, value: u32) {
        self.reflect_attribute("rowspan", &value.to_string());
    }
    fn headers(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("headers"))
    }
    fn set_headers(&self, value: &str) {
        self.reflect_attribute("headers", value);
    }
    fn cell_index(&self) -> i32 {
        let (doc, cell) = self.anchor();
        let parent = doc.borrow().parent(cell);
        if parent == NodeId::NONE || doc.borrow().tag(parent) != Some(atoms::TR) {
            return -1;
        }
        self.node_host(parent)
            .row_cells()
            .iter()
            .position(|candidate| *candidate == cell)
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1)
    }
    fn scope(&self) -> Result<String, JsError> {
        Ok(self.reflect_keyword("scope", &["row", "col", "rowgroup", "colgroup"], ""))
    }
    fn set_scope(&self, value: &str) {
        self.reflect_attribute("scope", value);
    }
    fn abbr(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("abbr"))
    }
    fn set_abbr(&self, value: &str) {
        self.reflect_attribute("abbr", value);
    }
}
