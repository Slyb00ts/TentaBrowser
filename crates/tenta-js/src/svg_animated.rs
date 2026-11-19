//! `SVGAnimatedString`, `SVGElement.className` and `SVGAElement.href` (SVG 2,
//! <https://svgwg.org/svg2-draft/types.html#InterfaceSVGAnimatedString>), as
//! a script the engine builds the first time a page asks for one of them.
//!
//! # Why
//!
//! On an SVG element `className` is not a string, and on an SVG `<a>` neither
//! is `href`: each is an object whose `baseVal` is the attribute. Script that
//! walks every link of a page asks which kind it has -
//! `a.href instanceof SVGAnimatedString ? a.href.animVal : a.href` - and
//! without the interface the question itself threw. Without the objects the
//! answer would also be wrong: an SVG element's `className` was the string
//! `Element` declares, which no browser gives.
//!
//! # What is here
//!
//!  - `SVGAnimatedString`: `baseVal`, which reads and writes the attribute,
//!    and `animVal`, which reads it.
//!  - `SVGElement.prototype.className`, reflecting `class`, and
//!    `SVGAElement.prototype.href`, reflecting `href` and, when the element
//!    has no `href`, the deprecated `xlink:href`. Each element answers the
//!    same object every time.
//!
//! # What is not, and what differs
//!
//!  - **Nothing animates.** There is no SMIL here, so `animVal` is always
//!    `baseVal`.
//!  - **`href` on SVG `<a>` only.** The other elements of SVG's
//!    `SVGURIReference` (`<use>`, `<image>`, `<script>`, the gradients and
//!    the rest) have no interface of their own in this engine, so no `href`.
//!  - **No other animated type**: `SVGAnimatedLength`, `SVGAnimatedRect` and
//!    the rest are absent.
//!
//! # How it is built
//!
//! The source evaluates to a function of no arguments that returns the
//! exports. It must not read `SVGAnimatedString` from the global, which is
//! the lazy name that runs it.

/// The script. Evaluates to `function ()` returning
/// `{ SVGAnimatedString, className(element), href(element) }`.
pub const SVG_ANIMATED_PRELUDE_SOURCE: &str = r#"
(function () {
  'use strict';
  const XLINK = 'http://www.w3.org/1999/xlink';
  const apply = Reflect.apply;
  const element = Element.prototype;
  const getAttribute = element.getAttribute, setAttribute = element.setAttribute;
  const getAttributeNS = element.getAttributeNS, setAttributeNS = element.setAttributeNS;
  const state = new WeakMap();
  const classNames = new WeakMap();
  const hrefs = new WeakMap();
  let constructing = false;

  function slots(value) {
    const s = state.get(value);
    if (s === undefined) throw new TypeError('Illegal invocation');
    return s;
  }

  function read(s) {
    if (!s.href) {
      const value = apply(getAttribute, s.element, ['class']);
      return value === null ? '' : value;
    }
    const own = apply(getAttribute, s.element, ['href']);
    if (own !== null) return own;
    const linked = apply(getAttributeNS, s.element, [XLINK, 'href']);
    return linked === null ? '' : linked;
  }

  function write(s, value) {
    if (!s.href) {
      apply(setAttribute, s.element, ['class', value]);
    } else if (apply(getAttribute, s.element, ['href']) === null &&
               apply(getAttributeNS, s.element, [XLINK, 'href']) !== null) {
      apply(setAttributeNS, s.element, [XLINK, 'xlink:href', value]);
    } else {
      apply(setAttribute, s.element, ['href', value]);
    }
  }

  class SVGAnimatedString {
    constructor() {
      if (!constructing) throw new TypeError('Illegal constructor');
    }
    get baseVal() { return read(slots(this)); }
    set baseVal(value) { write(slots(this), '' + value); }
    get animVal() { return read(slots(this)); }
  }

  ['baseVal', 'animVal'].forEach(function (name) {
    const descriptor = Object.getOwnPropertyDescriptor(SVGAnimatedString.prototype, name);
    descriptor.enumerable = true;
    Object.defineProperty(SVGAnimatedString.prototype, name, descriptor);
  });
  Object.defineProperty(SVGAnimatedString.prototype, Symbol.toStringTag, {
    value: 'SVGAnimatedString', configurable: true,
  });

  function of(made, target, href) {
    let animated = made.get(target);
    if (animated === undefined) {
      constructing = true;
      try {
        animated = new SVGAnimatedString();
      } finally {
        constructing = false;
      }
      state.set(animated, { element: target, href: href });
      made.set(target, animated);
    }
    return animated;
  }

  return {
    SVGAnimatedString: SVGAnimatedString,
    className: function (target) { return of(classNames, target, false); },
    href: function (target) { return of(hrefs, target, true); },
  };
})
"#;
