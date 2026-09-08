//! The XML grammar: a recursive-descent pass over the decoded document that
//! drives a [`TreeSink`].

use std::collections::{HashMap, HashSet};
use std::fmt;

use tenta_core::Atom;
use tenta_core::atom::atoms as core_atoms;

use crate::sink::{Attribute, ElementFlags, NodeOrText, ParseControl, QualName, TreeSink};
use crate::tree_builder::MAX_DOM_DEPTH;

/// Above this many attributes on one element the linear duplicate scan becomes
/// the dominant cost, so attribute names go into a hash set instead (the HTML
/// tokenizer uses the same threshold). A tag with thousands of attributes is
/// legal markup and must stay linear (`docs/spec/13-performance.md` §4).
const ATTR_SET_THRESHOLD: usize = 16;

/// A well-formedness error and where it was found.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct XmlError {
    /// What went wrong.
    pub message: String,
    /// 1-based line of the offending character.
    pub line: u64,
    /// 1-based column, in characters.
    pub column: u64,
    /// Byte offset into the decoded document.
    pub offset: usize,
}

impl fmt::Display for XmlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at line {} column {}",
            self.message, self.line, self.column
        )
    }
}

impl std::error::Error for XmlError {}

type Result<T> = std::result::Result<T, XmlError>;

/// Deepest element nesting the XML parser builds.
///
/// The HTML tree builder clamps at [`MAX_DOM_DEPTH`] and the DOM traversals
/// cap at `MAX_TRAVERSAL_DEPTH`; without a cap here the XML path would build
/// a tree deeper than any consumer can walk. XML has no error recovery, so a
/// document past the cap is a parse error rather than a clamped tree.
pub const MAX_XML_DEPTH: usize = MAX_DOM_DEPTH;

struct OpenElem<Handle> {
    handle: Handle,
    /// Name as written, so the end tag can be compared literally.
    name: String,
    /// Length of the namespace stack to restore when this element closes.
    scopes: usize,
}

/// The in-scope namespace bindings.
///
/// A reverse linear scan over a flat stack is quadratic when every level
/// declares a prefix, so the bindings are kept as one stack per prefix with a
/// flat order list for scope restore: lookup is a hash probe and a `last()`.
#[derive(Default)]
struct Bindings {
    /// Every declaration in declaration order; `order[i]` is the prefix whose
    /// stack the i-th declaration pushed onto.
    order: Vec<Atom>,
    /// Per prefix, the stack of URIs currently in scope. `EMPTY` is the
    /// default namespace.
    scopes: HashMap<Atom, Vec<Atom>>,
}

impl Bindings {
    #[inline]
    fn len(&self) -> usize {
        self.order.len()
    }

    fn push(&mut self, prefix: Atom, uri: Atom) {
        self.order.push(prefix);
        self.scopes.entry(prefix).or_default().push(uri);
    }

    fn truncate(&mut self, len: usize) {
        while self.order.len() > len {
            let prefix = self.order.pop().expect("order is longer than len");
            if let Some(stack) = self.scopes.get_mut(&prefix) {
                stack.pop();
            }
        }
    }

    #[inline]
    fn lookup(&self, prefix: Atom) -> Option<Atom> {
        self.scopes.get(&prefix)?.last().copied()
    }
}

struct Parser<'a, S: TreeSink> {
    text: &'a str,
    pos: usize,
    sink: &'a mut S,
    doc: S::Handle,
    open: Vec<OpenElem<S::Handle>>,
    bindings: Bindings,
    seen_root: bool,
    seen_doctype: bool,
    stopped: bool,
}

/// Parses `text` into `sink`.
pub(crate) fn parse<S: TreeSink>(text: &str, sink: &mut S) -> Result<()> {
    let doc = sink.document();
    let mut parser = Parser {
        text,
        pos: 0,
        sink,
        doc,
        open: Vec::new(),
        bindings: Bindings::default(),
        seen_root: false,
        seen_doctype: false,
        stopped: false,
    };
    parser.run()
}

impl<'a, S: TreeSink> Parser<'a, S> {
    fn error<T>(&self, message: impl Into<String>) -> Result<T> {
        Err(self.error_at(self.pos, message))
    }

    fn error_at(&self, offset: usize, message: impl Into<String>) -> XmlError {
        let consumed = &self.text[..offset.min(self.text.len())];
        let line = consumed.bytes().filter(|&byte| byte == b'\n').count() as u64 + 1;
        let column = consumed
            .rsplit_once('\n')
            .map_or(consumed, |(_, last)| last)
            .chars()
            .count() as u64
            + 1;
        XmlError {
            message: message.into(),
            line,
            column,
            offset,
        }
    }

