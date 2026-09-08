//! HTTP/2 client fingerprint (TentaBrowser patch, not upstream `h2`).
//!
//! An Akamai-style HTTP/2 fingerprint is built from four observable
//! properties of a client connection: the SETTINGS identifiers with their
//! order and values, the connection-level WINDOW_UPDATE that follows the
//! preface, whether PRIORITY information is sent, and the order of the
//! request pseudo-headers. This module makes all four configurable;
//! [`H2Fingerprint::upstream`] reproduces stock `h2` and is what a
//! [`crate::client::Builder`] uses unless [`crate::client::Builder::fingerprint`]
//! says otherwise.

use std::sync::Arc;

use http::header::HeaderName;

/// SETTINGS identifier: HPACK dynamic table size of the *receiving* peer.
pub const SETTINGS_HEADER_TABLE_SIZE: u16 = 0x1;
/// SETTINGS identifier: server push.
pub const SETTINGS_ENABLE_PUSH: u16 = 0x2;
/// SETTINGS identifier: maximum concurrent streams.
pub const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
/// SETTINGS identifier: stream-level initial flow-control window.
pub const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;
/// SETTINGS identifier: maximum frame payload.
pub const SETTINGS_MAX_FRAME_SIZE: u16 = 0x5;
/// SETTINGS identifier: maximum uncompressed header list.
pub const SETTINGS_MAX_HEADER_LIST_SIZE: u16 = 0x6;
/// SETTINGS identifier: extended CONNECT (RFC 8441).
pub const SETTINGS_ENABLE_CONNECT_PROTOCOL: u16 = 0x8;
/// SETTINGS identifier: RFC 9218 priorities, RFC 7540 priorities deprecated.
pub const SETTINGS_NO_RFC7540_PRIORITIES: u16 = 0x9;

/// One request pseudo-header, used to describe their encoding order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PseudoId {
    /// `:method`
    Method,
    /// `:authority`
    Authority,
    /// `:scheme`
    Scheme,
    /// `:path`
    Path,
}

/// The stream-priority fields Chrome puts in the first HEADERS frame of a
/// stream (RFC 7540 §6.2; the weight is the wire value, i.e. weight - 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadersPriority {
    /// Whether the dependency is exclusive.
    pub exclusive: bool,
    /// The stream this stream depends on; Chrome uses 0.
    pub dependency: u32,
    /// The wire weight, one less than the RFC 7540 weight; Chrome sends 255.
    pub weight: u8,
}

/// Everything about a client connection that an HTTP/2 fingerprint observes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H2Fingerprint {
    settings: Vec<(u16, u32)>,
    window_update: Option<u32>,
    pseudo_order: [PseudoId; 4],
    headers_priority: Option<HeadersPriority>,
    never_index: Arc<[HeaderName]>,
    huffman: HuffmanPolicy,
}

/// When the HPACK encoder Huffman-codes a string literal.
///
/// Upstream `h2` always does. Chromium emits a plain literal unless the
/// Huffman form is *strictly* shorter, which changes the bytes of every short
/// value — `?0`, `?1`, `1`, and most opaque tokens and cookie values — inside
/// a frame whose framing is otherwise reproduced exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HuffmanPolicy {
    /// Huffman-code every non-empty string (stock `h2`).
    Always,
    /// Huffman-code only when it saves at least one octet (Chromium).
    WhenShorter,
}

impl Default for H2Fingerprint {
    /// Chrome's shape; see [`H2Fingerprint::chrome`].
    fn default() -> Self {
        Self::chrome()
    }
}

impl H2Fingerprint {
    /// Stock `h2`: no forced SETTINGS order, no initial WINDOW_UPDATE, the
    /// `:method :scheme :authority :path` pseudo-header order of RFC 9113's
    /// examples, no priority in HEADERS.
    pub fn upstream() -> Self {
        Self {
            settings: Vec::new(),
            window_update: None,
            pseudo_order: [
                PseudoId::Method,
                PseudoId::Scheme,
                PseudoId::Authority,
                PseudoId::Path,
            ],
            headers_priority: None,
            never_index: Arc::from([] as [HeaderName; 0]),
            huffman: HuffmanPolicy::Always,
        }
    }

