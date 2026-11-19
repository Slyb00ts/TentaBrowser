//! The operations of a range that change the tree: splitting a `Text` node,
//! and deleting, extracting, cloning and inserting at a range's contents
//! (<https://dom.spec.whatwg.org/#interface-range>).
//!
//! The standard writes extraction and cloning as recursion: the node a range
//! starts in and the node it ends in each hang below a child of the common
//! ancestor, that child is cloned, and the same algorithm runs on the part of
//! the range inside it. Here the two descents are walked as paths instead (a
//! *spine* on each side), so that a range reaching a hundred thousand levels
//! down costs heap and not native stack. The order in which nodes leave the
//! tree is the standard's, because mutation observers and other live ranges
//! see it: the start side from the bottom up, then the children between the
//! two sides, then the end side from the top down.
//!
//! These functions take boundary points and change the tree. Every live range
//! of the document, the caller's included, follows through the ordinary
//! mutation paths; where the standard then sets the range itself, the new
//! point is returned for the caller to set.

use hashbrown::HashSet;
use tenta_core::atom::atoms;
use tenta_core::{NodeId, Result};

use super::{Document, utf16_len};
use crate::NodeKind;
use crate::error::{hierarchy_request, index_size, invalid_state};
use crate::mutation::Mutation;

/// A boundary point: a node and an offset into it, in UTF-16 code units for
/// character data and in children otherwise.
pub type RangePoint = (NodeId, u32);

/// How a range lies below the common ancestor of its two points.
struct RangeShape {
    common: NodeId,
    /// From the child of `common` that holds the start node down to the start
    /// node itself. Empty when the start node is `common`.
    start_spine: Vec<NodeId>,
    /// The same for the end node.
    end_spine: Vec<NodeId>,
    /// The children of `common` the range contains whole, in tree order.
    contained: Vec<NodeId>,
}

impl Document {
    /// The length of a node as ranges count it: code units of character data,
    /// children of anything else, and 0 for a doctype.
    pub fn node_length(&self, node: NodeId) -> u32 {
        let kind = self.kind(node);
        if kind.is_character_data_subclass() {
            utf16_len(self.text_data(node).unwrap_or_default())
        } else if kind == NodeKind::DocType {
            0
        } else {
            self.children(node).count() as u32
        }
    }

    /// The index of `node` among its parent's children; 0 without a parent.
    fn index_in_parent(&self, node: NodeId) -> u32 {
        let parent = self.parent(node);
        if !parent.is_some() {
            return 0;
        }
        self.children(parent)
            .position(|child| child == node)
            .unwrap_or(0) as u32
    }

    /// The path from the child of `ancestor` that holds `node` down to `node`.
    /// Empty when `node` is `ancestor`.
    fn spine(&self, ancestor: NodeId, node: NodeId) -> Vec<NodeId> {
        let mut path = Vec::new();
        let mut current = node;
        while current != ancestor && current.is_some() {
            path.push(current);
            current = self.parent(current);
        }
        path.reverse();
        path
    }

    fn range_shape(&self, start: RangePoint, end: RangePoint) -> RangeShape {
        // The first inclusive ancestor of the start node that is one of the
        // end node's too. The end node's chain is a set so that two deep
        // boundary points cost the sum of their depths and not the product.
        let end_chain: HashSet<NodeId> = std::iter::once(end.0)
            .chain(self.ancestors(end.0))
            .collect();
        let mut common = start.0;
        while !end_chain.contains(&common) {
            let parent = self.parent(common);
            if !parent.is_some() {
                break;
            }
            common = parent;
        }
        let start_spine = self.spine(common, start.0);
        let end_spine = self.spine(common, end.0);
        // A child of the common ancestor is contained when it lies after the
        // start and before the end: after the child that holds the start
        // node, or from the start offset when the range starts in the common
        // ancestor itself; and likewise up to the end.
        let from = match start_spine.first() {
            Some(first) => self.index_in_parent(*first) + 1,
            None => start.1,
        };
        let to = match end_spine.first() {
            Some(first) => self.index_in_parent(*first),
            None => end.1,
        };
        let contained = self
            .children(common)
            .enumerate()
            .filter(|(index, _)| (from..to).contains(&(*index as u32)))
            .map(|(_, child)| child)
            .collect();
        RangeShape {
            common,
            start_spine,
            end_spine,
            contained,
        }
    }

