//! The preload scanner (`docs/spec/04-dom-and-css.md` §3.2).
//!
//! A second, lightweight tokenizer pass runs ahead of the tree builder over
//! the same input and records subresource hints. Whether a hint becomes a
//! request depends on the page's tier (`docs/spec/02-architecture.md` §1.2);
//! the scanner only records, and it never resolves URLs — the caller does
//! that against the document's base URL.
//!
//! The scanner drives the real tokenizer, so `<script>`, `<style>`,
//! `<textarea>`, `<title>` and comment bodies cannot fool it. `<template>`
//! contents are skipped, and `<noscript>` contents are raw text when
//! scripting is enabled.

use tenta_core::Atom;
use tenta_core::atom::atoms as core_atoms;

use crate::input::Input;
use crate::names::Local;
use crate::scan::ScanMode;
use crate::tokenizer::states::RawKind;
use crate::tokenizer::{Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts};

/// What a hint is for. The tier decides whether it is fetched.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreloadKind {
    /// A classic script.
    Script,
    /// A stylesheet.
    Style,
    /// An image.
    Image,
    /// A font.
    Font,
    /// A generic fetch (`<link rel=preload as=fetch>`, `<iframe src>`).
    Fetch,
    /// A connection to warm up (`rel=preconnect`, `rel=dns-prefetch`).
    Preconnect,
    /// A module script (`type=module`, `rel=modulepreload`).
    Module,
    /// Not a resource: a `<base href>` changed the document's base URL, and
    /// `url` carries the new value.
    BaseChanged,
}

/// The attributes a fetch hint carries, as they appeared in the markup.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct PreloadAttrs {
    /// `crossorigin`.
    pub crossorigin: Option<String>,
    /// `integrity`.
    pub integrity: Option<String>,
    /// `type`.
    pub r#type: Option<String>,
    /// `media`.
    pub media: Option<String>,
    /// `as`.
    pub r#as: Option<String>,
    /// `referrerpolicy`.
    pub referrerpolicy: Option<String>,
    /// `fetchpriority`.
    pub fetchpriority: Option<String>,
    /// The raw `srcset` (or `imagesrcset`), if there was one.
    pub srcset: Option<String>,
    /// The raw `sizes` (or `imagesizes`), if there was one.
    pub sizes: Option<String>,
    /// `nomodule` was present.
    pub nomodule: bool,
    /// `defer` was present.
    pub defer: bool,
    /// `async` was present.
    pub r#async: bool,
}

/// One recorded subresource hint.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PreloadHint {
    /// What the hint is for.
    pub kind: PreloadKind,
    /// The raw attribute value; the caller resolves it against the base URL.
    pub url: String,
    /// The attributes that affect how the resource is fetched.
    pub attrs: PreloadAttrs,
}

/// Picks a candidate from a `srcset` attribute.
///
/// The scanner has no viewport or device pixel ratio, so it prefers a
/// candidate with no descriptor or a `1x` density and otherwise takes the
/// first one. `PreloadAttrs::srcset` keeps the raw value so the caller can
/// re-select once layout metrics exist.
pub fn choose_srcset_candidate(srcset: &str) -> Option<String> {
    let mut first: Option<&str> = None;
    for candidate in split_srcset(srcset) {
        let mut parts = candidate.split_ascii_whitespace();
        let Some(url) = parts.next() else { continue };
        let descriptor = parts.next();
        if first.is_none() {
            first = Some(url);
        }
        match descriptor {
            None | Some("1x") | Some("1.0x") => return Some(url.to_owned()),
            _ => {}
        }
    }
    first.map(str::to_owned)
}

/// Splits a `srcset` value into candidates.
///
/// A bare `split(',')` breaks `data:image/gif;base64,R0lGOD… 1x` in two. The
/// HTML "parse a srcset attribute" algorithm reads a URL that ends at
/// whitespace, so a comma only separates candidates when it follows the URL
/// (after its optional descriptor) rather than when it sits inside one.
fn split_srcset(srcset: &str) -> impl Iterator<Item = &str> {
    let mut rest = srcset;
    std::iter::from_fn(move || {
        loop {
            rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
            if rest.is_empty() {
                return None;
            }
            // The URL runs to the first whitespace; a trailing comma on it
            // ends the candidate, and everything after the whitespace up to
            // the next comma is the descriptor.
            let url_end = rest
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(rest.len());
            let (candidate, tail) = match rest[..url_end].strip_suffix(',') {
                Some(url) => (url, &rest[url_end..]),
                None => {
                    let descriptor_end = rest[url_end..]
                        .find(',')
                        .map(|offset| url_end + offset)
                        .unwrap_or(rest.len());
                    (&rest[..descriptor_end], &rest[descriptor_end..])
                }
            };
            rest = tail;
            let candidate = candidate.trim_matches(|c: char| c.is_ascii_whitespace());
            if !candidate.is_empty() {
                return Some(candidate);
            }
        }
    })
}

