// Derived from html5ever's `src/tokenizer/mod.rs`.
// Copyright 2014-2017 The html5ever Project Developers, MIT/Apache-2.0.
// See THIRD_PARTY_NOTICES.md ("Derived code").

//! The HTML tokenizer.
//!
//! The state machine follows html5ever, with two changes
//! (`docs/spec/04-dom-and-css.md` §3): the text states scan for their
//! terminators with [`crate::scan`] instead of stepping character by
//! character, and character tokens are `&str` slices of the input chunk
//! rather than owned buffers.

use std::borrow::Cow::{self, Borrowed};
use std::collections::HashSet;
use std::mem;

use crate::input::Input;
use crate::names::NameCache;
use crate::scan::{CharSet, ScanMode, count_newlines, find_next};
use crate::sink::{Attribute, QualName};
use tenta_core::Atom;
use tenta_core::atom::atoms as core_atoms;

/// Above this many attributes on one tag the linear duplicate scan becomes
/// the dominant cost of tokenizing it, so names go into a hash set instead
/// (html5ever does the same). A tag with thousands of attributes is legal
/// markup and must stay linear (`docs/spec/13-performance.md` §4).
const ATTR_SET_THRESHOLD: usize = 16;

pub(crate) mod char_ref;
#[cfg(test)]
mod conformance;
mod entities;
pub(crate) mod interface;
pub mod states;

pub(crate) use interface::{Doctype, Tag, TagKind, Token, TokenSink, TokenSinkResult};

use char_ref::{CharRef, CharRefTokenizer};
use states::{
    AttrValueKind::{DoubleQuoted, SingleQuoted, Unquoted},
    DoctypeIdKind::{Public, System},
    RawKind::{Rawtext, Rcdata, ScriptData, ScriptDataEscaped},
    ScriptEscapeKind::{DoubleEscaped, Escaped},
    State,
};

/// What one step of the state machine decided.
pub(crate) enum ProcessResult<Handle> {
    /// Step again immediately.
    Continue,
    /// Out of input; wait for the next chunk.
    Suspend,
    /// A `<script>` finished; the caller decides whether to run it.
    Script(Handle),
    /// A `<meta>` declared an encoding label.
    EncodingIndicator(String),
    /// The tree sink asked for the parse to end.
    Stop,
}

/// The result of feeding one chunk.
pub(crate) enum TokenizerResult<Handle> {
    /// The chunk was fully consumed.
    Done,
    Script(Handle),
    EncodingIndicator(String),
    Stopped,
}

fn option_push(target: &mut Option<String>, c: char) {
    match target {
        Some(s) => s.push(c),
        None => *target = Some(String::from(c)),
    }
}

/// If `c` is an ASCII letter, its lowercase form.
#[inline]
fn lower_ascii_letter(c: char) -> Option<char> {
    match c {
        'a'..='z' => Some(c),
        'A'..='Z' => Some((c as u8 - b'A' + b'a') as char),
        _ => None,
    }
}

/// Tokenizer configuration.
#[derive(Clone, Debug)]
pub(crate) struct TokenizerOpts {
    /// Drop a leading `U+FEFF`.
    pub discard_bom: bool,
    /// Initial state (fragment parsing and the tokenizer test suite).
    pub initial_state: Option<State>,
    /// Last start tag name, for "appropriate end tag" checks in fragments.
    pub last_start_tag_name: Option<String>,
    /// Which text scanner to use.
    pub scan_mode: ScanMode,
}

impl Default for TokenizerOpts {
    fn default() -> Self {
        TokenizerOpts {
            discard_bom: true,
            initial_state: None,
            last_start_tag_name: None,
            scan_mode: ScanMode::default(),
        }
    }
}

/// The HTML tokenizer.
pub(crate) struct Tokenizer {
    state: State,
    at_eof: bool,
    char_ref_tokenizer: Option<CharRefTokenizer>,
    /// Just-consumed character, re-read when `reconsume` is set.
    current_char: char,
    reconsume: bool,
    /// A `\r` was translated to `\n`; a following `\n` is dropped.
    pub(crate) ignore_lf: bool,
    discard_bom: bool,
    current_tag_kind: TagKind,
    current_tag_name: String,
    current_tag_self_closing: bool,
    current_tag_attrs: Vec<Attribute>,
    /// Names already seen on the current tag, used instead of the linear scan
    /// once the tag passes [`ATTR_SET_THRESHOLD`]. Filled lazily, so an
    /// ordinary tag never allocates it.
    current_tag_attr_names: HashSet<Atom>,
    current_attr_name: String,
    current_attr_value: String,
    current_comment: String,
    /// Target and data of the processing instruction being tokenized.
    current_pi_target: String,
    current_pi_data: String,
    current_doctype: Doctype,
    last_start_tag_name: Option<String>,
    temp_buf: String,
    current_line: u64,
    scan_mode: ScanMode,
    names: NameCache,
}

/// One unit of input from a text state.
enum Scanned<'a> {
    Char(char),
    Run(&'a str),
}

macro_rules! get_char {
    ($me:expr, $input:expr) => {
        match $me.get_char($input) {
            Some(c) => c,
            None => return ProcessResult::Suspend,
        }
    };
}

macro_rules! peek_char {
    ($me:expr, $input:expr) => {
        match $me.peek($input) {
            Some(c) => c,
            None => return ProcessResult::Suspend,
        }
    };
}

macro_rules! scan_run {
    ($me:expr, $input:expr, $set:expr) => {
        match $me.scan($input, $set) {
            Some(s) => s,
            None => return ProcessResult::Suspend,
        }
    };
}

macro_rules! eat {
    ($me:expr, $input:expr, $pat:expr) => {
        match $me.eat($input, $pat, true) {
            Some(b) => b,
            None => return ProcessResult::Suspend,
        }
    };
}

macro_rules! eat_exact {
    ($me:expr, $input:expr, $pat:expr) => {
        match $me.eat($input, $pat, false) {
            Some(b) => b,
            None => return ProcessResult::Suspend,
        }
    };
}

macro_rules! to {
    ($me:expr, $state:expr) => {{
        $me.state = $state;
        return ProcessResult::Continue;
    }};
}

macro_rules! reconsume {
    ($me:expr, $state:expr) => {{
        $me.reconsume = true;
        $me.state = $state;
        return ProcessResult::Continue;
    }};
}

impl Tokenizer {
    /// A tokenizer in its initial state.
    pub(crate) fn new(opts: TokenizerOpts) -> Tokenizer {
        Tokenizer {
            state: opts.initial_state.unwrap_or(State::Data),
            at_eof: false,
            char_ref_tokenizer: None,
            current_char: '\0',
            reconsume: false,
            ignore_lf: false,
            discard_bom: opts.discard_bom,
            current_tag_kind: TagKind::StartTag,
            current_tag_name: String::new(),
            current_tag_self_closing: false,
            current_tag_attrs: Vec::new(),
            current_tag_attr_names: HashSet::new(),
            current_attr_name: String::new(),
            current_attr_value: String::new(),
            current_comment: String::new(),
            current_pi_target: String::new(),
            current_pi_data: String::new(),
            current_doctype: Doctype::default(),
            last_start_tag_name: opts.last_start_tag_name,
            temp_buf: String::new(),
            current_line: 1,
            scan_mode: opts.scan_mode,
            names: NameCache::default(),
        }
    }

    /// Bytes before the cursor that the caller must keep so the tokenizer can
    /// still move the cursor backwards on the next chunk.
    pub(crate) fn rewind_reserve(&self) -> usize {
        self.char_ref_tokenizer
            .as_ref()
            .map_or(0, CharRefTokenizer::rewind_reserve)
    }

    /// Feeds one chunk.
    pub(crate) fn feed<S: TokenSink>(
        &mut self,
        input: &mut Input<'_>,
        sink: &mut S,
    ) -> TokenizerResult<S::Handle> {
        self.at_eof = input.at_eof();
        if input.rest().is_empty() {
            return TokenizerResult::Done;
        }
        if self.discard_bom {
            match input.peek() {
                Some('\u{feff}') => {
                    input.advance('\u{feff}'.len_utf8());
                    self.discard_bom = false;
                }
                Some(_) => self.discard_bom = false,
                None => return TokenizerResult::Done,
            }
        }
        self.run(input, sink)
    }

