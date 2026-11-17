//! `document.fonts`, `FontFaceSet` and `FontFace` (CSS Font Loading Module
//! Level 3), as a script the engine runs before any page script.
//!
//! # What this is and is not
//!
//! Script has no font to wait for in this engine: text is laid out with the
//! fonts the render tier already has, and nothing a page does through this API
//! fetches or decodes a font. So the API answers the way a document whose fonts
//! have all finished loading does — `document.fonts.ready` is resolved,
//! `status` is `"loaded"`, `check()` is `true`, and `load()` resolves — and a
//! page that waits for its fonts before it shows its content gets on with it.
//!
//! The deviations that follow from that:
//!
//!  - A `FontFace` made from a URL becomes `"loaded"` when `load()` is called,
//!    without a request; it never becomes `"error"`. One made from a buffer is
//!    `"loaded"` from the start, where the standard parses the data first.
//!  - A face added to `document.fonts` is remembered and iterated, and is not
//!    used to draw text.
//!  - `document.fonts` does not list the document's `@font-face` rules.
//!  - `loading`, `loadingdone` and `loadingerror` are never fired; the handler
//!    attributes exist and hold what they are given.
//!  - `load()` matches the font shorthand's family list against the faces in
//!    the set by name; a string it cannot read a family from resolves with an
//!    empty list where the standard rejects with a `SyntaxError`.