    /// The point a range collapses to once its contents are gone: its start,
    /// or when the start node is being emptied from below, the place in the
    /// common ancestor just after the child that holds it.
    fn collapsed_point(&self, shape: &RangeShape, start: RangePoint) -> RangePoint {
        match shape.start_spine.first() {
            Some(first) => (shape.common, self.index_in_parent(*first) + 1),
            None => start,
        }
    }

    fn check_range_points(&self, start: RangePoint, end: RangePoint) -> Result<()> {
        for (node, offset) in [start, end] {
            if !self.is_live(node) {
                return Err(invalid_state("a range boundary names a node that is gone"));
            }
            if offset > self.node_length(node) {
                return Err(index_size("a range boundary is past the end of its node"));
            }
        }
        Ok(())
    }

    fn is_character_data(&self, node: NodeId) -> bool {
        self.kind(node).is_character_data_subclass()
    }

    /// The children of `parent` after `child` (`after`), or before it.
    fn siblings_beside(&self, parent: NodeId, child: NodeId, after: bool) -> Vec<NodeId> {
        let index = self.index_in_parent(child) as usize;
        if after {
            self.children(parent).skip(index + 1).collect()
        } else {
            self.children(parent).take(index).collect()
        }
    }

    fn append(&mut self, parent: NodeId, child: NodeId) -> Result<()> {
        self.mutate(Mutation::Append { parent, child })
    }

    /// Makes each of `clones` the last child of the one before it, and the
    /// first of them the last child of `top`, from the bottom up.
    fn join_spine(&mut self, top: NodeId, clones: Vec<NodeId>) -> Result<()> {
        for pair in clones.windows(2).rev() {
            self.append(pair[0], pair[1])?;
        }
        if let Some(first) = clones.first() {
            self.append(top, *first)?;
        }
        Ok(())
    }

    /// A copy of the character-data node `node` that holds `count` code units
    /// of its data from `offset`.
    fn clone_data_slice(&mut self, node: NodeId, offset: u32, count: u32) -> Result<NodeId> {
        let text = self.substring_data(node, offset, count)?;
        let clone = self.shallow_clone(node)?;
        self.mutate(Mutation::SetText { node: clone, text })?;
        Ok(clone)
    }

    /// Splits the `Text` node `node` at `offset`
    /// (<https://dom.spec.whatwg.org/#concept-text-split>): the data from
    /// `offset` on moves to a new `Text` node, inserted after `node` when it
    /// has a parent, and returned.
    ///
    /// A live range boundary in the moved part of the data follows it into
    /// the new node, and one in the parent that stood just after `node` stays
    /// after the new node; those are the two fixups the split has beyond what
    /// an insertion and a deletion do by themselves.
    pub fn split_text(&mut self, node: NodeId, offset: u32) -> Result<NodeId> {
        if !self.is_live(node) || !matches!(self.kind(node), NodeKind::Text | NodeKind::CData) {
            return Err(hierarchy_request("only a Text node can be split"));
        }
        let length = self.node_length(node);
        if offset > length {
            return Err(index_size("offset is greater than the node's length"));
        }
        let count = length - offset;
        let text = self.substring_data(node, offset, count)?;
        let new_node = self.shallow_clone(node)?;
        self.mutate(Mutation::SetText {
            node: new_node,
            text,
        })?;
        let parent = self.parent(node);
        if parent.is_some() {
            let index = self.index_in_parent(node);
            self.mutate(Mutation::InsertBefore {
                parent,
                child: new_node,
                before: self.next_sibling(node),
            })?;
            self.move_ranges_into_split(node, offset, new_node, parent, index)?;
        }
        self.replace_data(node, offset, count, "")?;
        Ok(new_node)
    }