    fn run<S: TokenSink>(
        &mut self,
        input: &mut Input<'_>,
        sink: &mut S,
    ) -> TokenizerResult<S::Handle> {
        loop {
            match self.step(input, sink) {
                ProcessResult::Continue => (),
                ProcessResult::Suspend => return TokenizerResult::Done,
                ProcessResult::Script(node) => return TokenizerResult::Script(node),
                ProcessResult::EncodingIndicator(label) => {
                    return TokenizerResult::EncodingIndicator(label);
                }
                ProcessResult::Stop => return TokenizerResult::Stopped,
            }
        }
    }

    /// Signals the end of the document and drains the state machine.
    ///
    /// `input` must be the cursor left by the last [`Tokenizer::feed`]: the
    /// character reference sub-tokenizer un-consumes through it, so it needs
    /// the retained tail of the last chunk rather than an empty buffer.
    pub(crate) fn end<S: TokenSink>(
        &mut self,
        input: &mut Input<'_>,
        sink: &mut S,
    ) -> TokenizerResult<S::Handle> {
        if let Some(mut char_ref_tokenizer) = self.char_ref_tokenizer.take() {
            let char_ref = char_ref_tokenizer.end_of_file(self, input, sink);
            if let Some(ProcessResult::Stop) = self.process_char_ref(char_ref, sink) {
                return TokenizerResult::Stopped;
            }
        }

        self.at_eof = true;
        match self.run(input, sink) {
            TokenizerResult::Done => {}
            other => return other,
        }

        loop {
            match self.eof_step(sink) {
                ProcessResult::Continue => (),
                ProcessResult::Suspend => break,
                ProcessResult::Script(node) => return TokenizerResult::Script(node),
                ProcessResult::EncodingIndicator(label) => {
                    return TokenizerResult::EncodingIndicator(label);
                }
                ProcessResult::Stop => return TokenizerResult::Stopped,
            }
        }

        sink.end();
        TokenizerResult::Done
    }

    //§ preprocessing-the-input-stream

    fn preprocess(&mut self, mut c: char, input: &mut Input<'_>) -> Option<char> {
        if self.ignore_lf {
            self.ignore_lf = false;
            if c == '\n' {
                c = input.next_char()?;
            }
        }
        if c == '\r' {
            self.ignore_lf = true;
            c = '\n';
        }
        if c == '\n' {
            self.current_line += 1;
        }
        self.current_char = c;
        Some(c)
    }

    fn get_char(&mut self, input: &mut Input<'_>) -> Option<char> {
        if self.reconsume {
            self.reconsume = false;
            return Some(self.current_char);
        }
        let c = input.next_char()?;
        self.preprocess(c, input)
    }

    /// The next raw character, without newline normalisation.
    pub(crate) fn peek(&self, input: &Input<'_>) -> Option<char> {
        if self.reconsume {
            Some(self.current_char)
        } else {
            input.peek()
        }
    }

    /// Discards one raw character; pairs with [`Tokenizer::peek`].
    pub(crate) fn discard_char(&mut self, input: &mut Input<'_>) {
        if self.reconsume {
            self.reconsume = false;
        } else {
            input.next_char();
        }
    }