/// Recorded attribute lookup helper.
fn attr(tag: &Tag, name: Atom) -> Option<&str> {
    tag.attrs
        .iter()
        .find(|a| a.name.prefix.is_none() && a.name.local == name)
        .map(|a| a.value.as_str())
}

fn has_attr(tag: &Tag, name: Atom) -> bool {
    tag.attrs
        .iter()
        .any(|a| a.name.prefix.is_none() && a.name.local == name)
}

/// Check if a script type is a JavaScript MIME type (or empty/module).
/// Empty type is treated as JavaScript; `module` is also a script type.
fn is_javascript_type(type_str: Option<&str>) -> bool {
    match type_str.map(str::trim) {
        None | Some("") => true, // Empty or no type defaults to JavaScript
        Some(t) if t.eq_ignore_ascii_case("module") => true,
        Some(t) => {
            // Check for JavaScript MIME types. Browsers recognize both
            // `text/javascript` and `application/javascript` variants.
            let lower = t.to_ascii_lowercase();
            lower == "text/javascript"
                || lower == "application/javascript"
                || lower == "application/ecmascript"
                || lower == "text/ecmascript"
                || lower == "text/babel"
        }
    }
}

struct PreloadSink {
    scripting_enabled: bool,
    hints: Vec<PreloadHint>,
    /// Depth of open `<template>` elements; hints inside are not recorded.
    template_depth: u32,
    /// Depth of open `<picture>` elements, for `<source>`.
    picture_depth: u32,
    /// Interned attribute names the scanner looks for.
    names: PreloadNames,
}

struct PreloadNames {
    src: Atom,
    href: Atom,
    rel: Atom,
    r#as: Atom,
    r#type: Atom,
    media: Atom,
    srcset: Atom,
    sizes: Atom,
    imagesrcset: Atom,
    imagesizes: Atom,
    crossorigin: Atom,
    integrity: Atom,
    referrerpolicy: Atom,
    fetchpriority: Atom,
    nomodule: Atom,
    defer: Atom,
    r#async: Atom,
    poster: Atom,
    loading: Atom,
}

impl Default for PreloadNames {
    fn default() -> Self {
        PreloadNames {
            src: core_atoms::SRC,
            href: core_atoms::HREF,
            rel: core_atoms::REL,
            r#as: core_atoms::AS,
            r#type: core_atoms::TYPE,
            media: core_atoms::MEDIA,
            srcset: core_atoms::SRCSET,
            sizes: Atom::new("sizes"),
            imagesrcset: Atom::new("imagesrcset"),
            imagesizes: Atom::new("imagesizes"),
            crossorigin: core_atoms::CROSSORIGIN,
            integrity: core_atoms::INTEGRITY,
            referrerpolicy: Atom::new("referrerpolicy"),
            fetchpriority: Atom::new("fetchpriority"),
            nomodule: core_atoms::NOMODULE,
            defer: core_atoms::DEFER,
            r#async: core_atoms::ASYNC,
            poster: Atom::new("poster"),
            loading: Atom::new("loading"),
        }
    }
}

impl PreloadSink {
    fn common_attrs(&self, tag: &Tag) -> PreloadAttrs {
        PreloadAttrs {
            crossorigin: attr(tag, self.names.crossorigin).map(str::to_owned),
            integrity: attr(tag, self.names.integrity).map(str::to_owned),
            r#type: attr(tag, self.names.r#type).map(str::to_owned),
            media: attr(tag, self.names.media).map(str::to_owned),
            r#as: attr(tag, self.names.r#as).map(str::to_owned),
            referrerpolicy: attr(tag, self.names.referrerpolicy).map(str::to_owned),
            fetchpriority: attr(tag, self.names.fetchpriority).map(str::to_owned),
            srcset: attr(tag, self.names.srcset)
                .or_else(|| attr(tag, self.names.imagesrcset))
                .map(str::to_owned),
            sizes: attr(tag, self.names.sizes)
                .or_else(|| attr(tag, self.names.imagesizes))
                .map(str::to_owned),
            nomodule: has_attr(tag, self.names.nomodule),
            defer: has_attr(tag, self.names.defer),
            r#async: has_attr(tag, self.names.r#async),
        }
    }

    fn push(&mut self, kind: PreloadKind, url: String, attrs: PreloadAttrs) {
        if url.trim().is_empty() {
            return;
        }
        self.hints.push(PreloadHint { kind, url, attrs });
    }

