//! Interned **names** (`docs/spec/02-architecture.md` §7).
//!
//! An [`Atom`] is a 32-bit handle into a process-wide interner. Comparing two
//! atoms is an integer compare; the parser, selectors, style system and
//! bindings share the same table, so a tag name interned by the parser is
//! the same handle the bindings receive from JavaScript.
//!
//! Every name the engine needs is pre-interned as a constant (`atoms::*`), so
//! hot comparisons never touch the table and there is one place to add a name.
//!
//! ## What may be interned
//!
//! Only trusted static names: the set of element and attribute local names
//! defined in the HTML/XML specs, namespace URIs, and keywords the engine
//! itself compares against. Page-controlled names (unknown tag/attribute local
//! names, XML prefixes and namespace URIs, selector names from API calls) must
//! never reach this table. They are interned per document instead
//! (`tenta_dom::interner::DocInterner`), so they are freed when the document
//! is dropped. A page can produce unbounded distinct names, so interning them
//! process-wide would be a memory-exhaustion vector (`docs/spec/19-security.md` §5).
//!
//! Attribute **values** — ids, classes, anything a page can mint — must never
//! reach this table either. Values are interned per document, or compared as
//! strings; [`Atom::get`] exists so an untrusted string can be looked up
//! without being added.
//!
//! ## Deserialization safety
//!
//! [`Atom::new`] will intern an arbitrary string, making it suitable only for
//! trusted inputs (static config, fixtures, test data). Untrusted inputs from
//! network deserialization should use [`Atom::get`] first to avoid growing the
//! table, or should use per-document interning.

use std::fmt;
use std::sync::OnceLock;

use hashbrown::HashMap;
use parking_lot::RwLock;

/// Handle to an interned name.
///
/// The table holds names, not values, so it is bounded by the markup
/// vocabulary and is never reclaimed; see the module documentation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Atom(u32);

struct Interner {
    /// Owned strings, indexed by atom value. Boxed so the address is stable
    /// while `strings` grows (the map borrows from it).
    strings: Vec<Box<str>>,
    map: HashMap<&'static str, u32>,
}

// SAFETY: the `&'static str` keys point into boxes owned by `strings`, which
// are never dropped or moved (the Box keeps the heap address stable), and the
// interner itself lives for the whole process behind a `OnceLock`.

fn interner() -> &'static RwLock<Interner> {
    static INTERNER: OnceLock<RwLock<Interner>> = OnceLock::new();
    INTERNER.get_or_init(|| {
        let mut interner = Interner {
            strings: Vec::with_capacity(1024),
            map: HashMap::with_capacity(1024),
        };
        for (index, name) in atoms::STATIC_NAMES.iter().enumerate() {
            let atom = interner.intern_uncached(name);
            debug_assert_eq!(atom, index as u32, "static atom table out of order");
        }
        RwLock::new(interner)
    })
}

impl Interner {
    fn intern_uncached(&mut self, s: &str) -> u32 {
        if let Some(&index) = self.map.get(s) {
            return index;
        }
        let boxed: Box<str> = s.into();
        // SAFETY: see the module-level note; the box is pushed into `strings`
        // and never removed, so the pointer stays valid for 'static.
        let key: &'static str = unsafe { &*(boxed.as_ref() as *const str) };
        let index = u32::try_from(self.strings.len()).expect("atom table overflow");
        self.strings.push(boxed);
        self.map.insert(key, index);
        index
    }
}

impl Atom {
    /// Interns `s`, returning the existing atom if one exists.
    pub fn new(s: &str) -> Atom {
        {
            let guard = interner().read();
            if let Some(&index) = guard.map.get(s) {
                return Atom(index);
            }
        }
        let mut guard = interner().write();
        Atom(guard.intern_uncached(s))
    }

    /// Looks `s` up without interning it. Used on untrusted input paths
    /// (attribute values from the network) so an attacker cannot grow the
    /// table without bound.
    pub fn get(s: &str) -> Option<Atom> {
        interner().read().map.get(s).map(|&index| Atom(index))
    }