    /// The contents of a range leave the tree
    /// (<https://dom.spec.whatwg.org/#dom-range-deletecontents>). Returns the
    /// point the range collapses to.
    pub fn delete_range_contents(
        &mut self,
        start: RangePoint,
        end: RangePoint,
    ) -> Result<RangePoint> {
        self.check_range_points(start, end)?;
        if start == end {
            return Ok(start);
        }
        if start.0 == end.0 && self.is_character_data(start.0) {
            self.replace_data(start.0, start.1, end.1 - start.1, "")?;
            return Ok(start);
        }
        let shape = self.range_shape(start, end);
        let collapsed = self.collapsed_point(&shape, start);
        // Every node the range contains whose parent it does not, in tree
        // order: what hangs off the start side from the bottom up, the
        // children between the sides, then the end side from the top down.
        let mut doomed = Vec::new();
        if !shape.start_spine.is_empty() {
            if !self.is_character_data(start.0) {
                doomed.extend(self.children(start.0).skip(start.1 as usize));
            }
            for pair in shape.start_spine.windows(2).rev() {
                doomed.extend(self.siblings_beside(pair[0], pair[1], true));
            }
        }
        doomed.extend(shape.contained.iter().copied());
        if !shape.end_spine.is_empty() {
            for pair in shape.end_spine.windows(2) {
                doomed.extend(self.siblings_beside(pair[0], pair[1], false));
            }
            if !self.is_character_data(end.0) {
                doomed.extend(self.children(end.0).take(end.1 as usize));
            }
        }

        if self.is_character_data(start.0) {
            let length = self.node_length(start.0);
            self.replace_data(start.0, start.1, length - start.1, "")?;
        }
        for node in doomed {
            self.mutate(Mutation::Remove { child: node })?;
        }
        if self.is_character_data(end.0) {
            self.replace_data(end.0, 0, end.1, "")?;
        }
        Ok(collapsed)
    }

    /// The contents of a range move into a new `DocumentFragment`
    /// (<https://dom.spec.whatwg.org/#concept-range-extract>). Returns the
    /// fragment and the point the range collapses to.
    pub fn extract_range_contents(
        &mut self,
        start: RangePoint,
        end: RangePoint,
    ) -> Result<(NodeId, RangePoint)> {
        self.check_range_points(start, end)?;
        let fragment = self.create_fragment();
        if start == end {
            return Ok((fragment, start));
        }
        if start.0 == end.0 && self.is_character_data(start.0) {
            let clone = self.clone_data_slice(start.0, start.1, end.1 - start.1)?;
            self.append(fragment, clone)?;
            self.replace_data(start.0, start.1, end.1 - start.1, "")?;
            return Ok((fragment, start));
        }
        let shape = self.range_shape(start, end);
        self.refuse_contained_doctype(&shape)?;
        let collapsed = self.collapsed_point(&shape, start);

        // The start side. Each node of the spine gets a clone; the lowest is
        // filled first and each clone is put into the one above it before
        // that one takes the siblings that follow, so a clone's children are
        // in tree order and the nodes leave the tree from the bottom up.
        let mut below: Option<NodeId> = None;
        for (depth, node) in shape.start_spine.iter().copied().enumerate().rev() {
            let lowest = depth + 1 == shape.start_spine.len();
            let clone = if lowest && self.is_character_data(node) {
                let length = self.node_length(node);
                let clone = self.clone_data_slice(node, start.1, length - start.1)?;
                self.replace_data(node, start.1, length - start.1, "")?;
                clone
            } else {
                let clone = self.shallow_clone(node)?;
                let moved: Vec<NodeId> = if lowest {
                    self.children(node).skip(start.1 as usize).collect()
                } else {
                    let child = shape.start_spine[depth + 1];
                    let lower = below.take().expect("the clone of the node below");
                    self.append(clone, lower)?;
                    self.siblings_beside(node, child, true)
                };
                for child in moved {
                    self.append(clone, child)?;
                }
                clone
            };
            below = Some(clone);
        }
        if let Some(top) = below {
            self.append(fragment, top)?;
        }

        for child in shape.contained.iter().copied() {
            self.append(fragment, child)?;
        }

        // The end side. Nodes leave the tree from the top down: each clone
        // takes the siblings that precede the next node of the spine. The
        // clones are joined afterwards, from the bottom up, so that every
        // append is to a node without ancestors; joined on the way down each
        // would cost its depth.
        let mut end_clones = Vec::with_capacity(shape.end_spine.len());
        for (depth, node) in shape.end_spine.iter().copied().enumerate() {
            let lowest = depth + 1 == shape.end_spine.len();
            if lowest && self.is_character_data(node) {
                let clone = self.clone_data_slice(node, 0, end.1)?;
                self.replace_data(node, 0, end.1, "")?;
                end_clones.push(clone);
                break;
            }
            let clone = self.shallow_clone(node)?;
            let moved: Vec<NodeId> = if lowest {
                self.children(node).take(end.1 as usize).collect()
            } else {
                self.siblings_beside(node, shape.end_spine[depth + 1], false)
            };
            for child in moved {
                self.append(clone, child)?;
            }
            end_clones.push(clone);
        }
        self.join_spine(fragment, end_clones)?;
        Ok((fragment, collapsed))
    }