    fn handle_start_tag(&mut self, tag: &Tag) {
        if self.template_depth > 0 {
            return;
        }
        match tag.local {
            Local::Script => {
                let Some(src) = attr(tag, self.names.src) else {
                    return;
                };
                let attrs = self.common_attrs(tag);

                // Skip <script> tags whose type is not a JavaScript MIME type.
                if !is_javascript_type(attrs.r#type.as_deref()) {
                    return;
                }

                let kind = if attrs
                    .r#type
                    .as_deref()
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("module"))
                {
                    PreloadKind::Module
                } else {
                    PreloadKind::Script
                };
                let src = src.to_owned();
                self.push(kind, src, attrs);
            }

            Local::Link => {
                let Some(rel) = attr(tag, self.names.rel) else {
                    return;
                };
                let attrs = self.common_attrs(tag);
                let href = attr(tag, self.names.href).unwrap_or_default().to_owned();
                for keyword in rel.split_ascii_whitespace() {
                    let kind = if keyword.eq_ignore_ascii_case("stylesheet") {
                        PreloadKind::Style
                    } else if keyword.eq_ignore_ascii_case("modulepreload") {
                        PreloadKind::Module
                    } else if keyword.eq_ignore_ascii_case("preconnect")
                        || keyword.eq_ignore_ascii_case("dns-prefetch")
                    {
                        PreloadKind::Preconnect
                    } else if keyword.eq_ignore_ascii_case("preload") {
                        // `<link rel=preload>` without a valid `as` yields nothing.
                        match attrs.r#as.as_deref().map(str::trim) {
                            Some(value) if value.eq_ignore_ascii_case("script") => {
                                PreloadKind::Script
                            }
                            Some(value) if value.eq_ignore_ascii_case("style") => {
                                PreloadKind::Style
                            }
                            Some(value) if value.eq_ignore_ascii_case("image") => {
                                PreloadKind::Image
                            }
                            Some(value) if value.eq_ignore_ascii_case("font") => PreloadKind::Font,
                            Some(value) if value.eq_ignore_ascii_case("fetch") => {
                                PreloadKind::Fetch
                            }
                            Some(value) if value.eq_ignore_ascii_case("audio") => {
                                PreloadKind::Fetch
                            }
                            Some(value) if value.eq_ignore_ascii_case("video") => {
                                PreloadKind::Fetch
                            }
                            Some(value) if value.eq_ignore_ascii_case("track") => {
                                PreloadKind::Fetch
                            }
                            _ => continue, // Skip if no valid `as`
                        }
                    } else {
                        continue;
                    };
                    let url = if kind == PreloadKind::Image {
                        attrs
                            .srcset
                            .as_deref()
                            .and_then(choose_srcset_candidate)
                            .unwrap_or_else(|| href.clone())
                    } else {
                        href.clone()
                    };
                    self.push(kind, url, attrs.clone());
                }
            }

            Local::Img => {
                // Skip images with loading=lazy.
                if let Some(loading) = attr(tag, self.names.loading)
                    && loading.trim().eq_ignore_ascii_case("lazy")
                {
                    return;
                }

                let attrs = self.common_attrs(tag);
                let url = attrs
                    .srcset
                    .as_deref()
                    .and_then(choose_srcset_candidate)
                    .or_else(|| attr(tag, self.names.src).map(str::to_owned))
                    .unwrap_or_default();
                self.push(PreloadKind::Image, url, attrs);
            }

            Local::Source if self.picture_depth > 0 => {
                let attrs = self.common_attrs(tag);
                let url = attrs
                    .srcset
                    .as_deref()
                    .and_then(choose_srcset_candidate)
                    .or_else(|| attr(tag, self.names.src).map(str::to_owned))
                    .unwrap_or_default();
                self.push(PreloadKind::Image, url, attrs);
            }

            Local::Video => {
                let attrs = self.common_attrs(tag);
                let url = attr(tag, self.names.poster).unwrap_or_default().to_owned();
                self.push(PreloadKind::Image, url, attrs);
            }

            Local::Input => {
                // Only <input type=image src> yields a hint.
                let input_type = attr(tag, self.names.r#type).map(|t| t.trim()).unwrap_or("");
                if !input_type.eq_ignore_ascii_case("image") {
                    return;
                }

                let attrs = self.common_attrs(tag);
                let url = attr(tag, self.names.src).unwrap_or_default().to_owned();
                self.push(PreloadKind::Image, url, attrs);
            }

            Local::Base => {
                let url = attr(tag, self.names.href).unwrap_or_default().to_owned();
                self.push(PreloadKind::BaseChanged, url, PreloadAttrs::default());
            }

            _ => {}
        }
    }

    /// The tokenizer state a start tag switches to, if any.
    fn raw_state_for(&self, tag: &Tag) -> Option<RawKind> {
        match tag.local {
            Local::Title | Local::Textarea => Some(RawKind::Rcdata),
            Local::Style | Local::Xmp | Local::Iframe | Local::Noembed | Local::Noframes => {
                Some(RawKind::Rawtext)
            }
            Local::Script => Some(RawKind::ScriptData),
            Local::Noscript if self.scripting_enabled => Some(RawKind::Rawtext),
            _ => None,
        }
    }
}

