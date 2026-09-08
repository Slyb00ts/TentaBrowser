//! Name resolution for the parsers.
//!
//! The names themselves live in one place, `tenta_core::atom::atoms`
//! (`docs/spec/02-architecture.md` §7). This module adds the two things the
//! parsers need on top of that table: interning goes through a lock, so the
//! tokenizer resolves each distinct name once through a small parser-local
//! [`NameCache`]; and the cache also resolves the name to a [`Local`]
//! discriminant, which lets the tree builder dispatch on an integer instead
//! of comparing strings.

use tenta_core::Atom;
use tenta_core::atom::atoms as core_atoms;

use crate::sink::QualName;

/// The namespaces the HTML tree builder distinguishes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ns {
    /// `http://www.w3.org/1999/xhtml`.
    Html,
    /// `http://www.w3.org/1998/Math/MathML`.
    MathMl,
    /// `http://www.w3.org/2000/svg`.
    Svg,
    /// Any other namespace (only reachable through fragment context elements).
    Other,
}

impl Ns {
    /// The interned namespace URI.
    pub(crate) fn atom(self) -> Atom {
        match self {
            Ns::Html | Ns::Other => core_atoms::NS_HTML,
            Ns::MathMl => core_atoms::NS_MATHML,
            Ns::Svg => core_atoms::NS_SVG,
        }
    }

    /// Classifies an interned namespace URI.
    pub(crate) fn from_atom(atom: Atom) -> Ns {
        if atom == core_atoms::NS_HTML {
            Ns::Html
        } else if atom == core_atoms::NS_SVG {
            Ns::Svg
        } else if atom == core_atoms::NS_MATHML {
            Ns::MathMl
        } else {
            Ns::Other
        }
    }
}

macro_rules! local_names {
    ($($variant:ident => $text:literal),* $(,)?) => {
        /// Discriminant for the names the parsers dispatch on. Any other name
        /// is [`Local::Other`]; the interned [`Atom`] carries the real text.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        #[allow(missing_docs)]
        pub(crate) enum Local {
            Other,
            $($variant,)*
        }

        impl Local {
            /// Number of discriminants, for tables indexed by `Local`.
            pub(crate) const COUNT: usize = 1 $(+ { let _ = stringify!($variant); 1 })*;

            /// Classifies a name. Unknown names map to [`Local::Other`].
            pub(crate) fn classify(text: &str) -> Local {
                match text {
                    $($text => Local::$variant,)*
                    _ => Local::Other,
                }
            }
        }
    };
}

local_names! {
    A => "a",
    Address => "address",
    AnnotationXml => "annotation-xml",
    Applet => "applet",
    Area => "area",
    Article => "article",
    Aside => "aside",
    B => "b",
    Base => "base",
    Basefont => "basefont",
    Bgsound => "bgsound",
    Big => "big",
    Blockquote => "blockquote",
    Body => "body",
    Br => "br",
    Button => "button",
    Caption => "caption",
    Center => "center",
    Code => "code",
    Col => "col",
    Colgroup => "colgroup",
    Dd => "dd",
    Desc => "desc",
    Details => "details",
    Dialog => "dialog",
    Dir => "dir",
    Div => "div",
    Dl => "dl",
    Dt => "dt",
    Em => "em",
    Embed => "embed",
    Fieldset => "fieldset",
    Figcaption => "figcaption",
    Figure => "figure",
    Font => "font",
    Footer => "footer",
    ForeignObject => "foreignObject",
    Form => "form",
    Frame => "frame",
    Frameset => "frameset",
    H1 => "h1",
    H2 => "h2",
    H3 => "h3",
    H4 => "h4",
    H5 => "h5",
    H6 => "h6",
    Head => "head",
    Header => "header",
    Hgroup => "hgroup",
    Hr => "hr",
    Html => "html",
    I => "i",
    Iframe => "iframe",
    Image => "image",
    Img => "img",
    Input => "input",
    Isindex => "isindex",
    Keygen => "keygen",
    Li => "li",
    Link => "link",
    Listing => "listing",
    Main => "main",
    Malignmark => "malignmark",
    Marquee => "marquee",
    Math => "math",
    Menu => "menu",
    Meta => "meta",
    Mglyph => "mglyph",
    Mi => "mi",
    Mn => "mn",
    Mo => "mo",
    Ms => "ms",
    Mtext => "mtext",
    Nav => "nav",
    Nobr => "nobr",
    Noembed => "noembed",
    Noframes => "noframes",
    Noscript => "noscript",
    Object => "object",
    Ol => "ol",
    Optgroup => "optgroup",
    Option => "option",
    Output => "output",
    P => "p",
    Param => "param",
    Picture => "picture",
    Plaintext => "plaintext",
    Pre => "pre",
    Rb => "rb",
    Rp => "rp",
    Rt => "rt",
    Rtc => "rtc",
    Ruby => "ruby",
    S => "s",
    Script => "script",
    Search => "search",
    Section => "section",
    Select => "select",
    Small => "small",
    Source => "source",
    Span => "span",
    Strike => "strike",
    Strong => "strong",
    Style => "style",
    Sub => "sub",
    Summary => "summary",
    Sup => "sup",
    Svg => "svg",
    Table => "table",
    Tbody => "tbody",
    Td => "td",
    Template => "template",
    Textarea => "textarea",
    Tfoot => "tfoot",
    Th => "th",
    Thead => "thead",
    Title => "title",
    Tr => "tr",
    Track => "track",
    Tt => "tt",
    U => "u",
    Ul => "ul",
    Var => "var",
    Video => "video",
    Wbr => "wbr",
    Xmp => "xmp",
    // Attribute names.
    AttrAs => "as",
    AttrAsync => "async",
    AttrCharset => "charset",
    AttrColor => "color",
    AttrContent => "content",
    AttrCrossorigin => "crossorigin",
    AttrDefer => "defer",
    AttrEncoding => "encoding",
    AttrFace => "face",
    AttrFetchpriority => "fetchpriority",
    AttrHref => "href",
    AttrHttpEquiv => "http-equiv",
    AttrImagesizes => "imagesizes",
    AttrImagesrcset => "imagesrcset",
    AttrIntegrity => "integrity",
    AttrMedia => "media",
    AttrNomodule => "nomodule",
    AttrPoster => "poster",
    AttrReferrerpolicy => "referrerpolicy",
    AttrRel => "rel",
    AttrSize => "size",
    AttrSizes => "sizes",
    AttrSrc => "src",
    AttrSrcset => "srcset",
    AttrType => "type",
}