    /// The interned string. The returned reference lives for the process.
    pub fn as_str(self) -> &'static str {
        let guard = interner().read();
        let s: &str = &guard.strings[self.0 as usize];
        // SAFETY: strings are boxed and never removed from the table, so the
        // reference outlives the read guard (see the module-level note).
        unsafe { &*(s as *const str) }
    }

    /// Raw handle value, stable for the life of the process.
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Reconstructs an atom from a raw value produced by [`Atom::raw`] in
    /// this process. Panics on an unknown value.
    pub fn from_raw(raw: u32) -> Atom {
        let guard = interner().read();
        assert!(
            (raw as usize) < guard.strings.len(),
            "Atom::from_raw({raw}) is not an interned atom"
        );
        Atom(raw)
    }

    /// Number of interned strings in the process.
    pub fn count() -> usize {
        interner().read().strings.len()
    }

    /// ASCII case-insensitive equality against a string, without interning.
    pub fn eq_ignore_ascii_case(self, other: &str) -> bool {
        self.as_str().eq_ignore_ascii_case(other)
    }
}

impl fmt::Debug for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Atom({:?})", self.as_str())
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for Atom {
    fn from(s: &str) -> Self {
        Atom::new(s)
    }
}

impl PartialEq<str> for Atom {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Atom {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl AsRef<str> for Atom {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl serde::Serialize for Atom {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Interns the string, so every distinct value stays in the table for the
/// process lifetime. Deserialize atoms only from trusted configuration and
/// session files, never from page or network content: an attacker-controlled
/// stream of distinct strings would grow the table without bound
/// (`docs/spec/03-networking.md` N-5).
impl<'de> serde::Deserialize<'de> for Atom {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Ok(Atom::new(&s))
    }
}

/// Pre-interned atoms. The table order defines the constant values, so it
/// is append-only.
pub mod atoms {
    use super::Atom;

    /// Declares the static table.
    ///
    /// The index of each name is taken from an enum discriminant rather than
    /// from a recursive counter: a counter recurses once per name, which both
    /// hits the macro recursion limit and builds an `0u32 + 1u32 + …`
    /// expression per constant. This expansion is flat, so the table can grow
    /// to as many names as the engine needs.
    macro_rules! static_atoms {
        ($($name:ident = $value:literal),* $(,)?) => {
            pub(super) const STATIC_NAMES: &[&str] = &[$($value),*];

            #[allow(non_camel_case_types, dead_code, clippy::upper_case_acronyms)]
            #[repr(u32)]
            enum Index { $($name,)* }

            $(pub const $name: Atom = Atom(Index::$name as u32);)*
        };
    }