    /// A copy of the contents of a range in a new `DocumentFragment`
    /// (<https://dom.spec.whatwg.org/#concept-range-clone>). The tree is not
    /// changed.
    pub fn clone_range_contents(&mut self, start: RangePoint, end: RangePoint) -> Result<NodeId> {
        self.check_range_points(start, end)?;
        let fragment = self.create_fragment();
        if start == end {
            return Ok(fragment);
        }
        if start.0 == end.0 && self.is_character_data(start.0) {
            let clone = self.clone_data_slice(start.0, start.1, end.1 - start.1)?;
            self.append(fragment, clone)?;
            return Ok(fragment);
        }
        let shape = self.range_shape(start, end);
        self.refuse_contained_doctype(&shape)?;

        // The start side, built from the bottom up as extraction builds it:
        // a clone takes the clone of the node below it and then copies of
        // the siblings that follow.
        let mut below: Option<NodeId> = None;
        for (depth, node) in shape.start_spine.iter().copied().enumerate().rev() {
            let lowest = depth + 1 == shape.start_spine.len();
            let clone = if lowest && self.is_character_data(node) {
                let length = self.node_length(node);
                self.clone_data_slice(node, start.1, length - start.1)?
            } else {
                let clone = self.shallow_clone(node)?;
                let copied: Vec<NodeId> = if lowest {
                    self.children(node).skip(start.1 as usize).collect()
                } else {
                    let lower = below.take().expect("the clone of the node below");
                    self.append(clone, lower)?;
                    self.siblings_beside(node, shape.start_spine[depth + 1], true)
                };
                for child in copied {
                    let copy = self.clone_node(child, true)?;
                    self.append(clone, copy)?;
                }
                clone
            };
            below = Some(clone);
        }
        if let Some(top) = below {
            self.append(fragment, top)?;
        }

        for child in shape.contained.iter().copied() {
            let copy = self.clone_node(child, true)?;
            self.append(fragment, copy)?;
        }

        let mut end_clones = Vec::with_capacity(shape.end_spine.len());
        for (depth, node) in shape.end_spine.iter().copied().enumerate() {
            let lowest = depth + 1 == shape.end_spine.len();
            if lowest && self.is_character_data(node) {
                end_clones.push(self.clone_data_slice(node, 0, end.1)?);
                break;
            }
            let clone = self.shallow_clone(node)?;
            let copied: Vec<NodeId> = if lowest {
                self.children(node).take(end.1 as usize).collect()
            } else {
                self.siblings_beside(node, shape.end_spine[depth + 1], false)
            };
            for child in copied {
                let copy = self.clone_node(child, true)?;
                self.append(clone, copy)?;
            }
            end_clones.push(clone);
        }
        self.join_spine(fragment, end_clones)?;
        Ok(fragment)
    }

