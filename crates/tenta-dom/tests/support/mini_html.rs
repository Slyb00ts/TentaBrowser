//! A deliberately non-conformant HTML tokenizer.
//!
//! It exists only to drive `tenta_dom`'s `TreeSink` directly from the sink
//! tests, so the sink contract can be exercised without the conformant parser
//! in between. It handles the markup of the fixtures in `tests/fixtures/`:
//! doctype, comments, start and end tags, attributes, void elements, raw-text
//! elements and the common character references. It implies no tags, recovers
//! from nothing, and must never be used outside tests.
//!
//! Everything else parses through [`tenta_html::Parser`]; see `super::parse`.

#![allow(dead_code)]

use tenta_core::atom::atoms;
use tenta_core::{Atom, NodeId};
use tenta_dom::Document;
use tenta_html::{Attribute, ElementFlags, NodeOrText, ParseControl, QualName, TreeSink};

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

const RAW_TEXT: &[&str] = &["script", "style", "textarea", "title"];

/// Parses `html` into `document` through its `TreeSink`, exactly as the real
/// parser will.
pub fn parse_into(document: &mut Document, html: &str) {
    let root = TreeSink::document(document);
    // (tag name, the node children go into, the element that gets popped)
    let mut open: Vec<(String, NodeId, NodeId)> = Vec::new();
    let bytes = html.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let current = open.last().map(|(_, node, _)| *node).unwrap_or(root);
        if bytes[index] == b'<' {
            if html[index..].starts_with("<!--") {
                let end = html[index..]
                    .find("-->")
                    .map(|e| index + e)
                    .unwrap_or(bytes.len());
                let comment = document.create_comment(&html[index + 4..end]);
                document.append(&current, NodeOrText::Node(comment));
                index = (end + 3).min(bytes.len());
                continue;
            }
            if html[index..].len() > 2 && bytes[index + 1] == b'!' {
                let end = html[index..]
                    .find('>')
                    .map(|e| index + e)
                    .unwrap_or(bytes.len());
                let declaration = &html[index + 2..end];
                if declaration.to_ascii_lowercase().starts_with("doctype") {
                    let name = declaration[7..].trim().to_owned();
                    document.append_doctype_to_document(&name, "", "");
                }
                index = (end + 1).min(bytes.len());
                continue;
            }
            if html[index..].starts_with("</") {
                let end = html[index..]
                    .find('>')
                    .map(|e| index + e)
                    .unwrap_or(bytes.len());
                let name = html[index + 2..end].trim().to_ascii_lowercase();
                if let Some(position) = open
                    .iter()
                    .rposition(|(open_name, _, _)| *open_name == name)
                {
                    let closing: Vec<NodeId> = open
                        .drain(position..)
                        .rev()
                        .map(|(_, _, element)| element)
                        .collect();
                    for element in closing {
                        if document.pop(&element) == ParseControl::Stop {
                            return;
                        }
                    }
                }
                index = (end + 1).min(bytes.len());
                continue;
            }
            let Some(end) = find_tag_end(html, index) else {
                break;
            };
            let (name, attrs, self_closing) = parse_start_tag(&html[index + 1..end]);
            let element = match document.create_element(
                QualName::new(atoms::NS_HTML, Atom::new(&name)),
                attrs,
                ElementFlags {
                    template: name == "template",
                    ..ElementFlags::default()
                },
            ) {
                Ok(el) => el,
                Err(_) => {
                    // Skip invalid elements
                    index = end + 1;
                    continue;
                }
            };
            document.append(&current, NodeOrText::Node(element));
            index = end + 1;
            if self_closing || VOID.contains(&name.as_str()) {
                if document.pop(&element) == ParseControl::Stop {
                    return;
                }
                continue;
            }
            if RAW_TEXT.contains(&name.as_str()) {
                let close = format!("</{name}");
                let end = html[index..]
                    .to_ascii_lowercase()
                    .find(&close)
                    .map(|e| index + e)
                    .unwrap_or(bytes.len());
                if end > index {
                    let target = document.get_template_contents_or(element, name == "template");
                    document.append(&target, NodeOrText::Text(&html[index..end]));
                }
                index = end;
                continue;
            }
            let target = if name == "template" {
                document.get_template_contents(&element)
            } else {
                element
            };
            open.push((name, target, element));
            continue;
        }
        let end = html[index..]
            .find('<')
            .map(|e| index + e)
            .unwrap_or(bytes.len());
        let text = decode_entities(&html[index..end]);
        if !text.is_empty() {
            document.append(&current, NodeOrText::Text(&text));
        }
        index = end;
    }
    for (_, _, element) in open.into_iter().rev() {
        if document.pop(&element) == ParseControl::Stop {
            return;
        }
    }
}

trait TemplateTarget {
    fn get_template_contents_or(&mut self, element: NodeId, is_template: bool) -> NodeId;
}

impl TemplateTarget for Document {
    fn get_template_contents_or(&mut self, element: NodeId, is_template: bool) -> NodeId {
        if is_template {
            TreeSink::get_template_contents(self, &element)
        } else {
            element
        }
    }
}

fn find_tag_end(html: &str, start: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    let mut index = start + 1;
    let mut quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(q) if byte == q => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(index),
            None => {}
        }
        index += 1;
    }
    None
}

fn parse_start_tag(source: &str) -> (String, Vec<Attribute>, bool) {
    let source = source.trim_end();
    let (source, self_closing) = match source.strip_suffix('/') {
        Some(rest) => (rest, true),
        None => (source, false),
    };
    let mut chars = source.char_indices().peekable();
    let mut name = String::new();
    for (_, ch) in chars.by_ref() {
        if ch.is_whitespace() {
            break;
        }
        name.push(ch.to_ascii_lowercase());
    }
    let rest = match source.find(char::is_whitespace) {
        Some(position) if position < source.len() => &source[position..],
        _ => "",
    };
    (name, parse_attributes(rest), self_closing)
}

fn parse_attributes(source: &str) -> Vec<Attribute> {
    let mut attrs = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && bytes[index] != b'='
            && bytes[index] != b'>'
        {
            index += 1;
        }
        if index == start {
            break;
        }
        let name = source[start..index].to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let value = if index < bytes.len() && bytes[index] == b'=' {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index < bytes.len() && (bytes[index] == b'"' || bytes[index] == b'\'') {
                let quote = bytes[index];
                index += 1;
                let start = index;
                while index < bytes.len() && bytes[index] != quote {
                    index += 1;
                }
                let value = decode_entities(&source[start..index]);
                index = (index + 1).min(bytes.len());
                value
            } else {
                let start = index;
                while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                decode_entities(&source[start..index])
            }
        } else {
            String::new()
        };
        attrs.push(Attribute {
            name: QualName::new(atoms::EMPTY, Atom::new(&name)),
            value,
        });
    }
    attrs
}

fn decode_entities(source: &str) -> String {
    if !source.contains('&') {
        return source.to_owned();
    }
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(position) = rest.find('&') {
        out.push_str(&rest[..position]);
        rest = &rest[position..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let replacement = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{00A0}'),
            other => other
                .strip_prefix('#')
                .and_then(|number| match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => number.parse::<u32>().ok(),
                })
                .and_then(char::from_u32),
        };
        match replacement {
            Some(ch) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parses a full document from a string with the toy tokenizer.
pub fn parse(html: &str) -> Document {
    let mut document = Document::new();
    parse_into(&mut document, html);
    document
}