    /// Scans a run of text that contains no member of `set`, or returns the
    /// single next character when the run would be empty.
    fn scan<'a>(&mut self, input: &mut Input<'a>, set: CharSet) -> Option<Scanned<'a>> {
        if self.reconsume || self.ignore_lf {
            return self.get_char(input).map(Scanned::Char);
        }
        let text = input.text();
        let pos = input.pos();
        let bytes = &text.as_bytes()[pos..];
        if bytes.is_empty() {
            return None;
        }
        let len = find_next(self.scan_mode, bytes, set);
        if len == 0 {
            return self.get_char(input).map(Scanned::Char);
        }
        // Every member of every set is ASCII, so `len` is a char boundary.
        let run = &text[pos..pos + len];
        input.advance(len);
        self.current_line += count_newlines(self.scan_mode, run.as_bytes());
        Some(Scanned::Run(run))
    }

    /// ASCII(-insensitive) match against the next `pat.len()` bytes.
    /// `None` means "need more input".
    fn eat(&mut self, input: &mut Input<'_>, pat: &str, case_insensitive: bool) -> Option<bool> {
        debug_assert!(!self.reconsume, "eat() does not handle reconsume");
        if self.ignore_lf {
            self.ignore_lf = false;
            if input.peek() == Some('\n') {
                input.next_char();
            }
        }
        let rest = input.rest().as_bytes();
        let pattern = pat.as_bytes();
        let matches = |a: &[u8], b: &[u8]| {
            if case_insensitive {
                a.eq_ignore_ascii_case(b)
            } else {
                a == b
            }
        };
        if rest.len() >= pattern.len() {
            if matches(&rest[..pattern.len()], pattern) {
                input.advance(pattern.len());
                Some(true)
            } else {
                Some(false)
            }
        } else if matches(rest, &pattern[..rest.len()]) && !self.at_eof {
            None
        } else {
            Some(false)
        }
    }

    //§ tokenization

    fn process_token<S: TokenSink>(
        &mut self,
        sink: &mut S,
        token: Token<'_>,
    ) -> TokenSinkResult<S::Handle> {
        sink.process_token(token, self.current_line)
    }

    /// Emits a token that cannot change the tokenizer state.
    fn emit<S: TokenSink>(
        &mut self,
        sink: &mut S,
        token: Token<'_>,
    ) -> Option<ProcessResult<S::Handle>> {
        match sink.process_token(token, self.current_line) {
            TokenSinkResult::Continue => None,
            TokenSinkResult::Stop => Some(ProcessResult::Stop),
            _ => unreachable!("only tag tokens can change the tokenizer state"),
        }
    }

    pub(crate) fn emit_error<S: TokenSink>(&mut self, sink: &mut S, msg: Cow<'static, str>) {
        let _ = sink.process_token(Token::ParseError(msg), self.current_line);
    }

    fn bad_char_error<S: TokenSink>(&mut self, sink: &mut S) {
        let c = self.current_char;
        let state = self.state;
        self.emit_error(sink, Cow::from(format!("Saw {c} in state {state:?}")));
    }

    fn bad_eof_error<S: TokenSink>(&mut self, sink: &mut S) {
        let state = self.state;
        self.emit_error(sink, Cow::from(format!("Saw EOF in state {state:?}")));
    }

    fn emit_char<S: TokenSink>(
        &mut self,
        sink: &mut S,
        c: char,
    ) -> Option<ProcessResult<S::Handle>> {
        if c == '\0' {
            return self.emit(sink, Token::NullCharacter);
        }
        let mut buf = [0u8; 4];
        let text = c.encode_utf8(&mut buf);
        match sink.process_token(Token::Characters(text), self.current_line) {
            TokenSinkResult::Continue => None,
            TokenSinkResult::Stop => Some(ProcessResult::Stop),
            _ => unreachable!("only tag tokens can change the tokenizer state"),
        }
    }

    fn emit_chars<S: TokenSink>(
        &mut self,
        sink: &mut S,
        text: &str,
    ) -> Option<ProcessResult<S::Handle>> {
        if text.is_empty() {
            return None;
        }
        match sink.process_token(Token::Characters(text), self.current_line) {
            TokenSinkResult::Continue => None,
            TokenSinkResult::Stop => Some(ProcessResult::Stop),
            _ => unreachable!("only tag tokens can change the tokenizer state"),
        }
    }

    fn emit_temp_buf<S: TokenSink>(&mut self, sink: &mut S) -> Option<ProcessResult<S::Handle>> {
        let buf = mem::take(&mut self.temp_buf);
        let result = self.emit_chars(sink, &buf);
        self.temp_buf = buf;
        self.temp_buf.clear();
        result
    }

    fn emit_current_comment<S: TokenSink>(
        &mut self,
        sink: &mut S,
    ) -> Option<ProcessResult<S::Handle>> {
        let comment = mem::take(&mut self.current_comment);
        let result = match sink.process_token(Token::Comment(&comment), self.current_line) {
            TokenSinkResult::Continue => None,
            TokenSinkResult::Stop => Some(ProcessResult::Stop),
            _ => unreachable!("only tag tokens can change the tokenizer state"),
        };
        self.current_comment = comment;
        self.current_comment.clear();
        result
    }

    fn emit_current_pi<S: TokenSink>(&mut self, sink: &mut S) -> Option<ProcessResult<S::Handle>> {
        let target = mem::take(&mut self.current_pi_target);
        let data = mem::take(&mut self.current_pi_data);
        let token = Token::ProcessingInstruction {
            target: &target,
            data: &data,
        };
        let result = match sink.process_token(token, self.current_line) {
            TokenSinkResult::Continue => None,
            TokenSinkResult::Stop => Some(ProcessResult::Stop),
            _ => unreachable!("only tag tokens can change the tokenizer state"),
        };
        self.current_pi_target = target;
        self.current_pi_target.clear();
        self.current_pi_data = data;
        self.current_pi_data.clear();
        self.current_comment.clear();
        result
    }

    /// Whether the target collected so far can start a processing
    /// instruction: a non-empty ASCII name that is not reserved for XML.
    fn pi_target_is_valid(&self) -> bool {
        !self.current_pi_target.is_empty()
            && !(self.current_pi_target.len() >= 3
                && self.current_pi_target[..3].eq_ignore_ascii_case("xml"))
    }

    fn emit_current_doctype<S: TokenSink>(
        &mut self,
        sink: &mut S,
    ) -> Option<ProcessResult<S::Handle>> {
        let doctype = mem::take(&mut self.current_doctype);
        self.emit(sink, Token::Doctype(doctype))
    }

    fn emit_eof<S: TokenSink>(&mut self, sink: &mut S) -> Option<ProcessResult<S::Handle>> {
        self.emit(sink, Token::Eof)
    }

    fn discard_tag(&mut self) {
        self.current_tag_name.clear();
        self.current_tag_self_closing = false;
        self.current_tag_attrs.clear();
        self.current_tag_attr_names.clear();
    }

    fn create_tag(&mut self, kind: TagKind, c: char) {
        self.discard_tag();
        self.current_tag_name.push(c);
        self.current_tag_kind = kind;
    }

    fn have_appropriate_end_tag(&self) -> bool {
        match &self.last_start_tag_name {
            Some(last) => {
                self.current_tag_kind == TagKind::EndTag && *last == self.current_tag_name
            }
            None => false,
        }
    }

    fn create_attribute<S: TokenSink>(&mut self, sink: &mut S, c: char) {
        self.finish_attribute(sink);
        self.current_attr_name.push(c);
    }

    fn finish_attribute<S: TokenSink>(&mut self, sink: &mut S) {
        if self.current_attr_name.is_empty() {
            return;
        }
        let (local, _) = self.names.lookup(&self.current_attr_name);
        self.current_attr_name.clear();
        let duplicate = if self.current_tag_attrs.len() > ATTR_SET_THRESHOLD {
            if self.current_tag_attr_names.is_empty() {
                self.current_tag_attr_names.extend(
                    self.current_tag_attrs
                        .iter()
                        .filter(|a| a.name.prefix.is_none())
                        .map(|a| a.name.local),
                );
            }
            !self.current_tag_attr_names.insert(local)
        } else {
            self.current_tag_attrs
                .iter()
                .any(|a| a.name.prefix.is_none() && a.name.local == local)
        };
        if duplicate {
            self.emit_error(sink, Borrowed("Duplicate attribute"));
            self.current_attr_value.clear();
        } else {
            let value = mem::take(&mut self.current_attr_value);
            self.current_tag_attrs.push(Attribute {
                // The tree builder adjusts the namespace for foreign content.
                name: QualName::new(core_atoms::EMPTY, local),
                value,
            });
        }
    }

    fn emit_current_tag<S: TokenSink>(&mut self, sink: &mut S) -> ProcessResult<S::Handle> {
        self.finish_attribute(sink);

        let (name, local) = self.names.lookup(&self.current_tag_name);
        match self.current_tag_kind {
            TagKind::StartTag => {
                self.last_start_tag_name = Some(mem::take(&mut self.current_tag_name));
            }
            TagKind::EndTag => {
                if !self.current_tag_attrs.is_empty() {
                    self.emit_error(sink, Borrowed("Attributes on an end tag"));
                }
                if self.current_tag_self_closing {
                    self.emit_error(sink, Borrowed("Self-closing end tag"));
                }
                self.current_tag_name.clear();
            }
        }

        let tag = Tag {
            kind: self.current_tag_kind,
            name,
            local,
            self_closing: self.current_tag_self_closing,
            attrs: mem::take(&mut self.current_tag_attrs),
        };
        self.current_tag_attr_names.clear();
        self.current_tag_self_closing = false;

        match self.process_token(sink, Token::Tag(tag)) {
            TokenSinkResult::Continue => ProcessResult::Continue,
            TokenSinkResult::Plaintext => {
                self.state = State::Plaintext;
                ProcessResult::Continue
            }
            TokenSinkResult::Script(node) => {
                self.state = State::Data;
                ProcessResult::Script(node)
            }
            TokenSinkResult::RawData(kind) => {
                self.state = State::RawData(kind);
                ProcessResult::Continue
            }
            TokenSinkResult::EncodingIndicator(label) => ProcessResult::EncodingIndicator(label),
            TokenSinkResult::Stop => ProcessResult::Stop,
        }
    }

    fn doctype_id(&mut self, kind: states::DoctypeIdKind) -> &mut Option<String> {
        match kind {
            Public => &mut self.current_doctype.public_id,
            System => &mut self.current_doctype.system_id,
        }
    }

    fn clear_doctype_id(&mut self, kind: states::DoctypeIdKind) {
        match self.doctype_id(kind) {
            Some(s) => s.clear(),
            slot => *slot = Some(String::new()),
        }
    }

    fn start_consuming_character_reference(&mut self) {
        debug_assert!(
            self.char_ref_tokenizer.is_none(),
            "nested character references are impossible"
        );
        let in_attribute = matches!(self.state, State::AttributeValue(_));
        self.char_ref_tokenizer = Some(CharRefTokenizer::new(in_attribute));
    }

    fn step_char_ref_tokenizer<S: TokenSink>(
        &mut self,
        input: &mut Input<'_>,
        sink: &mut S,
    ) -> ProcessResult<S::Handle> {
        let mut tokenizer = self
            .char_ref_tokenizer
            .take()
            .expect("no character reference in progress");
        match tokenizer.step(self, input, sink) {
            char_ref::Status::Done(char_ref) => {
                if let Some(result) = self.process_char_ref(char_ref, sink) {
                    return result;
                }
                ProcessResult::Continue
            }
            char_ref::Status::Stuck => {
                self.char_ref_tokenizer = Some(tokenizer);
                ProcessResult::Suspend
            }
            char_ref::Status::Progress => {
                self.char_ref_tokenizer = Some(tokenizer);
                ProcessResult::Continue
            }
        }
    }

    fn process_char_ref<S: TokenSink>(
        &mut self,
        char_ref: CharRef,
        sink: &mut S,
    ) -> Option<ProcessResult<S::Handle>> {
        let CharRef {
            mut chars,
            mut num_chars,
        } = char_ref;
        if num_chars == 0 {
            chars[0] = '&';
            num_chars = 1;
        }
        for c in chars.iter().take(num_chars as usize).copied() {
            match self.state {
                State::Data | State::RawData(Rcdata) => {
                    if let Some(result) = self.emit_char(sink, c) {
                        return Some(result);
                    }
                }
                State::AttributeValue(_) => self.current_attr_value.push(c),
                state => unreachable!("state {state:?} is not a character reference return state"),
            }
        }
        None
    }
}

macro_rules! emit {
    ($me:expr, $sink:expr, $call:ident ( $($arg:expr),* )) => {
        if let Some(result) = $me.$call($sink $(, $arg)*) {
            return result;
        }
    };
}

