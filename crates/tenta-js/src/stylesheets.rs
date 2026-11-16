//! The seam between script and the page's stylesheet loader (CSS1).
//!
//! A `<style>` element's sheet is its text, which script can read from the
//! document. A `<link>`'s is what the page fetched, and only the page has it, so
//! `link.sheet` and `document.styleSheets` ask a [`StyleHost`]. Like the image
//! and geometry hosts, an implementation runs inside an engine callback: it must
//! not hold a borrow of the document across a call back into script.

use tenta_core::NodeId;
use url::Url;

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
}
