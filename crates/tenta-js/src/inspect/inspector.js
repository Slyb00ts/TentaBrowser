// The realm-side half of the inspection surface (crate::inspect).
//
// One closure per realm, evaluated by the backend right after the DOM is
// installed and before any page script runs. The page is hostile territory from
// then on: it can replace any global, any prototype method, add `toJSON` or
// accessors to `Object.prototype`, install proxies with throwing or endless
// traps, and spoof `Symbol.toStringTag`. So this script
//
//   * captures every intrinsic it will ever use at install time, and never
//     looks one up on a global or a prototype afterwards (the audit list is the
//     block of `const`s below);
//   * recognises kinds of object with the engine's own brand checks (`brand`),
//     never with `instanceof`, `Symbol.hasInstance` or `Symbol.toStringTag`;
//   * never touches a proxy: it is reported as an opaque object, and a prototype
//     chain is not walked through one;
//   * reads page objects only through own data property descriptors, so no
//     getter or trap of the page runs while describing a value;
//   * builds everything it hands to the host from fresh objects and serialises
//     it itself, so `toJSON`, `Array.prototype` and `Object.prototype` of the
//     page cannot reach it. Objects it parses from the host are read with `own`.
//
// What it cannot avoid running is what the host asks for: the expression, the
// function, a getter while a value is copied by value (as `JSON.stringify`
// would), and `Error.prepareStackTrace` while a stack is formatted.
//
// Handles live in `table`, a null-prototype object keyed by "<realm>.<n>": the
// table is the strong reference that keeps an object alive for as long as the
// host holds its id, and releasing an id (or its group) is deleting the entry.
(function (post, listening, brand, realm, handleLimit) {
  "use strict";

  // ---- captured intrinsics (the complete list of what is used after install)
  const G = globalThis;
  const Str = String;
  const Num = Number;
  const Err = Error;
  const TypeErr = TypeError;
  const Prom = Promise;
  const DateCtor = Date;
  const RegExpCtor = RegExp;
  const BigIntCtor = BigInt;
  const parse = JSON.parse;
  const apply = Reflect.apply;
  const defineProperty = Object.defineProperty;
  const getOwnPropertyDescriptor = Object.getOwnPropertyDescriptor;
  const getOwnPropertyNames = Object.getOwnPropertyNames;
  const getPrototypeOf = Object.getPrototypeOf;
  const objectCreate = Object.create;
  const keysOf = Object.keys;
  const isFiniteNumber = Num.isFinite;
  const dateNow = DateCtor.now;
  const functionToString = Function.prototype.toString;
  const symbolToString = Symbol.prototype.toString;
  const toStringTag = Symbol.toStringTag;
  const dateToString = DateCtor.prototype.toString;
  const dateGetTime = DateCtor.prototype.getTime;
  const regExpToString = RegExpCtor.prototype.toString;
  const regExpSource = getOwnPropertyDescriptor(RegExpCtor.prototype, "source").get;
  const regExpFlags = getOwnPropertyDescriptor(RegExpCtor.prototype, "flags").get;
  const mapSize = getOwnPropertyDescriptor(Map.prototype, "size").get;
  const setSize = getOwnPropertyDescriptor(Set.prototype, "size").get;
  const typedArrayLength = getOwnPropertyDescriptor(
    getPrototypeOf(Int8Array.prototype),
    "length",
  ).get;
  const promiseThen = Prom.prototype.then;
  const strSlice = String.prototype.slice;
  const strIndexOf = String.prototype.indexOf;
  const strLastIndexOf = String.prototype.lastIndexOf;
  const strSplit = String.prototype.split;
  const strTrim = String.prototype.trim;
  const strCharCodeAt = String.prototype.charCodeAt;
  const NodeProto = typeof G.Node === "function" ? G.Node.prototype : null;
  const nodeGetter = (proto, name) => {
    const d = proto === null ? undefined : getOwnPropertyDescriptor(proto, name);
    return d === undefined ? null : d.get;
  };
  const ElementProto = typeof G.Element === "function" ? G.Element.prototype : null;
  const nodeType = nodeGetter(NodeProto, "nodeType");
  const nodeName = nodeGetter(NodeProto, "nodeName");
  const localName = nodeGetter(ElementProto, "localName");
  const elementId = nodeGetter(ElementProto, "id");
  const elementClassName = nodeGetter(ElementProto, "className");

  // V8 keeps `stack` behind one shared accessor on every error; QuickJS keeps it
  // as plain data.
  const stackGetter = getOwnPropertyDescriptor(new Err(), "stack").get;

  const IMPLICIT_LIMIT = 1000;
  const MAX_NODES = 200000;
  const MAX_DEPTH = 100;
  const MAX_STACK_FRAMES = 10;
  const CLIP = 100;
  const FAILURE_TAG = {};

  const table = objectCreate(null);
  const groups = objectCreate(null);
  const bound = objectCreate(null);
  const counters = objectCreate(null);
  const timers = objectCreate(null);
  const ring = objectCreate(null);
  let ringPos = 0;
  let live = 0;
  let seq = 0;

  // ---------------------------------------------------------------- plumbing

  // A data property made without consulting the page: the descriptor has no
  // prototype, so `get`/`set`/`value` cannot be inherited from `Object.prototype`.
  function define(target, key, value, enumerable) {
    const d = objectCreate(null);
    d.value = value;
    d.writable = true;
    d.enumerable = enumerable;
    d.configurable = true;
    defineProperty(target, key, d);
  }

  function add(array, value) {
    define(array, array.length, value, true);
  }

  // An own data property of a host-parsed or script-made object.
  function own(object, key) {
    const d = getOwnPropertyDescriptor(object, key);
    return d === undefined ? undefined : d.value;
  }

  function str(s, a, b, fn) {
    return apply(fn, s, [a, b]);
  }

  function isObjectLike(v) {
    return (typeof v === "object" && v !== null) || typeof v === "function";
  }

  function isProxy(v) {
    return isObjectLike(v) && brand(v) === "proxy";
  }

  function fail(code, message) {
    return { tag: FAILURE_TAG, code: code, message: message };
  }

  function isFailure(e) {
    return isObjectLike(e) && !isProxy(e) && own(e, "tag") === FAILURE_TAG;
  }

  // ---------------------------------------------------------- serialisation

  const HEX = "0123456789abcdef";

  function quote(s) {
    let out = '"';
    for (let i = 0; i < s.length; i++) {
      const c = apply(strCharCodeAt, s, [i]);
      if (c === 34) out += '\\"';
      else if (c === 92) out += "\\\\";
      else if (c < 32) {
        out += "\\u00" + str(HEX, c >> 4, (c >> 4) + 1, strSlice) + str(HEX, c & 15, (c & 15) + 1, strSlice);
      } else out += str(s, i, i + 1, strSlice);
    }
    return out + '"';
  }

  // JSON for the objects this script builds itself.
  function ser(x) {
    if (x === null || x === undefined) return "null";
    switch (typeof x) {
      case "string": return quote(x);
      case "number": return isFiniteNumber(x) ? Str(x) : "null";
      case "boolean": return x ? "true" : "false";
    }
    let out;
    if (brand(x) === "array") {
      out = "[";
      for (let i = 0; i < x.length; i++) out += (i > 0 ? "," : "") + ser(x[i]);
      return out + "]";
    }
    out = "{";
    const names = keysOf(x);
    for (let i = 0; i < names.length; i++) {
      out += (i > 0 ? "," : "") + quote(names[i]) + ":" + ser(x[names[i]]);
    }
    return out + "}";
  }

  // ---------------------------------------------------------------- describe

  // A data property found by walking the prototype chain without invoking any
  // accessor or trap.
  function dataProp(v, key) {
    let p = v;
    for (let depth = 0; p !== null && depth < 100; depth++) {
      if (isProxy(p)) return undefined;
      const d = getOwnPropertyDescriptor(p, key);
      if (d !== undefined) return d.value;
      p = getPrototypeOf(p);
    }
    return undefined;
  }

  // The `stack` of an error: its own data property, or the engine's own
  // accessor. A getter the page installed is not run.
  function readStack(e) {
    const d = getOwnPropertyDescriptor(e, "stack");
    if (d === undefined) return undefined;
    const getter = own(d, "get");
    if (getter !== undefined) {
      return stackGetter !== undefined && getter === stackGetter ? apply(stackGetter, e, []) : undefined;
    }
    return own(d, "value");
  }

  function classNameOf(v) {
    try {
      let p = v;
      for (let depth = 0; p !== null && depth < 100; depth++) {
        if (isProxy(p)) break;
        const c = own(p, "constructor");
        if (typeof c === "function" && !isProxy(c)) {
          const n = own(c, "name");
          if (typeof n === "string" && n !== "") return n;
        }
        p = getPrototypeOf(p);
      }
    } catch (e) {}
    return "Object";
  }

  function inheritsFromNode(v) {
    if (NodeProto === null) return false;
    let p = getPrototypeOf(v);
    for (let depth = 0; p !== null && depth < 100; depth++) {
      if (p === NodeProto) return true;
      if (isProxy(p)) return false;
      p = getPrototypeOf(p);
    }
    return false;
  }

  function subtypeOf(v) {
    if (typeof v === "function") return isProxy(v) ? "proxy" : null;
    const kind = brand(v);
    if (kind !== "") return kind;
    try {
      if (inheritsFromNode(v)) return "node";
    } catch (e) {}
    return null;
  }

  function plainString(v) {
    try {
      if (typeof v === "symbol") return apply(symbolToString, v, []);
      if (!isObjectLike(v)) return Str(v);
      if (isProxy(v)) return "Proxy";
      if (brand(v) === "error") return errorHead(v);
      return classNameOf(v);
    } catch (e) {
      return "[unprintable]";
    }
  }

  function clip(s) {
    return s.length > CLIP ? str(s, 0, CLIP, strSlice) + "..." : s;
  }

  // "Name: message" as `Error.prototype.toString` forms it, read without
  // running page code.
  function errorHead(v) {
    const name = dataProp(v, "name");
    const message = dataProp(v, "message");
    const n = typeof name === "string" ? name : "Error";
    const m = typeof message === "string" ? message : "";
    if (m === "") return n;
    return n === "" ? m : n + ": " + m;
  }

  function nodeDescription(v) {
    try {
      if (nodeType !== null && apply(nodeType, v, []) === 1 && localName !== null) {
        let s = Str(apply(localName, v, []));
        const id = elementId === null ? "" : apply(elementId, v, []);
        if (typeof id === "string" && id !== "") s += "#" + id;
        const cls = elementClassName === null ? "" : apply(elementClassName, v, []);
        if (typeof cls === "string") {
          const parts = apply(strSplit, apply(strTrim, cls, []), [" "]);
          for (let i = 0; i < parts.length; i++) if (parts[i] !== "") s += "." + parts[i];
        }
        return s;
      }
      return nodeName === null ? "Node" : Str(apply(nodeName, v, []));
    } catch (e) {
      return "Node";
    }
  }

  function sizeOf(v, getter) {
    try {
      return apply(getter, v, []);
    } catch (e) {
      return 0;
    }
  }

  function descriptionOf(v, type, subtype, className) {
    try {
      if (type === "symbol") return apply(symbolToString, v, []);
      if (subtype === "proxy") return "Proxy";
      if (type === "function") return apply(functionToString, v, []);
      switch (subtype) {
        case "array": return className + "(" + own(v, "length") + ")";
        case "typedarray": return className + "(" + apply(typedArrayLength, v, []) + ")";
        case "map": return className + "(" + sizeOf(v, mapSize) + ")";
        case "set": return className + "(" + sizeOf(v, setSize) + ")";
        case "regexp": return apply(regExpToString, v, []);
        case "date": return apply(dateToString, v, []);
        case "node": return nodeDescription(v);
        case "error": {
          const s = readStack(v);
          if (typeof s !== "string" || s === "") return errorHead(v);
          return stackOf(v, framesOf(s, 0, true));
        }
      }
    } catch (e) {}
    return className;
  }

  function primitivePreview(name, v) {
    const t = typeof v;
    if (v === null) return { name: name, kind: "object", subtype: "null", value: "null" };
    switch (t) {
      case "string": return { name: name, kind: t, value: clip(v) };
      case "bigint": return { name: name, kind: t, value: Str(v) + "n" };
      case "symbol": return { name: name, kind: t, value: plainString(v) };
      case "undefined": return { name: name, kind: t, value: "undefined" };
      case "number":
      case "boolean": return { name: name, kind: t, value: Str(v) };
    }
    const subtype = subtypeOf(v);
    const type = t === "function" ? "function" : "object";
    return {
      name: name,
      kind: type,
      subtype: subtype,
      value: clip(descriptionOf(v, type, subtype, classNameOf(v))),
    };
  }

  function previewOf(v, subtype, description) {
    const indexed = subtype === "array" || subtype === "typedarray";
    const limit = indexed ? 100 : 5;
    const props = [];
    let overflow = false;
    try {
      const names = keysOf(v);
      for (let i = 0; i < names.length; i++) {
        if (props.length >= limit) {
          overflow = true;
          break;
        }
        const d = getOwnPropertyDescriptor(v, names[i]);
        if (d === undefined) continue;
        const isAccessor = own(d, "get") !== undefined || own(d, "set") !== undefined;
        add(props, isAccessor ? { name: names[i], kind: "accessor" } : primitivePreview(names[i], own(d, "value")));
      }
    } catch (e) {}
    return { subtype: subtype, description: description, overflow: overflow, properties: props };
  }

  function register(v, group, implicit) {
    if (implicit) {
      // Make room first, so the bound never turns away an exception report.
      const old = ring[ringPos];
      if (old !== undefined) release(old);
    } else if (live >= handleLimit) {
      throw fail("handle_limit", "too many live remote objects (" + handleLimit + ")");
    }
    const id = realm + "." + ++seq;
    table[id] = { v: v, group: group };
    live++;
    if (group !== null) {
      let members = groups[group];
      if (members === undefined) members = groups[group] = objectCreate(null);
      members[id] = true;
    }
    if (implicit) {
      ring[ringPos] = id;
      ringPos = (ringPos + 1) % IMPLICIT_LIMIT;
    }
    return id;
  }

  function release(id) {
    const entry = table[id];
    if (entry === undefined) return false;
    delete table[id];
    live--;
    if (entry.group !== null) {
      const members = groups[entry.group];
      if (members !== undefined) delete members[id];
    }
    return true;
  }

  function releaseGroup(name) {
    const members = groups[name];
    if (members === undefined) return;
    const ids = keysOf(members);
    for (let i = 0; i < ids.length; i++) {
      if (table[ids[i]] !== undefined) {
        delete table[ids[i]];
        live--;
      }
    }
    delete groups[name];
  }

  function lookup(id) {
    if (typeof id !== "string") throw fail("invalid_handle", "object id must be a string");
    const dot = str(id, ".", undefined, strIndexOf);
    if (dot < 0 || str(id, 0, dot, strSlice) !== Str(realm)) {
      throw fail("stale_handle", "the object belongs to an execution context that no longer exists");
    }
    const entry = table[id];
    if (entry === undefined) throw fail("unknown_handle", "could not find an object with the given id");
    return entry.v;
  }

  function numberValue(v) {
    if (v !== v) return { t: "number", u: "NaN" };
    if (v === Infinity) return { t: "number", u: "Infinity" };
    if (v === -Infinity) return { t: "number", u: "-Infinity" };
    if (v === 0 && 1 / v < 0) return { t: "number", u: "-0" };
    return { t: "number", v: v };
  }

  function handleFor(v, o) {
    const type = typeof v === "function" ? "function" : typeof v === "symbol" ? "symbol" : "object";
    let subtype = null;
    let className = "Symbol";
    if (type !== "symbol") {
      subtype = subtypeOf(v);
      className = subtype === "proxy" ? "Object" : classNameOf(v);
    }
    const description = descriptionOf(v, type, subtype, className);
    const id = register(v, o.group, o.implicit === true);
    return {
      t: "handle",
      id: id,
      ty: type,
      st: subtype,
      cn: className,
      d: description,
      p: o.preview === true && type === "object" && subtype !== "proxy" ? previewOf(v, subtype, description) : null,
    };
  }

  function describeRemote(v, o) {
    switch (typeof v) {
      case "undefined": return { t: "undefined" };
      case "boolean": return { t: "bool", v: v };
      case "string": return { t: "string", v: v };
      case "number": return numberValue(v);
      case "bigint": return { t: "bigint", v: Str(v) };
    }
    if (v === null) return { t: "null" };
    return handleFor(v, o);
  }

  // ---------------------------------------------------------------- by value

  function byValue(root) {
    let nodes = 0;
    const path = [];

    function walk(v, depth) {
      if (++nodes > MAX_NODES) throw fail("not_serializable", "the value is too large to return by value");
      switch (typeof v) {
        case "function":
        case "symbol":
          return { t: "undefined" };
        case "object":
          break;
        default:
          return describeRemote(v, null);
      }
      if (v === null) return { t: "null" };
      const kind = brand(v);
      if (kind === "proxy") throw fail("not_serializable", "Object couldn't be returned by value");
      if (depth > MAX_DEPTH) throw fail("not_serializable", "Object reference chain is too long");
      for (let i = 0; i < path.length; i++) {
        if (path[i] === v) throw fail("not_serializable", "Object reference chain is too long");
      }
      if (kind === "date") {
        const time = apply(dateGetTime, v, []);
        return { t: "date", v: time !== time ? null : time };
      }
      if (kind === "regexp") {
        return { t: "regexp", s: apply(regExpSource, v, []), f: apply(regExpFlags, v, []) };
      }
      add(path, v);
      let out;
      if (kind === "array") {
        const items = [];
        const length = own(v, "length");
        for (let i = 0; i < length; i++) add(items, walk(v[i], depth + 1));
        out = { t: "array", v: items };
      } else {
        const entries = [];
        const names = keysOf(v);
        for (let i = 0; i < names.length; i++) {
          const member = v[names[i]];
          if (typeof member === "function" || typeof member === "symbol") continue;
          add(entries, [names[i], walk(member, depth + 1)]);
        }
        out = { t: "object", v: entries };
      }
      path.length = path.length - 1;
      return out;
    }

    if (typeof root === "function" || typeof root === "symbol") {
      throw fail("not_serializable", "Object couldn't be returned by value");
    }
    return walk(root, 0);
  }

  function fromTagged(d) {
    const t = own(d, "t");
    const v = own(d, "v");
    switch (t) {
      case "undefined": return undefined;
      case "null": return null;
      case "bool":
      case "string": return v;
      case "number": {
        const u = own(d, "u");
        if (u === undefined) return v;
        return u === "NaN" ? NaN : u === "Infinity" ? Infinity : u === "-Infinity" ? -Infinity : -0;
      }
      case "bigint": return BigIntCtor(v);
      case "array": {
        const out = [];
        for (let i = 0; i < v.length; i++) add(out, fromTagged(v[i]));
        return out;
      }
      case "object": {
        const out = {};
        for (let i = 0; i < v.length; i++) define(out, v[i][0], fromTagged(v[i][1]), true);
        return out;
      }
      case "date": return new DateCtor(v === null ? NaN : v);
      case "regexp": return new RegExpCtor(own(d, "s"), own(d, "f"));
      case "handle": return lookup(own(d, "id"));
    }
    throw fail("invalid_argument", "unknown argument encoding");
  }

  // ------------------------------------------------------------------ stacks

  // "    at fn (url:12:34)", "    at url:12:34" and V8's nested eval form all
  // end in "<url>:<line>:<column>" with an optional closing parenthesis.
  function frameOf(text) {
    let s = apply(strTrim, text, []);
    if (str(s, 0, 3, strSlice) !== "at ") return null;
    s = str(s, 3, undefined, strSlice);
    const open = str(s, " (", undefined, strIndexOf);
    const name = open >= 0 ? str(s, 0, open, strSlice) : "";
    if (s.length > 0 && str(s, s.length - 1, undefined, strSlice) === ")") s = str(s, 0, s.length - 1, strSlice);
    const c2 = str(s, ":", undefined, strLastIndexOf);
    const c1 = c2 > 0 ? str(s, ":", c2 - 1, strLastIndexOf) : -1;
    if (c1 < 0) return { fn: name, url: s, line: null, col: null, raw: text };
    const line = Num(str(s, c1 + 1, c2, strSlice));
    const col = Num(str(s, c2 + 1, undefined, strSlice));
    const head = str(s, 0, c1, strSlice);
    const space = str(head, " ", undefined, strLastIndexOf);
    let url = space >= 0 ? str(head, space + 1, undefined, strSlice) : head;
    if (str(url, 0, 1, strSlice) === "(") url = str(url, 1, undefined, strSlice);
    return {
      fn: name,
      url: url,
      line: isFiniteNumber(line) ? line - 1 : null,
      col: isFiniteNumber(col) ? col - 1 : null,
      raw: text,
    };
  }

  function framesOf(stack, skip, unlimited) {
    const frames = [];
    if (typeof stack !== "string") return frames;
    const lines = apply(strSplit, stack, ["\n"]);
    let seen = 0;
    for (let i = 0; i < lines.length && (unlimited || frames.length < MAX_STACK_FRAMES); i++) {
      const frame = frameOf(lines[i]);
      if (frame === null) continue;
      if (seen++ < skip) continue;
      add(frames, frame);
    }
    return frames;
  }

  function publicFrames(frames) {
    const out = [];
    for (let i = 0; i < frames.length && i < MAX_STACK_FRAMES; i++) {
      add(out, { fn: frames[i].fn, url: frames[i].url, line: frames[i].line, col: frames[i].col });
    }
    return out;
  }

  // An engine's `stack` may or may not begin with the error's own "Name:
  // message" line (V8 does, QuickJS lists frames only); the description and the
  // stack the host sees always do.
  function stackOf(v, frames) {
    let text = errorHead(v);
    for (let i = 0; i < frames.length; i++) text += "\n" + frames[i].raw;
    return text;
  }

  function currentStack(skip) {
    try {
      return publicFrames(framesOf(new Err().stack, skip + 1));
    } catch (e) {
      return [];
    }
  }

  // --------------------------------------------------------------- exceptions

  function exceptionDetails(e, o, hint) {
    let stack = null;
    let frames = [];
    try {
      if (isObjectLike(e) && !isProxy(e)) {
        const s = readStack(e);
        if (typeof s === "string") {
          const all = framesOf(s, 0, true);
          stack = stackOf(e, all);
          frames = publicFrames(all);
        }
      }
    } catch (x) {}
    let line = hint.line;
    let col = hint.col;
    if ((line === null || line === undefined) && frames.length > 0) {
      line = frames[0].line;
      col = frames[0].col;
    }
    return {
      text: hint.text,
      message: plainString(e),
      stack: stack,
      frames: frames,
      line: line === undefined ? null : line,
      col: col === undefined ? null : col,
      ex: describeRemote(e, { group: o.group, implicit: o.implicit, preview: true }),
    };
  }

  // ------------------------------------------------------------- properties

  function hasOwn(object, key) {
    return getOwnPropertyDescriptor(object, key) !== undefined;
  }

  // The members of the object `id` names, read through own property
  // descriptors so no getter, setter or trap of the page runs. The handles it
  // makes join the group of the object they were read from, as in Chromium.
  function properties(a) {
    const id = own(a, "id");
    const root = lookup(id);
    const o = {
      group: table[id].group,
      implicit: false,
      preview: own(a, "preview") === true,
      value: false,
    };
    const ownOnly = own(a, "own") === true;
    const accessorsOnly = own(a, "accessors") === true;
    const out = [];
    if (!isObjectLike(root) || isProxy(root)) return out;
    let holder = root;
    for (let depth = 0; holder !== null && depth < 100; depth++) {
      if (isProxy(holder)) break;
      const names = getOwnPropertyNames(holder);
      for (let i = 0; i < names.length; i++) {
        const d = getOwnPropertyDescriptor(holder, names[i]);
        if (d === undefined) continue;
        const entry = {
          n: names[i],
          own: depth === 0,
          c: own(d, "configurable") === true,
          e: own(d, "enumerable") === true,
        };
        if (hasOwn(d, "get") || hasOwn(d, "set")) {
          const getter = own(d, "get");
          const setter = own(d, "set");
          if (getter !== undefined) entry.g = describeRemote(getter, o);
          if (setter !== undefined) entry.s = describeRemote(setter, o);
        } else {
          if (accessorsOnly) continue;
          entry.w = own(d, "writable") === true;
          entry.v = describeRemote(own(d, "value"), o);
        }
        add(out, entry);
      }
      if (depth === 0 && !accessorsOnly) {
        const proto = getPrototypeOf(holder);
        if (proto !== null) {
          add(out, { n: "__proto__", own: true, c: true, e: false, w: true, v: describeRemote(proto, o) });
        }
      }
      if (ownOnly) break;
      holder = getPrototypeOf(holder);
    }
    return out;
  }

  // ----------------------------------------------------------- finish a result

  function settings(o) {
    const group = own(o, "group");
    return {
      value: own(o, "value") === true,
      preview: own(o, "preview") === true,
      group: typeof group === "string" ? group : null,
      implicit: false,
      wait: own(o, "wait") === true,
      ticket: own(o, "ticket"),
    };
  }

  function outcome(v, o) {
    return { r: o.value ? byValue(v) : describeRemote(v, o) };
  }

  function failure(e) {
    if (isFailure(e)) return { fail: { code: e.code, message: e.message } };
    return { fail: { code: "internal", message: plainString(e) } };
  }

  function thrown(e, o) {
    try {
      return { x: exceptionDetails(e, o, { text: "Uncaught", line: null, col: null }) };
    } catch (f) {
      return failure(f);
    }
  }

  function settled(ticket, out) {
    post("settled", ser({ ticket: ticket, out: out }));
  }

  // Reacts to `promise` without trusting it: the `then` used is the captured
  // one, and if the promise's own species lookup throws, a promise of ours
  // adopts it instead.
  function react(promise, o) {
    const onValue = function (result) {
      let out;
      try {
        out = outcome(result, o);
      } catch (e) {
        out = failure(e);
      }
      settled(o.ticket, out);
    };
    const onReason = function (reason) {
      settled(o.ticket, thrown(reason, o));
    };
    try {
      apply(promiseThen, promise, [onValue, onReason]);
    } catch (e) {
      const adopted = new Prom(function (resolve) {
        resolve(promise);
      });
      apply(promiseThen, adopted, [onValue, onReason]);
    }
  }

  function finish(v, o) {
    try {
      if (o.wait && isObjectLike(v) && brand(v) === "promise") {
        react(v, o);
        return { pending: true };
      }
      return outcome(v, o);
    } catch (e) {
      return failure(e);
    }
  }

  // Arguments and receiver for a call the host makes natively: [failure json or
  // undefined, receiver, ...arguments].
  function prepare(a) {
    const out = [undefined];
    try {
      const self = own(a, "self");
      add(out, self === null ? G : lookup(self));
      const args = own(a, "args");
      for (let i = 0; i < args.length; i++) add(out, fromTagged(args[i]));
    } catch (e) {
      out[0] = ser(failure(e));
    }
    return out;
  }

  // ----------------------------------------------------------------- console

  function describeForEvent(v) {
    try {
      return describeRemote(v, { group: "console", implicit: true, preview: true });
    } catch (e) {
      return { t: "string", v: plainString(v) };
    }
  }

  function emitConsole(kind, args, skip) {
    if (!listening()) return;
    try {
      const described = [];
      for (let i = 0; i < args.length; i++) add(described, describeForEvent(args[i]));
      post("console", ser({ k: kind, a: described, s: currentStack(skip + 1), ts: dateNow() }));
    } catch (e) {}
  }

  function labelOf(args) {
    return args.length > 0 && args[0] !== undefined ? plainString(args[0]) : "default";
  }

  const consoleObject = {};
  function method(name, kind) {
    define(
      consoleObject,
      name,
      {
        [name]: function () {
          emitConsole(kind, arguments, 1);
        },
      }[name],
      true,
    );
  }
  method("log", "log");
  method("debug", "debug");
  method("info", "info");
  method("error", "error");
  method("warn", "warning");
  method("dir", "dir");
  method("dirxml", "dirxml");
  method("table", "table");
  method("trace", "trace");
  method("clear", "clear");
  method("group", "startGroup");
  method("groupCollapsed", "startGroupCollapsed");
  method("groupEnd", "endGroup");
  define(
    consoleObject,
    "assert",
    function assert(condition) {
      if (condition) return;
      const rest = [];
      for (let i = 1; i < arguments.length; i++) add(rest, arguments[i]);
      emitConsole("assert", rest.length > 0 ? rest : ["Assertion failed"], 1);
    },
    true,
  );
  define(
    consoleObject,
    "count",
    function count() {
      const label = labelOf(arguments);
      counters[label] = (counters[label] || 0) + 1;
      emitConsole("count", [label + ": " + counters[label]], 1);
    },
    true,
  );
  define(
    consoleObject,
    "countReset",
    function countReset() {
      counters[labelOf(arguments)] = 0;
    },
    true,
  );
  define(
    consoleObject,
    "time",
    function time() {
      timers[labelOf(arguments)] = dateNow();
    },
    true,
  );
  function elapsed(kind, args) {
    const label = labelOf(args);
    const started = timers[label];
    if (started === undefined) return;
    emitConsole(kind, [label + ": " + (dateNow() - started) + "ms"], 2);
  }
  define(
    consoleObject,
    "timeLog",
    function timeLog() {
      elapsed("log", arguments);
    },
    true,
  );
  define(
    consoleObject,
    "timeEnd",
    function timeEnd() {
      elapsed("timeEnd", arguments);
      delete timers[labelOf(arguments)];
    },
    true,
  );
  define(consoleObject, toStringTag, "console", false);
  define(G, "console", consoleObject, false);

  // ---------------------------------------------------------------- bindings

  function bind(name) {
    if (bound[name] === true) return;
    const binding = {
      [name]: function () {
        if (arguments.length !== 1 || typeof arguments[0] !== "string") {
          throw new TypeErr("Invalid arguments: a binding takes one string");
        }
        post("binding", ser({ n: name, p: arguments[0] }));
      },
    }[name];
    define(G, name, binding, false);
    bound[name] = true;
  }

  function unbind(name) {
    if (bound[name] !== true) return;
    delete G[name];
    delete bound[name];
  }

  // ---------------------------------------------------------------- dispatch

  // `mode` tells "finish" how to read `extra`: a completion "value", a "thrown"
  // exception, or a "notfn" (the host evaluated something that is not a
  // function).
  return function dispatch(op, argument, extra, mode) {
    try {
      const a = argument === "" ? null : parse(argument);
      switch (op) {
        case "finish": {
          const o = settings(own(a, "o"));
          if (mode === "value") return ser(finish(extra, o));
          if (mode === "notfn") {
            return ser(thrown(new TypeErr("the function declaration did not evaluate to a function"), o));
          }
          return ser(thrown(extra, o));
        }
        case "prepare": return prepare(a);
        case "release": return ser({ r: release(own(a, "id")) });
        case "release_group":
          releaseGroup(own(a, "group"));
          return ser({ r: true });
        case "live": return ser({ r: live });
        case "realm": return ser({ r: realm });
        case "props": return ser({ r: properties(a) });
        case "bind":
          bind(own(a, "name"));
          return ser({ r: true });
        case "unbind":
          unbind(own(a, "name"));
          return ser({ r: true });
        case "exception":
          return ser({
            r: exceptionDetails(extra, { group: "exception", implicit: true }, {
              text: own(a, "text"),
              line: own(a, "line"),
              col: own(a, "col"),
            }),
          });
      }
      return ser({ fail: { code: "invalid_argument", message: "unknown operation " + op } });
    } catch (e) {
      return ser(failure(e));
    }
  };
})