impl TokenSink for PreloadSink {
    type Handle = ();

    fn process_token(&mut self, token: Token<'_>, _line_number: u64) -> TokenSinkResult<()> {
        let Token::Tag(tag) = token else {
            return TokenSinkResult::Continue;
        };
        match tag.kind {
            TagKind::StartTag => {
                match tag.local {
                    Local::Template => self.template_depth += 1,
                    Local::Picture => self.picture_depth += 1,
                    _ => {}
                }
                self.handle_start_tag(&tag);
                if tag.self_closing {
                    return TokenSinkResult::Continue;
                }
                match tag.local {
                    Local::Plaintext => TokenSinkResult::Plaintext,
                    _ => match self.raw_state_for(&tag) {
                        Some(kind) => TokenSinkResult::RawData(kind),
                        None => TokenSinkResult::Continue,
                    },
                }
            }
            TagKind::EndTag => {
                match tag.local {
                    Local::Template => self.template_depth = self.template_depth.saturating_sub(1),
                    Local::Picture => self.picture_depth = self.picture_depth.saturating_sub(1),
                    _ => {}
                }
                TokenSinkResult::Continue
            }
        }
    }
}

/// A streaming preload scanner.
///
/// Feed it the same decoded text as the parser; it returns the hints it found
/// in each chunk.
pub struct PreloadScanner {
    tokenizer: Tokenizer,
    sink: PreloadSink,
    pending: String,
    pending_start: usize,
}

impl PreloadScanner {
    /// A scanner for a document whose scripting flag is `scripting_enabled`.
    pub fn new(scripting_enabled: bool) -> PreloadScanner {
        PreloadScanner::with_scan_mode(scripting_enabled, ScanMode::default())
    }

    /// A scanner that uses a specific text scanner, for the differential test
    /// (`docs/spec/13-performance.md` §5).
    pub fn with_scan_mode(scripting_enabled: bool, scan_mode: ScanMode) -> PreloadScanner {
        PreloadScanner {
            tokenizer: Tokenizer::new(TokenizerOpts {
                scan_mode,
                ..TokenizerOpts::default()
            }),
            sink: PreloadSink {
                scripting_enabled,
                hints: Vec::new(),
                template_depth: 0,
                picture_depth: 0,
                names: PreloadNames::default(),
            },
            pending: String::new(),
            pending_start: 0,
        }
    }

    /// Scans a chunk of decoded text and returns the hints it found.
    pub fn feed(&mut self, text: &str) -> Vec<PreloadHint> {
        self.run(text, false)
    }

    /// Scans the end of the input and returns any remaining hints.
    pub fn finish(&mut self) -> Vec<PreloadHint> {
        self.run("", true)
    }

    /// Scans a whole document in one call.
    pub fn scan(text: &str, scripting_enabled: bool) -> Vec<PreloadHint> {
        let mut scanner = PreloadScanner::new(scripting_enabled);
        let mut hints = scanner.feed(text);
        hints.extend(scanner.finish());
        hints
    }

    fn run(&mut self, text: &str, last: bool) -> Vec<PreloadHint> {
        let owned;
        let (buffer, start) = if self.pending.is_empty() {
            (text, 0usize)
        } else {
            let mut pending = std::mem::take(&mut self.pending);
            pending.push_str(text);
            owned = pending;
            (owned.as_str(), self.pending_start)
        };
        if buffer.is_empty() && !last {
            return std::mem::take(&mut self.sink.hints);
        }
        let mut input = Input::new(buffer, start, last);
        loop {
            match self.tokenizer.feed(&mut input, &mut self.sink) {
                crate::tokenizer::TokenizerResult::Done => break,
                crate::tokenizer::TokenizerResult::Script(()) => continue,
                crate::tokenizer::TokenizerResult::EncodingIndicator(_) => continue,
                crate::tokenizer::TokenizerResult::Stopped => break,
            }
        }
        if last {
            let _ = self.tokenizer.end(&mut input, &mut self.sink);
        }
        let position = input.pos();
        let mut retained_from = position - self.tokenizer.rewind_reserve().min(position);
        while !buffer.is_char_boundary(retained_from) {
            retained_from -= 1;
        }
        self.pending.clear();
        self.pending.push_str(&buffer[retained_from..]);
        self.pending_start = position - retained_from;
        std::mem::take(&mut self.sink.hints)
    }
}