    static_atoms! {
        EMPTY = "",
        HTML = "html",
        HEAD = "head",
        BODY = "body",
        TITLE = "title",
        META = "meta",
        LINK = "link",
        SCRIPT = "script",
        STYLE = "style",
        DIV = "div",
        SPAN = "span",
        P = "p",
        A = "a",
        IMG = "img",
        UL = "ul",
        OL = "ol",
        LI = "li",
        TABLE = "table",
        TR = "tr",
        TD = "td",
        TH = "th",
        FORM = "form",
        INPUT = "input",
        BUTTON = "button",
        SELECT = "select",
        OPTION = "option",
        TEXTAREA = "textarea",
        LABEL = "label",
        IFRAME = "iframe",
        VIDEO = "video",
        AUDIO = "audio",
        SOURCE = "source",
        CANVAS = "canvas",
        SVG = "svg",
        TEMPLATE = "template",
        SLOT = "slot",
        BR = "br",
        HR = "hr",
        H1 = "h1",
        H2 = "h2",
        H3 = "h3",
        H4 = "h4",
        H5 = "h5",
        H6 = "h6",
        NAV = "nav",
        MAIN = "main",
        HEADER = "header",
        FOOTER = "footer",
        SECTION = "section",
        ARTICLE = "article",
        ASIDE = "aside",
        PRE = "pre",
        CODE = "code",
        EM = "em",
        STRONG = "strong",
        B = "b",
        I = "i",
        NOSCRIPT = "noscript",
        // attributes
        ID = "id",
        CLASS = "class",
        HREF = "href",
        SRC = "src",
        ALT = "alt",
        NAME = "name",
        TYPE = "type",
        VALUE = "value",
        REL = "rel",
        CONTENT = "content",
        CHARSET = "charset",
        HTTP_EQUIV = "http-equiv",
        LANG = "lang",
        DIR = "dir",
        ROLE = "role",
        ARIA_LABEL = "aria-label",
        TABINDEX = "tabindex",
        HIDDEN = "hidden",
        DISABLED = "disabled",
        CHECKED = "checked",
        SELECTED = "selected",
        PLACEHOLDER = "placeholder",
        ACTION = "action",
        METHOD = "method",
        FOR = "for",
        WIDTH = "width",
        HEIGHT = "height",
        SRCSET = "srcset",
        LOADING = "loading",
        ASYNC = "async",
        DEFER = "defer",
        NOMODULE = "nomodule",
        CROSSORIGIN = "crossorigin",
        INTEGRITY = "integrity",
        NONCE = "nonce",
        MEDIA = "media",
        AS = "as",
        // namespaces
        NS_HTML = "http://www.w3.org/1999/xhtml",
        NS_SVG = "http://www.w3.org/2000/svg",
        NS_MATHML = "http://www.w3.org/1998/Math/MathML",
        NS_XLINK = "http://www.w3.org/1999/xlink",
        NS_XML = "http://www.w3.org/XML/1998/namespace",
        NS_XMLNS = "http://www.w3.org/2000/xmlns/",
        // Appended for the extractors, the serialisers and the parsers
        // (this table is the only place a name is declared).
        ABBR = "abbr",
        ADDRESS = "address",
        ANNOTATION_XML = "annotation-xml",
        AREA = "area",
        BASE = "base",
        BDI = "bdi",
        BDO = "bdo",
        BIG = "big",
        BLOCKQUOTE = "blockquote",
        CAPTION = "caption",
        CENTER = "center",
        CITE = "cite",
        COL = "col",
        COLGROUP = "colgroup",
        DATA = "data",
        DATALIST = "datalist",
        DD = "dd",
        DEL = "del",
        DETAILS = "details",
        DFN = "dfn",
        DIALOG = "dialog",
        DL = "dl",
        DT = "dt",
        EMBED = "embed",
        FIELDSET = "fieldset",
        FIGCAPTION = "figcaption",
        FIGURE = "figure",
        FONT = "font",
        FOREIGN_OBJECT = "foreignObject",
        FRAME = "frame",
        FRAMESET = "frameset",
        HGROUP = "hgroup",
        INS = "ins",
        KBD = "kbd",
        LEGEND = "legend",
        LISTING = "listing",
        MAP = "map",
        MARK = "mark",
        MARQUEE = "marquee",
        MATH = "math",
        MENU = "menu",
        METER = "meter",
        NOEMBED = "noembed",
        NOFRAMES = "noframes",
        OBJECT = "object",
        OPTGROUP = "optgroup",
        OUTPUT = "output",
        PARAM = "param",
        PICTURE = "picture",
        PLAINTEXT = "plaintext",
        PROGRESS = "progress",
        Q = "q",
        RP = "rp",
        RT = "rt",
        RUBY = "ruby",
        S = "s",
        SAMP = "samp",
        SEARCH = "search",
        SELECTEDCONTENT = "selectedcontent",
        SMALL = "small",
        STRIKE = "strike",
        SUB = "sub",
        SUMMARY = "summary",
        SUP = "sup",
        TBODY = "tbody",
        TFOOT = "tfoot",
        THEAD = "thead",
        TIME = "time",
        TRACK = "track",
        TT = "tt",
        U = "u",
        VAR = "var",
        WBR = "wbr",
        XMP = "xmp",
        ARIA_CHECKED = "aria-checked",
        ARIA_DESCRIBEDBY = "aria-describedby",
        ARIA_DISABLED = "aria-disabled",
        ARIA_EXPANDED = "aria-expanded",
        ARIA_HIDDEN = "aria-hidden",
        ARIA_LABELLEDBY = "aria-labelledby",
        ARIA_LEVEL = "aria-level",
        ARIA_REQUIRED = "aria-required",
        ARIA_SELECTED = "aria-selected",
        ARIA_VALUENOW = "aria-valuenow",
        AUTOCOMPLETE = "autocomplete",
        COLSPAN = "colspan",
        CONTENTEDITABLE = "contenteditable",
        DATETIME = "datetime",
        DOWNLOAD = "download",
        ENCTYPE = "enctype",
        ITEMID = "itemid",
        ITEMPROP = "itemprop",
        ITEMREF = "itemref",
        ITEMSCOPE = "itemscope",
        ITEMTYPE = "itemtype",
        LIST = "list",
        MAX = "max",
        MAXLENGTH = "maxlength",
        MIN = "min",
        MULTIPLE = "multiple",
        OPEN = "open",
        PATTERN = "pattern",
        PING = "ping",
        PROPERTY = "property",
        READONLY = "readonly",
        REQUIRED = "required",
        REVERSED = "reversed",
        ROWSPAN = "rowspan",
        START = "start",
        STEP = "step",
        TARGET = "target",
        APPLICATION_LD_JSON = "application/ld+json",
        CANONICAL = "canonical",
        XML = "xml",
        XMLNS = "xmlns",
        XLINK = "xlink",
        DEFINITION_URL = "definitionURL",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_idempotent_and_comparable_by_handle() {
        let a = Atom::new("data-testid");
        let b = Atom::new("data-testid");
        assert_eq!(a, b);
        assert_eq!(a.raw(), b.raw());
        assert_eq!(a.as_str(), "data-testid");
        assert_eq!(a, "data-testid");
        assert_ne!(a, Atom::new("data-testId"));
    }

    #[test]
    fn static_atoms_match_their_strings() {
        assert_eq!(atoms::DIV.as_str(), "div");
        assert_eq!(Atom::new("div"), atoms::DIV);
        assert_eq!(Atom::new(""), atoms::EMPTY);
        assert_eq!(atoms::NS_HTML.as_str(), "http://www.w3.org/1999/xhtml");
        assert_eq!(Atom::from_raw(atoms::CLASS.raw()), atoms::CLASS);
    }

    #[test]
    fn get_does_not_intern() {
        let before = Atom::count();
        assert!(Atom::get("definitely-not-interned-yet-xyz").is_none());
        assert_eq!(Atom::count(), before);
        let atom = Atom::new("definitely-not-interned-yet-xyz");
        assert_eq!(Atom::get("definitely-not-interned-yet-xyz"), Some(atom));
    }

    #[test]
    fn atoms_survive_table_growth() {
        let early = Atom::new("early-atom");
        let early_str = early.as_str();
        for i in 0..5000 {
            Atom::new(&format!("filler-{i}"));
        }
        assert_eq!(early.as_str(), "early-atom");
        assert!(std::ptr::eq(early_str, early.as_str()));
    }

    #[test]
    fn concurrent_interning_agrees() {
        let handles: Vec<_> = (0..8)
            .map(|t| {
                std::thread::spawn(move || {
                    (0..200)
                        .map(|i| Atom::new(&format!("shared-{}", (i + t) % 50)).raw())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            for raw in handle.join().unwrap() {
                let atom = Atom::from_raw(raw);
                assert!(atom.as_str().starts_with("shared-"));
            }
        }
    }
}