    /// Inserts `node` at the start of a range
    /// (<https://dom.spec.whatwg.org/#concept-range-insert>). Returns the
    /// point just after what was inserted, which is where a range that was
    /// collapsed has to end so that it contains the node.
    pub fn insert_node_at_range(&mut self, start: RangePoint, node: NodeId) -> Result<RangePoint> {
        self.check_range_points(start, start)?;
        if !self.is_live(node) {
            return Err(invalid_state("the node to insert is gone"));
        }
        let (start_node, start_offset) = start;
        let start_kind = self.kind(start_node);
        let is_text = matches!(start_kind, NodeKind::Text | NodeKind::CData);
        if matches!(
            start_kind,
            NodeKind::ProcessingInstruction | NodeKind::Comment
        ) || (is_text && !self.parent(start_node).is_some())
            || start_node == node
        {
            return Err(hierarchy_request(
                "a node cannot be inserted at this range's start",
            ));
        }
        let mut reference = if is_text {
            start_node
        } else {
            self.children(start_node)
                .nth(start_offset as usize)
                .unwrap_or(NodeId::NONE)
        };
        let parent = if reference.is_some() {
            self.parent(reference)
        } else {
            start_node
        };
        // Validity is settled before the text is split, so that an insertion
        // that cannot happen leaves the text whole.
        self.validate_insert(parent, node, reference)?;
        if is_text {
            reference = self.split_text(start_node, start_offset)?;
        }
        if node == reference {
            reference = self.next_sibling(reference);
        }
        if self.parent(node).is_some() {
            self.mutate(Mutation::Remove { child: node })?;
        }
        let mut new_offset = if reference.is_some() {
            self.index_in_parent(reference)
        } else {
            self.node_length(parent)
        };
        new_offset += if self.kind(node) == NodeKind::Fragment {
            self.node_length(node)
        } else {
            1
        };
        self.mutate(Mutation::InsertBefore {
            parent,
            child: node,
            before: reference,
        })?;
        Ok((parent, new_offset))
    }

    /// Parses `html` as a fragment for a range that starts in `start_node`
    /// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-range-createcontextualfragment>).
    ///
    /// The markup is parsed as the children of the element the range starts
    /// in (for a start in character data, its parent), and of a `<body>` when
    /// there is none or it is the `<html>` element. Unlike the nodes
    /// `innerHTML` parses, a `<script>` in the result has not started: it runs
    /// when the fragment is inserted into a document.
    pub fn create_contextual_fragment(&mut self, start_node: NodeId, html: &str) -> Result<NodeId> {
        if !self.is_live(start_node) {
            return Err(invalid_state("the range's start node is gone"));
        }
        let element = match self.kind(start_node) {
            NodeKind::Element => Some(start_node),
            kind if kind.is_character_data_subclass() => {
                let parent = self.parent(start_node);
                (parent.is_some() && self.kind(parent) == NodeKind::Element).then_some(parent)
            }
            _ => None,
        };
        let context = element
            .and_then(|element| self.element_name(element))
            .filter(|name| !(name.ns == atoms::NS_HTML && name.local == atoms::HTML))
            .unwrap_or_else(super::body_context);
        let parsed_root = self.parse_fragment(context, html)?;
        let fragment = self.create_fragment();
        let parsed: Vec<NodeId> = self.children(parsed_root).collect();
        for child in parsed {
            self.append(fragment, child)?;
        }
        self.mutate(Mutation::Remove { child: parsed_root })?;
        let scripts: Vec<NodeId> = self
            .descendants(fragment)
            .filter(|node| {
                self.tag(*node) == Some(atoms::SCRIPT)
                    && matches!(self.namespace(*node), Some(ns) if ns == atoms::NS_HTML || ns == atoms::NS_SVG)
            })
            .collect();
        for script in scripts {
            self.set_script_started(script, false);
        }
        Ok(fragment)
    }

    /// Whether a range partially contains a node that is not a `Text` node,
    /// which is what `surroundContents()` refuses
    /// (<https://dom.spec.whatwg.org/#dom-range-surroundcontents> step 1).
    pub fn range_splits_a_non_text_node(&self, start: RangePoint, end: RangePoint) -> bool {
        let shape = self.range_shape(start, end);
        shape
            .start_spine
            .iter()
            .chain(shape.end_spine.iter())
            .any(|node| !matches!(self.kind(*node), NodeKind::Text | NodeKind::CData))
    }

    /// A range that contains a doctype whole cannot be extracted or cloned.
    fn refuse_contained_doctype(&self, shape: &RangeShape) -> Result<()> {
        if shape
            .contained
            .iter()
            .any(|child| self.kind(*child) == NodeKind::DocType)
        {
            return Err(hierarchy_request(
                "a range that contains a doctype cannot be extracted or cloned",
            ));
        }
        Ok(())
    }
}