/// A resolved element name: namespace, dispatch discriminant and the interned
/// local name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ElemName {
    pub ns: Ns,
    pub local: Local,
    pub atom: Atom,
}

impl ElemName {
    /// An HTML-namespace element name.
    pub(crate) fn html(local: Local, atom: Atom) -> ElemName {
        ElemName {
            ns: Ns::Html,
            local,
            atom,
        }
    }

    /// True for an HTML-namespace element with the given local name.
    #[inline]
    pub(crate) fn is_html(&self, local: Local) -> bool {
        self.ns == Ns::Html && self.local == local
    }

    /// The public, sink-facing form of this name.
    pub(crate) fn qual(&self) -> QualName {
        QualName::new(self.ns.atom(), self.atom)
    }
}

/// Resolves a [`QualName`] coming back from the sink (only used for the
/// fragment context element, which the parsers did not create themselves).
pub(crate) fn elem_name_of(name: &QualName) -> ElemName {
    ElemName {
        ns: Ns::from_atom(name.ns),
        local: Local::classify(name.local.as_str()),
        atom: name.local,
    }
}

/// Fixed size of [`NameCache`]; a power of two so the index is a mask.
const CACHE_SLOTS: usize = 1024;

struct CacheEntry {
    text: Box<str>,
    atom: Atom,
    local: Local,
}

/// A direct-mapped cache from name text to `(Atom, Local)`.
///
/// Interning goes through a process-wide lock, and a page repeats a handful of
/// tag and attribute names thousands of times. A collision simply evicts, so
/// the cache never grows and never needs invalidation.
pub(crate) struct NameCache {
    slots: Box<[Option<CacheEntry>]>,
}

impl Default for NameCache {
    fn default() -> Self {
        NameCache {
            slots: std::iter::repeat_with(|| None).take(CACHE_SLOTS).collect(),
        }
    }
}

/// FxHash-style string hash; names are short, so a byte-at-a-time mix is
/// cheaper than setting up a stronger hasher.
#[inline]
fn hash_name(text: &str) -> usize {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in text.as_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash >> 17) as usize & (CACHE_SLOTS - 1)
}

impl NameCache {
    /// Interns `text` and classifies it, caching the result.
    pub(crate) fn lookup(&mut self, text: &str) -> (Atom, Local) {
        let slot = hash_name(text);
        if let Some(entry) = &self.slots[slot]
            && &*entry.text == text
        {
            return (entry.atom, entry.local);
        }
        let atom = Atom::new(text);
        let local = Local::classify(text);
        self.slots[slot] = Some(CacheEntry {
            text: text.into(),
            atom,
            local,
        });
        (atom, local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_returns_stable_atoms_and_discriminants() {
        let mut cache = NameCache::default();
        let (atom, local) = cache.lookup("div");
        assert_eq!(local, Local::Div);
        assert_eq!(atom, Atom::new("div"));
        // Second lookup hits the cache and must agree.
        assert_eq!(cache.lookup("div"), (atom, local));
        let (atom, local) = cache.lookup("data-x");
        assert_eq!(local, Local::Other);
        assert_eq!(atom.as_str(), "data-x");
    }

    #[test]
    fn cache_survives_collisions() {
        let mut cache = NameCache::default();
        let names: Vec<String> = (0..4096).map(|i| format!("n{i}")).collect();
        for name in &names {
            let (atom, _) = cache.lookup(name);
            assert_eq!(atom.as_str(), name.as_str());
        }
        for name in &names {
            let (atom, _) = cache.lookup(name);
            assert_eq!(atom.as_str(), name.as_str());
        }
    }
}
