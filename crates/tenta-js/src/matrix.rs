//! `DOMMatrix`, `DOMMatrixReadOnly`, `DOMPoint`, `DOMPointReadOnly` and
//! `DOMQuad` (Geometry Interfaces Module Level 1,
//! <https://drafts.fxtf.org/geometry/>), as a script the engine builds the
//! first time a page asks for one of them.
//!
//! # Why a script
//!
//! The interfaces are sixteen numbers and arithmetic on them. Nothing in
//! them touches the document, the network or the host, so there is no engine
//! state to keep in Rust, and most pages never ask for a matrix.
//!
//! # What is here
//!
//!  - `DOMMatrixReadOnly` and `DOMMatrix`: the constructor from nothing, from
//!    six or sixteen numbers and from a transform list; `fromMatrix()`,
//!    `fromFloat32Array()`, `fromFloat64Array()`; `a`-`f` and `m11`-`m44`,
//!    `is2D`, `isIdentity`; `translate()`, `scale()`, `scaleNonUniform()`,
//!    `scale3d()`, `rotate()`, `rotateFromVector()`, `rotateAxisAngle()`,
//!    `skewX()`, `skewY()`, `multiply()`, `flipX()`, `flipY()`, `inverse()`,
//!    `transformPoint()`, `toFloat32Array()`, `toFloat64Array()`, `toJSON()`
//!    and the stringifier; on `DOMMatrix` the setters, the `…Self()`
//!    methods, `preMultiplySelf()`, `invertSelf()` and `setMatrixValue()`.
//!  - `DOMPointReadOnly` and `DOMPoint`: `x`, `y`, `z`, `w`, `fromPoint()`,
//!    `matrixTransform()`, `toJSON()`.
//!  - `DOMQuad`: `p1`-`p4`, `fromRect()`, `fromQuad()`, `getBounds()`,
//!    `toJSON()`.
//!  - The standard's legacy aliases: `WebKitCSSMatrix` and `SVGMatrix` are
//!    `DOMMatrix`, `SVGPoint` is `DOMPoint`.
//!
//! # What is not, and what differs
//!
//!  - **A transform list is parsed here, not by the CSS parser.** The
//!    functions are `matrix()`, `matrix3d()`, `translate()`, `translateX()`,
//!    `translateY()`, `translateZ()`, `translate3d()`, `scale()`, `scaleX()`,
//!    `scaleY()`, `scaleZ()`, `scale3d()`, `rotate()`, `rotateX()`,
//!    `rotateY()`, `rotateZ()`, `rotate3d()`, `skew()`, `skewX()`, `skewY()`
//!    and `perspective()`, with numbers, absolute lengths and angles. A
//!    relative length or a percentage is a `SyntaxError`, as the standard
//!    has it; so is `calc()` and a percentage in `scale()`, which a browser
//!    takes.
//!  - **Not serializable.** `structuredClone()` and `postMessage()` do not
//!    carry a matrix, a point or a quad.
//!  - **Nothing of the engine's answers with one.** No `getCTM()`, no
//!    `getScreenCTM()`, no `DOMQuad` from `getBoxQuads()`.
//!  - **The string constructor is not limited to a window**, there being no
//!    worker to limit it in.
//!
//! # How it is built
//!
//! The source evaluates to a function of no arguments that returns the
//! exports. It reads `DOMRect`, which the generated bindings define, and must
//! not read the names it defines from the global, which are the lazy names
//! that run it.
//!
//! A matrix is kept as the standard writes it out: sixteen numbers in the
//! order `m11, m12, m13, m14, m21, … m44`, the order of `toFloat64Array()`.

