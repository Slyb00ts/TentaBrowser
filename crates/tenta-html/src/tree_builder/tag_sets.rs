// Derived from html5ever's `src/tree_builder/tag_sets.rs`.
// Copyright 2014-2017 The html5ever Project Developers, MIT/Apache-2.0.
// See THIRD_PARTY_NOTICES.md ("Derived code").

//! The element-name sets the tree construction algorithm tests against.

use crate::names::{ElemName, Local, Ns};

/// The MathML text integration points.
pub(crate) fn mathml_text_integration_point(name: ElemName) -> bool {
    name.ns == Ns::MathMl
        && matches!(
            name.local,
            Local::Mi | Local::Mo | Local::Mn | Local::Ms | Local::Mtext
        )
}

/// The SVG HTML integration points (`annotation-xml` is handled separately).
pub(crate) fn svg_html_integration_point(name: ElemName) -> bool {
    name.ns == Ns::Svg
        && matches!(
            name.local,
            Local::ForeignObject | Local::Desc | Local::Title
        )
}

fn html_default_scope(name: ElemName) -> bool {
    name.ns == Ns::Html
        && matches!(
            name.local,
            Local::Applet
                | Local::Caption
                | Local::Html
                | Local::Table
                | Local::Td
                | Local::Th
                | Local::Marquee
                | Local::Object
                | Local::Select
                | Local::Template
        )
}

/// The "particular element in scope" set.
pub(crate) fn default_scope(name: ElemName) -> bool {
    html_default_scope(name)
        || mathml_text_integration_point(name)
        || svg_html_integration_point(name)
}

/// The "in list item scope" set.
pub(crate) fn list_item_scope(name: ElemName) -> bool {
    default_scope(name) || (name.ns == Ns::Html && matches!(name.local, Local::Ol | Local::Ul))
}

/// The "in button scope" set.
pub(crate) fn button_scope(name: ElemName) -> bool {
    default_scope(name) || name.is_html(Local::Button)
}

/// The "in table scope" set.
pub(crate) fn table_scope(name: ElemName) -> bool {
    name.ns == Ns::Html && matches!(name.local, Local::Html | Local::Table | Local::Template)
}

/// Elements the "in table body" mode clears back to.
pub(crate) fn table_body_context(name: ElemName) -> bool {
    name.ns == Ns::Html
        && matches!(
            name.local,
            Local::Tbody | Local::Tfoot | Local::Thead | Local::Template | Local::Html
        )
}

/// Elements the "in row" mode clears back to.
pub(crate) fn table_row_context(name: ElemName) -> bool {
    name.ns == Ns::Html && matches!(name.local, Local::Tr | Local::Template | Local::Html)
}

/// `<td>` and `<th>`.
pub(crate) fn td_th(name: ElemName) -> bool {
    name.ns == Ns::Html && matches!(name.local, Local::Td | Local::Th)
}

/// The set for "generate implied end tags".
pub(crate) fn cursory_implied_end(name: ElemName) -> bool {
    name.ns == Ns::Html
        && matches!(
            name.local,
            Local::Dd
                | Local::Dt
                | Local::Li
                | Local::Option
                | Local::Optgroup
                | Local::P
                | Local::Rb
                | Local::Rp
                | Local::Rt
                | Local::Rtc
        )
}

/// The set for "generate all implied end tags thoroughly".
pub(crate) fn thorough_implied_end(name: ElemName) -> bool {
    cursory_implied_end(name)
        || (name.ns == Ns::Html
            && matches!(
                name.local,
                Local::Caption
                    | Local::Colgroup
                    | Local::Tbody
                    | Local::Td
                    | Local::Tfoot
                    | Local::Th
                    | Local::Thead
                    | Local::Tr
            ))
}

/// `<h1>` … `<h6>`.
pub(crate) fn heading_tag(name: ElemName) -> bool {
    name.ns == Ns::Html
        && matches!(
            name.local,
            Local::H1 | Local::H2 | Local::H3 | Local::H4 | Local::H5 | Local::H6
        )
}

/// The "special" element set.
pub(crate) fn special_tag(name: ElemName) -> bool {
    if name.ns != Ns::Html {
        return false;
    }
    matches!(
        name.local,
        Local::Address
            | Local::Applet
            | Local::Area
            | Local::Article
            | Local::Aside
            | Local::Base
            | Local::Basefont
            | Local::Bgsound
            | Local::Blockquote
            | Local::Body
            | Local::Br
            | Local::Button
            | Local::Caption
            | Local::Center
            | Local::Col
            | Local::Colgroup
            | Local::Dd
            | Local::Details
            | Local::Dir
            | Local::Div
            | Local::Dl
            | Local::Dt
            | Local::Embed
            | Local::Fieldset
            | Local::Figcaption
            | Local::Figure
            | Local::Footer
            | Local::Form
            | Local::Frame
            | Local::Frameset
            | Local::H1
            | Local::H2
            | Local::H3
            | Local::H4
            | Local::H5
            | Local::H6
            | Local::Head
            | Local::Header
            | Local::Hgroup
            | Local::Hr
            | Local::Html
            | Local::Iframe
            | Local::Img
            | Local::Input
            | Local::Isindex
            | Local::Li
            | Local::Link
            | Local::Listing
            | Local::Main
            | Local::Marquee
            | Local::Menu
            | Local::Meta
            | Local::Nav
            | Local::Noembed
            | Local::Noframes
            | Local::Noscript
            | Local::Object
            | Local::Ol
            | Local::P
            | Local::Param
            | Local::Plaintext
            | Local::Pre
            | Local::Script
            | Local::Search
            | Local::Section
            | Local::Select
            | Local::Source
            | Local::Style
            | Local::Summary
            | Local::Table
            | Local::Tbody
            | Local::Td
            | Local::Template
            | Local::Textarea
            | Local::Tfoot
            | Local::Th
            | Local::Thead
            | Local::Title
            | Local::Tr
            | Local::Track
            | Local::Ul
            | Local::Wbr
            | Local::Xmp
    )
}

/// Elements that may still be open at the end of the body without an error.
pub(crate) fn body_end_ok(name: ElemName) -> bool {
    name.ns == Ns::Html
        && matches!(
            name.local,
            Local::Dd
                | Local::Dt
                | Local::Li
                | Local::Optgroup
                | Local::Option
                | Local::P
                | Local::Rp
                | Local::Rt
                | Local::Tbody
                | Local::Td
                | Local::Tfoot
                | Local::Th
                | Local::Thead
                | Local::Tr
                | Local::Body
                | Local::Html
        )
}

/// Elements that enable foster parenting when they are the current node.
pub(crate) fn foster_target(name: ElemName) -> bool {
    name.ns == Ns::Html
        && matches!(
            name.local,
            Local::Table | Local::Tbody | Local::Tfoot | Local::Thead | Local::Tr
        )
}

/// Formatting elements (the ones the active formatting list can hold).
pub(crate) fn formatting_tag(name: ElemName) -> bool {
    name.ns == Ns::Html
        && matches!(
            name.local,
            Local::A
                | Local::B
                | Local::Big
                | Local::Code
                | Local::Em
                | Local::Font
                | Local::I
                | Local::Nobr
                | Local::S
                | Local::Small
                | Local::Strike
                | Local::Strong
                | Local::Tt
                | Local::U
        )
}
