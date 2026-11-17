//! `TreeWalker`, `NodeIterator` and `NodeFilter` (DOM §6): the traversal
//! interfaces, as a script the engine runs before any page script.
//!
//! They are a script and not a binding because every step of a walk may call
//! the page's filter, which can run arbitrary script and mutate the tree the
//! walk is in; the algorithms read the tree again after each call, through
//! `Node`'s own accessors taken before page script could replace them.
//!
//! What is left out: a `NodeIterator` is not adjusted when its reference node
//! is removed from the tree (the standard's pre-removal steps).

/// Defines `NodeFilter`, `TreeWalker`, `NodeIterator`,
/// `Document.prototype.createTreeWalker` and
/// `Document.prototype.createNodeIterator`. Run once when a page's engine is
/// built; does nothing in a realm without `Document` and `Node`.
pub const TRAVERSAL_PRELUDE_SOURCE: &str = r#"(function () {
  'use strict';
  var Doc = globalThis.Document, NodeCtor = globalThis.Node;
  if (typeof Doc !== 'function' || typeof NodeCtor !== 'function') return;
  var define = Object.defineProperty, apply = Reflect.apply, TypeErr = TypeError;
  var Exception = globalThis.DOMException, States = WeakMap;
  var mapGet = States.prototype.get, mapSet = States.prototype.set;
  function accessor(name) {
    var descriptor = Object.getOwnPropertyDescriptor(NodeCtor.prototype, name);
    return descriptor && descriptor.get;
  }
  var parentGet = accessor('parentNode'), firstGet = accessor('firstChild'),
      lastGet = accessor('lastChild'), nextGet = accessor('nextSibling'),
      previousGet = accessor('previousSibling'), typeGet = accessor('nodeType');
  if (!parentGet || !firstGet || !lastGet || !nextGet || !previousGet || !typeGet) return;
  function parentOf(node) { return apply(parentGet, node, []); }
  function firstOf(node) { return apply(firstGet, node, []); }
  function lastOf(node) { return apply(lastGet, node, []); }
  function nextOf(node) { return apply(nextGet, node, []); }
  function previousOf(node) { return apply(previousGet, node, []); }
  function isNode(value) {
    if (value === null || (typeof value !== 'object' && typeof value !== 'function')) return false;
    try { apply(typeGet, value, []); return true; } catch (error) { return false; }
  }

  var ACCEPT = 1, REJECT = 2, SKIP = 3;
  var walkers = new States(), iterators = new States();

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
    var found = apply(mapGet, map, [object]);
    if (found === undefined) throw new TypeErr('Illegal invocation');
    return found;
  }

  // DOM "filter a node within a walker or an iterator".
  function filterNode(traverser, node) {
    if (traverser.active) {
      throw new Exception('The traversal is already running its filter.', 'InvalidStateError');
    }
    if (!((traverser.whatToShow >>> (apply(typeGet, node, []) - 1)) & 1)) return SKIP;
    var filter = traverser.filter;
    if (filter === null) return ACCEPT;
    traverser.active = true;
    var result;
    try {
      if (typeof filter === 'function') {
        result = filter(node);
      } else {
        var accept = filter.acceptNode;
        if (typeof accept !== 'function') {
          throw new TypeErr("The filter has no callable 'acceptNode'.");
        }
        result = apply(accept, filter, [node]);
      }
    } finally {
      traverser.active = false;
    }
    return Number(result) & 0xffff;
  }

  function newTraverser(root, whatToShow, filter, what) {
    if (!isNode(root)) {
      throw new TypeErr("Failed to execute '" + what + "' on 'Document': parameter 1 is not of type 'Node'.");
    }
    if (filter === undefined) filter = null;
    if (filter !== null && typeof filter !== 'object' && typeof filter !== 'function') {
      throw new TypeErr("Failed to execute '" + what + "' on 'Document': parameter 3 is not an object.");
    }
    return {
      root: root,
      whatToShow: whatToShow === undefined ? 0xffffffff : whatToShow >>> 0,
      filter: filter,
      active: false,
    };
  }

  // ---- NodeFilter ----------------------------------------------------------

  function NodeFilter() { throw new TypeErr('Illegal constructor'); }
  var constants = {
    FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3,
    SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 0x1, SHOW_ATTRIBUTE: 0x2, SHOW_TEXT: 0x4,
    SHOW_CDATA_SECTION: 0x8, SHOW_ENTITY_REFERENCE: 0x10, SHOW_ENTITY: 0x20,
    SHOW_PROCESSING_INSTRUCTION: 0x40, SHOW_COMMENT: 0x80, SHOW_DOCUMENT: 0x100,
    SHOW_DOCUMENT_TYPE: 0x200, SHOW_DOCUMENT_FRAGMENT: 0x400, SHOW_NOTATION: 0x800,
  };
  for (var constant in constants) {
    define(NodeFilter, constant,
      { value: constants[constant], writable: false, enumerable: true, configurable: false });
  }
  define(globalThis, 'NodeFilter',
    { value: NodeFilter, writable: true, enumerable: false, configurable: true });

  // ---- TreeWalker ----------------------------------------------------------

  function TreeWalker() { throw new TypeErr('Illegal constructor'); }
  var walkerProto = TreeWalker.prototype;
  function walker(object) { return state(walkers, object); }

  getter(walkerProto, 'root', function () { return walker(this).root; });
  getter(walkerProto, 'whatToShow', function () { return walker(this).whatToShow; });
  getter(walkerProto, 'filter', function () { return walker(this).filter; });
  getter(walkerProto, 'currentNode',
    function () { return walker(this).current; },
    function (node) {
      var w = walker(this);
      if (!isNode(node)) {
        throw new TypeErr("Failed to set the 'currentNode' property on 'TreeWalker': the value is not of type 'Node'.");
      }
      w.current = node;
    });

  method(walkerProto, 'parentNode', function parentNode() {
    var w = walker(this), node = w.current;
    while (node !== null && node !== w.root) {
      node = parentOf(node);
      if (node !== null && filterNode(w, node) === ACCEPT) {
        w.current = node;
        return node;
      }
    }
    return null;
  });

  function traverseChildren(w, first) {
    var node = first ? firstOf(w.current) : lastOf(w.current);
    while (node !== null) {
      var result = filterNode(w, node);
      if (result === ACCEPT) {
        w.current = node;
        return node;
      }
      if (result === SKIP) {
        var child = first ? firstOf(node) : lastOf(node);
        if (child !== null) {
          node = child;
          continue;
        }
      }
      while (node !== null) {
        var sibling = first ? nextOf(node) : previousOf(node);
        if (sibling !== null) {
          node = sibling;
          break;
        }
        var parent = parentOf(node);
        if (parent === null || parent === w.root || parent === w.current) return null;
        node = parent;
      }
    }
    return null;
  }
  method(walkerProto, 'firstChild', function firstChild() {
    return traverseChildren(walker(this), true);
  });
  method(walkerProto, 'lastChild', function lastChild() {
    return traverseChildren(walker(this), false);
  });

  function traverseSiblings(w, next) {
    var node = w.current;
    if (node === w.root) return null;
    for (;;) {
      var sibling = next ? nextOf(node) : previousOf(node);
      while (sibling !== null) {
        node = sibling;
        var result = filterNode(w, node);
        if (result === ACCEPT) {
          w.current = node;
          return node;
        }
        sibling = next ? firstOf(node) : lastOf(node);
        if (result === REJECT || sibling === null) {
          sibling = next ? nextOf(node) : previousOf(node);
        }
      }
      node = parentOf(node);
      if (node === null || node === w.root) return null;
      if (filterNode(w, node) === ACCEPT) return null;
    }
  }
  method(walkerProto, 'previousSibling', function previousSibling() {
    return traverseSiblings(walker(this), false);
  });
  method(walkerProto, 'nextSibling', function nextSibling() {
    return traverseSiblings(walker(this), true);
  });

  method(walkerProto, 'previousNode', function previousNode() {
    var w = walker(this), node = w.current;
    while (node !== w.root) {
      var sibling = previousOf(node);
      while (sibling !== null) {
        node = sibling;
        var result = filterNode(w, node);
        var last;
        while (result !== REJECT && (last = lastOf(node)) !== null) {
          node = last;
          result = filterNode(w, node);
        }
        if (result === ACCEPT) {
          w.current = node;
          return node;
        }
        sibling = previousOf(node);
      }
      if (node === w.root) return null;
      var parent = parentOf(node);
      if (parent === null) return null;
      node = parent;
      if (filterNode(w, node) === ACCEPT) {
        w.current = node;
        return node;
      }
    }
    return null;
  });

  method(walkerProto, 'nextNode', function nextNode() {
    var w = walker(this), node = w.current, result = ACCEPT;
    for (;;) {
      var child;
      while (result !== REJECT && (child = firstOf(node)) !== null) {
        node = child;
        result = filterNode(w, node);
        if (result === ACCEPT) {
          w.current = node;
          return node;
        }
      }
      var sibling = null, temporary = node;
      while (temporary !== null) {
        if (temporary === w.root) return null;
        sibling = nextOf(temporary);
        if (sibling !== null) {
          node = sibling;
          break;
        }
        temporary = parentOf(temporary);
      }
      // The current node was outside the root and the walk left its tree.
      if (sibling === null) return null;
      result = filterNode(w, node);
      if (result === ACCEPT) {
        w.current = node;
        return node;
      }
    }
  });
  interfaceObject('TreeWalker', TreeWalker);

  // ---- NodeIterator --------------------------------------------------------

  function NodeIterator() { throw new TypeErr('Illegal constructor'); }
  var iteratorProto = NodeIterator.prototype;
  function iterator(object) { return state(iterators, object); }

  // The node after `node` in tree order, inside `root`.
  function following(node, root) {
    var child = firstOf(node);
    if (child !== null) return child;
    while (node !== null && node !== root) {
      var sibling = nextOf(node);
      if (sibling !== null) return sibling;
      node = parentOf(node);
    }
    return null;
  }
  // The node before `node` in tree order, inside `root`.
  function preceding(node, root) {
    if (node === root) return null;
    var sibling = previousOf(node);
    if (sibling === null) return parentOf(node);
    var last;
    while ((last = lastOf(sibling)) !== null) sibling = last;
    return sibling;
  }
  function traverse(it, next) {
    var node = it.reference, before = it.before;
    for (;;) {
      if (next) {
        if (!before) {
          node = following(node, it.root);
          if (node === null) return null;
        } else {
          before = false;
        }
      } else if (before) {
        node = preceding(node, it.root);
        if (node === null) return null;
      } else {
        before = true;
      }
      if (filterNode(it, node) === ACCEPT) break;
    }
    it.reference = node;
    it.before = before;
    return node;
  }

  getter(iteratorProto, 'root', function () { return iterator(this).root; });
  getter(iteratorProto, 'referenceNode', function () { return iterator(this).reference; });
  getter(iteratorProto, 'pointerBeforeReferenceNode', function () { return iterator(this).before; });
  getter(iteratorProto, 'whatToShow', function () { return iterator(this).whatToShow; });
  getter(iteratorProto, 'filter', function () { return iterator(this).filter; });
  method(iteratorProto, 'nextNode', function nextNode() { return traverse(iterator(this), true); });
  method(iteratorProto, 'previousNode', function previousNode() {
    return traverse(iterator(this), false);
  });
  method(iteratorProto, 'detach', function detach() { iterator(this); });
  interfaceObject('NodeIterator', NodeIterator);

  // ---- Document ------------------------------------------------------------

  // `whatToShow` and `filter` are optional, so each function's length is 1.
  method(Doc.prototype, 'createTreeWalker', function createTreeWalker(root) {
    if (arguments.length === 0) {
      throw new TypeErr("Failed to execute 'createTreeWalker' on 'Document': 1 argument required, but only 0 present.");
    }
    var w = newTraverser(root, arguments[1], arguments[2], 'createTreeWalker');
    w.current = root;
    var object = Object.create(walkerProto);
    apply(mapSet, walkers, [object, w]);
    return object;
  });
  method(Doc.prototype, 'createNodeIterator', function createNodeIterator(root) {
    if (arguments.length === 0) {
      throw new TypeErr("Failed to execute 'createNodeIterator' on 'Document': 1 argument required, but only 0 present.");
    }
    var it = newTraverser(root, arguments[1], arguments[2], 'createNodeIterator');
    it.reference = root;
    it.before = true;
    var object = Object.create(iteratorProto);
    apply(mapSet, iterators, [object, it]);
    return object;
  });
})();"#;