/// The script. Evaluates to `function ()` returning the interface objects
/// under their names, the aliases included.
pub const MATRIX_PRELUDE_SOURCE: &str = r#"
(function () {
  'use strict';
  const matrices = new WeakMap();
  const points = new WeakMap();
  const quads = new WeakMap();
  const define = Object.defineProperty;
  const own = Object.prototype.hasOwnProperty;
  const Rect = globalThis.DOMRect;
  const Float32 = Float32Array, Float64 = Float64Array;
  const INTERNAL = {};
  const RADIANS = Math.PI / 180;

  const NAMES = ['m11', 'm12', 'm13', 'm14', 'm21', 'm22', 'm23', 'm24',
                 'm31', 'm32', 'm33', 'm34', 'm41', 'm42', 'm43', 'm44'];
  const ALIASES = [['a', 0], ['b', 1], ['c', 4], ['d', 5], ['e', 12], ['f', 13]];
  // What a 2D matrix has in the elements a 2D matrix does not use.
  const FLAT = [];
  FLAT[2] = 0; FLAT[3] = 0; FLAT[6] = 0; FLAT[7] = 0; FLAT[8] = 0; FLAT[9] = 0;
  FLAT[10] = 1; FLAT[11] = 0; FLAT[14] = 0; FLAT[15] = 1;

  // WebIDL's `unrestricted double`.
  function double(value) { return +value; }

  function identity() { return [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]; }

  // a x b.
  function product(a, b) {
    const out = new Array(16);
    for (let column = 0; column < 4; column++) {
      for (let row = 0; row < 4; row++) {
        let sum = 0;
        for (let k = 0; k < 4; k++) sum += a[k * 4 + row] * b[column * 4 + k];
        out[column * 4 + row] = sum;
      }
    }
    return out;
  }

  function post(s, other) { s.m = product(s.m, other); }

  function doTranslate(s, tx, ty, tz) {
    const t = identity();
    t[12] = tx; t[13] = ty; t[14] = tz;
    post(s, t);
    if (tz !== 0) s.is2D = false;
  }

  function doScale(s, sx, sy, sz, ox, oy, oz) {
    doTranslate(s, ox, oy, oz);
    const t = identity();
    t[0] = sx; t[5] = sy; t[10] = sz;
    post(s, t);
    doTranslate(s, -ox, -oy, -oz);
    if (sz !== 1 || oz !== 0) s.is2D = false;
  }

  // The rotation by `angle` radians about (x, y, z). A vector that cannot be
  // normalized rotates nothing.
  function rotation(x, y, z, angle) {
    const t = identity();
    const length = Math.hypot(x, y, z);
    if (length === 0) return t;
    x /= length; y /= length; z /= length;
    const c = Math.cos(angle), s = Math.sin(angle), k = 1 - c;
    t[0] = c + k * x * x; t[1] = k * x * y + s * z; t[2] = k * x * z - s * y;
    t[4] = k * x * y - s * z; t[5] = c + k * y * y; t[6] = k * y * z + s * x;
    t[8] = k * x * z + s * y; t[9] = k * y * z - s * x; t[10] = c + k * z * z;
    return t;
  }

  function doRotate(s, rx, ry, rz) {
    rx = double(rx);
    if (ry === undefined && rz === undefined) {
      rz = rx; rx = 0; ry = 0;
    } else {
      ry = ry === undefined ? 0 : double(ry);
      rz = rz === undefined ? 0 : double(rz);
    }
    if (rx !== 0 || ry !== 0) s.is2D = false;
    if (rz !== 0) post(s, rotation(0, 0, 1, rz * RADIANS));
    if (ry !== 0) post(s, rotation(0, 1, 0, ry * RADIANS));
    if (rx !== 0) post(s, rotation(1, 0, 0, rx * RADIANS));
  }

  function doRotateFromVector(s, x, y) {
    post(s, rotation(0, 0, 1, x === 0 && y === 0 ? 0 : Math.atan2(y, x)));
  }

  function doRotateAxisAngle(s, x, y, z, angle) {
    post(s, rotation(x, y, z, angle * RADIANS));
    if (x !== 0 || y !== 0) s.is2D = false;
  }

  function doSkew(s, ax, ay) {
    const t = identity();
    t[4] = Math.tan(ax); t[1] = Math.tan(ay);
    post(s, t);
  }

  function doInvert(s) {
    const m = s.m, inv = new Array(16);
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] +
             m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] -
             m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] +
             m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] -
              m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] -
             m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] +
             m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] -
             m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] +
              m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] +
             m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] -
             m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] +
              m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] -
              m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] -
             m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] +
             m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] -
              m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] +
              m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
    const determinant = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if (determinant === 0 || !isFinite(determinant)) {
      s.m = new Array(16).fill(NaN);
      s.is2D = false;
      return;
    }
    for (let i = 0; i < 16; i++) inv[i] /= determinant;
    s.m = inv;
  }

  // (x, y, z, w) through the matrix `m`.
  function transformed(m, p) {
    return [
      m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12] * p[3],
      m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13] * p[3],
      m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14] * p[3],
      m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15] * p[3],
    ];
  }

  // ---- a transform list -------------------------------------------------

  const NUMBER = /^[+-]?(?:\d*\.\d+|\d+)(?:e[+-]?\d+)?$/i;
  const DIMENSION = /^([+-]?(?:\d*\.\d+|\d+)(?:e[+-]?\d+)?)([a-z]*)$/i;
  const LENGTHS = { px: 1, cm: 96 / 2.54, mm: 96 / 25.4, q: 96 / 101.6, in: 96, pt: 96 / 72, pc: 16 };
  const ANGLES = { deg: RADIANS, grad: Math.PI / 200, rad: 1, turn: 2 * Math.PI };

  function syntax(text) {
    return new DOMException("Failed to parse '" + text + "'.", 'SyntaxError');
  }

  function number(text, whole) {
    if (!NUMBER.test(text)) throw syntax(whole);
    return +text;
  }

  // A length in pixels or an angle in radians. A zero needs no unit.
  function dimension(text, units, whole) {
    const match = DIMENSION.exec(text);
    if (match === null) throw syntax(whole);
    const value = +match[1], unit = match[2].toLowerCase();
    if (unit === '') {
      if (value !== 0) throw syntax(whole);
      return 0;
    }
    if (!own.call(units, unit)) throw syntax(whole);
    return value * units[unit];
  }

  function applyFunction(s, name, args, whole) {
    const count = function (least, most) {
      if (args.length < least || args.length > most) throw syntax(whole);
    };
    const n = function (i) { return number(args[i], whole); };
    const length = function (i) { return dimension(args[i], LENGTHS, whole); };
    const angle = function (i) { return dimension(args[i], ANGLES, whole); };
    switch (name) {
      case 'matrix': {
        count(6, 6);
        const t = identity();
        t[0] = n(0); t[1] = n(1); t[4] = n(2); t[5] = n(3); t[12] = n(4); t[13] = n(5);
        post(s, t);
        break;
      }
      case 'matrix3d': {
        count(16, 16);
        const t = new Array(16);
        for (let i = 0; i < 16; i++) t[i] = n(i);
        post(s, t);
        s.is2D = false;
        break;
      }
      case 'translate':
        count(1, 2);
        doTranslate(s, length(0), args.length > 1 ? length(1) : 0, 0);
        break;
      case 'translatex': count(1, 1); doTranslate(s, length(0), 0, 0); break;
      case 'translatey': count(1, 1); doTranslate(s, 0, length(0), 0); break;
      case 'translatez': count(1, 1); doTranslate(s, 0, 0, length(0)); s.is2D = false; break;
      case 'translate3d':
        count(3, 3);
        doTranslate(s, length(0), length(1), length(2));
        s.is2D = false;
        break;
      case 'scale': {
        count(1, 2);
        const x = n(0);
        doScale(s, x, args.length > 1 ? n(1) : x, 1, 0, 0, 0);
        break;
      }
      case 'scalex': count(1, 1); doScale(s, n(0), 1, 1, 0, 0, 0); break;
      case 'scaley': count(1, 1); doScale(s, 1, n(0), 1, 0, 0, 0); break;
      case 'scalez': count(1, 1); doScale(s, 1, 1, n(0), 0, 0, 0); s.is2D = false; break;
      case 'scale3d': count(3, 3); doScale(s, n(0), n(1), n(2), 0, 0, 0); s.is2D = false; break;
      case 'rotate': count(1, 1); post(s, rotation(0, 0, 1, angle(0))); break;
      case 'rotatex': count(1, 1); post(s, rotation(1, 0, 0, angle(0))); s.is2D = false; break;
      case 'rotatey': count(1, 1); post(s, rotation(0, 1, 0, angle(0))); s.is2D = false; break;
      case 'rotatez': count(1, 1); post(s, rotation(0, 0, 1, angle(0))); s.is2D = false; break;
      case 'rotate3d':
        count(4, 4);
        post(s, rotation(n(0), n(1), n(2), angle(3)));
        s.is2D = false;
        break;
      case 'skew': count(1, 2); doSkew(s, angle(0), args.length > 1 ? angle(1) : 0); break;
      case 'skewx': count(1, 1); doSkew(s, angle(0), 0); break;
      case 'skewy': count(1, 1); doSkew(s, 0, angle(0)); break;
      case 'perspective': {
        count(1, 1);
        if (args[0].toLowerCase() !== 'none') {
          let depth = length(0);
          if (depth < 0) throw syntax(whole);
          if (depth < 1) depth = 1;
          const t = identity();
          t[11] = -1 / depth;
          post(s, t);
        }
        s.is2D = false;
        break;
      }
      default: throw syntax(whole);
    }
  }

  function parse(text) {
    const s = { m: identity(), is2D: true };
    const list = text.trim();
    if (list === '' || list.toLowerCase() === 'none') return s;
    const one = /([a-z][a-z0-9]*)\(([^()]*)\)\s*/iy;
    let at = 0;
    while (at < list.length) {
      one.lastIndex = at;
      const match = one.exec(list);
      if (match === null) throw syntax(text);
      at = one.lastIndex;
      const args = match[2].split(',');
      for (let i = 0; i < args.length; i++) args[i] = args[i].trim();
      applyFunction(s, match[1].toLowerCase(), args, text);
    }
    return s;
  }

  // ---- dictionaries -----------------------------------------------------

  function dictionary(value, what) {
    if (value === undefined || value === null) return {};
    if (typeof value !== 'object' && typeof value !== 'function') {
      throw new TypeError('The provided value is not of type ' + what + '.');
    }
    return value;
  }

  function sameValueZero(a, b) { return a === b || (a !== a && b !== b); }

  // "Validate and fixup" a DOMMatrixInit: the matrix it describes.
  function fixup(init) {
    const known = matrices.get(init);
    if (known !== undefined) return { m: known.m.slice(), is2D: known.is2D };
    init = dictionary(init, "'DOMMatrixInit'");
    const m = identity();
    for (let i = 0; i < ALIASES.length; i++) {
      const index = ALIASES[i][1];
      let short = init[ALIASES[i][0]], long = init[NAMES[index]];
      if (short !== undefined) short = double(short);
      if (long !== undefined) long = double(long);
      if (short !== undefined && long !== undefined && !sameValueZero(short, long)) {
        throw new TypeError("The '" + ALIASES[i][0] + "' and '" + NAMES[index] +
          "' members of a DOMMatrixInit disagree.");
      }
      if (long !== undefined) m[index] = long;
      else if (short !== undefined) m[index] = short;
    }
    let flat = true;
    for (let index = 0; index < 16; index++) {
      if (FLAT[index] === undefined) continue;
      let value = init[NAMES[index]];
      if (value === undefined) continue;
      value = double(value);
      m[index] = value;
      if (value !== FLAT[index]) flat = false;
    }
    let is2D = init.is2D;
    if (is2D === undefined) {
      is2D = flat;
    } else {
      is2D = !!is2D;
      if (is2D && !flat) {
        throw new TypeError("A DOMMatrixInit with 'is2D' true has a 3D member.");
      }
    }
    return { m: m, is2D: is2D };
  }

  function pointInit(value) {
    const known = points.get(value);
    if (known !== undefined) return known.slice();
    value = dictionary(value, "'DOMPointInit'");
    const x = value.x, y = value.y, z = value.z, w = value.w;
    return [x === undefined ? 0 : double(x), y === undefined ? 0 : double(y),
            z === undefined ? 0 : double(z), w === undefined ? 1 : double(w)];
  }

  function fromNumbers(numbers) {
    if (numbers.length === 6) {
      const m = identity();
      m[0] = numbers[0]; m[1] = numbers[1]; m[4] = numbers[2]; m[5] = numbers[3];
      m[12] = numbers[4]; m[13] = numbers[5];
      return { m: m, is2D: true };
    }
    if (numbers.length === 16) {
      const m = new Array(16);
      for (let i = 0; i < 16; i++) m[i] = numbers[i];
      return { m: m, is2D: false };
    }
    throw new TypeError('A matrix is made of 6 or 16 numbers, not ' + numbers.length + '.');
  }

  // ---- DOMMatrixReadOnly, DOMMatrix ---------------------------------------

  function matrixOf(value) {
    const s = matrices.get(value);
    if (s === undefined) throw new TypeError('Illegal invocation');
    return s;
  }

  function copyOf(value) {
    const s = matrixOf(value);
    return { m: s.m.slice(), is2D: s.is2D };
  }

  function typed(array, Type, name) {
    if (!(array instanceof Type)) {
      throw new TypeError('The provided value is not of type ' + name + '.');
    }
    return fromNumbers(array);
  }

  class DOMMatrixReadOnly {
    constructor(init) {
      let s;
      if (init === INTERNAL) {
        s = arguments[1];
      } else if (init === undefined) {
        s = { m: identity(), is2D: true };
      } else if (init !== null && typeof init === 'object' &&
                 typeof init[Symbol.iterator] === 'function') {
        s = fromNumbers(Array.from(init, double));
      } else {
        s = parse('' + init);
      }
      matrices.set(this, s);
    }

    static fromMatrix(other) { return new DOMMatrixReadOnly(INTERNAL, fixup(other)); }
    static fromFloat32Array(array32) {
      return new DOMMatrixReadOnly(INTERNAL, typed(array32, Float32, "'Float32Array'"));
    }
    static fromFloat64Array(array64) {
      return new DOMMatrixReadOnly(INTERNAL, typed(array64, Float64, "'Float64Array'"));
    }

    get is2D() { return matrixOf(this).is2D; }
    get isIdentity() {
      const m = matrixOf(this).m;
      for (let i = 0; i < 16; i++) {
        if (m[i] !== (i % 5 === 0 ? 1 : 0)) return false;
      }
      return true;
    }

    translate(tx = 0, ty = 0, tz = 0) {
      const s = copyOf(this);
      doTranslate(s, double(tx), double(ty), double(tz));
      return new DOMMatrix(INTERNAL, s);
    }
    scale(scaleX = 1, scaleY, scaleZ = 1, originX = 0, originY = 0, originZ = 0) {
      const s = copyOf(this);
      scaleX = double(scaleX);
      doScale(s, scaleX, scaleY === undefined ? scaleX : double(scaleY), double(scaleZ),
              double(originX), double(originY), double(originZ));
      return new DOMMatrix(INTERNAL, s);
    }
    scaleNonUniform(scaleX = 1, scaleY = 1) {
      const s = copyOf(this);
      doScale(s, double(scaleX), double(scaleY), 1, 0, 0, 0);
      return new DOMMatrix(INTERNAL, s);
    }
    scale3d(scale = 1, originX = 0, originY = 0, originZ = 0) {
      const s = copyOf(this);
      scale = double(scale);
      doScale(s, scale, scale, scale, double(originX), double(originY), double(originZ));
      return new DOMMatrix(INTERNAL, s);
    }
    rotate(rotX = 0, rotY, rotZ) {
      const s = copyOf(this);
      doRotate(s, rotX, rotY, rotZ);
      return new DOMMatrix(INTERNAL, s);
    }
    rotateFromVector(x = 0, y = 0) {
      const s = copyOf(this);
      doRotateFromVector(s, double(x), double(y));
      return new DOMMatrix(INTERNAL, s);
    }
    rotateAxisAngle(x = 0, y = 0, z = 0, angle = 0) {
      const s = copyOf(this);
      doRotateAxisAngle(s, double(x), double(y), double(z), double(angle));
      return new DOMMatrix(INTERNAL, s);
    }
    skewX(sx = 0) {
      const s = copyOf(this);
      doSkew(s, double(sx) * RADIANS, 0);
      return new DOMMatrix(INTERNAL, s);
    }
    skewY(sy = 0) {
      const s = copyOf(this);
      doSkew(s, 0, double(sy) * RADIANS);
      return new DOMMatrix(INTERNAL, s);
    }
    multiply(other) {
      const s = copyOf(this), by = fixup(other);
      post(s, by.m);
      if (!by.is2D) s.is2D = false;
      return new DOMMatrix(INTERNAL, s);
    }
    flipX() {
      const s = copyOf(this), t = identity();
      t[0] = -1;
      post(s, t);
      return new DOMMatrix(INTERNAL, s);
    }
    flipY() {
      const s = copyOf(this), t = identity();
      t[5] = -1;
      post(s, t);
      return new DOMMatrix(INTERNAL, s);
    }
    inverse() {
      const s = copyOf(this);
      doInvert(s);
      return new DOMMatrix(INTERNAL, s);
    }
    transformPoint(point) {
      const p = transformed(matrixOf(this).m, pointInit(point));
      return new DOMPoint(p[0], p[1], p[2], p[3]);
    }
    toFloat32Array() { return new Float32(matrixOf(this).m); }
    toFloat64Array() { return new Float64(matrixOf(this).m); }
    toString() {
      const s = matrixOf(this), m = s.m;
      for (let i = 0; i < 16; i++) {
        if (!isFinite(m[i])) {
          throw new DOMException('A matrix with a non-finite element has no string form.',
            'InvalidStateError');
        }
      }
      if (s.is2D) {
        return 'matrix(' + m[0] + ', ' + m[1] + ', ' + m[4] + ', ' + m[5] + ', ' +
          m[12] + ', ' + m[13] + ')';
      }
      return 'matrix3d(' + m.join(', ') + ')';
    }
    toJSON() {
      const s = matrixOf(this), m = s.m, json = {};
      for (let i = 0; i < ALIASES.length; i++) json[ALIASES[i][0]] = m[ALIASES[i][1]];
      for (let i = 0; i < 16; i++) json[NAMES[i]] = m[i];
      json.is2D = s.is2D;
      json.isIdentity = this.isIdentity;
      return json;
    }
  }

  class DOMMatrix extends DOMMatrixReadOnly {
    static fromMatrix(other) { return new DOMMatrix(INTERNAL, fixup(other)); }
    static fromFloat32Array(array32) {
      return new DOMMatrix(INTERNAL, typed(array32, Float32, "'Float32Array'"));
    }
    static fromFloat64Array(array64) {
      return new DOMMatrix(INTERNAL, typed(array64, Float64, "'Float64Array'"));
    }

    multiplySelf(other) {
      const s = matrixOf(this), by = fixup(other);
      post(s, by.m);
      if (!by.is2D) s.is2D = false;
      return this;
    }
    preMultiplySelf(other) {
      const s = matrixOf(this), by = fixup(other);
      s.m = product(by.m, s.m);
      if (!by.is2D) s.is2D = false;
      return this;
    }
    translateSelf(tx = 0, ty = 0, tz = 0) {
      doTranslate(matrixOf(this), double(tx), double(ty), double(tz));
      return this;
    }
    scaleSelf(scaleX = 1, scaleY, scaleZ = 1, originX = 0, originY = 0, originZ = 0) {
      const s = matrixOf(this);
      scaleX = double(scaleX);
      doScale(s, scaleX, scaleY === undefined ? scaleX : double(scaleY), double(scaleZ),
              double(originX), double(originY), double(originZ));
      return this;
    }
    scale3dSelf(scale = 1, originX = 0, originY = 0, originZ = 0) {
      const s = matrixOf(this);
      scale = double(scale);
      doScale(s, scale, scale, scale, double(originX), double(originY), double(originZ));
      return this;
    }
    rotateSelf(rotX = 0, rotY, rotZ) {
      doRotate(matrixOf(this), rotX, rotY, rotZ);
      return this;
    }
    rotateFromVectorSelf(x = 0, y = 0) {
      doRotateFromVector(matrixOf(this), double(x), double(y));
      return this;
    }
    rotateAxisAngleSelf(x = 0, y = 0, z = 0, angle = 0) {
      doRotateAxisAngle(matrixOf(this), double(x), double(y), double(z), double(angle));
      return this;
    }
    skewXSelf(sx = 0) {
      doSkew(matrixOf(this), double(sx) * RADIANS, 0);
      return this;
    }
    skewYSelf(sy = 0) {
      doSkew(matrixOf(this), 0, double(sy) * RADIANS);
      return this;
    }
    invertSelf() {
      doInvert(matrixOf(this));
      return this;
    }
    setMatrixValue(transformList) {
      const s = matrixOf(this);
      if (arguments.length < 1) {
        throw new TypeError("Failed to execute 'setMatrixValue' on 'DOMMatrix': 1 argument " +
          'required, but only 0 present.');
      }
      const parsed = parse('' + transformList);
      s.m = parsed.m;
      s.is2D = parsed.is2D;
      return this;
    }
  }

  function elementGetter(index) {
    return function () { return matrixOf(this).m[index]; };
  }
  function elementSetter(index) {
    return function (value) {
      const s = matrixOf(this);
      value = double(value);
      s.m[index] = value;
      if (FLAT[index] !== undefined && value !== FLAT[index]) s.is2D = false;
    };
  }
  function elementAccessors(name, index) {
    define(DOMMatrixReadOnly.prototype, name, {
      get: elementGetter(index), enumerable: true, configurable: true,
    });
    define(DOMMatrix.prototype, name, {
      get: elementGetter(index), set: elementSetter(index), enumerable: true, configurable: true,
    });
  }
  for (let i = 0; i < ALIASES.length; i++) elementAccessors(ALIASES[i][0], ALIASES[i][1]);
  for (let i = 0; i < 16; i++) elementAccessors(NAMES[i], i);

  // ---- DOMPointReadOnly, DOMPoint -----------------------------------------

  function pointOf(value) {
    const p = points.get(value);
    if (p === undefined) throw new TypeError('Illegal invocation');
    return p;
  }

  class DOMPointReadOnly {
    constructor(x = 0, y = 0, z = 0, w = 1) {
      points.set(this, [double(x), double(y), double(z), double(w)]);
    }
    static fromPoint(other) {
      const p = pointInit(other);
      return new DOMPointReadOnly(p[0], p[1], p[2], p[3]);
    }
    matrixTransform(matrix) {
      const p = transformed(fixup(matrix).m, pointOf(this));
      return new DOMPoint(p[0], p[1], p[2], p[3]);
    }
    toJSON() {
      const p = pointOf(this);
      return { x: p[0], y: p[1], z: p[2], w: p[3] };
    }
  }

  class DOMPoint extends DOMPointReadOnly {
    static fromPoint(other) {
      const p = pointInit(other);
      return new DOMPoint(p[0], p[1], p[2], p[3]);
    }
  }

  ['x', 'y', 'z', 'w'].forEach(function (name, index) {
    const get = function () { return pointOf(this)[index]; };
    define(DOMPointReadOnly.prototype, name, { get: get, enumerable: true, configurable: true });
    define(DOMPoint.prototype, name, {
      get: function () { return pointOf(this)[index]; },
      set: function (value) { pointOf(this)[index] = double(value); },
      enumerable: true, configurable: true,
    });
  });

  // ---- DOMQuad ------------------------------------------------------------

  function quadOf(value) {
    const q = quads.get(value);
    if (q === undefined) throw new TypeError('Illegal invocation');
    return q;
  }

  function pointFrom(init) {
    const p = pointInit(init);
    return new DOMPoint(p[0], p[1], p[2], p[3]);
  }

  class DOMQuad {
    constructor(p1, p2, p3, p4) {
      quads.set(this, [pointFrom(p1), pointFrom(p2), pointFrom(p3), pointFrom(p4)]);
    }
    static fromRect(other) {
      other = dictionary(other, "'DOMRectInit'");
      const x = other.x === undefined ? 0 : double(other.x);
      const y = other.y === undefined ? 0 : double(other.y);
      const width = other.width === undefined ? 0 : double(other.width);
      const height = other.height === undefined ? 0 : double(other.height);
      return new DOMQuad({ x: x, y: y }, { x: x + width, y: y },
        { x: x + width, y: y + height }, { x: x, y: y + height });
    }
    static fromQuad(other) {
      other = dictionary(other, "'DOMQuadInit'");
      return new DOMQuad(other.p1, other.p2, other.p3, other.p4);
    }
    get p1() { return quadOf(this)[0]; }
    get p2() { return quadOf(this)[1]; }
    get p3() { return quadOf(this)[2]; }
    get p4() { return quadOf(this)[3]; }
    getBounds() {
      const q = quadOf(this);
      const a = pointOf(q[0]), b = pointOf(q[1]), c = pointOf(q[2]), d = pointOf(q[3]);
      const left = Math.min(a[0], b[0], c[0], d[0]), top = Math.min(a[1], b[1], c[1], d[1]);
      const right = Math.max(a[0], b[0], c[0], d[0]), bottom = Math.max(a[1], b[1], c[1], d[1]);
      return new Rect(left, top, right - left, bottom - top);
    }
    toJSON() {
      const q = quadOf(this);
      return { p1: q[0], p2: q[1], p3: q[2], p4: q[3] };
    }
  }

  // WebIDL's members are enumerable, where a class's are not.
  function finish(Interface, name) {
    [Interface, Interface.prototype].forEach(function (target) {
      Object.getOwnPropertyNames(target).forEach(function (key) {
        if (key === 'constructor' || key === 'prototype' || key === 'length' || key === 'name') {
          return;
        }
        const descriptor = Object.getOwnPropertyDescriptor(target, key);
        if (!descriptor.enumerable) {
          descriptor.enumerable = true;
          define(target, key, descriptor);
        }
      });
    });
    define(Interface.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }
  finish(DOMMatrixReadOnly, 'DOMMatrixReadOnly');
  finish(DOMMatrix, 'DOMMatrix');
  finish(DOMPointReadOnly, 'DOMPointReadOnly');
  finish(DOMPoint, 'DOMPoint');
  finish(DOMQuad, 'DOMQuad');

  return {
    DOMMatrix: DOMMatrix,
    DOMMatrixReadOnly: DOMMatrixReadOnly,
    DOMPoint: DOMPoint,
    DOMPointReadOnly: DOMPointReadOnly,
    DOMQuad: DOMQuad,
    WebKitCSSMatrix: DOMMatrix,
    SVGMatrix: DOMMatrix,
    SVGPoint: DOMPoint,
  };
})
"#;
