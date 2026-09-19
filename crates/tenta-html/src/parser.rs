//! The public parser: streaming input, encoding sniffing and the preload
//! scanner tied together (`docs/spec/04-dom-and-css.md` §3).

use encoding_rs::Encoding;

use crate::encoding::{Confidence, Decoder, PRESCAN_LIMIT, encoding_from_meta_label, sniff};
use crate::input::Input;
use crate::preload::{PreloadHint, PreloadScanner};
use crate::scan::ScanMode;
use crate::sink::{ElementFlags, QualName, QuirksMode, TreeSink};
use crate::tokenizer::{Tokenizer, TokenizerOpts, TokenizerResult};
use crate::tree_builder::{TreeBuilder, TreeBuilderOpts};

/// What a [`Parser::feed`] call decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParseStatus {
    /// The chunk was consumed; keep feeding.
    Continue,
    /// The tree sink returned [`crate::sink::ParseControl::Stop`]; the parser
    /// made no further sink calls and the tree stays as built so far.
    Stopped,
    /// A `<meta>` declared an encoding that differs from the one in use and
    /// the current encoding is only tentative. The caller must throw the tree
    /// away and re-parse from the first byte with
    /// [`Parser::restart_with_encoding`].
    EncodingChanged(&'static Encoding),
    /// A `</script>` closed a parser-blocking script and the parse is paused
    /// on it. Only a parser built with [`ParserOptions::pause_on_script`] ever
    /// reports this. The element is taken with
    /// [`Parser::take_pending_script`], and [`Parser::resume`] continues the
    /// parse once the script has run (`docs/spec/05-javascript.md` §6).
    ///
    /// Input fed while the parse is paused is buffered, not parsed: the
    /// document is still "the one the script sees", so no markup after the
    /// script may reach the tree before the script has run.
    Script,
}

/// Parser configuration.
///
/// The lifetime is only used by the preload callback; `ParserOptions::default()`
/// is `ParserOptions<'static>`.
pub struct ParserOptions<'cb> {
    /// Whether the document's scripting flag is set. It changes how
    /// `<noscript>` is parsed.
    pub scripting_enabled: bool,
    /// Whether a `</script>` pauses the parse and reports
    /// [`ParseStatus::Script`]. Only a caller that can actually run the script
    /// sets this; a Tier 1 caller leaves it off and the parser walks past
    /// script elements as it does today.
    pub pause_on_script: bool,
    /// The context element for the fragment parsing algorithm. When set, the
    /// parser creates the element in the sink and parses a fragment.
    pub fragment_context: Option<QualName>,
    /// Forces a quirks mode instead of deriving it from the DOCTYPE.
    pub quirks_mode: Option<QuirksMode>,
    /// Whether the document comes from an `<iframe srcdoc>` attribute.
    pub iframe_srcdoc: bool,
    /// Whether to drop the DOCTYPE instead of appending it to the document.
    pub drop_doctype: bool,
    /// Which text scanner the tokenizer uses; the scalar mode exists for the
    /// differential test (`docs/spec/13-performance.md` §5).
    pub scan_mode: ScanMode,
    /// BCP 47 language tag used to pick the fallback encoding.
    pub default_locale: String,
    /// The encoding to decode bytes with. When `None`, the parser sniffs.
    pub encoding: Option<&'static Encoding>,
    /// The `charset` parameter of the response's `Content-Type`, if any.
    pub transport_charset: Option<String>,
    /// Receives subresource hints ahead of the tree builder. `None` means the
    /// preload scanner is not built at all.
    pub preload: Option<Box<dyn FnMut(PreloadHint) + 'cb>>,
    /// Maximum number of nodes (elements, text, comments) the tree builder
    /// creates. Default 2^22 (4_194_304). Past this limit, formatting
    /// reconstruction becomes a no-op and new element start tags are treated
    /// as parse errors. Text and comment insertion continues unbounded.
    pub max_dom_nodes: usize,
}

impl Default for ParserOptions<'_> {
    fn default() -> Self {
        ParserOptions {
            scripting_enabled: true,
            pause_on_script: false,
            fragment_context: None,
            quirks_mode: None,
            iframe_srcdoc: false,
            drop_doctype: false,
            scan_mode: ScanMode::default(),
            default_locale: String::from("en"),
            encoding: None,
            transport_charset: None,
            preload: None,
            max_dom_nodes: 4_194_304, // 2^22
        }
    }
}

/// The parts of [`ParserOptions`] a restart has to reproduce.
#[derive(Clone)]
struct RestartOptions {
    scripting_enabled: bool,
    pause_on_script: bool,
    fragment_context: Option<QualName>,
    quirks_mode: Option<QuirksMode>,
    iframe_srcdoc: bool,
    drop_doctype: bool,
    scan_mode: ScanMode,
    default_locale: String,
    transport_charset: Option<String>,
    max_dom_nodes: usize,
}