impl Tokenizer {
    /// Runs the state machine until it needs more input or the sink
    /// interrupts it.
    //
    // Every state is a `loop` so that an arm can fall through to the next
    // character without re-entering the dispatch, the way html5ever's `go!`
    // macros work. States whose arms all transition away never repeat, which
    // is what `never_loop` reports; keeping the shape uniform is what makes
    // the states readable against the standard's prose.
    #[allow(clippy::never_loop)]
    fn step<S: TokenSink>(
        &mut self,
        input: &mut Input<'_>,
        sink: &mut S,
    ) -> ProcessResult<S::Handle> {
        if self.char_ref_tokenizer.is_some() {
            return self.step_char_ref_tokenizer(input, sink);
        }

        match self.state {
            //§ data-state
            State::Data => loop {
                match scan_run!(self, input, CharSet::DataOrRcdata) {
                    Scanned::Char('\0') => {
                        self.bad_char_error(sink);
                        emit!(self, sink, emit_char('\0'));
                    }
                    Scanned::Char('&') => {
                        self.start_consuming_character_reference();
                        return ProcessResult::Continue;
                    }
                    Scanned::Char('<') => to!(self, State::TagOpen),
                    Scanned::Char(c) => emit!(self, sink, emit_char(c)),
                    Scanned::Run(text) => emit!(self, sink, emit_chars(text)),
                }
            },

            //§ rcdata-state
            State::RawData(Rcdata) => loop {
                match scan_run!(self, input, CharSet::DataOrRcdata) {
                    Scanned::Char('\0') => {
                        self.bad_char_error(sink);
                        emit!(self, sink, emit_char('\u{fffd}'));
                    }
                    Scanned::Char('&') => {
                        self.start_consuming_character_reference();
                        return ProcessResult::Continue;
                    }
                    Scanned::Char('<') => to!(self, State::RawLessThanSign(Rcdata)),
                    Scanned::Char(c) => emit!(self, sink, emit_char(c)),
                    Scanned::Run(text) => emit!(self, sink, emit_chars(text)),
                }
            },

            //§ rawtext-state script-data-state
            State::RawData(kind @ (Rawtext | ScriptData)) => loop {
                match scan_run!(self, input, CharSet::Rawtext) {
                    Scanned::Char('\0') => {
                        self.bad_char_error(sink);
                        emit!(self, sink, emit_char('\u{fffd}'));
                    }
                    Scanned::Char('<') => to!(self, State::RawLessThanSign(kind)),
                    Scanned::Char(c) => emit!(self, sink, emit_char(c)),
                    Scanned::Run(text) => emit!(self, sink, emit_chars(text)),
                }
            },

            //§ script-data-escaped-state script-data-double-escaped-state
            State::RawData(ScriptDataEscaped(escape)) => loop {
                match scan_run!(self, input, CharSet::ScriptEscaped) {
                    Scanned::Char('\0') => {
                        self.bad_char_error(sink);
                        emit!(self, sink, emit_char('\u{fffd}'));
                    }
                    Scanned::Char('-') => {
                        emit!(self, sink, emit_char('-'));
                        to!(self, State::ScriptDataEscapedDash(escape));
                    }
                    Scanned::Char('<') => {
                        if escape == DoubleEscaped {
                            emit!(self, sink, emit_char('<'));
                        }
                        to!(self, State::RawLessThanSign(ScriptDataEscaped(escape)));
                    }
                    Scanned::Char(c) => emit!(self, sink, emit_char(c)),
                    Scanned::Run(text) => emit!(self, sink, emit_chars(text)),
                }
            },

            //§ plaintext-state
            State::Plaintext => loop {
                match scan_run!(self, input, CharSet::Plaintext) {
                    Scanned::Char('\0') => {
                        self.bad_char_error(sink);
                        emit!(self, sink, emit_char('\u{fffd}'));
                    }
                    Scanned::Char(c) => emit!(self, sink, emit_char(c)),
                    Scanned::Run(text) => emit!(self, sink, emit_chars(text)),
                }
            },

            //§ tag-open-state
            State::TagOpen => loop {
                match get_char!(self, input) {
                    '!' => to!(self, State::MarkupDeclarationOpen),
                    '/' => to!(self, State::EndTagOpen),
                    '?' => {
                        self.bad_char_error(sink);
                        self.current_comment.clear();
                        self.current_comment.push('?');
                        self.current_pi_target.clear();
                        self.current_pi_data.clear();
                        to!(self, State::ProcessingInstructionTarget);
                    }
                    c => match lower_ascii_letter(c) {
                        Some(lower) => {
                            self.create_tag(TagKind::StartTag, lower);
                            to!(self, State::TagName);
                        }
                        None => {
                            self.bad_char_error(sink);
                            emit!(self, sink, emit_char('<'));
                            reconsume!(self, State::Data);
                        }
                    },
                }
            },

            //§ end-tag-open-state
            State::EndTagOpen => loop {
                match get_char!(self, input) {
                    '>' => {
                        self.bad_char_error(sink);
                        to!(self, State::Data);
                    }
                    c => match lower_ascii_letter(c) {
                        Some(lower) => {
                            self.create_tag(TagKind::EndTag, lower);
                            to!(self, State::TagName);
                        }
                        None => {
                            self.bad_char_error(sink);
                            self.current_comment.clear();
                            reconsume!(self, State::BogusComment);
                        }
                    },
                }
            },

            //§ tag-name-state
            State::TagName => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => to!(self, State::BeforeAttributeName),
                    '/' => to!(self, State::SelfClosingStartTag),
                    '>' => {
                        self.state = State::Data;
                        return self.emit_current_tag(sink);
                    }
                    '\0' => {
                        self.bad_char_error(sink);
                        self.current_tag_name.push('\u{fffd}');
                    }
                    c => self.current_tag_name.push(c.to_ascii_lowercase()),
                }
            },

            //§ script-data-escaped-less-than-sign-state
            State::RawLessThanSign(ScriptDataEscaped(Escaped)) => loop {
                match get_char!(self, input) {
                    '/' => {
                        self.temp_buf.clear();
                        to!(self, State::RawEndTagOpen(ScriptDataEscaped(Escaped)));
                    }
                    c => match lower_ascii_letter(c) {
                        Some(lower) => {
                            self.temp_buf.clear();
                            self.temp_buf.push(lower);
                            emit!(self, sink, emit_char('<'));
                            emit!(self, sink, emit_char(c));
                            to!(self, State::ScriptDataEscapeStart(DoubleEscaped));
                        }
                        None => {
                            emit!(self, sink, emit_char('<'));
                            reconsume!(self, State::RawData(ScriptDataEscaped(Escaped)));
                        }
                    },
                }
            },

            //§ script-data-double-escaped-less-than-sign-state
            State::RawLessThanSign(ScriptDataEscaped(DoubleEscaped)) => loop {
                match get_char!(self, input) {
                    '/' => {
                        self.temp_buf.clear();
                        emit!(self, sink, emit_char('/'));
                        to!(self, State::ScriptDataDoubleEscapeEnd);
                    }
                    _ => reconsume!(self, State::RawData(ScriptDataEscaped(DoubleEscaped))),
                }
            },

            //§ rcdata-less-than-sign-state rawtext-less-than-sign-state script-data-less-than-sign-state
            State::RawLessThanSign(kind) => loop {
                match get_char!(self, input) {
                    '/' => {
                        self.temp_buf.clear();
                        to!(self, State::RawEndTagOpen(kind));
                    }
                    '!' if kind == ScriptData => {
                        emit!(self, sink, emit_char('<'));
                        emit!(self, sink, emit_char('!'));
                        to!(self, State::ScriptDataEscapeStart(Escaped));
                    }
                    _ => {
                        emit!(self, sink, emit_char('<'));
                        reconsume!(self, State::RawData(kind));
                    }
                }
            },

            //§ rcdata-end-tag-open-state rawtext-end-tag-open-state script-data-end-tag-open-state
            State::RawEndTagOpen(kind) => loop {
                let c = get_char!(self, input);
                match lower_ascii_letter(c) {
                    Some(lower) => {
                        self.create_tag(TagKind::EndTag, lower);
                        self.temp_buf.push(c);
                        to!(self, State::RawEndTagName(kind));
                    }
                    None => {
                        emit!(self, sink, emit_char('<'));
                        emit!(self, sink, emit_char('/'));
                        reconsume!(self, State::RawData(kind));
                    }
                }
            },

            //§ rcdata-end-tag-name-state rawtext-end-tag-name-state script-data-end-tag-name-state
            State::RawEndTagName(kind) => loop {
                let c = get_char!(self, input);
                if self.have_appropriate_end_tag() {
                    match c {
                        '\t' | '\n' | '\x0C' | ' ' => {
                            self.temp_buf.clear();
                            to!(self, State::BeforeAttributeName);
                        }
                        '/' => {
                            self.temp_buf.clear();
                            to!(self, State::SelfClosingStartTag);
                        }
                        '>' => {
                            self.temp_buf.clear();
                            self.state = State::Data;
                            return self.emit_current_tag(sink);
                        }
                        _ => (),
                    }
                }
                match lower_ascii_letter(c) {
                    Some(lower) => {
                        self.current_tag_name.push(lower);
                        self.temp_buf.push(c);
                    }
                    None => {
                        self.discard_tag();
                        emit!(self, sink, emit_char('<'));
                        emit!(self, sink, emit_char('/'));
                        emit!(self, sink, emit_temp_buf());
                        reconsume!(self, State::RawData(kind));
                    }
                }
            },