    /// Chrome's shape (144, desktop): SETTINGS `1=65536, 2=0, 4=6291456,
    /// 6=262144`, a 15 663 105-octet connection WINDOW_UPDATE after the
    /// preface, `:method :authority :scheme :path`, and HEADERS carrying an
    /// exclusive dependency on stream 0 with wire weight 255.
    ///
    /// Profiles override every one of these values; this constructor is the
    /// documented default so a caller that only wants "not stock h2" gets a
    /// coherent set.
    pub fn chrome() -> Self {
        Self {
            settings: vec![
                (SETTINGS_HEADER_TABLE_SIZE, 65_536),
                (SETTINGS_ENABLE_PUSH, 0),
                (SETTINGS_INITIAL_WINDOW_SIZE, 6_291_456),
                (SETTINGS_MAX_HEADER_LIST_SIZE, 262_144),
            ],
            window_update: Some(15_663_105),
            pseudo_order: [
                PseudoId::Method,
                PseudoId::Authority,
                PseudoId::Scheme,
                PseudoId::Path,
            ],
            headers_priority: Some(HeadersPriority {
                exclusive: true,
                dependency: 0,
                weight: 255,
            }),
            never_index: Arc::from([] as [HeaderName; 0]),
            huffman: HuffmanPolicy::WhenShorter,
        }
    }

    /// Sets the SETTINGS identifiers, their order and their values. An empty
    /// list sends an empty SETTINGS frame.
    ///
    /// # Panics
    ///
    /// Panics if a value is outside the range RFC 9113 allows for that
    /// identifier, matching the behaviour of the `Builder` setters.
    pub fn with_settings(mut self, settings: Vec<(u16, u32)>) -> Self {
        for &(id, value) in &settings {
            match id {
                SETTINGS_ENABLE_PUSH | SETTINGS_ENABLE_CONNECT_PROTOCOL => {
                    assert!(value <= 1, "SETTINGS id {id} takes 0 or 1, got {value}");
                }
                SETTINGS_INITIAL_WINDOW_SIZE => {
                    assert!(value < 1 << 31, "INITIAL_WINDOW_SIZE out of range: {value}");
                }
                SETTINGS_MAX_FRAME_SIZE => {
                    assert!(
                        (16_384..1 << 24).contains(&value),
                        "MAX_FRAME_SIZE out of range: {value}"
                    );
                }
                _ => {}
            }
        }
        self.settings = settings;
        self
    }

    /// Sets the connection-level WINDOW_UPDATE increment sent right after the
    /// preface, or `None` to send none.
    pub fn with_window_update(mut self, increment: Option<u32>) -> Self {
        self.window_update = increment;
        self
    }

    /// Sets the encoding order of the four request pseudo-headers.
    ///
    /// # Panics
    ///
    /// Panics if `order` is not a permutation of the four pseudo-headers.
    pub fn with_pseudo_order(mut self, order: [PseudoId; 4]) -> Self {
        for id in [
            PseudoId::Method,
            PseudoId::Authority,
            PseudoId::Scheme,
            PseudoId::Path,
        ] {
            assert!(
                order.contains(&id),
                "pseudo-header order must be a permutation, {id:?} is missing"
            );
        }
        self.pseudo_order = order;
        self
    }

    /// Sets the priority fields written into request HEADERS frames, or
    /// `None` for HEADERS without the PRIORITY flag.
    pub fn with_headers_priority(mut self, priority: Option<HeadersPriority>) -> Self {
        self.headers_priority = priority;
        self
    }

    /// Sets the header names the HPACK encoder must emit as *never indexed*
    /// literals regardless of the value's own sensitivity flag.
    pub fn with_never_index(mut self, names: impl Into<Arc<[HeaderName]>>) -> Self {
        self.never_index = names.into();
        self
    }

    /// Sets when string literals are Huffman-coded.
    pub fn with_huffman(mut self, policy: HuffmanPolicy) -> Self {
        self.huffman = policy;
        self
    }