/// Defines `FontFace`, `FontFaceSet` and `Document.prototype.fonts`. Run once
/// when a page's engine is built; does nothing in a realm without `Document`
/// and `EventTarget`.
pub const FONTS_PRELUDE_SOURCE: &str = r#"(function () {
  'use strict';
  var Doc = globalThis.Document, Target = globalThis.EventTarget;
  if (typeof Doc !== 'function' || typeof Target !== 'function') return;
  var define = Object.defineProperty, TypeErr = TypeError, Str = String;
  var PromiseCtor = Promise, States = WeakMap, List = Array;
  var faces = new States(), sets = new States(), byDocument = new States();
  var internal = {};

  function method(target, name, fn) {
    define(target, name, { value: fn, writable: true, enumerable: true, configurable: true });
  }
  function getter(target, name, get, set) {
    define(target, name, { get: get, set: set, enumerable: true, configurable: true });
  }
  function interfaceObject(name, Constructor) {
    define(Constructor.prototype, Symbol.toStringTag,
      { value: name, writable: false, enumerable: false, configurable: true });
    define(globalThis, name,
      { value: Constructor, writable: true, enumerable: false, configurable: true });
  }
  function state(map, object) {
    var found = map.get(object);
    if (found === undefined) throw new TypeErr('Illegal invocation');
    return found;
  }

  // ---- FontFace -----------------------------------------------------------

  var DESCRIPTORS = [
    ['style', 'normal'], ['weight', 'normal'], ['stretch', 'normal'],
    ['unicodeRange', 'U+0-10FFFF'], ['featureSettings', 'normal'],
    ['variationSettings', 'normal'], ['display', 'auto'], ['ascentOverride', 'normal'],
    ['descentOverride', 'normal'], ['lineGapOverride', 'normal']
  ];

  function markLoaded(face, record) {
    if (record.status === 'loaded') return;
    record.status = 'loaded';
    record.resolve(face);
  }

  function FontFace(family, source) {
    if (new.target === undefined) {
      throw new TypeErr("Failed to construct 'FontFace': Please use the 'new' operator");
    }
    if (arguments.length < 2) {
      throw new TypeErr("Failed to construct 'FontFace': 2 arguments required, but only " +
        arguments.length + ' present.');
    }
    var given = arguments.length > 2 && arguments[2] !== null && arguments[2] !== undefined
      ? arguments[2] : {};
    var record = { family: Str(family), status: 'unloaded', resolve: null, loaded: null };
    for (var i = 0; i < DESCRIPTORS.length; i++) {
      var value = given[DESCRIPTORS[i][0]];
      record[DESCRIPTORS[i][0]] = value === undefined ? DESCRIPTORS[i][1] : Str(value);
    }
    record.loaded = new PromiseCtor(function (resolve) { record.resolve = resolve; });
    faces.set(this, record);
    // Binary data is the font itself, so there is nothing left to load.
    if (typeof source !== 'string') markLoaded(this, record);
  }
  getter(FontFace.prototype, 'family',
    function () { return state(faces, this).family; },
    function (value) { state(faces, this).family = Str(value); });
  DESCRIPTORS.forEach(function (descriptor) {
    var name = descriptor[0];
    getter(FontFace.prototype, name,
      function () { return state(faces, this)[name]; },
      function (value) { state(faces, this)[name] = Str(value); });
  });
  getter(FontFace.prototype, 'status', function () { return state(faces, this).status; });
  getter(FontFace.prototype, 'loaded', function () { return state(faces, this).loaded; });
  method(FontFace.prototype, 'load', function load() {
    var record = state(faces, this);
    markLoaded(this, record);
    return record.loaded;
  });
  interfaceObject('FontFace', FontFace);

  // ---- FontFaceSet --------------------------------------------------------

  // The families a `font` shorthand names: everything after the size, split
  // at commas, without quotes and in lower case.
  function familiesOf(font) {
    var match = /(?:^|\s)[\d.]+(?:[a-z%]+)(?:\s*\/\s*[^\s,]+)?\s+(.+)$/i.exec(Str(font));
    if (match === null) return [];
    return match[1].split(',').map(function (family) {
      return family.trim().replace(/^(['"])(.*)\1$/, '$2').toLowerCase();
    }).filter(function (family) { return family !== ''; });
  }

  class FontFaceSet extends Target {
    constructor(token) {
      if (token !== internal) throw new TypeErr('Illegal constructor');
      super();
      var self = this;
      sets.set(this, {
        members: new Set(),
        ready: PromiseCtor.resolve(self),
        handlers: { loading: null, loadingdone: null, loadingerror: null }
      });
    }
    get size() { return state(sets, this).members.size; }
    get status() { state(sets, this); return 'loaded'; }
    get ready() { return state(sets, this).ready; }
    add(face) {
      var record = state(sets, this);
      if (!faces.has(face)) {
        throw new TypeErr("Failed to execute 'add' on 'FontFaceSet': parameter 1 is not of type 'FontFace'.");
      }
      record.members.add(face);
      return this;
    }
    delete(face) { return state(sets, this).members.delete(face); }
    clear() { state(sets, this).members.clear(); }
    has(face) { return state(sets, this).members.has(face); }
    forEach(callback) {
      var record = state(sets, this), self = this, thisArg = arguments[1];
      if (typeof callback !== 'function') {
        throw new TypeErr("Failed to execute 'forEach' on 'FontFaceSet': parameter 1 is not of type 'Function'.");
      }
      List.from(record.members).forEach(function (face) {
        callback.call(thisArg, face, face, self);
      });
    }
    entries() { return state(sets, this).members.entries(); }
    values() { return state(sets, this).members.values(); }
    keys() { return state(sets, this).members.values(); }
    [Symbol.iterator]() { return state(sets, this).members.values(); }
    check(font) {
      state(sets, this);
      if (arguments.length < 1) {
        throw new TypeErr("Failed to execute 'check' on 'FontFaceSet': 1 argument required, but only 0 present.");
      }
      return true;
    }
    load(font) {
      var record;
      try {
        record = state(sets, this);
        if (arguments.length < 1) {
          throw new TypeErr("Failed to execute 'load' on 'FontFaceSet': 1 argument required, but only 0 present.");
        }
      } catch (error) {
        return PromiseCtor.reject(error);
      }
      var wanted = familiesOf(font), matched = [];
      record.members.forEach(function (face) {
        var faceRecord = faces.get(face);
        var name = faceRecord.family.trim().replace(/^(['"])(.*)\1$/, '$2').toLowerCase();
        if (wanted.indexOf(name) !== -1) {
          markLoaded(face, faceRecord);
          matched.push(face);
        }
      });
      return PromiseCtor.resolve(matched);
    }
  }
  ['loading', 'loadingdone', 'loadingerror'].forEach(function (type) {
    getter(FontFaceSet.prototype, 'on' + type,
      function () { return state(sets, this).handlers[type]; },
      function (value) {
        state(sets, this).handlers[type] =
          typeof value === 'function' || (typeof value === 'object' && value !== null) ? value : null;
      });
  });
  ['size', 'status', 'ready'].forEach(function (name) {
    var descriptor = Object.getOwnPropertyDescriptor(FontFaceSet.prototype, name);
    descriptor.enumerable = true;
    define(FontFaceSet.prototype, name, descriptor);
  });
  ['add', 'delete', 'clear', 'has', 'forEach', 'entries', 'values', 'keys', 'check', 'load']
    .forEach(function (name) {
      var descriptor = Object.getOwnPropertyDescriptor(FontFaceSet.prototype, name);
      descriptor.enumerable = true;
      define(FontFaceSet.prototype, name, descriptor);
    });
  interfaceObject('FontFaceSet', FontFaceSet);

  getter(Doc.prototype, 'fonts', function () {
    if (!(this instanceof Doc)) throw new TypeErr('Illegal invocation');
    var set = byDocument.get(this);
    if (set === undefined) {
      set = new FontFaceSet(internal);
      byDocument.set(this, set);
    }
    return set;
  });
})();
"#;
