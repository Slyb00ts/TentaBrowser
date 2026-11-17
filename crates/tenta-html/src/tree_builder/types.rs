// Derived from html5ever's `src/tree_builder/types.rs`.
// Copyright 2014-2017 The html5ever Project Developers, MIT/Apache-2.0.
// See THIRD_PARTY_NOTICES.md ("Derived code").

//! Types internal to the tree builder.

use crate::names::ElemName;
use crate::tokenizer::Tag;
use crate::tokenizer::states::RawKind;

/// One insertion mode of the tree construction algorithm.
#[derive(PartialEq, Eq, Copy, Clone, Debug)]
#[allow(missing_docs)]
pub(crate) enum InsertionMode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InHeadNoscript,
    AfterHead,
    InBody,
    Text,
    InTable,
    InTableText,
    InCaption,
    InColumnGroup,
    InTableBody,
    InRow,
    InCell,
    InTemplate,
    AfterBody,
    InFrameset,
    AfterFrameset,
    AfterAfterBody,
    AfterAfterFrameset,
}

/// A comment or a processing instruction: the tree construction rules place
/// both in the same positions, only the node they create differs.
#[derive(PartialEq, Eq, Copy, Clone, Debug)]
pub(crate) enum CommentData<'a> {
    Comment(&'a str),
    Pi { target: &'a str, data: &'a str },
}

/// Whether a character token has been split into whitespace and non-whitespace
/// parts yet.
#[derive(PartialEq, Eq, Copy, Clone, Debug)]
pub(crate) enum SplitStatus {
    NotSplit,
    Whitespace,
    NotWhitespace,
}

/// A token as the tree builder sees it. Text and comments are slices of the
/// input chunk.
#[derive(PartialEq, Eq, Clone, Debug)]
pub(crate) enum Token<'a> {
    Tag(Tag),
    Comment(CommentData<'a>),
    Characters(SplitStatus, &'a str),
    NullCharacter,
    Eof,
}

/// What one insertion-mode step decided.
pub(crate) enum ProcessResult<'a, Handle> {
    Done,
    DoneAckSelfClosing,
    SplitWhitespace(&'a str),
    Reprocess(InsertionMode, Token<'a>),
    Script(Handle),
    ToPlaintext,
    ToRawData(RawKind),
    EncodingIndicator(String),
}

/// An entry in the list of active formatting elements.
pub(crate) enum FormatEntry<Handle> {
    /// An element in the list. The boolean is true if the element is currently
    /// on the stack of open elements (i.e., still open). This flag is set when
    /// the entry is pushed and cleared when the element is popped or removed
    /// from the open elements stack, avoiding a full-stack scan during
    /// reconstruction.
    Element(Handle, Tag, bool),
    Marker,
}

/// An element on the stack of open elements, with its name cached so the
/// tree builder never has to ask the sink for it.
#[derive(Clone)]
pub(crate) struct OpenElem<Handle> {
    pub handle: Handle,
    pub name: ElemName,
    /// True for a MathML `annotation-xml` that is an HTML integration point.
    pub annotation_xml_integration_point: bool,
    /// For a `<template>` that declared a shadow root: the root, which takes
    /// the place of its template contents. Such a template is on the stack
    /// only and never in the tree.
    pub shadow_root: Option<Handle>,
    /// Depth of this element; `<html>` = 1.
    pub depth: u32,
    /// Conservative upper bound on the deepest element depth in its subtree.
    pub deepest: u32,
}

/// Where a node is to be inserted.
#[derive(Clone)]
pub(crate) enum InsertionPoint<Handle> {
    /// As the last child of this parent.
    LastChild(Handle),
    /// Foster parenting: before `element` if it has a parent, otherwise as the
    /// last child of `prev_element`.
    TableFosterParenting {
        element: Handle,
        prev_element: Handle,
    },
}