    /// When this fingerprint Huffman-codes a string literal.
    pub fn huffman_policy(&self) -> HuffmanPolicy {
        self.huffman
    }

    /// The SETTINGS identifiers and values in wire order.
    pub fn settings(&self) -> &[(u16, u32)] {
        &self.settings
    }

    /// The connection-level WINDOW_UPDATE increment, if any.
    pub fn window_update(&self) -> Option<u32> {
        self.window_update
    }

    /// The pseudo-header encoding order.
    pub fn pseudo_order(&self) -> [PseudoId; 4] {
        self.pseudo_order
    }

    /// The priority fields written into request HEADERS frames, if any.
    pub fn headers_priority(&self) -> Option<HeadersPriority> {
        self.headers_priority
    }

    /// The header names forced to never-indexed literals.
    pub fn never_index(&self) -> &[HeaderName] {
        &self.never_index
    }

    pub(crate) fn encoding(&self) -> EncodeFingerprint {
        EncodeFingerprint {
            pseudo_order: self.pseudo_order,
            headers_priority: self.headers_priority,
            never_index: Arc::clone(&self.never_index),
            huffman: self.huffman,
        }
    }
}

/// The part of a fingerprint the HPACK encoder and the HEADERS encoder need;
/// held by the connection's `hpack::Encoder`, cheap to clone.
#[derive(Debug, Clone)]
pub(crate) struct EncodeFingerprint {
    pseudo_order: [PseudoId; 4],
    headers_priority: Option<HeadersPriority>,
    never_index: Arc<[HeaderName]>,
    huffman: HuffmanPolicy,
}

impl Default for EncodeFingerprint {
    fn default() -> Self {
        H2Fingerprint::upstream().encoding()
    }
}

impl EncodeFingerprint {
    pub(crate) fn pseudo_order(&self) -> [PseudoId; 4] {
        self.pseudo_order
    }

    pub(crate) fn headers_priority(&self) -> Option<HeadersPriority> {
        self.headers_priority
    }

    pub(crate) fn is_never_indexed(&self, name: &HeaderName) -> bool {
        self.never_index.iter().any(|n| n == name)
    }

    pub(crate) fn huffman(&self) -> HuffmanPolicy {
        self.huffman
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_defaults_match_the_documented_wire_image() {
        let fp = H2Fingerprint::default();
        assert_eq!(
            fp.settings(),
            &[(1, 65_536), (2, 0), (4, 6_291_456), (6, 262_144)]
        );
        assert_eq!(fp.window_update(), Some(15_663_105));
        assert_eq!(
            fp.pseudo_order(),
            [
                PseudoId::Method,
                PseudoId::Authority,
                PseudoId::Scheme,
                PseudoId::Path
            ]
        );
        assert_eq!(
            fp.headers_priority(),
            Some(HeadersPriority {
                exclusive: true,
                dependency: 0,
                weight: 255
            })
        );
    }

    #[test]
    fn upstream_keeps_stock_h2_behaviour() {
        let fp = H2Fingerprint::upstream();
        assert!(fp.settings().is_empty());
        assert_eq!(fp.window_update(), None);
        assert_eq!(fp.headers_priority(), None);
        assert_eq!(
            fp.pseudo_order(),
            [
                PseudoId::Method,
                PseudoId::Scheme,
                PseudoId::Authority,
                PseudoId::Path
            ]
        );
    }

    #[test]
    #[should_panic(expected = "MAX_FRAME_SIZE out of range")]
    fn invalid_setting_values_are_rejected() {
        let _ = H2Fingerprint::upstream().with_settings(vec![(SETTINGS_MAX_FRAME_SIZE, 1)]);
    }

    #[test]
    #[should_panic(expected = "must be a permutation")]
    fn pseudo_order_must_be_a_permutation() {
        let _ = H2Fingerprint::upstream().with_pseudo_order([
            PseudoId::Method,
            PseudoId::Method,
            PseudoId::Scheme,
            PseudoId::Path,
        ]);
    }
}
