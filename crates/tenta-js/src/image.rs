//! The image seam between script and the page's image loader (IM4).
//!
//! An `<img>` loads when its element enters the document or its source
//! attributes change, whether or not the element is connected (`new Image()`
//! preloaders), so the loading cannot live in the engine: the page owns the
//! fetch, the decode and the render session that keeps the pixels. This module
//! is the vocabulary the two sides share, and nothing here does any work.
//!
//! * [`ImageHost`] answers what script reads (`complete`, `naturalWidth`,
//!   `naturalHeight`, `currentSrc`) and is told when script changed an
//!   attribute that can select another image.
//! * [`ImageEvent`] is what the page hands back when a load ended: the engine
//!   fires `load` or `error` at the element and settles the `decode()` promises
//!   waiting on it.
//!
//! Like the geometry host, an implementation runs inside an engine callback,
//! so it must not hold a borrow of the document across a call back into
//! script, and a host that cannot answer returns the neutral snapshot.

use tenta_core::{NodeId, NodeRef};

/// What script reads from an image element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageSnapshot {
    /// HTML's "completely available" or "broken", or an element with nothing
    /// to load. `false` while a request is outstanding.
    pub complete: bool,
    /// The density-corrected natural width in CSS pixels, `0` until loaded.
    pub natural_width: u32,
    /// The density-corrected natural height in CSS pixels, `0` until loaded.
    pub natural_height: u32,
    /// The URL the element selected, `""` when it selected none.
    pub current_src: String,
    /// The load ended in failure: `decode()` rejects.
    pub broken: bool,
}

/// How a load ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageEventKind {
    /// The image decoded: fire `load`.
    Load,
    /// The fetch or the decode failed: fire `error`.
    Error,
    /// Nothing is loading for the element after all (no source, the same
    /// source as before, a lazy image): fire nothing, and let go of the element
    /// the engine pinned when script touched its source.
    Release,
}

impl ImageEventKind {
    /// The event type fired at the element.
    pub const fn event_type(self) -> &'static str {
        match self {
            ImageEventKind::Load => "load",
            ImageEventKind::Error => "error",
            ImageEventKind::Release => "",
        }
    }
}

/// A load that ended, for the engine to report at the element.
///
/// The element is named by a generational reference, so an element the engine
/// reclaimed since the load began is skipped instead of being confused with a
/// node that reused its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageEvent {
    /// The element whose load ended.
    pub node: NodeRef,
    /// How it ended.
    pub kind: ImageEventKind,
}

/// The page's answers about image elements.
pub trait ImageHost {
    /// What script reads from `node` right now. An element whose source
    /// attributes changed since the loader last looked is reported as loading
    /// (`complete` false), because HTML's "update the image data" runs
    /// synchronously up to the point it starts the fetch.
    fn snapshot(&self, node: NodeId) -> ImageSnapshot;

    /// Script changed an attribute of `node` that can select another image
    /// (`src`, `srcset`, `sizes`, `loading`, `type`, `media`, `poster`), or
    /// cloned an element that has one. The page looks at the element on its
    /// next turn; this must not call back into script.
    ///
    /// Returns whether the page will report an [`ImageEvent`] for the element
    /// (a load, an error or a [`ImageEventKind::Release`]). When it does, the
    /// engine keeps the element alive until then, so an image script created
    /// and dropped (`new Image().src = url`) still loads and still fires its
    /// handlers, as in a browser. A host that loads nothing returns `false`.
    fn attribute_changed(&self, node: NodeId) -> bool;
}

/// The `Image` constructor (HTML, `HTMLImageElement`'s named constructor):
/// `new Image(width, height)` is an `<img>` element of the current document with
/// those attributes.
///
/// A script rather than a binding, because `tenta-idl` has no
/// `[LegacyFactoryFunction]` yet (the model parses it; neither backend emits
/// it). Written to be indistinguishable where a page can look: `length` is 0
/// (the arguments are read from `arguments`), `name` is `Image`, `prototype` is
/// `HTMLImageElement.prototype` and neither writable nor configurable,
/// `new.target` is honoured (a subclass gets its own prototype), calling
/// without `new` throws, an `undefined` argument is an absent one, and
/// `toString()` reads like a native function. It uses the `createElement` and
/// `setAttribute` that existed when it was defined, so a page that patches them
/// later cannot see or change what `new Image()` does. Not matched: `Image`
/// has an own `toString` property, which a native function does not.
///
/// A backend runs it once, when it is given an [`ImageHost`], and only when the
/// page has not defined an `Image` of its own.
pub const IMAGE_CONSTRUCTOR_SOURCE: &str = "(function () {
  'use strict';
  if (Object.prototype.hasOwnProperty.call(globalThis, 'Image')) return;
  var doc = document;
  var create = Document.prototype.createElement;
  var setAttribute = Element.prototype.setAttribute;
  var proto = HTMLImageElement.prototype;
  var apply = Function.prototype.call;
  var setPrototypeOf = Object.setPrototypeOf;
  var toText = String;
  function Image() {
    if (!new.target) {
      throw new TypeError(\"Failed to construct 'Image': Please use the 'new' operator, this DOM object constructor cannot be called as a function.\");
    }
    var image = apply.call(create, doc, 'img');
    if (arguments.length > 0 && arguments[0] !== undefined) {
      apply.call(setAttribute, image, 'width', toText(arguments[0] >>> 0));
    }
    if (arguments.length > 1 && arguments[1] !== undefined) {
      apply.call(setAttribute, image, 'height', toText(arguments[1] >>> 0));
    }
    if (new.target !== Image) setPrototypeOf(image, new.target.prototype);
    return image;
  }
  Object.defineProperty(Image, 'prototype', { value: proto, writable: false, enumerable: false, configurable: false });
  Object.defineProperty(Image, 'toString', { value: function toString() { return 'function Image() { [native code] }'; }, writable: true, enumerable: false, configurable: true });
  Object.defineProperty(globalThis, 'Image', { value: Image, writable: true, configurable: true, enumerable: false });
})();";