/// The scanner and the callback it feeds, kept together so they are moved
/// and borrowed as one.
type PreloadCallback<'cb> = (PreloadScanner, Box<dyn FnMut(PreloadHint) + 'cb>);

/// A streaming HTML parser writing into a [`TreeSink`].
pub struct Parser<'cb, S: TreeSink> {
    tokenizer: Tokenizer,
    tree_builder: TreeBuilder<S::Handle, S>,
    /// Text carried over from the previous chunk, plus the already consumed
    /// prefix the tokenizer may still need to move back over.
    pending: String,
    pending_start: usize,
    decoder: Option<Decoder>,
    /// Mirrors the decoder's encoding so it stays readable while the decoder
    /// is temporarily moved out for a borrow-free decode.
    current_encoding: Option<&'static Encoding>,
    confidence: Confidence,
    /// Bytes buffered while the encoding is still unknown.
    sniff_buffer: Option<Vec<u8>>,
    preload: Option<PreloadCallback<'cb>>,
    options: RestartOptions,
    stopped: bool,
    restart: Option<&'static Encoding>,
    /// The script element the parse is paused on, until the caller takes it.
    pending_script: Option<S::Handle>,
}

impl<'cb, S: TreeSink> Parser<'cb, S> {
    /// A parser writing into `sink`.
    pub fn new(mut sink: S, options: ParserOptions<'cb>) -> Parser<'cb, S> {
        let ParserOptions {
            scripting_enabled,
            pause_on_script,
            fragment_context,
            quirks_mode,
            iframe_srcdoc,
            drop_doctype,
            scan_mode,
            default_locale,
            encoding,
            transport_charset,
            preload,
            max_dom_nodes,
        } = options;

        let tree_builder_opts = TreeBuilderOpts {
            scripting_enabled,
            iframe_srcdoc,
            drop_doctype,
            quirks_mode: quirks_mode.unwrap_or_default(),
            max_dom_nodes,
        };

        let (tree_builder, initial_state) = match &fragment_context {
            Some(name) => {
                let mut flags = ElementFlags::default();
                if name.local == tenta_core::atom::atoms::TEMPLATE
                    && name.ns == tenta_core::atom::atoms::NS_HTML
                {
                    flags.template = true;
                }
                let context = sink.create_element(*name, Vec::new(), flags);
                let tree_builder =
                    TreeBuilder::new_for_fragment(sink, context, None, tree_builder_opts);
                let state = tree_builder.tokenizer_state_for_context_elem(scripting_enabled);
                (tree_builder, Some(state))
            }
            None => (TreeBuilder::new(sink, tree_builder_opts), None),
        };

        let tokenizer = Tokenizer::new(TokenizerOpts {
            discard_bom: true,
            initial_state,
            last_start_tag_name: None,
            scan_mode,
        });

        let (decoder, confidence, sniff_buffer) = match encoding {
            Some(encoding) => (Some(Decoder::new(encoding)), Confidence::Certain, None),
            None => (None, Confidence::Tentative, Some(Vec::new())),
        };

        Parser {
            tokenizer,
            tree_builder,
            pending: String::new(),
            pending_start: 0,
            decoder,
            current_encoding: encoding,
            confidence,
            sniff_buffer,
            preload: preload.map(|callback| (PreloadScanner::new(scripting_enabled), callback)),
            options: RestartOptions {
                scripting_enabled,
                pause_on_script,
                fragment_context,
                quirks_mode,
                iframe_srcdoc,
                drop_doctype,
                scan_mode,
                default_locale,
                transport_charset,
                max_dom_nodes,
            },
            stopped: false,
            restart: None,
            pending_script: None,
        }
    }

    /// The encoding currently in use, or `None` while the parser is still
    /// buffering bytes to sniff it.
    pub fn encoding(&self) -> Option<&'static Encoding> {
        self.current_encoding
    }

    /// Whether the parse ended early because the sink asked it to.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// The sink, with the tree as it stands right now.
    ///
    /// Meaningful mainly while the parse is paused on a script
    /// ([`ParseStatus::Script`]): that tree is exactly what the script is
    /// entitled to see. A sink shared with the script (an `Rc` in
    /// `tenta-dom`) does not need this.
    pub fn sink(&self) -> &S {
        &self.tree_builder.sink
    }

    /// Takes the script element the parse is paused on, if any.
    ///
    /// The parse stays paused until [`Self::resume`], so the caller may run
    /// the script against the very tree the parser is writing into: everything
    /// before the `</script>` is in the document, and nothing after it is.
    pub fn take_pending_script(&mut self) -> Option<S::Handle> {
        let script = self.pending_script.take()?;
        // Handing a script out makes the document script-observable, and a
        // restart re-parses from byte zero into a fresh sink — which would run
        // every script so far a second time, with its side effects already
        // committed. The encoding in use becomes certain at this point, so a
        // late `<meta charset>` is ignored rather than triggering an unsound
        // restart. WHATWG's answer is to navigate again; the parser cannot do
        // that, and mojibake beats running a page's scripts twice.
        self.confidence = Confidence::Certain;
        Some(script)
    }

