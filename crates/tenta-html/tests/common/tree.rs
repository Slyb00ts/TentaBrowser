//! A small owned tree that implements [`TreeSink`], plus the html5lib
//! serialisation the conformance runners compare against.
//!
//! `tenta-dom` implements the same sink over the arena; this one exists so
//! `tenta-html` can be tested without depending on it.

// Each test binary uses a different part of this module.
#![allow(dead_code)]

use std::borrow::Cow;

use tenta_html::sink::{
    Attribute, ElementFlags, NodeOrText, ParseControl, QualName, QuirksMode, TreeSink,
};

/// Index of a node in [`Tree::nodes`].
pub type Handle = usize;

#[derive(Debug, Clone)]
pub enum NodeData {
    Document,
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },
    Element {
        name: QualName,
        attrs: Vec<Attribute>,
        /// The separate fragment a `<template>` owns.
        template_contents: Option<Handle>,
        mathml_annotation_xml_integration_point: bool,
    },
    Text(String),
    Comment(String),
    ProcessingInstruction {
        target: String,
        data: String,
    },
    /// A `<template>` element's contents.
    Fragment,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub data: NodeData,
    pub parent: Option<Handle>,
    pub children: Vec<Handle>,
}

/// An owned document tree.
#[derive(Debug)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub quirks_mode: QuirksMode,
    pub errors: Vec<String>,
    /// Elements whose end tag makes the parser stop, matched by
    /// `(local name, id attribute)`.
    pub stop_on: Option<(String, String)>,
    /// Names popped after `stop_on` matched; must stay empty.
    pub pops_after_stop: Vec<String>,
    stopped: bool,
}

impl Default for Tree {
    fn default() -> Self {
        Tree::new()
    }
}

impl Tree {
    pub fn new() -> Tree {
        Tree {
            nodes: vec![Node {
                data: NodeData::Document,
                parent: None,
                children: Vec::new(),
            }],
            quirks_mode: QuirksMode::NoQuirks,
            errors: Vec::new(),
            stop_on: None,
            pops_after_stop: Vec::new(),
            stopped: false,
        }
    }

    fn push(&mut self, data: NodeData) -> Handle {
        self.nodes.push(Node {
            data,
            parent: None,
            children: Vec::new(),
        });
        self.nodes.len() - 1
    }

    fn detach(&mut self, node: Handle) {
        if let Some(parent) = self.nodes[node].parent.take() {
            self.nodes[parent].children.retain(|&child| child != node);
        }
    }

    fn insert_at(&mut self, parent: Handle, index: usize, child: Handle) {
        self.detach(child);
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.insert(index, child);
    }

    fn append_node(&mut self, parent: Handle, child: Handle) {
        let index = self.nodes[parent].children.len();
        self.insert_at(parent, index, child);
    }

    /// Appends text, merging with a preceding text node.
    fn append_text(&mut self, parent: Handle, text: &str) {
        if let Some(&last) = self.nodes[parent].children.last()
            && let NodeData::Text(existing) = &mut self.nodes[last].data
        {
            existing.push_str(text);
            return;
        }
        let node = self.push(NodeData::Text(text.to_owned()));
        self.append_node(parent, node);
    }

    fn insert_text_before(&mut self, parent: Handle, index: usize, text: &str) {
        if index > 0 {
            let previous = self.nodes[parent].children[index - 1];
            if let NodeData::Text(existing) = &mut self.nodes[previous].data {
                existing.push_str(text);
                return;
            }
        }
        let node = self.push(NodeData::Text(text.to_owned()));
        self.insert_at(parent, index, node);
    }

    fn index_in_parent(&self, node: Handle) -> Option<(Handle, usize)> {
        let parent = self.nodes[node].parent?;
        let index = self.nodes[parent]
            .children
            .iter()
            .position(|&child| child == node)?;
        Some((parent, index))
    }

