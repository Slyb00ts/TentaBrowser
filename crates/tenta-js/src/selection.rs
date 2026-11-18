//! `Selection`, `window.getSelection()` and `document.getSelection()`
//! (Selection API, <https://w3c.github.io/selection-api/>), as a script the
//! engine builds the first time a page asks for one of them.
//!
//! # Why a script
//!
//! A selection is at most one `Range` and a direction. Every algorithm of the
//! standard is a few boundary-point comparisons and a new `Range`, and the
//! engine's `Range` already does both and already follows the tree when it is
//! mutated. Nobody selects anything in a page an agent drives, so the state
//! is what script puts there: a click handler asking whether text is selected,
//! a copy button selecting a node, an editor saving and restoring its caret.
//!
//! # What is here
//!
//!  - `Selection`: `anchorNode`, `anchorOffset`, `focusNode`, `focusOffset`,
//!    `isCollapsed`, `rangeCount`, `type`, `direction`, `getRangeAt()`,
//!    `addRange()`, `removeRange()`, `removeAllRanges()`, `empty()`,
//!    `collapse()`, `setPosition()`, `collapseToStart()`, `collapseToEnd()`,
//!    `extend()`, `setBaseAndExtent()`, `selectAllChildren()`,
//!    `containsNode()`, `getComposedRanges()` and the stringifier.
//!  - One `Selection` per document with a window; `getSelection()` on a
//!    document without one answers `null`.
//!  - `selectionchange` on the document, queued once however many changes one
//!    task makes.
//!
//! # What is not, and what differs
//!
//!  - **No `modify()` and no `deleteFromDocument()`.** The first needs layout
//!    to know what a word or a line is; the second needs
//!    `Range.deleteContents()`, which this engine does not have. Both are
//!    absent rather than present and failing.
//!  - **The stringifier is the range's.** The standard asks for the text as
//!    rendered; this is the text of the nodes, so hidden text is included.
//!  - **Nothing a host does selects.** A click, a key or `select()` on a text
//!    control does not move the selection, and no `selectstart` is fired.
//!  - **A range changed directly** (`getRangeAt(0).setStart(…)`) is the
//!    selection's range, as the standard says, but queues no `selectionchange`.
//!
//! # How it is built
//!
//! The source evaluates to a function of one argument, `queueTask`, the
//! host's way to run a function as a task; it returns the exports. The script
//! must not read `Selection` or `getSelection` from the global, which are
//! the lazy names that run it.

