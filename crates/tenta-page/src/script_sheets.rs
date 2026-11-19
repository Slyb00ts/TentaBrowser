//! Stylesheets as script has left them, for the render session (S1).
//!
//! The render session lays out from sheet text the page collects from the
//! document (`styles.rs`). Script changes sheets without changing the
//! document: `insertRule()` on the sheet of a `<style>`, a property set on a
//! rule's `style`, `sheet.disabled`, a constructed sheet in
//! `adoptedStyleSheets`. The engine hands each such sheet to the page's
//! [`StyleHost`](tenta_js::stylesheets::StyleHost) as a
//! [`ScriptSheet`]; this module keeps them per document, and the collection
//! reads the sheet's current text from here in place of the element's source.
//!
//! Nothing is recomputed per edit. A sheet reports a revision, the sum of
//! which is part of the style signature, and its text is asked for only when
//! the signature moved and the sources are assembled again.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use tenta_core::NodeRef;
use tenta_js::stylesheets::ScriptSheet;

thread_local! {
    /// The registry of each live document, by document id. The collection
    /// has the document and not the page's style host, and the id is in both.
    static BY_DOCUMENT: RefCell<HashMap<u64, Weak<ScriptSheets>>> = RefCell::new(HashMap::new());
}

/// What script did to the sheet of one `<style>` or `<link>` element.
pub(crate) struct ElementEdit {
    /// The sheet's rules, when script changed them; `None` keeps the source.
    pub text: Option<String>,
    pub disabled: bool,
}

/// One constructed sheet in an `adoptedStyleSheets` list.
pub(crate) struct AdoptedSheet {
    /// The shadow root whose list it is in; `None` is the document's.
    pub scope: Option<NodeRef>,
    pub text: String,
    pub disabled: bool,
}

/// The sheet object script has for an element, with the fingerprint of the
/// source it was made from. Weak: the engine owns the sheet, and one it has
/// dropped has nothing left to say.
struct ElementSheet {
    source: u64,
    sheet: Weak<dyn ScriptSheet>,
}

/// One tree's `adoptedStyleSheets`, in order.
struct AdoptedList {
    /// The shadow root whose list it is; `None` is the document's.
    scope: Option<NodeRef>,
    sheets: Vec<Rc<dyn ScriptSheet>>,
}

/// The script sheets of one document.
pub(crate) struct ScriptSheets {
    document_id: u64,
    elements: RefCell<HashMap<NodeRef, ElementSheet>>,
    adopted: RefCell<Vec<AdoptedList>>,
}

impl ScriptSheets {
    /// The registry of the document `document_id`, made and published.
    pub(crate) fn for_document(document_id: u64) -> Rc<ScriptSheets> {
        let sheets = Rc::new(ScriptSheets {
            document_id,
            elements: RefCell::new(HashMap::new()),
            adopted: RefCell::new(Vec::new()),
        });
        BY_DOCUMENT.with(|cell| {
            cell.borrow_mut()
                .insert(document_id, Rc::downgrade(&sheets));
        });
        sheets
    }

    /// The registry of the document `document_id`, if script has one.
    pub(crate) fn of(document_id: u64) -> Option<Rc<ScriptSheets>> {
        BY_DOCUMENT.with(|cell| cell.borrow().get(&document_id).and_then(Weak::upgrade))
    }

    pub(crate) fn set_element(&self, owner: NodeRef, source: u64, sheet: &Rc<dyn ScriptSheet>) {
        let mut elements = self.elements.borrow_mut();
        // Elements come and go; an entry whose sheet is gone says nothing.
        if elements.len() >= 64 {
            elements.retain(|_, known| known.sheet.strong_count() > 0);
        }
        elements.insert(
            owner,
            ElementSheet {
                source,
                sheet: Rc::downgrade(sheet),
            },
        );
    }

    pub(crate) fn set_adopted(&self, scope: Option<NodeRef>, sheets: Vec<Rc<dyn ScriptSheet>>) {
        let mut adopted = self.adopted.borrow_mut();
        adopted.retain(|list| list.scope != scope);
        if !sheets.is_empty() {
            adopted.push(AdoptedList { scope, sheets });
        }
    }

    /// A value that moves whenever any answer of this registry would: which
    /// sheets are registered and adopted, and the revision of each.
    ///
    /// This runs on every layout question script asks, so it does not hash
    /// with SipHash: each sheet contributes one well-mixed word. The words of
    /// the element sheets are summed, which makes the map's iteration order
    /// irrelevant, and mixing each before the sum is what keeps two edits
    /// from cancelling.
    pub(crate) fn stamp(&self) -> u64 {
        let mut stamp = 0u64;
        for (owner, known) in self.elements.borrow().iter() {
            let Some(sheet) = known.sheet.upgrade() else {
                continue;
            };
            let word = mix(node_word(*owner) ^ known.source).wrapping_add(sheet.revision());
            stamp = stamp.wrapping_add(mix(word));
        }
        for list in self.adopted.borrow().iter() {
            // The order of a list is part of what it says.
            let mut word = mix(list.scope.map_or(u64::MAX, node_word));
            for sheet in &list.sheets {
                let identity = Rc::as_ptr(sheet) as *const () as usize as u64;
                word = mix(word ^ identity).wrapping_add(sheet.revision());
            }
            stamp = stamp.wrapping_add(mix(word));
        }
        stamp
    }

    /// What script did to the sheet of `owner`, when the sheet object it has
    /// was made from the source `source` identifies.
    pub(crate) fn element(&self, owner: NodeRef, source: u64) -> Option<ElementEdit> {
        let elements = self.elements.borrow();
        let known = elements.get(&owner)?;
        if known.source != source {
            return None;
        }
        let sheet = known.sheet.upgrade()?;
        Some(ElementEdit {
            text: sheet.text(),
            disabled: sheet.disabled(),
        })
    }

    /// Every adopted sheet: the document's first, then each shadow root's in
    /// the order the lists were first assigned.
    pub(crate) fn adopted(&self) -> Vec<AdoptedSheet> {
        let adopted = self.adopted.borrow();
        let mut lists: Vec<_> = adopted.iter().collect();
        lists.sort_by_key(|list| list.scope.is_some());
        lists
            .into_iter()
            .flat_map(|list| {
                list.sheets.iter().map(|sheet| AdoptedSheet {
                    scope: list.scope,
                    text: sheet.text().unwrap_or_default(),
                    disabled: sheet.disabled(),
                })
            })
            .collect()
    }
}

fn node_word(node: NodeRef) -> u64 {
    (u64::from(node.generation) << 32) ^ u64::from(node.id.raw())
}

/// The finalizer of SplitMix64: every input bit reaches every output bit.
fn mix(mut word: u64) -> u64 {
    word ^= word >> 30;
    word = word.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    word ^= word >> 27;
    word = word.wrapping_mul(0x94d0_49bb_1331_11eb);
    word ^ (word >> 31)
}

impl Drop for ScriptSheets {
    fn drop(&mut self) {
        // `try_with`: the thread's registry may already be gone at teardown.
        let _ = BY_DOCUMENT.try_with(|cell| {
            if let Ok(mut documents) = cell.try_borrow_mut()
                && documents
                    .get(&self.document_id)
                    .is_some_and(|known| known.strong_count() == 0)
            {
                documents.remove(&self.document_id);
            }
        });
    }
}
