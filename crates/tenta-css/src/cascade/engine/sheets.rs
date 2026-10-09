//! Parsing of the user-agent and author sheets, including `@import` served
//! from the bodies the host prefetched.

use std::cell::RefCell;

use cssparser::{Parser, ParserInput, SourceLocation};
use servo_arc::Arc;
use style::context::QuirksMode;
use style::media_queries::MediaList;
use style::parser::ParserContext;
use style::shared_lock::{Locked, SharedRwLock};
use style::stylesheets::import_rule::{
    ImportLayer, ImportRule, ImportSheet, ImportSupportsCondition,
};
use style::stylesheets::{
    AllowImportRules, CssRuleType, DocumentStyleSheet, Origin, Stylesheet, StylesheetLoader,
    UrlExtraData,
};
use style::values::CssUrl;
use style_traits::ParsingMode;
use url::{Position, Url};

use crate::cascade::types::{ImportedSheet, StyleSource};

/// The deepest chain of nested `@import`s that is followed. The host already
/// bounds what it fetches; this keeps the parser's recursion bounded for any
/// producer of [`StyleSource`].
const MAX_IMPORT_DEPTH: usize = 16;

/// A user-agent sheet. Parsed as if for a standards-mode document: the sheet
/// is written in standard CSS and quirks mode only changes how the author's
/// unitless numbers read.
pub(super) fn user_agent_sheet(text: &str, lock: &SharedRwLock) -> DocumentStyleSheet {
    let base = Url::parse("about:blank").expect("about:blank is a valid URL");
    DocumentStyleSheet(Arc::new(Stylesheet::from_str(
        text,
        UrlExtraData::from(base),
        Origin::UserAgent,
        Arc::new(lock.wrap(MediaList::empty())),
        lock.clone(),
        None,
        None,
        QuirksMode::NoQuirks,
        AllowImportRules::No,
    )))
}

/// The author sheet of `source`, or `None` for a disabled one. Its `@import`s
/// resolve against `source.imports`.
pub(super) fn author_sheet(
    source: &StyleSource,
    lock: &SharedRwLock,
    quirks: QuirksMode,
) -> Option<DocumentStyleSheet> {
    if source.disabled {
        return None;
    }
    let url_data = UrlExtraData::from(source.url.clone());
    let media = media_list(source.media.as_deref(), &url_data, quirks);
    let imports = ImportServer {
        imports: &source.imports,
        quirks,
        active: RefCell::new(vec![lookup_key(&source.url).to_owned()]),
    };
    Some(DocumentStyleSheet(Arc::new(Stylesheet::from_str(
        &source.text,
        url_data,
        Origin::Author,
        Arc::new(lock.wrap(media)),
        lock.clone(),
        Some(&imports),
        None,
        quirks,
        AllowImportRules::Yes,
    ))))
}

/// The `media` attribute as Stylo reads it; absent or blank matches everything.
fn media_list(text: Option<&str>, url_data: &UrlExtraData, quirks: QuirksMode) -> MediaList {
    let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
        return MediaList::empty();
    };
    let mut context = ParserContext::new(
        Origin::Author,
        url_data,
        Some(CssRuleType::Media),
        ParsingMode::DEFAULT,
        quirks,
        Default::default(),
        None,
        None,
        Default::default(),
    );
    let mut input = ParserInput::new(text);
    MediaList::parse(&mut context, &mut Parser::new(&mut input))
}

/// The part of a URL that identifies a stylesheet: everything but the
/// fragment, which is never sent to a server.
fn lookup_key(url: &Url) -> &str {
    &url[..Position::AfterQuery]
}

/// Serves `@import` rules from prefetched bodies.
///
/// Stylo calls it while parsing, once per rule, and evaluates the rule's
/// `media`, `layer()` and `supports()` itself from what it passes in.
struct ImportServer<'a> {
    imports: &'a [ImportedSheet],
    quirks: QuirksMode,
    /// Lookup keys of the sheets being parsed, outermost first. A request for
    /// one of them is a cycle and gets nothing, as does one past the depth cap.
    active: RefCell<Vec<String>>,
}

impl ImportServer<'_> {
    /// The prefetched sheet for `url`, unless following it would loop.
    fn find(&self, url: &CssUrl) -> Option<&ImportedSheet> {
        let key = lookup_key(url.url()?);
        let active = self.active.borrow();
        if active.len() >= MAX_IMPORT_DEPTH || active.iter().any(|open| open == key) {
            return None;
        }
        self.imports
            .iter()
            .find(|imported| lookup_key(&imported.url) == key)
    }

    fn parse(
        &self,
        sheet: &ImportedSheet,
        lock: &SharedRwLock,
        media: Arc<Locked<MediaList>>,
    ) -> Stylesheet {
        self.active
            .borrow_mut()
            .push(lookup_key(&sheet.url).to_owned());
        let parsed = Stylesheet::from_str(
            &sheet.text,
            UrlExtraData::from(sheet.final_url.clone()),
            Origin::Author,
            media,
            lock.clone(),
            Some(self),
            None,
            self.quirks,
            AllowImportRules::Yes,
        );
        self.active.borrow_mut().pop();
        parsed
    }
}

impl StylesheetLoader for ImportServer<'_> {
    fn request_stylesheet(
        &self,
        url: CssUrl,
        location: SourceLocation,
        lock: &SharedRwLock,
        media: Arc<Locked<MediaList>>,
        supports: Option<ImportSupportsCondition>,
        layer: ImportLayer,
    ) -> Arc<Locked<ImportRule>> {
        let stylesheet = if supports.as_ref().is_some_and(|supports| !supports.enabled) {
            ImportSheet::new_refused()
        } else {
            // A missing body is a failed fetch: an empty sheet that still
            // carries the rule's media and layer, as a browser's failed import.
            let parsed = match self.find(&url) {
                Some(sheet) => self.parse(sheet, lock, media),
                None => Stylesheet::from_str(
                    "",
                    UrlExtraData::from(
                        Url::parse("about:blank").expect("about:blank is a valid URL"),
                    ),
                    Origin::Author,
                    media,
                    lock.clone(),
                    None,
                    None,
                    self.quirks,
                    AllowImportRules::No,
                ),
            };
            ImportSheet::new(Arc::new(parsed))
        };
        Arc::new(lock.wrap(ImportRule {
            url,
            stylesheet,
            supports,
            layer,
            source_location: location,
        }))
    }
}