            //§ script-data-double-escape-start-state
            State::ScriptDataEscapeStart(DoubleEscaped) => loop {
                let c = get_char!(self, input);
                match c {
                    '\t' | '\n' | '\x0C' | ' ' | '/' | '>' => {
                        let escape = if self.temp_buf == "script" {
                            DoubleEscaped
                        } else {
                            Escaped
                        };
                        emit!(self, sink, emit_char(c));
                        to!(self, State::RawData(ScriptDataEscaped(escape)));
                    }
                    _ => match lower_ascii_letter(c) {
                        Some(lower) => {
                            self.temp_buf.push(lower);
                            emit!(self, sink, emit_char(c));
                        }
                        None => reconsume!(self, State::RawData(ScriptDataEscaped(Escaped))),
                    },
                }
            },

            //§ script-data-escape-start-state
            State::ScriptDataEscapeStart(Escaped) => loop {
                match get_char!(self, input) {
                    '-' => {
                        emit!(self, sink, emit_char('-'));
                        to!(self, State::ScriptDataEscapeStartDash);
                    }
                    _ => reconsume!(self, State::RawData(ScriptData)),
                }
            },

            //§ script-data-escape-start-dash-state
            State::ScriptDataEscapeStartDash => loop {
                match get_char!(self, input) {
                    '-' => {
                        emit!(self, sink, emit_char('-'));
                        to!(self, State::ScriptDataEscapedDashDash(Escaped));
                    }
                    _ => reconsume!(self, State::RawData(ScriptData)),
                }
            },

            //§ script-data-escaped-dash-state script-data-double-escaped-dash-state
            State::ScriptDataEscapedDash(escape) => loop {
                match get_char!(self, input) {
                    '-' => {
                        emit!(self, sink, emit_char('-'));
                        to!(self, State::ScriptDataEscapedDashDash(escape));
                    }
                    '<' => {
                        if escape == DoubleEscaped {
                            emit!(self, sink, emit_char('<'));
                        }
                        to!(self, State::RawLessThanSign(ScriptDataEscaped(escape)));
                    }
                    '\0' => {
                        self.bad_char_error(sink);
                        emit!(self, sink, emit_char('\u{fffd}'));
                        to!(self, State::RawData(ScriptDataEscaped(escape)));
                    }
                    c => {
                        emit!(self, sink, emit_char(c));
                        to!(self, State::RawData(ScriptDataEscaped(escape)));
                    }
                }
            },

            //§ script-data-escaped-dash-dash-state script-data-double-escaped-dash-dash-state
            State::ScriptDataEscapedDashDash(escape) => loop {
                match get_char!(self, input) {
                    '-' => emit!(self, sink, emit_char('-')),
                    '<' => {
                        if escape == DoubleEscaped {
                            emit!(self, sink, emit_char('<'));
                        }
                        to!(self, State::RawLessThanSign(ScriptDataEscaped(escape)));
                    }
                    '>' => {
                        emit!(self, sink, emit_char('>'));
                        to!(self, State::RawData(ScriptData));
                    }
                    '\0' => {
                        self.bad_char_error(sink);
                        emit!(self, sink, emit_char('\u{fffd}'));
                        to!(self, State::RawData(ScriptDataEscaped(escape)));
                    }
                    c => {
                        emit!(self, sink, emit_char(c));
                        to!(self, State::RawData(ScriptDataEscaped(escape)));
                    }
                }
            },

            //§ script-data-double-escape-end-state
            State::ScriptDataDoubleEscapeEnd => loop {
                let c = get_char!(self, input);
                match c {
                    '\t' | '\n' | '\x0C' | ' ' | '/' | '>' => {
                        let escape = if self.temp_buf == "script" {
                            Escaped
                        } else {
                            DoubleEscaped
                        };
                        emit!(self, sink, emit_char(c));
                        to!(self, State::RawData(ScriptDataEscaped(escape)));
                    }
                    _ => match lower_ascii_letter(c) {
                        Some(lower) => {
                            self.temp_buf.push(lower);
                            emit!(self, sink, emit_char(c));
                        }
                        None => reconsume!(self, State::RawData(ScriptDataEscaped(DoubleEscaped))),
                    },
                }
            },

