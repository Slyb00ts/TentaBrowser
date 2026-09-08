// Derived from html5ever's `src/tokenizer/interface.rs`.
// Copyright 2014-2017 The html5ever Project Developers, MIT/Apache-2.0.
// See THIRD_PARTY_NOTICES.md ("Derived code").

//! The tokens the tokenizer produces and the sink that receives them.

use std::borrow::Cow;

use tenta_core::Atom;

use crate::names::Local;
use crate::sink::Attribute;
use crate::tokenizer::states::RawKind;

/// A `DOCTYPE` token.
#[derive(PartialEq, Eq, Clone, Debug, Default)]
pub(crate) struct Doctype {
    pub name: Option<String>,
    pub public_id: Option<String>,
    pub system_id: Option<String>,
    pub force_quirks: bool,
}

/// Start or end tag.
#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub(crate) enum TagKind {
    StartTag,
    EndTag,
}

/// A tag token, with its attributes already de-duplicated.
#[derive(PartialEq, Eq, Clone, Debug)]
pub(crate) struct Tag {
    pub kind: TagKind,
    pub name: Atom,
    pub local: Local,
    pub self_closing: bool,
    pub attrs: Vec<Attribute>,
}

impl Tag {
    /// True when both tags have the same kind, name and attributes, ignoring
    /// attribute order and the self-closing flag (the "noah's ark" clause of
    /// the active formatting elements algorithm).
    pub(crate) fn equiv_modulo_attr_order(&self, other: &Tag) -> bool {
        if self.kind != other.kind || self.name != other.name {
            return false;
        }
        if self.attrs.len() != other.attrs.len() {
            return false;
        }
        self.attrs.iter().all(|a| {
            other
                .attrs
                .iter()
                .any(|b| a.name == b.name && a.value == b.value)
        })
    }

    /// The value of an attribute in no namespace, if present.
    pub(crate) fn attr(&self, local: Atom) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.name.prefix.is_none() && a.name.local == local)
            .map(|a| a.value.as_str())
    }
}

/// One token. Text and comments borrow from the current input chunk or from
/// the tokenizer's own scratch buffer, so nothing is copied on the way to the
/// tree sink.
#[derive(PartialEq, Eq, Debug)]
pub(crate) enum Token<'a> {
    Doctype(Doctype),
    Tag(Tag),
    Comment(&'a str),
    /// `<?target data>`; the HTML standard's bogus comment rule still applies
    /// to a target that is not an ASCII name.
    ProcessingInstruction {
        target: &'a str,
        data: &'a str,
    },
    Characters(&'a str),
    /// A literal `U+0000` in the data state.
    NullCharacter,
    Eof,
    ParseError(Cow<'static, str>),
}

/// What the token sink wants the tokenizer to do next.
#[must_use]
pub(crate) enum TokenSinkResult<Handle> {
    Continue,
    /// A `<script>` element was closed; Tier 2 runs it before tokenizing on.
    Script(Handle),
    Plaintext,
    RawData(RawKind),
    /// A `<meta>` declared an encoding label.
    EncodingIndicator(String),
    /// The tree sink asked for the parse to end.
    Stop,
}

/// A consumer of tokens.
pub(crate) trait TokenSink {
    /// Node handle type of the underlying tree sink.
    type Handle;

    /// Processes one token.
    fn process_token(
        &mut self,
        token: Token<'_>,
        line_number: u64,
    ) -> TokenSinkResult<Self::Handle>;

    /// Signals the end of the input.
    fn end(&mut self) {}

    /// Used by the markup declaration open state to decide whether `<![CDATA[`
    /// starts a CDATA section or a bogus comment.
    fn adjusted_current_node_present_but_not_in_html_namespace(&self) -> bool {
        false
    }
}
