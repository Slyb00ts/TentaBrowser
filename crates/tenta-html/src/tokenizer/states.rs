// Derived from html5ever's `src/tokenizer/states.rs`.
// Copyright 2014-2017 The html5ever Project Developers, MIT/Apache-2.0.
// See THIRD_PARTY_NOTICES.md ("Derived code").

//! Tokenizer states, one per state of the HTML tokenization algorithm.

/// Whether a script data escape is single or double.
#[derive(PartialEq, Eq, Copy, Clone, Hash, Debug)]
pub enum ScriptEscapeKind {
    /// `script data escaped`.
    Escaped,
    /// `script data double escaped`.
    DoubleEscaped,
}

/// Which DOCTYPE identifier is being read.
#[derive(PartialEq, Eq, Copy, Clone, Hash, Debug)]
pub enum DoctypeIdKind {
    /// The public identifier.
    Public,
    /// The system identifier.
    System,
}

/// The flavour of a text state that ends at a matching end tag.
#[derive(PartialEq, Eq, Copy, Clone, Hash, Debug)]
pub enum RawKind {
    /// RCDATA (`<title>`, `<textarea>`): character references are expanded.
    Rcdata,
    /// RAWTEXT (`<style>`, `<iframe>`, …).
    Rawtext,
    /// Script data (`<script>`).
    ScriptData,
    /// Script data inside a `<!--` escape.
    ScriptDataEscaped(ScriptEscapeKind),
}

/// How the current attribute value is quoted.
#[derive(PartialEq, Eq, Copy, Clone, Hash, Debug)]
pub enum AttrValueKind {
    /// No quotes.
    Unquoted,
    /// `'…'`.
    SingleQuoted,
    /// `"…"`.
    DoubleQuoted,
}

/// A tokenizer state.
#[derive(PartialEq, Eq, Copy, Clone, Hash, Debug)]
#[allow(missing_docs)]
pub enum State {
    Data,
    Plaintext,
    TagOpen,
    EndTagOpen,
    TagName,
    RawData(RawKind),
    RawLessThanSign(RawKind),
    RawEndTagOpen(RawKind),
    RawEndTagName(RawKind),
    ScriptDataEscapeStart(ScriptEscapeKind),
    ScriptDataEscapeStartDash,
    ScriptDataEscapedDash(ScriptEscapeKind),
    ScriptDataEscapedDashDash(ScriptEscapeKind),
    ScriptDataDoubleEscapeEnd,
    BeforeAttributeName,
    AttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    AttributeValue(AttrValueKind),
    AfterAttributeValueQuoted,
    SelfClosingStartTag,
    BogusComment,
    /// `<?` followed by a name: the target of a processing instruction. An
    /// invalid target falls back to [`State::BogusComment`], which keeps the
    /// text consumed so far in `current_comment`.
    ProcessingInstructionTarget,
    ProcessingInstructionDataStart,
    ProcessingInstructionData,
    /// Processing instruction data that just saw a `?`. If the next character is `>`,
    /// the PI terminates. Otherwise, the `?` is data and we reconsume the character.
    ProcessingInstructionDataAfterQuestion,
    MarkupDeclarationOpen,
    CommentStart,
    CommentStartDash,
    Comment,
    CommentLessThanSign,
    CommentLessThanSignBang,
    CommentLessThanSignBangDash,
    CommentLessThanSignBangDashDash,
    CommentEndDash,
    CommentEnd,
    CommentEndBang,
    Doctype,
    BeforeDoctypeName,
    DoctypeName,
    AfterDoctypeName,
    AfterDoctypeKeyword(DoctypeIdKind),
    BeforeDoctypeIdentifier(DoctypeIdKind),
    DoctypeIdentifierDoubleQuoted(DoctypeIdKind),
    DoctypeIdentifierSingleQuoted(DoctypeIdKind),
    AfterDoctypeIdentifier(DoctypeIdKind),
    BetweenDoctypePublicAndSystemIdentifiers,
    BogusDoctype,
    CdataSection,
    CdataSectionBracket,
    CdataSectionEnd,
}
