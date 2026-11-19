//! The seam between script and the page's stylesheets (CSS1, S1).
//!
//! A `<style>` element's sheet is its text, which script can read from the
//! document. A `<link>`'s is what the page fetched, and only the page has it, so
//! `link.sheet` and `document.styleSheets` ask a [`StyleHost`]. Like the image
//! and geometry hosts, an implementation runs inside an engine callback: it must
//! not hold a borrow of the document across a call back into script.
//!
//! The other direction is what script does to a sheet. The page lays out from
//! sheet text, so a sheet that script has edited through CSSOM, and one that
//! script constructed and adopted, reach it as a [`ScriptSheet`]: something
//! the page asks for the sheet's current text when it next needs styles.

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use tenta_core::{NodeId, NodeRef};
use url::Url;

/// A stylesheet as script sees it now, for the page that applies it.
///
/// The page keeps the object and asks again whenever it assembles the
/// document's styles; nothing is pushed at it per edit. [`Self::revision`] is
/// what tells it that asking again would give a different answer.
pub trait ScriptSheet {
    /// Changes whenever [`Self::text`] or [`Self::disabled`] would.
    fn revision(&self) -> u64;

    /// The sheet's rules, serialized, when they are no longer what the sheet's
    /// own source text parses to: `None` for the sheet of a `<style>` or
    /// `<link>` that script has only read, whose source the page already has.
    /// A constructed sheet has no source, so it always answers.
    ///
    /// `@import` rules are not part of this text; the page keeps the ones the
    /// source began with.
    fn text(&self) -> Option<String>;

    /// `sheet.disabled`.
    fn disabled(&self) -> bool;
}

/// Identifies the source text a `<style>` or `<link>` sheet object was made
/// from: `href` is a link's final URL, and `None` for `<style>`. Script makes
/// a new sheet object when this changes, and the page ignores what script did
/// to a sheet made from text the element no longer has.
pub fn source_fingerprint(href: Option<&Url>, text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    href.map(Url::as_str).hash(&mut hasher);
    text.hash(&mut hasher);
    hasher.finish()
}

/// The text of a stylesheet a `<link>` loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedSheet {
    /// The URL the text came from after redirects, which its relative URLs
    /// resolve against.
    pub url: Url,
    /// The decoded text.
    pub text: std::rc::Rc<str>,
    /// The sheet is opaque to script: fetched from another origin without a
    /// passing CORS check. Its rules cannot be read or changed (`SecurityError`),
    /// though it applies and `link.sheet` is not null.
    pub opaque: bool,
}

/// The page's answers about linked stylesheets.
pub trait StyleHost {
    /// The sheet `node` (a `<link rel=stylesheet>`) loaded, or `None` while it
    /// is loading, after it failed, and for a link that loads nothing.
    fn linked_sheet(&self, node: NodeId) -> Option<LinkedSheet>;

    /// Changes whenever a sheet finishes loading, so a list built from the
    /// document and these answers can be reused while neither moved.
    fn epoch(&self) -> u64 {
        0
    }

    /// Script has a sheet object for the `<style>` or `<link>` element
    /// `owner`, made from the source text `source` identifies
    /// ([`source_fingerprint`]). The page applies what script does to it for
    /// as long as the element still has that source.
    fn element_sheet(&self, _owner: NodeRef, _source: u64, _sheet: Rc<dyn ScriptSheet>) {}

    /// `adoptedStyleSheets` of the document (`scope` is `None`) or of the
    /// shadow root `scope` was assigned: these constructed sheets, in order,
    /// apply after the tree's own.
    fn adopted_sheets(&self, _scope: Option<NodeRef>, _sheets: Vec<Rc<dyn ScriptSheet>>) {}
}