    fn rest(&self) -> &'a str {
        &self.text[self.pos..]
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.rest().starts_with(prefix)
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn skip_whitespace(&mut self) -> bool {
        let skipped = self
            .rest()
            .find(|c: char| !matches!(c, ' ' | '\t' | '\r' | '\n'))
            .unwrap_or(self.rest().len());
        self.pos += skipped;
        skipped > 0
    }

    fn run(&mut self) -> Result<()> {
        if self.starts_with("<?xml") {
            self.xml_declaration()?;
        }
        while self.pos < self.text.len() && !self.stopped {
            if self.starts_with("<!--") {
                self.comment()?;
            } else if self.starts_with("<?") {
                self.processing_instruction()?;
            } else if self.starts_with("<![CDATA[") {
                if self.open.is_empty() {
                    return self.error("CDATA section outside the root element");
                }
                self.cdata()?;
            } else if self.starts_with("<!DOCTYPE") {
                self.doctype()?;
            } else if self.starts_with("</") {
                self.end_tag()?;
            } else if self.starts_with("<") {
                self.start_tag()?;
            } else {
                self.text_run()?;
            }
        }
        if self.stopped {
            return Ok(());
        }
        if let Some(open) = self.open.last() {
            let name = open.name.clone();
            return self.error(format!("end of input inside <{name}>"));
        }
        if !self.seen_root {
            return self.error("no root element");
        }
        Ok(())
    }

    /// `<?xml version="1.0" encoding="…" standalone="…"?>`; the encoding was
    /// already handled by the caller's decoder.
    fn xml_declaration(&mut self) -> Result<()> {
        let Some(end) = self.rest().find("?>") else {
            return self.error("unterminated XML declaration");
        };
        self.pos += end + 2;
        Ok(())
    }

    fn comment(&mut self) -> Result<()> {
        let start = self.pos + 4;
        let Some(end) = self.text[start..].find("-->") else {
            return self.error("unterminated comment");
        };
        let data = &self.text[start..start + end];
        if data.contains("--") {
            return self.error("\"--\" inside a comment");
        }
        self.pos = start + end + 3;
        let normalized = normalize_newlines(data);
        let node = self.sink.create_comment(&normalized);
        self.append(node);
        Ok(())
    }

    fn processing_instruction(&mut self) -> Result<()> {
        let start = self.pos + 2;
        let Some(end) = self.text[start..].find("?>") else {
            return self.error("unterminated processing instruction");
        };
        let body = &self.text[start..start + end];
        let (target, data) = match body.find([' ', '\t', '\r', '\n']) {
            Some(index) => (&body[..index], body[index..].trim_start()),
            None => (body, ""),
        };
        if target.is_empty() || !is_xml_name(target) {
            return self.error("invalid processing instruction target");
        }
        if target.eq_ignore_ascii_case("xml") {
            return self.error("\"xml\" is a reserved processing instruction target");
        }
        self.pos = start + end + 2;
        let normalized = normalize_newlines(data);
        let node = self.sink.create_pi(target, &normalized);
        self.append(node);
        Ok(())
    }

    fn cdata(&mut self) -> Result<()> {
        let start = self.pos + "<![CDATA[".len();
        let Some(end) = self.text[start..].find("]]>") else {
            return self.error("unterminated CDATA section");
        };
        let data = &self.text[start..start + end];
        self.pos = start + end + 3;
        let normalized = normalize_newlines(data);
        self.append_text(&normalized);
        Ok(())
    }

    /// `<!DOCTYPE name (ExternalID)? ('[' internal subset ']')? '>'`. The
    /// internal subset is skipped: no entity beyond the predefined five is
    /// ever declared.
    fn doctype(&mut self) -> Result<()> {
        if self.seen_root || self.seen_doctype {
            return self.error("misplaced doctype");
        }
        self.seen_doctype = true;
        self.pos += "<!DOCTYPE".len();
        self.skip_whitespace();
        let name = self.take_name()?.to_owned();
        self.skip_whitespace();

        let mut public_id = String::new();
        let mut system_id = String::new();
        if self.starts_with("PUBLIC") {
            self.pos += "PUBLIC".len();
            self.skip_whitespace();
            public_id = self.take_quoted()?;
            self.skip_whitespace();
            system_id = self.take_quoted()?;
        } else if self.starts_with("SYSTEM") {
            self.pos += "SYSTEM".len();
            self.skip_whitespace();
            system_id = self.take_quoted()?;
        }
        self.skip_whitespace();

        if self.starts_with("[") {
            self.skip_internal_subset()?;
            self.skip_whitespace();
        }
        if !self.starts_with(">") {
            return self.error("unterminated doctype");
        }
        self.pos += 1;
        self.sink
            .append_doctype_to_document(&name, &public_id, &system_id);
        Ok(())
    }