    /// Continues a parse paused on a script, without new input.
    ///
    /// Abandons a pending script the caller never took: resuming says the
    /// caller is done with the pause. Taking the script first is the contract
    /// (a debug build asserts it).
    pub fn resume(&mut self) -> ParseStatus {
        if let Some(encoding) = self.restart {
            return ParseStatus::EncodingChanged(encoding);
        }
        if self.stopped {
            return ParseStatus::Stopped;
        }
        // Resuming is the caller's statement that it is done with the pause.
        // A script it never took cannot have run, but reporting the pause
        // again would hang a driver that locates the script some other way
        // (through the shared document, say) — a silent hang is worse than a
        // skipped script, and the debug assertion catches the mistake where
        // it is made.
        debug_assert!(
            self.pending_script.is_none(),
            "resume() before take_pending_script() abandons the script"
        );
        self.pending_script = None;
        self.run("", false)
    }

    /// Builds a fresh parser for a re-parse after
    /// [`ParseStatus::EncodingChanged`].
    ///
    /// The caller supplies a new, empty sink: the tree built so far is not
    /// salvageable, and every byte of the document must be fed again.
    pub fn restart_with_encoding(self, sink: S, encoding: &'static Encoding) -> Parser<'cb, S> {
        let options = self.options.clone();
        let preload = self.preload.map(|(_, callback)| callback);
        Parser::new(
            sink,
            ParserOptions {
                scripting_enabled: options.scripting_enabled,
                pause_on_script: options.pause_on_script,
                fragment_context: options.fragment_context,
                quirks_mode: options.quirks_mode,
                iframe_srcdoc: options.iframe_srcdoc,
                drop_doctype: options.drop_doctype,
                scan_mode: options.scan_mode,
                default_locale: options.default_locale,
                encoding: Some(encoding),
                transport_charset: options.transport_charset,
                preload,
                max_dom_nodes: options.max_dom_nodes,
            },
        )
    }

    /// Feeds raw bytes, decoding them with the sniffed or configured encoding.
    pub fn feed(&mut self, bytes: &[u8]) -> ParseStatus {
        self.feed_bytes(bytes, false)
    }

    /// Feeds already decoded text. The encoding is reported as UTF-8.
    pub fn feed_str(&mut self, text: &str) -> ParseStatus {
        if let Some(encoding) = self.restart {
            return ParseStatus::EncodingChanged(encoding);
        }
        if self.stopped {
            return ParseStatus::Stopped;
        }
        if self.decoder.is_none() {
            self.decoder = Some(Decoder::new(encoding_rs::UTF_8));
            self.current_encoding = Some(encoding_rs::UTF_8);
            self.sniff_buffer = None;
            // The caller decoded the input, so a late <meta charset> cannot
            // change it: the encoding is certain and the declaration is only
            // a confirmation.
            self.confidence = Confidence::Certain;
        }
        self.run(text, false)
    }

    /// Ends the parse and returns the tree.
    pub fn finish(mut self) -> S::Output {
        // Finishing abandons any pending script: the caller is asking for the
        // tree, so the buffered tail is parsed rather than dropped. Pausing is
        // turned off for the drain as well — clearing the handle alone would
        // let the drain stop on the *next* script and lose both the rest of
        // the document and the end-of-file steps that pop the open elements.
        self.options.pause_on_script = false;
        self.pending_script = None;
        if self.restart.is_none() && !self.stopped {
            let _ = self.feed_bytes(b"", true);
        }
        if self.restart.is_none() && !self.stopped {
            let _ = self.run("", true);
        }
        if let Some((scanner, callback)) = &mut self.preload {
            for hint in scanner.finish() {
                callback(hint);
            }
        }
        self.tree_builder.finish()
    }

    fn feed_bytes(&mut self, bytes: &[u8], last: bool) -> ParseStatus {
        if let Some(encoding) = self.restart {
            return ParseStatus::EncodingChanged(encoding);
        }
        if self.stopped {
            return ParseStatus::Stopped;
        }

        // Buffer until there is enough input to run the sniffing algorithm.
        if let Some(buffer) = &mut self.sniff_buffer {
            buffer.extend_from_slice(bytes);
            if buffer.len() < PRESCAN_LIMIT && !last {
                return ParseStatus::Continue;
            }
            let buffered = std::mem::take(buffer);
            self.sniff_buffer = None;
            let sniffed = sniff(
                &buffered,
                self.options.transport_charset.as_deref(),
                &self.options.default_locale,
            );
            self.confidence = sniffed.confidence;
            self.decoder = Some(Decoder::new(sniffed.encoding));
            self.current_encoding = Some(sniffed.encoding);
            return self.decode_and_run(&buffered[sniffed.bom_length..], last);
        }

        self.decode_and_run(bytes, last)
    }

    fn decode_and_run(&mut self, bytes: &[u8], last: bool) -> ParseStatus {
        let mut decoder = self.decoder.take().expect("decoder is set");
        let (owned, borrowed) = decoder.decode(bytes, last);
        let mut status = ParseStatus::Continue;
        if !owned.is_empty() {
            status = self.run(owned, false);
        }
        // `Script` is not the end of the parse, so the rest of the decoded
        // chunk must still be handed over; `run` buffers it behind the pause.
        if matches!(status, ParseStatus::Continue | ParseStatus::Script) && !borrowed.is_empty() {
            status = self.run(borrowed, false);
        }
        self.decoder = Some(decoder);
        status
    }

    fn run(&mut self, text: &str, last: bool) -> ParseStatus {
        if let Some((scanner, callback)) = &mut self.preload {
            for hint in scanner.feed(text) {
                callback(hint);
            }
        }

        // Paused on a script: the input keeps arriving from the network but
        // must not reach the tree builder, so it queues behind the unconsumed
        // tail and is tokenized by `resume`. The preload scanner above still
        // sees it, which is the point of scanning ahead of the parse.
        if self.pending_script.is_some() {
            self.pending.push_str(text);
            return ParseStatus::Script;
        }

        let owned;
        let (buffer, start) = if self.pending.is_empty() {
            (text, 0usize)
        } else {
            let mut pending = std::mem::take(&mut self.pending);
            pending.push_str(text);
            owned = pending;
            (owned.as_str(), self.pending_start)
        };
        if buffer.len() <= start && !last {
            // Nothing new to tokenize. The retained tail was moved out of
            // `self.pending` above, so it has to go back: it holds the bytes
            // a character reference may still un-consume into, and
            // `pending_start` already indexes into it.
            self.pending.push_str(buffer);
            return ParseStatus::Continue;
        }

        let mut input = Input::new(buffer, start, last);
        let status = loop {
            match self.tokenizer.feed(&mut input, &mut self.tree_builder) {
                TokenizerResult::Done => break ParseStatus::Continue,
                TokenizerResult::Script(node) => {
                    // A caller that cannot run scripts simply tokenizes on,
                    // which is what every Tier 1 parse does.
                    if !self.options.pause_on_script {
                        continue;
                    }
                    self.pending_script = Some(node);
                    break ParseStatus::Script;
                }
                TokenizerResult::EncodingIndicator(label) => {
                    match self.handle_encoding_indicator(&label) {
                        Some(encoding) => break ParseStatus::EncodingChanged(encoding),
                        None => continue,
                    }
                }
                TokenizerResult::Stopped => {
                    self.stopped = true;
                    break ParseStatus::Stopped;
                }
            }
        };

        let status = match status {
            // The end of the input drains the state machine through the same
            // cursor, so the character reference tokenizer can un-consume.
            ParseStatus::Continue if last => {
                match self.tokenizer.end(&mut input, &mut self.tree_builder) {
                    TokenizerResult::Stopped => {
                        self.stopped = true;
                        ParseStatus::Stopped
                    }
                    TokenizerResult::EncodingIndicator(label) => {
                        match self.handle_encoding_indicator(&label) {
                            Some(encoding) => ParseStatus::EncodingChanged(encoding),
                            None => ParseStatus::Continue,
                        }
                    }
                    _ => ParseStatus::Continue,
                }
            }
            other => other,
        };

        let position = input.pos();
        // Keep enough of the tail that the character reference tokenizer can
        // still un-consume across the chunk boundary, rounded down to a
        // character boundary.
        let mut retained_from = position - self.tokenizer.rewind_reserve().min(position);
        while !buffer.is_char_boundary(retained_from) {
            retained_from -= 1;
        }
        self.pending.clear();
        self.pending.push_str(&buffer[retained_from..]);
        self.pending_start = position - retained_from;
        status
    }

    /// Applies the `<meta>` charset rules. Returns the encoding to re-parse
    /// with, or `None` when the declaration can be ignored.
    fn handle_encoding_indicator(&mut self, label: &str) -> Option<&'static Encoding> {
        if self.confidence == Confidence::Certain {
            return None;
        }
        let declared = encoding_from_meta_label(label)?;
        if self.current_encoding == Some(declared) {
            // Same encoding: the tentative guess is confirmed.
            self.confidence = Confidence::Certain;
            return None;
        }
        self.restart = Some(declared);
        Some(declared)
    }
}