    fn ancestor_named(&self, node: Handle, name: &str) -> Option<Handle> {
        let mut current = self.nodes[node].parent;
        while let Some(handle) = current {
            if self.is_html_element(handle, name) {
                return Some(handle);
            }
            current = self.nodes[handle].parent;
        }
        None
    }

    fn is_html_element(&self, node: Handle, name: &str) -> bool {
        matches!(&self.nodes[node].data,
            NodeData::Element { name: qual, .. }
                if qual.local.as_str() == name
                    && qual.ns.as_str() == "http://www.w3.org/1999/xhtml")
    }

    fn has_attribute(&self, node: Handle, name: &str) -> bool {
        matches!(&self.nodes[node].data,
            NodeData::Element { attrs, .. }
                if attrs.iter().any(|attr| attr.name.local.as_str() == name))
    }

    /// Descendants with the given HTML local name, in tree order.
    fn descendants_named(&self, root: Handle, name: &str) -> Vec<Handle> {
        let mut found = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if node != root && self.is_html_element(node, name) {
                found.push(node);
            }
            stack.extend(self.nodes[node].children.iter().rev().copied());
        }
        found.sort_unstable();
        found
    }

    fn deep_clone(&mut self, node: Handle) -> Handle {
        let data = match &self.nodes[node].data {
            NodeData::Element {
                name,
                attrs,
                mathml_annotation_xml_integration_point,
                ..
            } => NodeData::Element {
                name: *name,
                attrs: attrs.clone(),
                template_contents: None,
                mathml_annotation_xml_integration_point: *mathml_annotation_xml_integration_point,
            },
            other => other.clone(),
        };
        let clone = self.push(data);
        for child in self.nodes[node].children.clone() {
            let child_clone = self.deep_clone(child);
            self.append_node(clone, child_clone);
        }
        clone
    }

    pub fn depth_of_deepest_element(&self) -> usize {
        let mut deepest = 0;
        let mut stack = vec![(0usize, 0usize)];
        while let Some((node, depth)) = stack.pop() {
            let depth = match &self.nodes[node].data {
                NodeData::Element {
                    template_contents, ..
                } => {
                    deepest = deepest.max(depth + 1);
                    let next_depth = depth + 1;
                    // Template contents are at depth+1 (a fragment's children).
                    if let Some(contents) = template_contents {
                        stack.push((*contents, next_depth));
                    }
                    next_depth
                }
                _ => depth,
            };
            for &child in &self.nodes[node].children {
                stack.push((child, depth));
            }
        }
        deepest
    }

    pub fn element_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| matches!(node.data, NodeData::Element { .. }))
            .count()
    }

    /// `(elements, nodes)` reachable from the document, template contents
    /// included; nodes the parser created but never inserted are not counted.
    pub fn reachable_counts(&self) -> (usize, usize) {
        let mut elements = 0;
        let mut nodes = 0;
        let mut stack = vec![0usize];
        while let Some(node) = stack.pop() {
            nodes += 1;
            if let NodeData::Element {
                template_contents, ..
            } = &self.nodes[node].data
            {
                elements += 1;
                if let Some(contents) = template_contents {
                    stack.push(*contents);
                }
            }
            stack.extend(self.nodes[node].children.iter().copied());
        }
        (elements, nodes)
    }
}

impl TreeSink for Tree {
    type Handle = Handle;
    type Output = Tree;

    fn document(&mut self) -> Handle {
        0
    }