    fn skip_internal_subset(&mut self) -> Result<()> {
        self.pos += 1;
        loop {
            let Some(c) = self.peek() else {
                return self.error("unterminated internal subset");
            };
            match c {
                ']' => {
                    self.pos += 1;
                    return Ok(());
                }
                '\'' | '"' => {
                    let _ = self.take_quoted()?;
                }
                '<' if self.starts_with("<!--") => self.skip_comment()?,
                _ => self.pos += c.len_utf8(),
            }
        }
    }

    fn skip_comment(&mut self) -> Result<()> {
        let start = self.pos + 4;
        let Some(end) = self.text[start..].find("-->") else {
            return self.error("unterminated comment");
        };
        self.pos = start + end + 3;
        Ok(())
    }

    fn take_quoted(&mut self) -> Result<String> {
        let Some(quote @ ('"' | '\'')) = self.peek() else {
            return self.error("expected a quoted value");
        };
        let start = self.pos + 1;
        let Some(end) = self.text[start..].find(quote) else {
            return self.error("unterminated quoted value");
        };
        self.pos = start + end + 1;
        Ok(self.text[start..start + end].to_owned())
    }

    fn take_name(&mut self) -> Result<&'a str> {
        let rest = self.rest();
        let mut end = 0;
        for (index, c) in rest.char_indices() {
            let ok = if index == 0 {
                is_name_start_char(c)
            } else {
                is_name_char(c)
            };
            if !ok {
                break;
            }
            end = index + c.len_utf8();
        }
        if end == 0 {
            return self.error("expected a name");
        }
        self.pos += end;
        Ok(&rest[..end])
    }

    fn start_tag(&mut self) -> Result<()> {
        let tag_start = self.pos;
        if self.seen_root && self.open.is_empty() {
            return self.error("more than one root element");
        }
        self.pos += 1;
        let name = self.take_name()?;
        let mut attrs: Vec<(&'a str, String, usize)> = Vec::new();
        let mut attr_names_set: Option<HashSet<&'a str>> = None;
        let self_closing;

        loop {
            let had_space = self.skip_whitespace();
            if self.starts_with("/>") {
                self.pos += 2;
                self_closing = true;
                break;
            }
            if self.starts_with(">") {
                self.pos += 1;
                self_closing = false;
                break;
            }
            if self.peek().is_none() {
                return self.error("unterminated start tag");
            }
            if !had_space {
                return self.error("expected whitespace between attributes");
            }
            let attr_start = self.pos;
            let attr_name = self.take_name()?;
            self.skip_whitespace();
            if !self.starts_with("=") {
                return self.error("attribute without a value");
            }
            self.pos += 1;
            self.skip_whitespace();
            let value = self.attribute_value()?;

            // Duplicate detection: use linear scan up to threshold, then switch to HashSet.
            let is_duplicate = if let Some(ref mut set) = attr_names_set {
                !set.insert(attr_name)
            } else if attrs.len() > ATTR_SET_THRESHOLD {
                attr_names_set = Some(attrs.iter().map(|(n, _, _)| *n).collect());
                !attr_names_set.as_mut().unwrap().insert(attr_name)
            } else {
                attrs.iter().any(|(n, _, _)| *n == attr_name)
            };

            if is_duplicate {
                return Err(self.error_at(attr_start, format!("duplicate attribute {attr_name}")));
            }
            attrs.push((attr_name, value, attr_start));
        }

        // Namespace declarations take effect on the element that carries them.
        let scopes = self.bindings.len();
        for (attr_name, value, offset) in &attrs {
            let prefix = match *attr_name {
                "xmlns" => core_atoms::EMPTY,
                name => match name.strip_prefix("xmlns:") {
                    Some("") => {
                        return Err(self.error_at(*offset, "empty namespace prefix"));
                    }
                    Some(prefix) => {
                        if value.is_empty() {
                            return Err(self.error_at(
                                *offset,
                                "a prefixed namespace declaration cannot be empty",
                            ));
                        }
                        Atom::new(prefix)
                    }
                    None => continue,
                },
            };
            self.bindings.push(prefix, Atom::new(value));
        }

        let qual = self.resolve(name, false, tag_start)?;
        let mut resolved = Vec::with_capacity(attrs.len());
        let mut resolved_names_set: Option<HashSet<QualName>> = None;
        for (attr_name, value, offset) in attrs {
            let name = self.resolve(attr_name, true, offset)?;

            // Duplicate detection after namespace resolution: use linear scan up to threshold, then HashSet.
            let is_duplicate = if let Some(ref mut set) = resolved_names_set {
                !set.insert(name)
            } else if resolved.len() > ATTR_SET_THRESHOLD {
                resolved_names_set = Some(
                    resolved
                        .iter()
                        .map(|a: &Attribute| a.name)
                        .collect::<std::collections::HashSet<_>>(),
                );
                !resolved_names_set.as_mut().unwrap().insert(name)
            } else {
                resolved.iter().any(|other: &Attribute| other.name == name)
            };

            if is_duplicate {
                return Err(self.error_at(
                    offset,
                    format!("duplicate attribute {attr_name} after namespace resolution"),
                ));
            }
            resolved.push(Attribute { name, value });
        }

        let handle = self
            .sink
            .create_element(qual, resolved, ElementFlags::default());
        self.append(handle.clone());
        self.seen_root = true;

        if self_closing {
            self.bindings.truncate(scopes);
            self.pop(&handle);
        } else {
            if self.open.len() >= MAX_XML_DEPTH {
                return Err(self.error_at(
                    tag_start,
                    format!("element nesting deeper than {MAX_XML_DEPTH} elements"),
                ));
            }
            self.open.push(OpenElem {
                handle,
                name: name.to_owned(),
                scopes,
            });
        }
        Ok(())
    }

    fn attribute_value(&mut self) -> Result<String> {
        let Some(quote @ ('"' | '\'')) = self.peek() else {
            return self.error("attribute value must be quoted");
        };
        self.pos += 1;
        let mut value = String::new();
        loop {
            let Some(c) = self.peek() else {
                return self.error("unterminated attribute value");
            };
            match c {
                _ if c == quote => {
                    self.pos += 1;
                    return Ok(value);
                }
                '<' => return self.error("\"<\" in an attribute value"),
                '&' => self.reference(&mut value)?,
                // Attribute-value normalisation: every literal whitespace
                // character becomes a space.
                '\r' => {
                    self.pos += 1;
                    if self.starts_with("\n") {
                        self.pos += 1;
                    }
                    value.push(' ');
                }
                '\n' | '\t' => {
                    self.pos += 1;
                    value.push(' ');
                }
                _ => {
                    self.pos += c.len_utf8();
                    value.push(c);
                }
            }
        }
    }

    fn end_tag(&mut self) -> Result<()> {
        let tag_start = self.pos;
        self.pos += 2;
        let name = self.take_name()?;
        self.skip_whitespace();
        if !self.starts_with(">") {
            return self.error("unterminated end tag");
        }
        self.pos += 1;

        let Some(open) = self.open.pop() else {
            return Err(self.error_at(tag_start, format!("</{name}> without a start tag")));
        };
        if open.name != name {
            return Err(self.error_at(
                tag_start,
                format!("</{name}> does not close <{}>", open.name),
            ));
        }
        self.bindings.truncate(open.scopes);
        self.pop(&open.handle);
        Ok(())
    }

    fn text_run(&mut self) -> Result<()> {
        let rest = self.rest();
        let end = rest.find('<').unwrap_or(rest.len());
        let run = &rest[..end];

        if self.open.is_empty() {
            if let Some(offset) = run.find(|c: char| !matches!(c, ' ' | '\t' | '\r' | '\n')) {
                return Err(self.error_at(self.pos + offset, "text outside the root element"));
            }
            self.pos += end;
            return Ok(());
        }

        if let Some(offset) = run.find("]]>") {
            return Err(self.error_at(self.pos + offset, "\"]]>\" in character data"));
        }
        if let Some(offset) = run.find('\0') {
            return Err(self.error_at(self.pos + offset, "U+0000 in character data"));
        }

        if !run.contains(['&', '\r']) {
            self.pos += end;
            if !run.is_empty() {
                self.append_text(run);
            }
            return Ok(());
        }

        let mut text = String::with_capacity(run.len());
        while self.pos < self.text.len() {
            let Some(c) = self.peek() else { break };
            match c {
                '<' => break,
                '&' => self.reference(&mut text)?,
                '\r' => {
                    self.pos += 1;
                    if self.starts_with("\n") {
                        self.pos += 1;
                    }
                    text.push('\n');
                }
                _ => {
                    self.pos += c.len_utf8();
                    text.push(c);
                }
            }
        }
        self.append_text(&text);
        Ok(())
    }

    /// An entity or character reference. Only the five predefined entities
    /// exist: an internal subset is skipped, so anything else is an error.
    fn reference(&mut self, out: &mut String) -> Result<()> {
        let start = self.pos;
        let rest = &self.text[start + 1..];
        let Some(end) = rest.find(';') else {
            return self.error("unterminated entity reference");
        };
        let name = &rest[..end];
        self.pos = start + 1 + end + 1;

        if let Some(digits) = name.strip_prefix('#') {
            let value = match digits.strip_prefix(['x', 'X']) {
                Some(hex) if !hex.is_empty() => u32::from_str_radix(hex, 16).ok(),
                Some(_) => None,
                None if !digits.is_empty() => digits.parse::<u32>().ok(),
                None => None,
            };
            let Some(c) = value.and_then(char::from_u32).filter(|&c| is_xml_char(c)) else {
                return Err(self.error_at(start, format!("invalid character reference &{name};")));
            };
            out.push(c);
            return Ok(());
        }

        let replacement = match name {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                return Err(self.error_at(start, format!("undeclared entity &{name};")));
            }
        };
        out.push(replacement);
        Ok(())
    }

    /// Resolves a possibly prefixed name against the in-scope bindings.
    fn resolve(&self, name: &str, is_attribute: bool, offset: usize) -> Result<QualName> {
        let (prefix, local) = match name.split_once(':') {
            Some((prefix, local)) => (Some(prefix), local),
            None => (None, name),
        };
        if local.is_empty() || !is_xml_name(local) || local.contains(':') {
            return Err(self.error_at(offset, format!("invalid name {name}")));
        }

        // The two reserved prefixes are bound for the life of the document.
        if name == "xmlns" && is_attribute {
            return Ok(QualName {
                ns: core_atoms::NS_XMLNS,
                prefix: None,
                local: Atom::new("xmlns"),
            });
        }
        let Some(prefix) = prefix else {
            let ns = if is_attribute {
                core_atoms::EMPTY
            } else {
                self.lookup(core_atoms::EMPTY).unwrap_or(core_atoms::EMPTY)
            };
            return Ok(QualName {
                ns,
                prefix: None,
                local: Atom::new(local),
            });
        };
        if !is_xml_name(prefix) {
            return Err(self.error_at(offset, format!("invalid namespace prefix in {name}")));
        }

        let ns = match prefix {
            "xml" => core_atoms::NS_XML,
            "xmlns" => core_atoms::NS_XMLNS,
            _ => match self.lookup(Atom::new(prefix)) {
                Some(uri) if uri != core_atoms::EMPTY => uri,
                _ => {
                    return Err(self.error_at(offset, format!("unbound namespace prefix {prefix}")));
                }
            },
        };
        Ok(QualName {
            ns,
            prefix: Some(Atom::new(prefix)),
            local: Atom::new(local),
        })
    }

    fn lookup(&self, prefix: Atom) -> Option<Atom> {
        self.bindings.lookup(prefix)
    }

    fn append(&mut self, node: S::Handle) {
        match self.open.last() {
            Some(open) => {
                let parent = open.handle.clone();
                self.sink.append(&parent, NodeOrText::Node(node));
            }
            None => {
                let doc = self.doc.clone();
                self.sink.append(&doc, NodeOrText::Node(node));
            }
        }
    }

    fn append_text(&mut self, text: &str) {
        if let Some(open) = self.open.last() {
            let parent = open.handle.clone();
            self.sink.append(&parent, NodeOrText::Text(text));
        }
    }

    fn pop(&mut self, handle: &S::Handle) {
        if self.sink.pop(handle) == ParseControl::Stop {
            self.stopped = true;
        }
    }
}

/// XML line-end handling: `\r\n` and a lone `\r` become `\n`.
fn normalize_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

fn is_xml_char(c: char) -> bool {
    matches!(c, '\u{9}' | '\u{A}' | '\u{D}'
        | '\u{20}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}'
        | '\u{10000}'..='\u{10FFFF}')
}

fn is_name_start_char(c: char) -> bool {
    matches!(c, ':' | 'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

fn is_name_char(c: char) -> bool {
    is_name_start_char(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}'
            | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

fn is_xml_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if is_name_start_char(first) => chars.all(is_name_char),
        _ => false,
    }
}
