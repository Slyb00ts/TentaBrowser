//! DOM exception names mapped onto [`tenta_core::Error`].
//!
//! The public error type has no DOM-specific variants, so DOM exceptions
//! travel in `Error::Other` with the DOM standard's name as the machine
//! readable `code` (`docs/spec/04-dom-and-css.md` §1.1 names
//! `HierarchyRequestError` and `InvalidStateError` explicitly).

use tenta_core::Error;

fn dom(code: &str, message: impl Into<String>) -> Error {
    Error::Other {
        code: code.to_owned(),
        message: message.into(),
    }
}

/// The insertion would build a cycle or place a node where its kind is not
/// allowed.
pub fn hierarchy_request(message: impl Into<String>) -> Error {
    dom("HierarchyRequestError", message)
}

/// A stale [`tenta_core::NodeRef`], or an operation on a node that is not in
/// the state the operation requires.
pub fn invalid_state(message: impl Into<String>) -> Error {
    dom("InvalidStateError", message)
}

/// A node that should have been a child of another was not.
pub fn not_found(message: impl Into<String>) -> Error {
    dom("NotFoundError", message)
}

/// A selector or other string could not be parsed.
pub fn syntax(message: impl Into<String>) -> Error {
    dom("SyntaxError", message)
}

/// The modification cannot be performed at this position (e.g. setting
/// `outerHTML` on a node whose parent is the document, or `insertAdjacentHTML`
/// with `beforebegin`/`afterend` on a node without an element parent).
pub fn no_modification_allowed(message: impl Into<String>) -> Error {
    dom("NoModificationAllowedError", message)
}

/// An index or offset is outside the allowed range (e.g. a `CharacterData`
/// offset past the node's length).
pub fn index_size(message: impl Into<String>) -> Error {
    dom("IndexSizeError", message)
}

/// The DOM exception name of an error produced by this crate, if it has one.
pub fn dom_exception_name(error: &Error) -> Option<&str> {
    match error {
        Error::Other { code, .. } => Some(code.as_str()),
        _ => None,
    }
}
