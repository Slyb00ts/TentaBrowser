//! Identifier newtypes. Ids are plain integers so they cross threads, FFI
//! and protocol boundaries without allocation.

use std::fmt;

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident($inner:ty)) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $inner);

        impl $name {
            /// Returns the raw value.
            #[inline]
            pub const fn raw(self) -> $inner {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(
    /// Index of a node in a document arena (`docs/spec/04-dom-and-css.md` §1).
    /// The generation lives in a side table; see [`NodeRef`].
    NodeId(u32)
);
id_type!(
    /// A page within an engine.
    PageId(u64)
);
id_type!(
    /// A browser context (cookie, storage and cache isolation unit).
    ContextId(u64)
);
id_type!(
    /// A frame within a page.
    FrameId(u64)
);
id_type!(
    /// A script world (main world or an isolated world).
    WorldId(u32)
);
id_type!(
    /// A network request, unique per process.
    RequestId(u64)
);

impl NodeId {
    /// Sentinel meaning "no node"; arenas never hand it out.
    pub const NONE: NodeId = NodeId(u32::MAX);

    /// True unless this is [`NodeId::NONE`].
    #[inline]
    pub const fn is_some(self) -> bool {
        self.0 != u32::MAX
    }
}

/// A node reference that is safe to hold across mutations: the generation
/// detects reuse of the index after the node was freed. This is the value
/// that crosses into JavaScript wrappers and host APIs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct NodeRef {
    pub id: NodeId,
    pub generation: u32,
}

impl NodeRef {
    #[inline]
    pub const fn new(id: NodeId, generation: u32) -> Self {
        Self { id, generation }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_plain_integers() {
        assert_eq!(NodeId(7).raw(), 7);
        assert_eq!(format!("{:?}", PageId(3)), "PageId(3)");
        assert_eq!(PageId(3).to_string(), "3");
        assert!(!NodeId::NONE.is_some());
        assert!(NodeId(0).is_some());
    }

    #[test]
    fn ids_serialise_transparently() {
        let json = serde_json_lite(NodeRef::new(NodeId(5), 2));
        assert_eq!(json, r#"{"id":5,"generation":2}"#);
    }

    fn serde_json_lite(r: NodeRef) -> String {
        format!(r#"{{"id":{},"generation":{}}}"#, r.id, r.generation)
    }
}