/// The script. Evaluates to `function (queueTask)` returning
/// `{ Selection, windowSelection(), documentSelection(document) }`.
pub const SELECTION_PRELUDE_SOURCE: &str = r#"
(function (queueTask) {
  'use strict';
  const pageDocument = document;
  const state = new WeakMap();
  const selections = new WeakMap();
  let constructing = false;

  function fail(name, message) {
    return new DOMException(message, name);
  }

  function slots(selection) {
    const s = state.get(selection);
    if (s === undefined) throw new TypeError('Illegal invocation');
    return s;
  }

  function requireArguments(args, count, method) {
    if (args.length < count) {
      throw new TypeError("Failed to execute '" + method + "' on 'Selection': " + count +
        ' argument' + (count === 1 ? '' : 's') + ' required, but only ' + args.length + ' present.');
    }
  }

  function requireNode(value, method) {
    if (!(value instanceof Node)) {
      throw new TypeError("Failed to execute '" + method +
        "' on 'Selection': parameter 1 is not of type 'Node'.");
    }
  }

  // DOM's "length" of a node.
  function lengthOf(node) {
    switch (node.nodeType) {
      case 10: return 0;
      case 3: case 4: case 7: case 8: return node.data.length;
      default: return node.childNodes.length;
    }
  }

  function inDocument(s, node) {
    return node.getRootNode() === s.document;
  }

  function rangeOf(s, startNode, startOffset, endNode, endOffset) {
    const range = s.document.createRange();
    range.setStart(startNode, startOffset);
    range.setEnd(endNode, endOffset);
    return range;
  }

  // Where (node, offset) is relative to (fromNode, fromOffset): -1 before,
  // 0 the same point, 1 after.
  function position(s, fromNode, fromOffset, node, offset) {
    const point = s.document.createRange();
    point.setStart(fromNode, fromOffset);
    point.collapse(true);
    return point.comparePoint(node, offset);
  }

  function changed(s) {
    if (s.scheduled) return;
    s.scheduled = true;
    queueTask(function () {
      s.scheduled = false;
      s.document.dispatchEvent(new Event('selectionchange'));
    });
  }

  function set(s, range, backwards) {
    s.range = range;
    s.backwards = backwards;
    changed(s);
  }

  function clear(s) {
    if (s.range !== null) set(s, null, false);
  }

  // The range from the anchor to the focus, whichever comes first.
  function span(s, anchorNode, anchorOffset, focusNode, focusOffset) {
    if (position(s, anchorNode, anchorOffset, focusNode, focusOffset) < 0) {
      set(s, rangeOf(s, focusNode, focusOffset, anchorNode, anchorOffset), true);
    } else {
      set(s, rangeOf(s, anchorNode, anchorOffset, focusNode, focusOffset), false);
    }
  }

  class Selection {
    constructor() {
      if (!constructing) throw new TypeError('Illegal constructor');
    }

    get anchorNode() {
      const s = slots(this);
      if (s.range === null) return null;
      return s.backwards ? s.range.endContainer : s.range.startContainer;
    }

    get anchorOffset() {
      const s = slots(this);
      if (s.range === null) return 0;
      return s.backwards ? s.range.endOffset : s.range.startOffset;
    }

    get focusNode() {
      const s = slots(this);
      if (s.range === null) return null;
      return s.backwards ? s.range.startContainer : s.range.endContainer;
    }

    get focusOffset() {
      const s = slots(this);
      if (s.range === null) return 0;
      return s.backwards ? s.range.startOffset : s.range.endOffset;
    }

    get isCollapsed() {
      const s = slots(this);
      return s.range === null || s.range.collapsed;
    }

    get rangeCount() {
      return slots(this).range === null ? 0 : 1;
    }

    get type() {
      const s = slots(this);
      if (s.range === null) return 'None';
      return s.range.collapsed ? 'Caret' : 'Range';
    }

    get direction() {
      const s = slots(this);
      if (s.range === null || s.range.collapsed) return 'none';
      return s.backwards ? 'backward' : 'forward';
    }

    getRangeAt(index) {
      const s = slots(this);
      requireArguments(arguments, 1, 'getRangeAt');
      if ((index >>> 0) !== 0 || s.range === null) {
        throw fail('IndexSizeError',
          "Failed to execute 'getRangeAt' on 'Selection': " + (index >>> 0) + ' is not a valid index.');
      }
      return s.range;
    }

    addRange(range) {
      const s = slots(this);
      requireArguments(arguments, 1, 'addRange');
      if (!(range instanceof Range)) {
        throw new TypeError("Failed to execute 'addRange' on 'Selection': parameter 1 is not of type 'Range'.");
      }
      if (!inDocument(s, range.startContainer) || s.range !== null) return;
      set(s, range, false);
    }

    removeRange(range) {
      const s = slots(this);
      requireArguments(arguments, 1, 'removeRange');
      if (!(range instanceof Range)) {
        throw new TypeError("Failed to execute 'removeRange' on 'Selection': parameter 1 is not of type 'Range'.");
      }
      if (s.range !== range) {
        throw fail('NotFoundError',
          "Failed to execute 'removeRange' on 'Selection': the range is not the selection's.");
      }
      set(s, null, false);
    }

    removeAllRanges() {
      clear(slots(this));
    }

    empty() {
      clear(slots(this));
    }

    collapse(node, offset) {
      const s = slots(this);
      requireArguments(arguments, 1, 'collapse');
      if (node === null || node === undefined) {
        clear(s);
        return;
      }
      requireNode(node, 'collapse');
      offset >>>= 0;
      if (node.nodeType === 10) {
        throw fail('InvalidNodeTypeError',
          "Failed to execute 'collapse' on 'Selection': the node is a doctype.");
      }
      if (offset > lengthOf(node)) {
        throw fail('IndexSizeError',
          "Failed to execute 'collapse' on 'Selection': the offset " + offset + ' is larger than the node\'s length.');
      }
      if (!inDocument(s, node)) return;
      set(s, rangeOf(s, node, offset, node, offset), false);
    }

    setPosition(node, offset) {
      requireArguments(arguments, 1, 'setPosition');
      this.collapse(node, offset);
    }

    collapseToStart() {
      const s = slots(this);
      if (s.range === null) {
        throw fail('InvalidStateError',
          "Failed to execute 'collapseToStart' on 'Selection': there is no selection.");
      }
      const range = s.range;
      set(s, rangeOf(s, range.startContainer, range.startOffset, range.startContainer, range.startOffset), false);
    }

    collapseToEnd() {
      const s = slots(this);
      if (s.range === null) {
        throw fail('InvalidStateError',
          "Failed to execute 'collapseToEnd' on 'Selection': there is no selection.");
      }
      const range = s.range;
      set(s, rangeOf(s, range.endContainer, range.endOffset, range.endContainer, range.endOffset), false);
    }

    extend(node, offset) {
      const s = slots(this);
      requireArguments(arguments, 1, 'extend');
      requireNode(node, 'extend');
      offset >>>= 0;
      if (!inDocument(s, node)) return;
      if (s.range === null) {
        throw fail('InvalidStateError',
          "Failed to execute 'extend' on 'Selection': there is no selection.");
      }
      span(s, this.anchorNode, this.anchorOffset, node, offset);
    }

    setBaseAndExtent(anchorNode, anchorOffset, focusNode, focusOffset) {
      const s = slots(this);
      requireArguments(arguments, 4, 'setBaseAndExtent');
      requireNode(anchorNode, 'setBaseAndExtent');
      if (!(focusNode instanceof Node)) {
        throw new TypeError("Failed to execute 'setBaseAndExtent' on 'Selection': parameter 3 is not of type 'Node'.");
      }
      anchorOffset >>>= 0;
      focusOffset >>>= 0;
      if (anchorOffset > lengthOf(anchorNode) || focusOffset > lengthOf(focusNode)) {
        throw fail('IndexSizeError',
          "Failed to execute 'setBaseAndExtent' on 'Selection': an offset is larger than its node's length.");
      }
      if (!inDocument(s, anchorNode) || !inDocument(s, focusNode)) return;
      span(s, anchorNode, anchorOffset, focusNode, focusOffset);
    }

    selectAllChildren(node) {
      const s = slots(this);
      requireArguments(arguments, 1, 'selectAllChildren');
      requireNode(node, 'selectAllChildren');
      if (node.nodeType === 10) {
        throw fail('InvalidNodeTypeError',
          "Failed to execute 'selectAllChildren' on 'Selection': the node is a doctype.");
      }
      if (!inDocument(s, node)) return;
      set(s, rangeOf(s, node, 0, node, node.childNodes.length), false);
    }

    containsNode(node, allowPartialContainment) {
      const s = slots(this);
      requireArguments(arguments, 1, 'containsNode');
      requireNode(node, 'containsNode');
      if (s.range === null || !inDocument(s, node)) return false;
      if (allowPartialContainment) return s.range.intersectsNode(node);
      return s.range.comparePoint(node, 0) === 0 &&
        s.range.comparePoint(node, lengthOf(node)) === 0;
    }

    getComposedRanges() {
      const range = slots(this).range;
      if (range === null) return [];
      return [new StaticRange({
        startContainer: range.startContainer,
        startOffset: range.startOffset,
        endContainer: range.endContainer,
        endOffset: range.endOffset,
      })];
    }

    toString() {
      const range = slots(this).range;
      return range === null ? '' : range.toString();
    }
  }

  // WebIDL members are enumerable; class members are not.
  for (const name of Object.getOwnPropertyNames(Selection.prototype)) {
    if (name === 'constructor') continue;
    const descriptor = Object.getOwnPropertyDescriptor(Selection.prototype, name);
    descriptor.enumerable = true;
    Object.defineProperty(Selection.prototype, name, descriptor);
  }
  Object.defineProperty(Selection.prototype, Symbol.toStringTag, {
    value: 'Selection', writable: false, enumerable: false, configurable: true,
  });

  function selectionOf(document) {
    let selection = selections.get(document);
    if (selection === undefined) {
      constructing = true;
      try {
        selection = new Selection();
      } finally {
        constructing = false;
      }
      state.set(selection, { document: document, range: null, backwards: false, scheduled: false });
      selections.set(document, selection);
    }
    return selection;
  }

  return {
    Selection: Selection,
    windowSelection: function () {
      return selectionOf(pageDocument);
    },
    // A document that is not the window's has no browsing context, and a
    // document without one has no selection.
    documentSelection: function (document) {
      return document === pageDocument ? selectionOf(pageDocument) : null;
    },
  };
})
"#;