    fn create_element(
        &mut self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> Handle {
        let node = self.push(NodeData::Element {
            name,
            attrs,
            template_contents: None,
            mathml_annotation_xml_integration_point: flags.mathml_annotation_xml_integration_point,
        });
        if flags.template {
            let contents = self.push(NodeData::Fragment);
            self.nodes[contents].parent = Some(node);
            if let NodeData::Element {
                template_contents, ..
            } = &mut self.nodes[node].data
            {
                *template_contents = Some(contents);
            }
        }
        node
    }

    fn create_comment(&mut self, text: &str) -> Handle {
        self.push(NodeData::Comment(text.to_owned()))
    }

    fn create_pi(&mut self, target: &str, data: &str) -> Handle {
        self.push(NodeData::ProcessingInstruction {
            target: target.to_owned(),
            data: data.to_owned(),
        })
    }

    fn append(&mut self, parent: &Handle, child: NodeOrText<'_, Handle>) {
        match child {
            NodeOrText::Node(node) => self.append_node(*parent, node),
            NodeOrText::Text(text) => self.append_text(*parent, text),
        }
    }

    fn append_before_sibling(&mut self, sibling: &Handle, child: NodeOrText<'_, Handle>) {
        let Some((parent, index)) = self.index_in_parent(*sibling) else {
            return;
        };
        match child {
            NodeOrText::Node(node) => self.insert_at(parent, index, node),
            NodeOrText::Text(text) => self.insert_text_before(parent, index, text),
        }
    }

    fn append_based_on_parent_node(
        &mut self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<'_, Handle>,
    ) {
        if self.nodes[*element].parent.is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(&mut self, name: &str, public_id: &str, system_id: &str) {
        let node = self.push(NodeData::Doctype {
            name: name.to_owned(),
            public_id: public_id.to_owned(),
            system_id: system_id.to_owned(),
        });
        self.append_node(0, node);
    }

    fn add_attrs_if_missing(&mut self, target: &Handle, attrs: Vec<Attribute>) {
        let NodeData::Element {
            attrs: existing, ..
        } = &mut self.nodes[*target].data
        else {
            return;
        };
        for attr in attrs {
            if !existing.iter().any(|old| old.name == attr.name) {
                existing.push(attr);
            }
        }
    }

    fn remove_from_parent(&mut self, target: &Handle) {
        self.detach(*target);
    }

    fn reparent_children(&mut self, node: &Handle, new_parent: &Handle) {
        let children = std::mem::take(&mut self.nodes[*node].children);
        for child in children {
            self.nodes[child].parent = Some(*new_parent);
            self.nodes[*new_parent].children.push(child);
        }
    }

    fn get_template_contents(&mut self, target: &Handle) -> Handle {
        match self.nodes[*target].data {
            NodeData::Element {
                template_contents: Some(contents),
                ..
            } => contents,
            _ => panic!("get_template_contents on a non-template element"),
        }
    }

    fn element_name(&self, target: &Handle) -> QualName {
        match &self.nodes[*target].data {
            NodeData::Element { name, .. } => *name,
            _ => panic!("element_name on a non-element node"),
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &Handle) -> bool {
        matches!(
            self.nodes[*target].data,
            NodeData::Element {
                mathml_annotation_xml_integration_point: true,
                ..
            }
        )
    }

    fn set_quirks_mode(&mut self, mode: QuirksMode) {
        self.quirks_mode = mode;
    }

    fn pop(&mut self, node: &Handle) -> ParseControl {
        if let NodeData::Element { name, attrs, .. } = &self.nodes[*node].data {
            let local = name.local.as_str().to_owned();
            if self.stopped {
                self.pops_after_stop.push(local);
                return ParseControl::Stop;
            }
            if let Some((stop_name, stop_id)) = &self.stop_on
                && local == *stop_name
                && attrs
                    .iter()
                    .any(|a| a.name.local.as_str() == "id" && a.value == *stop_id)
            {
                self.stopped = true;
                return ParseControl::Stop;
            }
        }
        ParseControl::Continue
    }

    fn maybe_clone_option_into_selectedcontent(&mut self, option: &Handle) {
        let Some(select) = self.ancestor_named(*option, "select") else {
            return;
        };
        let options = self.descendants_named(select, "option");
        let selected = options
            .iter()
            .rev()
            .find(|&&node| self.has_attribute(node, "selected"))
            .or(options.first())
            .copied();
        if selected != Some(*option) {
            return;
        }
        let Some(target) = self
            .descendants_named(select, "selectedcontent")
            .first()
            .copied()
        else {
            return;
        };
        for child in std::mem::take(&mut self.nodes[target].children) {
            self.nodes[child].parent = None;
        }
        for child in self.nodes[*option].children.clone() {
            let clone = self.deep_clone(child);
            self.append_node(target, clone);
        }
    }

    fn parse_error(&mut self, msg: Cow<'static, str>) {
        self.errors.push(msg.into_owned());
    }

    fn finish(self) -> Tree {
        self
    }
}

fn namespace_prefix(ns: &str) -> &'static str {
    match ns {
        "http://www.w3.org/2000/svg" => "svg ",
        "http://www.w3.org/1998/Math/MathML" => "math ",
        _ => "",
    }
}

fn attribute_prefix(ns: &str) -> &'static str {
    match ns {
        "http://www.w3.org/1999/xlink" => "xlink ",
        "http://www.w3.org/XML/1998/namespace" => "xml ",
        "http://www.w3.org/2000/xmlns/" => "xmlns ",
        _ => "",
    }
}

/// Serialises a tree in the html5lib tree-construction format.
pub fn serialize(tree: &Tree, root: Handle, skip_root: bool) -> String {
    let mut out = String::new();
    if skip_root {
        for &child in &tree.nodes[root].children {
            serialize_node(tree, child, 0, &mut out);
        }
    } else {
        serialize_node(tree, root, 0, &mut out);
    }
    out
}

fn indent(out: &mut String, depth: usize) {
    out.push_str("| ");
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn serialize_node(tree: &Tree, node: Handle, depth: usize, out: &mut String) {
    match &tree.nodes[node].data {
        NodeData::Document | NodeData::Fragment => {
            for &child in &tree.nodes[node].children {
                serialize_node(tree, child, depth, out);
            }
            return;
        }
        NodeData::Doctype {
            name,
            public_id,
            system_id,
        } => {
            indent(out, depth);
            out.push_str("<!DOCTYPE ");
            out.push_str(name);
            if !public_id.is_empty() || !system_id.is_empty() {
                out.push_str(&format!(" \"{public_id}\" \"{system_id}\""));
            }
            out.push_str(">\n");
        }
        NodeData::Text(text) => {
            indent(out, depth);
            out.push('"');
            out.push_str(text);
            out.push_str("\"\n");
        }
        NodeData::Comment(text) => {
            indent(out, depth);
            out.push_str("<!-- ");
            out.push_str(text);
            out.push_str(" -->\n");
        }
        NodeData::ProcessingInstruction { target, data } => {
            indent(out, depth);
            out.push_str(&format!("<?{target} {data}?>\n"));
        }
        NodeData::Element {
            name,
            attrs,
            template_contents,
            ..
        } => {
            indent(out, depth);
            out.push('<');
            out.push_str(namespace_prefix(name.ns.as_str()));
            out.push_str(name.local.as_str());
            out.push_str(">\n");

            let mut sorted: Vec<&Attribute> = attrs.iter().collect();
            sorted.sort_by_key(|attr| {
                let prefix = attr
                    .name
                    .prefix
                    .map(|_| attribute_prefix(attr.name.ns.as_str()))
                    .unwrap_or("");
                let mut key: Vec<u16> = prefix.encode_utf16().collect();
                key.extend(attr.name.local.as_str().encode_utf16());
                key
            });
            for attr in sorted {
                indent(out, depth + 1);
                if attr.name.prefix.is_some() {
                    out.push_str(attribute_prefix(attr.name.ns.as_str()));
                }
                out.push_str(attr.name.local.as_str());
                out.push_str(&format!("=\"{}\"\n", attr.value));
            }

            if let Some(contents) = template_contents {
                indent(out, depth + 1);
                out.push_str("content\n");
                for &child in &tree.nodes[*contents].children {
                    serialize_node(tree, child, depth + 2, out);
                }
            }
        }
    }
    for &child in &tree.nodes[node].children {
        serialize_node(tree, child, depth + 1, out);
    }
}
