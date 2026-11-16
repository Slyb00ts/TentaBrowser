//! A navigation the document's script asked for.
//!
//! Script cannot replace its own document: `location.assign`, a form
//! submission or an activated link only says where the page should go next.
//! The request waits on the document, which is the one object the script
//! world and the page that owns the document both hold, until the page takes
//! it after the script entry that made it has returned.

use url::Url;

/// The entity body of a form submission by POST.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostData {
    /// The `Content-Type` header value, boundary included for multipart.
    pub content_type: String,
    pub body: Vec<u8>,
}

/// Where script asked the page to navigate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationRequest {
    pub url: Url,
    /// `Some` for a POST navigation.
    pub post: Option<PostData>,
    /// The navigation replaces the current session history entry
    /// (`location.replace`) instead of adding one.
    pub replace: bool,
}

impl NavigationRequest {
    /// A GET navigation that adds a history entry.
    pub fn get(url: Url) -> Self {
        Self {
            url,
            post: None,
            replace: false,
        }
    }
}