            //§ before-attribute-name-state
            State::BeforeAttributeName => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => (),
                    '/' => to!(self, State::SelfClosingStartTag),
                    '>' => {
                        self.state = State::Data;
                        return self.emit_current_tag(sink);
                    }
                    '\0' => {
                        self.bad_char_error(sink);
                        self.create_attribute(sink, '\u{fffd}');
                        to!(self, State::AttributeName);
                    }
                    c => {
                        match lower_ascii_letter(c) {
                            Some(lower) => self.create_attribute(sink, lower),
                            None => {
                                if matches!(c, '"' | '\'' | '<' | '=') {
                                    self.bad_char_error(sink);
                                }
                                self.create_attribute(sink, c);
                            }
                        }
                        to!(self, State::AttributeName);
                    }
                }
            },

            //§ attribute-name-state
            State::AttributeName => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => to!(self, State::AfterAttributeName),
                    '/' => to!(self, State::SelfClosingStartTag),
                    '=' => to!(self, State::BeforeAttributeValue),
                    '>' => {
                        self.state = State::Data;
                        return self.emit_current_tag(sink);
                    }
                    '\0' => {
                        self.bad_char_error(sink);
                        self.current_attr_name.push('\u{fffd}');
                    }
                    c => match lower_ascii_letter(c) {
                        Some(lower) => self.current_attr_name.push(lower),
                        None => {
                            if matches!(c, '"' | '\'' | '<') {
                                self.bad_char_error(sink);
                            }
                            self.current_attr_name.push(c);
                        }
                    },
                }
            },

            //§ after-attribute-name-state
            State::AfterAttributeName => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => (),
                    '/' => to!(self, State::SelfClosingStartTag),
                    '=' => to!(self, State::BeforeAttributeValue),
                    '>' => {
                        self.state = State::Data;
                        return self.emit_current_tag(sink);
                    }
                    '\0' => {
                        self.bad_char_error(sink);
                        self.create_attribute(sink, '\u{fffd}');
                        to!(self, State::AttributeName);
                    }
                    c => {
                        match lower_ascii_letter(c) {
                            Some(lower) => self.create_attribute(sink, lower),
                            None => {
                                if matches!(c, '"' | '\'' | '<') {
                                    self.bad_char_error(sink);
                                }
                                self.create_attribute(sink, c);
                            }
                        }
                        to!(self, State::AttributeName);
                    }
                }
            },

            //§ before-attribute-value-state
            State::BeforeAttributeValue => loop {
                match peek_char!(self, input) {
                    '\t' | '\n' | '\r' | '\x0C' | ' ' => self.discard_char(input),
                    '"' => {
                        self.discard_char(input);
                        to!(self, State::AttributeValue(DoubleQuoted));
                    }
                    '\'' => {
                        self.discard_char(input);
                        to!(self, State::AttributeValue(SingleQuoted));
                    }
                    '>' => {
                        self.discard_char(input);
                        self.bad_char_error(sink);
                        self.state = State::Data;
                        return self.emit_current_tag(sink);
                    }
                    _ => to!(self, State::AttributeValue(Unquoted)),
                }
            },

            //§ attribute-value-(double-quoted)-state
            State::AttributeValue(DoubleQuoted) => loop {
                match scan_run!(self, input, CharSet::AttrDoubleQuoted) {
                    Scanned::Char('"') => to!(self, State::AfterAttributeValueQuoted),
                    Scanned::Char('&') => {
                        self.start_consuming_character_reference();
                        return ProcessResult::Continue;
                    }
                    Scanned::Char('\0') => {
                        self.bad_char_error(sink);
                        self.current_attr_value.push('\u{fffd}');
                    }
                    Scanned::Char(c) => self.current_attr_value.push(c),
                    Scanned::Run(text) => self.current_attr_value.push_str(text),
                }
            },

            //§ attribute-value-(single-quoted)-state
            State::AttributeValue(SingleQuoted) => loop {
                match scan_run!(self, input, CharSet::AttrSingleQuoted) {
                    Scanned::Char('\'') => to!(self, State::AfterAttributeValueQuoted),
                    Scanned::Char('&') => {
                        self.start_consuming_character_reference();
                        return ProcessResult::Continue;
                    }
                    Scanned::Char('\0') => {
                        self.bad_char_error(sink);
                        self.current_attr_value.push('\u{fffd}');
                    }
                    Scanned::Char(c) => self.current_attr_value.push(c),
                    Scanned::Run(text) => self.current_attr_value.push_str(text),
                }
            },

            //§ attribute-value-(unquoted)-state
            State::AttributeValue(Unquoted) => loop {
                match scan_run!(self, input, CharSet::AttrUnquoted) {
                    Scanned::Char('\t' | '\n' | '\x0C' | ' ') => {
                        to!(self, State::BeforeAttributeName)
                    }
                    Scanned::Char('&') => {
                        self.start_consuming_character_reference();
                        return ProcessResult::Continue;
                    }
                    Scanned::Char('>') => {
                        self.state = State::Data;
                        return self.emit_current_tag(sink);
                    }
                    Scanned::Char('\0') => {
                        self.bad_char_error(sink);
                        self.current_attr_value.push('\u{fffd}');
                    }
                    Scanned::Char(c) => {
                        if matches!(c, '"' | '\'' | '<' | '=' | '`') {
                            self.bad_char_error(sink);
                        }
                        self.current_attr_value.push(c);
                    }
                    Scanned::Run(text) => self.current_attr_value.push_str(text),
                }
            },

            //§ after-attribute-value-(quoted)-state
            State::AfterAttributeValueQuoted => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => to!(self, State::BeforeAttributeName),
                    '/' => to!(self, State::SelfClosingStartTag),
                    '>' => {
                        self.state = State::Data;
                        return self.emit_current_tag(sink);
                    }
                    _ => {
                        self.bad_char_error(sink);
                        reconsume!(self, State::BeforeAttributeName);
                    }
                }
            },

            //§ self-closing-start-tag-state
            State::SelfClosingStartTag => loop {
                match get_char!(self, input) {
                    '>' => {
                        self.current_tag_self_closing = true;
                        self.state = State::Data;
                        return self.emit_current_tag(sink);
                    }
                    _ => {
                        self.bad_char_error(sink);
                        reconsume!(self, State::BeforeAttributeName);
                    }
                }
            },

            //§ comment-start-state
            State::CommentStart => loop {
                match get_char!(self, input) {
                    '-' => to!(self, State::CommentStartDash),
                    '\0' => {
                        self.bad_char_error(sink);
                        self.current_comment.push('\u{fffd}');
                        to!(self, State::Comment);
                    }
                    '>' => {
                        self.bad_char_error(sink);
                        emit!(self, sink, emit_current_comment());
                        to!(self, State::Data);
                    }
                    c => {
                        self.current_comment.push(c);
                        to!(self, State::Comment);
                    }
                }
            },

            //§ comment-start-dash-state
            State::CommentStartDash => loop {
                match get_char!(self, input) {
                    '-' => to!(self, State::CommentEnd),
                    '\0' => {
                        self.bad_char_error(sink);
                        self.current_comment.push_str("-\u{fffd}");
                        to!(self, State::Comment);
                    }
                    '>' => {
                        self.bad_char_error(sink);
                        emit!(self, sink, emit_current_comment());
                        to!(self, State::Data);
                    }
                    c => {
                        self.current_comment.push('-');
                        self.current_comment.push(c);
                        to!(self, State::Comment);
                    }
                }
            },

            //§ comment-state
            State::Comment => loop {
                match get_char!(self, input) {
                    '<' => {
                        self.current_comment.push('<');
                        to!(self, State::CommentLessThanSign);
                    }
                    '-' => to!(self, State::CommentEndDash),
                    '\0' => {
                        self.bad_char_error(sink);
                        self.current_comment.push('\u{fffd}');
                    }
                    c => self.current_comment.push(c),
                }
            },

            //§ comment-less-than-sign-state
            State::CommentLessThanSign => loop {
                match get_char!(self, input) {
                    '!' => {
                        self.current_comment.push('!');
                        to!(self, State::CommentLessThanSignBang);
                    }
                    '<' => self.current_comment.push('<'),
                    _ => reconsume!(self, State::Comment),
                }
            },

            //§ comment-less-than-sign-bang-state
            State::CommentLessThanSignBang => loop {
                match get_char!(self, input) {
                    '-' => to!(self, State::CommentLessThanSignBangDash),
                    _ => reconsume!(self, State::Comment),
                }
            },

            //§ comment-less-than-sign-bang-dash-state
            State::CommentLessThanSignBangDash => loop {
                match get_char!(self, input) {
                    '-' => to!(self, State::CommentLessThanSignBangDashDash),
                    _ => reconsume!(self, State::CommentEndDash),
                }
            },

            //§ comment-less-than-sign-bang-dash-dash-state
            State::CommentLessThanSignBangDashDash => loop {
                match get_char!(self, input) {
                    '>' => reconsume!(self, State::CommentEnd),
                    _ => {
                        self.bad_char_error(sink);
                        reconsume!(self, State::CommentEnd);
                    }
                }
            },

            //§ comment-end-dash-state
            State::CommentEndDash => loop {
                match get_char!(self, input) {
                    '-' => to!(self, State::CommentEnd),
                    '\0' => {
                        self.bad_char_error(sink);
                        self.current_comment.push_str("-\u{fffd}");
                        to!(self, State::Comment);
                    }
                    c => {
                        self.current_comment.push('-');
                        self.current_comment.push(c);
                        to!(self, State::Comment);
                    }
                }
            },

            //§ comment-end-state
            State::CommentEnd => loop {
                match get_char!(self, input) {
                    '>' => {
                        emit!(self, sink, emit_current_comment());
                        to!(self, State::Data);
                    }
                    '!' => to!(self, State::CommentEndBang),
                    '-' => self.current_comment.push('-'),
                    _ => {
                        self.current_comment.push_str("--");
                        reconsume!(self, State::Comment);
                    }
                }
            },

            //§ comment-end-bang-state
            State::CommentEndBang => loop {
                match get_char!(self, input) {
                    '-' => {
                        self.current_comment.push_str("--!");
                        to!(self, State::CommentEndDash);
                    }
                    '>' => {
                        self.bad_char_error(sink);
                        emit!(self, sink, emit_current_comment());
                        to!(self, State::Data);
                    }
                    '\0' => {
                        self.bad_char_error(sink);
                        self.current_comment.push_str("--!\u{fffd}");
                        to!(self, State::Comment);
                    }
                    c => {
                        self.current_comment.push_str("--!");
                        self.current_comment.push(c);
                        to!(self, State::Comment);
                    }
                }
            },

            //§ doctype-state
            State::Doctype => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => to!(self, State::BeforeDoctypeName),
                    '>' => reconsume!(self, State::BeforeDoctypeName),
                    _ => {
                        self.bad_char_error(sink);
                        reconsume!(self, State::BeforeDoctypeName);
                    }
                }
            },

            //§ before-doctype-name-state
            State::BeforeDoctypeName => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => (),
                    '\0' => {
                        self.bad_char_error(sink);
                        self.current_doctype = Doctype::default();
                        option_push(&mut self.current_doctype.name, '\u{fffd}');
                        to!(self, State::DoctypeName);
                    }
                    '>' => {
                        self.bad_char_error(sink);
                        self.current_doctype = Doctype::default();
                        self.current_doctype.force_quirks = true;
                        emit!(self, sink, emit_current_doctype());
                        to!(self, State::Data);
                    }
                    c => {
                        self.current_doctype = Doctype::default();
                        option_push(&mut self.current_doctype.name, c.to_ascii_lowercase());
                        to!(self, State::DoctypeName);
                    }
                }
            },

            //§ doctype-name-state
            State::DoctypeName => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => {
                        self.temp_buf.clear();
                        to!(self, State::AfterDoctypeName);
                    }
                    '>' => {
                        emit!(self, sink, emit_current_doctype());
                        to!(self, State::Data);
                    }
                    '\0' => {
                        self.bad_char_error(sink);
                        option_push(&mut self.current_doctype.name, '\u{fffd}');
                    }
                    c => option_push(&mut self.current_doctype.name, c.to_ascii_lowercase()),
                }
            },

            //§ after-doctype-name-state
            State::AfterDoctypeName => loop {
                if eat!(self, input, "public") {
                    to!(self, State::AfterDoctypeKeyword(Public));
                } else if eat!(self, input, "system") {
                    to!(self, State::AfterDoctypeKeyword(System));
                } else {
                    match get_char!(self, input) {
                        '\t' | '\n' | '\x0C' | ' ' => (),
                        '>' => {
                            emit!(self, sink, emit_current_doctype());
                            to!(self, State::Data);
                        }
                        _ => {
                            self.bad_char_error(sink);
                            self.current_doctype.force_quirks = true;
                            reconsume!(self, State::BogusDoctype);
                        }
                    }
                }
            },

            //§ after-doctype-public-keyword-state after-doctype-system-keyword-state
            State::AfterDoctypeKeyword(kind) => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => to!(self, State::BeforeDoctypeIdentifier(kind)),
                    '"' => {
                        self.bad_char_error(sink);
                        self.clear_doctype_id(kind);
                        to!(self, State::DoctypeIdentifierDoubleQuoted(kind));
                    }
                    '\'' => {
                        self.bad_char_error(sink);
                        self.clear_doctype_id(kind);
                        to!(self, State::DoctypeIdentifierSingleQuoted(kind));
                    }
                    '>' => {
                        self.bad_char_error(sink);
                        self.current_doctype.force_quirks = true;
                        emit!(self, sink, emit_current_doctype());
                        to!(self, State::Data);
                    }
                    _ => {
                        self.bad_char_error(sink);
                        self.current_doctype.force_quirks = true;
                        reconsume!(self, State::BogusDoctype);
                    }
                }
            },

            //§ before-doctype-public-identifier-state before-doctype-system-identifier-state
            State::BeforeDoctypeIdentifier(kind) => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => (),
                    '"' => {
                        self.clear_doctype_id(kind);
                        to!(self, State::DoctypeIdentifierDoubleQuoted(kind));
                    }
                    '\'' => {
                        self.clear_doctype_id(kind);
                        to!(self, State::DoctypeIdentifierSingleQuoted(kind));
                    }
                    '>' => {
                        self.bad_char_error(sink);
                        self.current_doctype.force_quirks = true;
                        emit!(self, sink, emit_current_doctype());
                        to!(self, State::Data);
                    }
                    _ => {
                        self.bad_char_error(sink);
                        self.current_doctype.force_quirks = true;
                        reconsume!(self, State::BogusDoctype);
                    }
                }
            },

            //§ doctype-identifier-(double-quoted)-state
            State::DoctypeIdentifierDoubleQuoted(kind) => loop {
                match get_char!(self, input) {
                    '"' => to!(self, State::AfterDoctypeIdentifier(kind)),
                    '\0' => {
                        self.bad_char_error(sink);
                        option_push(self.doctype_id(kind), '\u{fffd}');
                    }
                    '>' => {
                        self.bad_char_error(sink);
                        self.current_doctype.force_quirks = true;
                        emit!(self, sink, emit_current_doctype());
                        to!(self, State::Data);
                    }
                    c => option_push(self.doctype_id(kind), c),
                }
            },

            //§ doctype-identifier-(single-quoted)-state
            State::DoctypeIdentifierSingleQuoted(kind) => loop {
                match get_char!(self, input) {
                    '\'' => to!(self, State::AfterDoctypeIdentifier(kind)),
                    '\0' => {
                        self.bad_char_error(sink);
                        option_push(self.doctype_id(kind), '\u{fffd}');
                    }
                    '>' => {
                        self.bad_char_error(sink);
                        self.current_doctype.force_quirks = true;
                        emit!(self, sink, emit_current_doctype());
                        to!(self, State::Data);
                    }
                    c => option_push(self.doctype_id(kind), c),
                }
            },

            //§ after-doctype-public-identifier-state
            State::AfterDoctypeIdentifier(Public) => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => {
                        to!(self, State::BetweenDoctypePublicAndSystemIdentifiers)
                    }
                    '>' => {
                        emit!(self, sink, emit_current_doctype());
                        to!(self, State::Data);
                    }
                    '"' => {
                        self.bad_char_error(sink);
                        self.clear_doctype_id(System);
                        to!(self, State::DoctypeIdentifierDoubleQuoted(System));
                    }
                    '\'' => {
                        self.bad_char_error(sink);
                        self.clear_doctype_id(System);
                        to!(self, State::DoctypeIdentifierSingleQuoted(System));
                    }
                    _ => {
                        self.bad_char_error(sink);
                        self.current_doctype.force_quirks = true;
                        reconsume!(self, State::BogusDoctype);
                    }
                }
            },

            //§ after-doctype-system-identifier-state
            State::AfterDoctypeIdentifier(System) => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => (),
                    '>' => {
                        emit!(self, sink, emit_current_doctype());
                        to!(self, State::Data);
                    }
                    _ => {
                        self.bad_char_error(sink);
                        reconsume!(self, State::BogusDoctype);
                    }
                }
            },

            //§ between-doctype-public-and-system-identifiers-state
            State::BetweenDoctypePublicAndSystemIdentifiers => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => (),
                    '>' => {
                        emit!(self, sink, emit_current_doctype());
                        to!(self, State::Data);
                    }
                    '"' => {
                        self.clear_doctype_id(System);
                        to!(self, State::DoctypeIdentifierDoubleQuoted(System));
                    }
                    '\'' => {
                        self.clear_doctype_id(System);
                        to!(self, State::DoctypeIdentifierSingleQuoted(System));
                    }
                    _ => {
                        self.bad_char_error(sink);
                        self.current_doctype.force_quirks = true;
                        reconsume!(self, State::BogusDoctype);
                    }
                }
            },

            //§ bogus-doctype-state
            State::BogusDoctype => loop {
                match get_char!(self, input) {
                    '>' => {
                        emit!(self, sink, emit_current_doctype());
                        to!(self, State::Data);
                    }
                    '\0' => self.bad_char_error(sink),
                    _ => (),
                }
            },

            //§ processing-instruction-target-state
            //
            // Not in the HTML standard's tokenizer: `<?name …>` builds a
            // processing instruction and anything else falls back to the
            // standard's bogus comment (see the `<?` cases of
            // `tests/html5lib-tests/tree-construction/processing-instructions.dat`).
            State::ProcessingInstructionTarget => loop {
                match get_char!(self, input) {
                    c @ ('\t' | '\n' | '\x0C' | ' ') => {
                        if self.pi_target_is_valid() {
                            to!(self, State::ProcessingInstructionDataStart);
                        }
                        self.current_comment.push(c);
                        to!(self, State::BogusComment);
                    }
                    '>' => {
                        if self.pi_target_is_valid() {
                            emit!(self, sink, emit_current_pi());
                        } else {
                            emit!(self, sink, emit_current_comment());
                        }
                        to!(self, State::Data);
                    }
                    '?' => {
                        if self.pi_target_is_valid() {
                            reconsume!(self, State::ProcessingInstructionData);
                        }
                        self.current_comment.push('?');
                        to!(self, State::BogusComment);
                    }
                    '\0' => {
                        self.bad_char_error(sink);
                        self.current_comment.push('\u{fffd}');
                        to!(self, State::BogusComment);
                    }
                    c => {
                        let first = self.current_pi_target.is_empty();
                        let ok = if first {
                            c.is_ascii_alphabetic() || c == '_'
                        } else {
                            c.is_ascii_alphanumeric() || c == '_' || c == '-'
                        };
                        self.current_comment.push(c);
                        if ok {
                            self.current_pi_target.push(c);
                        } else {
                            to!(self, State::BogusComment);
                        }
                    }
                }
            },

            //§ processing-instruction-data-start-state
            State::ProcessingInstructionDataStart => loop {
                match get_char!(self, input) {
                    '\t' | '\n' | '\x0C' | ' ' => (),
                    '>' => {
                        emit!(self, sink, emit_current_pi());
                        to!(self, State::Data);
                    }
                    _ => reconsume!(self, State::ProcessingInstructionData),
                }
            },

            //§ processing-instruction-data-state
            State::ProcessingInstructionData => loop {
                match get_char!(self, input) {
                    '>' => {
                        emit!(self, sink, emit_current_pi());
                        to!(self, State::Data);
                    }
                    '?' => {
                        // Transition to ProcessingInstructionDataAfterQuestion to check
                        // if the next character is `>`. This avoids suspending mid-look-ahead.
                        to!(self, State::ProcessingInstructionDataAfterQuestion);
                    }
                    '\0' => {
                        self.bad_char_error(sink);
                        self.current_pi_data.push('\u{fffd}');
                    }
                    c => self.current_pi_data.push(c),
                }
            },

            //§ processing-instruction-data-after-question-state
            State::ProcessingInstructionDataAfterQuestion => loop {
                match get_char!(self, input) {
                    '>' => {
                        // The `?>` ends the PI; the `?` was part of the terminator, not data.
                        emit!(self, sink, emit_current_pi());
                        to!(self, State::Data);
                    }
                    '?' => {
                        // The previous `?` was data; this one may still start
                        // the terminator.
                        self.current_pi_data.push('?');
                    }
                    c => {
                        // The `?` was data, not the start of a terminator.
                        self.current_pi_data.push('?');
                        match c {
                            '\0' => {
                                self.bad_char_error(sink);
                                self.current_pi_data.push('\u{fffd}');
                            }
                            _ => self.current_pi_data.push(c),
                        }
                        to!(self, State::ProcessingInstructionData);
                    }
                }
            },

            //§ bogus-comment-state
            State::BogusComment => loop {
                match get_char!(self, input) {
                    '>' => {
                        emit!(self, sink, emit_current_comment());
                        to!(self, State::Data);
                    }
                    '\0' => {
                        self.bad_char_error(sink);
                        self.current_comment.push('\u{fffd}');
                    }
                    c => self.current_comment.push(c),
                }
            },

            //§ markup-declaration-open-state
            State::MarkupDeclarationOpen => loop {
                if eat_exact!(self, input, "--") {
                    self.current_comment.clear();
                    to!(self, State::CommentStart);
                } else if eat!(self, input, "doctype") {
                    to!(self, State::Doctype);
                } else {
                    if sink.adjusted_current_node_present_but_not_in_html_namespace()
                        && eat_exact!(self, input, "[CDATA[")
                    {
                        self.temp_buf.clear();
                        to!(self, State::CdataSection);
                    }
                    self.bad_char_error(sink);
                    self.current_comment.clear();
                    to!(self, State::BogusComment);
                }
            },

            //§ cdata-section-state
            State::CdataSection => loop {
                match get_char!(self, input) {
                    ']' => to!(self, State::CdataSectionBracket),
                    '\0' => {
                        emit!(self, sink, emit_temp_buf());
                        emit!(self, sink, emit_char('\0'));
                    }
                    c => self.temp_buf.push(c),
                }
            },

            //§ cdata-section-bracket-state
            State::CdataSectionBracket => match get_char!(self, input) {
                ']' => to!(self, State::CdataSectionEnd),
                _ => {
                    self.temp_buf.push(']');
                    reconsume!(self, State::CdataSection);
                }
            },

            //§ cdata-section-end-state
            State::CdataSectionEnd => loop {
                match get_char!(self, input) {
                    ']' => self.temp_buf.push(']'),
                    '>' => {
                        emit!(self, sink, emit_temp_buf());
                        to!(self, State::Data);
                    }
                    _ => {
                        self.temp_buf.push_str("]]");
                        reconsume!(self, State::CdataSection);
                    }
                }
            },
        }
    }

    fn eof_step<S: TokenSink>(&mut self, sink: &mut S) -> ProcessResult<S::Handle> {
        match self.state {
            State::Data | State::RawData(Rcdata | Rawtext | ScriptData) | State::Plaintext => {
                emit!(self, sink, emit_eof());
                ProcessResult::Suspend
            }

            State::TagName
            | State::RawData(ScriptDataEscaped(_))
            | State::BeforeAttributeName
            | State::AttributeName
            | State::AfterAttributeName
            | State::AttributeValue(_)
            | State::AfterAttributeValueQuoted
            | State::SelfClosingStartTag
            | State::ScriptDataEscapedDash(_)
            | State::ScriptDataEscapedDashDash(_) => {
                self.bad_eof_error(sink);
                to!(self, State::Data);
            }

            State::BeforeAttributeValue => reconsume!(self, State::AttributeValue(Unquoted)),

            State::TagOpen => {
                self.bad_eof_error(sink);
                emit!(self, sink, emit_char('<'));
                to!(self, State::Data);
            }

            State::EndTagOpen => {
                self.bad_eof_error(sink);
                emit!(self, sink, emit_char('<'));
                emit!(self, sink, emit_char('/'));
                to!(self, State::Data);
            }

            State::RawLessThanSign(ScriptDataEscaped(DoubleEscaped)) => {
                to!(self, State::RawData(ScriptDataEscaped(DoubleEscaped)))
            }

            State::RawLessThanSign(kind) => {
                emit!(self, sink, emit_char('<'));
                to!(self, State::RawData(kind));
            }

            State::RawEndTagOpen(kind) => {
                emit!(self, sink, emit_char('<'));
                emit!(self, sink, emit_char('/'));
                to!(self, State::RawData(kind));
            }

            State::RawEndTagName(kind) => {
                emit!(self, sink, emit_char('<'));
                emit!(self, sink, emit_char('/'));
                emit!(self, sink, emit_temp_buf());
                to!(self, State::RawData(kind));
            }

            State::ScriptDataEscapeStart(kind) => {
                to!(self, State::RawData(ScriptDataEscaped(kind)))
            }

            State::ScriptDataEscapeStartDash => to!(self, State::RawData(ScriptData)),

            State::ScriptDataDoubleEscapeEnd => {
                to!(self, State::RawData(ScriptDataEscaped(DoubleEscaped)))
            }

            State::CommentStart
            | State::CommentStartDash
            | State::Comment
            | State::CommentEndDash
            | State::CommentEnd
            | State::CommentEndBang => {
                self.bad_eof_error(sink);
                emit!(self, sink, emit_current_comment());
                to!(self, State::Data);
            }

            State::CommentLessThanSign | State::CommentLessThanSignBang => {
                reconsume!(self, State::Comment)
            }

            State::CommentLessThanSignBangDash => reconsume!(self, State::CommentEndDash),

            State::CommentLessThanSignBangDashDash => reconsume!(self, State::CommentEnd),

            State::Doctype | State::BeforeDoctypeName => {
                self.bad_eof_error(sink);
                self.current_doctype = Doctype::default();
                self.current_doctype.force_quirks = true;
                emit!(self, sink, emit_current_doctype());
                to!(self, State::Data);
            }

            State::DoctypeName
            | State::AfterDoctypeName
            | State::AfterDoctypeKeyword(_)
            | State::BeforeDoctypeIdentifier(_)
            | State::DoctypeIdentifierDoubleQuoted(_)
            | State::DoctypeIdentifierSingleQuoted(_)
            | State::AfterDoctypeIdentifier(_)
            | State::BetweenDoctypePublicAndSystemIdentifiers => {
                self.bad_eof_error(sink);
                self.current_doctype.force_quirks = true;
                emit!(self, sink, emit_current_doctype());
                to!(self, State::Data);
            }

            State::BogusDoctype => {
                emit!(self, sink, emit_current_doctype());
                to!(self, State::Data);
            }

            State::BogusComment => {
                emit!(self, sink, emit_current_comment());
                to!(self, State::Data);
            }

            // An unterminated processing instruction is dropped, unlike an
            // unterminated comment.
            State::ProcessingInstructionTarget
            | State::ProcessingInstructionDataStart
            | State::ProcessingInstructionData
            | State::ProcessingInstructionDataAfterQuestion => {
                self.bad_eof_error(sink);
                self.current_comment.clear();
                self.current_pi_target.clear();
                self.current_pi_data.clear();
                to!(self, State::Data);
            }

            State::MarkupDeclarationOpen => {
                self.bad_char_error(sink);
                self.current_comment.clear();
                to!(self, State::BogusComment);
            }

            State::CdataSection => {
                emit!(self, sink, emit_temp_buf());
                self.bad_eof_error(sink);
                to!(self, State::Data);
            }

            State::CdataSectionBracket => {
                self.temp_buf.push(']');
                to!(self, State::CdataSection);
            }

            State::CdataSectionEnd => {
                self.temp_buf.push_str("]]");
                to!(self, State::CdataSection);
            }
        }
    }
}
