// Page-side half of the CDP Input and DOM domains.
//
// The engine has no focus model, no pointer or keyboard default actions and no
// IntersectionObserver yet (HT3, SC1 and the observer packages own those), but
// Puppeteer and Playwright need them before their first click or keystroke.
// This script supplies the smallest version of each. It is registered as an
// init script of every page the CDP server creates and is meant to disappear
// as the native features land; none of it exists for pages other hosts open.
//
// Events dispatched here are synthetic: `isTrusted` is false.
//
// Trust and detectability. This script runs inside the page's own trust
// domain: a page can read it, replace its globals or call `__tentaCdp`. It
// captures the intrinsics it relies on when it is installed, before any page
// script runs, so a casual page cannot subvert it, and that is all the
// protection it has. Its additions (`IntersectionObserver`, `MutationObserver`,
// focus and geometry members, `__tentaCdp`) are visible to a page that looks,
// so it is fingerprintable. It is an automation aid and not a stealth layer.
(function (global) {
  'use strict';
  if (global.__tentaCdp) return;

  const doc = global.document;
  const defineProperty = Object.defineProperty;
  const assign = Object.assign;
  const apply = Reflect.apply;
  const SafeMap = Map;
  const SafeSet = Set;
  const SafeWeakMap = WeakMap;
  let wakeObservers = () => {};
  const indexOf = Array.prototype.indexOf;
  const arraySlice = Array.prototype.slice;
  const clearTimeoutOf = global.clearTimeout;
  const setTimeoutOf = global.setTimeout;
  const getterOf = (proto, name) => {
    const descriptor = proto && Object.getOwnPropertyDescriptor(proto, name);
    return descriptor && descriptor.get;
  };
  const childNodesOf = getterOf(global.Node && global.Node.prototype, 'childNodes');
  const nodeTypeOf = getterOf(global.Node && global.Node.prototype, 'nodeType');
  const parentNodeOf = getterOf(global.Node && global.Node.prototype, 'parentNode');
  const elementProto0 = global.Element && global.Element.prototype;
  const getAttributeNamesOf = elementProto0 && elementProto0.getAttributeNames;
  const getAttributeOf = elementProto0 && elementProto0.getAttribute;
  const querySelectorAllOf = global.Document && global.Document.prototype.querySelectorAll;
  const childrenOf = (node) => (childNodesOf ? apply(childNodesOf, node, []) : node.childNodes);
  const has = (el, name) => el.hasAttribute(name);
  const attr = (el, name) => (el.getAttribute(name) || '').toLowerCase();

  // ---- focus ------------------------------------------------------------

  let active = null;
  const selections = new SafeWeakMap();

  const TEXT_TYPES = ['', 'text', 'search', 'tel', 'url', 'email', 'password', 'number'];

  function isTextControl(el) {
    if (!el || el.nodeType !== 1) return false;
    if (el.localName === 'textarea') return true;
    return el.localName === 'input' && TEXT_TYPES.indexOf(attr(el, 'type')) !== -1;
  }

  function focusable(el) {
    if (!el || el.nodeType !== 1 || !el.isConnected) return false;
    if (has(el, 'tabindex')) return !isNaN(parseInt(el.getAttribute('tabindex'), 10));
    const tag = el.localName;
    if (tag === 'input') return !has(el, 'disabled') && attr(el, 'type') !== 'hidden';
    if (tag === 'textarea' || tag === 'select' || tag === 'button') return !has(el, 'disabled');
    if ((tag === 'a' || tag === 'area') && has(el, 'href')) return true;
    const editable = el.getAttribute('contenteditable');
    return editable !== null && editable.toLowerCase() !== 'false';
  }

  function current() {
    if (active && !active.isConnected) active = null;
    return active;
  }

  function fireFocus(type, target, related, bubbles) {
    target.dispatchEvent(
      new global.FocusEvent(type, { bubbles, composed: true, relatedTarget: related }),
    );
  }

  function focusElement(el) {
    if (!focusable(el)) return;
    const previous = current();
    if (previous === el) return;
    if (previous) {
      active = null;
      fireFocus('blur', previous, el, false);
      fireFocus('focusout', previous, el, true);
    }
    active = el;
    fireFocus('focus', el, previous, false);
    fireFocus('focusin', el, previous, true);
  }

  function blurElement(el) {
    if (current() !== el) return;
    active = null;
    fireFocus('blur', el, null, false);
    fireFocus('focusout', el, null, true);
  }

  const elementProto = global.HTMLElement && global.HTMLElement.prototype;
  if (elementProto) {
    if (typeof elementProto.focus !== 'function') {
      defineProperty(elementProto, 'focus', {
        value: function focus() {
          focusElement(this);
        },
        writable: true,
        configurable: true,
      });
    }
    if (typeof elementProto.blur !== 'function') {
      defineProperty(elementProto, 'blur', {
        value: function blur() {
          blurElement(this);
        },
        writable: true,
        configurable: true,
      });
    }
  }
  const documentProto = global.Document && global.Document.prototype;
  if (documentProto) {
    defineProperty(documentProto, 'activeElement', {
      get: function activeElement() {
        return current() || this.body || this.documentElement;
      },
      configurable: true,
    });
    if (typeof documentProto.hasFocus !== 'function') {
      defineProperty(documentProto, 'hasFocus', {
        value: function hasFocus() {
          return true;
        },
        writable: true,
        configurable: true,
      });
    }
  }

  // There is no scrolling: everything laid out is "in view".
  const baseProto = global.Element && global.Element.prototype;
  if (baseProto) {
    for (const name of ['scrollIntoView', 'scrollIntoViewIfNeeded']) {
      if (typeof baseProto[name] !== 'function') {
        defineProperty(baseProto, name, {
          value: function () {},
          writable: true,
          configurable: true,
        });
      }
    }
  }

  // ---- IntersectionObserver (geometry stub) ------------------------------

  if (typeof global.IntersectionObserver !== 'function') {
    const zeroRect = () => ({
      x: 0, y: 0, width: 0, height: 0, top: 0, right: 0, bottom: 0, left: 0,
    });
    class IntersectionObserver {
      constructor(callback, options) {
        if (typeof callback !== 'function') {
          throw new TypeError('IntersectionObserver callback must be a function');
        }
        const init = options || {};
        defineProperty(this, '_callback', { value: callback });
        defineProperty(this, '_targets', { value: new Set() });
        this.root = init.root || null;
        this.rootMargin = init.rootMargin || '0px';
        const t = init.threshold;
        this.thresholds = Array.isArray(t) ? t.slice() : [t === undefined ? 0 : t];
      }
      observe(target) {
        if (this._targets.has(target)) return;
        this._targets.add(target);
        // A laid-out element counts as fully visible; layout does not reach
        // script yet, so a connected element is the best answer available.
        setTimeoutOf(() => {
          if (!this._targets.has(target)) return;
          const visible = target.isConnected;
          this._callback.call(
            this,
            [{
              target,
              isIntersecting: visible,
              intersectionRatio: visible ? 1 : 0,
              boundingClientRect: zeroRect(),
              intersectionRect: zeroRect(),
              rootBounds: null,
              time: 0,
            }],
            this,
          );
        }, 0);
      }
      unobserve(target) {
        this._targets.delete(target);
      }
      disconnect() {
        this._targets.clear();
      }
      takeRecords() {
        return [];
      }
    }
    defineProperty(global, 'IntersectionObserver', {
      value: IntersectionObserver,
      writable: true,
      configurable: true,
    });
  }

  // ---- NodeFilter and TreeWalker (preorder walks) ----------------------------

  // Puppeteer's selector waits walk the trees mutation records name. Only the
  // forward walk is provided: `nextNode`, `currentNode`, `whatToShow` and a
  // filter callback.
  if (typeof global.NodeFilter === 'undefined') {
    const NodeFilter = {
      FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3,
      SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 0x1, SHOW_ATTRIBUTE: 0x2, SHOW_TEXT: 0x4,
      SHOW_CDATA_SECTION: 0x8, SHOW_PROCESSING_INSTRUCTION: 0x40, SHOW_COMMENT: 0x80,
      SHOW_DOCUMENT: 0x100, SHOW_DOCUMENT_TYPE: 0x200, SHOW_DOCUMENT_FRAGMENT: 0x400,
    };
    defineProperty(global, 'NodeFilter', { value: NodeFilter, writable: true, configurable: true });
  }
  if (documentProto && typeof documentProto.createTreeWalker !== 'function') {
    defineProperty(documentProto, 'createTreeWalker', {
      value: function createTreeWalker(root, whatToShow, filter) {
        const mask = whatToShow === undefined ? 0xffffffff : whatToShow >>> 0;
        const accepts = (node) => {
          if (!((mask >>> (apply(nodeTypeOf, node, []) - 1)) & 1)) return false;
          if (!filter) return true;
          const verdict = typeof filter === 'function' ? filter(node) : filter.acceptNode(node);
          return verdict === 1;
        };
        const walker = {
          root,
          whatToShow: mask,
          filter: filter || null,
          currentNode: root,
          nextNode() {
            let node = walker.currentNode;
            for (;;) {
              const kids = childrenOf(node);
              if (kids.length > 0) {
                node = kids[0];
              } else {
                while (node !== root && !node.nextSibling) node = apply(parentNodeOf, node, []);
                if (node === root) return null;
                node = node.nextSibling;
              }
              if (accepts(node)) {
                walker.currentNode = node;
                return node;
              }
            }
          },
        };
        return walker;
      },
      writable: true,
      configurable: true,
    });
  }

  // ---- MutationObserver (snapshot diff) ---------------------------------------

  // The engine has no MutationObserver. This stand-in compares a snapshot of
  // each observed tree on a backing-off timer and delivers `childList`,
  // `attributes` and `characterData` records for what changed since the last
  // look, only when something did. Records carry the node that changed;
  // sibling pointers and the order of changes within one interval are not
  // reproduced, and a change made and undone inside an interval is invisible.
  if (typeof global.MutationObserver !== 'function') {
    const observers = new SafeSet();
    // Every look costs one serialization of the observed tree, compared with
    // the last one as a string: no node wrappers are made unless the markup
    // changed (a wrapper costs far more than the comparison). Only then is the
    // tree walked, up to this many nodes into document order; changes beyond
    // are not reported. The delay between looks backs off from 50 ms to 1 s
    // while nothing changes and starts over at a change, at `observe` and at
    // any host command.
    const MAX_SNAPSHOT_NODES = 1000;
    const MIN_DELAY = 50;
    const MAX_DELAY = 1000;
    let timer = null;
    let delay = MIN_DELAY;

    // A node with more children than this is tracked by its child count
    // alone: listing them makes a wrapper per child, which is what is costly.
    // Its records name no added or removed nodes.
    const MAX_CHILDREN = 200;

    const childrenState = (node) => {
      const list = childrenOf(node);
      return list.length > MAX_CHILDREN
        ? { children: null, count: list.length }
        : { children: apply(arraySlice, list, []), count: list.length };
    };

    const stateOf = (node) => {
      const type = apply(nodeTypeOf, node, []);
      if (type === 1) {
        const attrs = new SafeMap();
        const names = apply(getAttributeNamesOf, node, []);
        for (let i = 0; i < names.length; i += 1) {
          attrs.set(names[i], apply(getAttributeOf, node, [names[i]]));
        }
        return assign(childrenState(node), { attrs, data: null });
      }
      if (type === 3 || type === 8 || type === 4) {
        return { children: [], count: 0, attrs: null, data: node.data };
      }
      return assign(childrenState(node), { attrs: null, data: null });
    };

    // Walks `root` (and its subtree when asked) into a node -> state map.
    const snapshot = (root, subtree) => {
      const states = new SafeMap();
      const pending = [root];
      while (pending.length > 0 && states.size < MAX_SNAPSHOT_NODES) {
        const node = pending.pop();
        const state = stateOf(node);
        states.set(node, state);
        if (subtree || node === root) {
          const kids = state.children || [];
          for (let i = 0; i < kids.length; i += 1) {
            if (subtree) pending.push(kids[i]);
          }
        }
      }
      return states;
    };

    const record = (type, target, extra) => assign({
      type,
      target,
      addedNodes: [],
      removedNodes: [],
      previousSibling: null,
      nextSibling: null,
      attributeName: null,
      attributeNamespace: null,
      oldValue: null,
    }, extra);

    const diff = (entry) => {
      const { options, before } = entry;
      const after = snapshot(entry.target, options.subtree);
      const records = [];
      after.forEach((now, node) => {
        const was = before.get(node);
        if (!was) return;
        if (options.childList) {
          if (now.children === null || was.children === null) {
            if (now.count !== was.count) records.push(record('childList', node, {}));
          } else {
            const same = now.children.length === was.children.length
              && now.children.every((child, i) => child === was.children[i]);
            if (!same) {
              const nowSet = new SafeSet(now.children);
              const wasSet = new SafeSet(was.children);
              const added = now.children.filter((child) => !wasSet.has(child));
              const removed = was.children.filter((child) => !nowSet.has(child));
              if (added.length > 0 || removed.length > 0) {
                records.push(record('childList', node, { addedNodes: added, removedNodes: removed }));
              }
            }
          }
        }
        if (options.attributes && now.attrs && was.attrs) {
          const names = new SafeSet();
          now.attrs.forEach((value, name) => names.add(name));
          was.attrs.forEach((value, name) => names.add(name));
          names.forEach((name) => {
            if (options.attributeFilter && apply(indexOf, options.attributeFilter, [name]) === -1) return;
            if (now.attrs.get(name) === was.attrs.get(name)) return;
            records.push(record('attributes', node, {
              attributeName: name,
              oldValue: options.attributeOldValue ? (was.attrs.get(name) === undefined ? null : was.attrs.get(name)) : null,
            }));
          });
        }
        if (options.characterData && now.data !== null && now.data !== was.data) {
          records.push(record('characterData', node, {
            oldValue: options.characterDataOldValue ? was.data : null,
          }));
        }
      });
      entry.before = after;
      return records;
    };

    const signature = (node) => {
      const type = apply(nodeTypeOf, node, []);
      if (type === 9) return node.documentElement ? node.documentElement.outerHTML : '';
      return type === 1 ? node.outerHTML : String(node.data);
    };

    const collect = (observer) => {
      const records = [];
      observer._entries.forEach((entry) => {
        const sig = signature(entry.target);
        if (sig === entry.sig) return;
        entry.sig = sig;
        const found = diff(entry);
        for (let i = 0; i < found.length; i += 1) records.push(found[i]);
      });
      return records;
    };

    const schedule = () => {
      if (timer !== null) clearTimeoutOf(timer);
      timer = setTimeoutOf(tick, delay);
    };

    function tick() {
      timer = null;
      let changed = false;
      observers.forEach((observer) => {
        const records = collect(observer);
        if (records.length > 0) {
          changed = true;
          observer._callback.call(observer, records, observer);
        }
      });
      delay = changed ? MIN_DELAY : Math.min(delay * 2, MAX_DELAY);
      if (observers.size > 0) schedule();
    }

    const stopIfIdle = () => {
      if (observers.size === 0 && timer !== null) {
        clearTimeoutOf(timer);
        timer = null;
      }
    };

    wakeObservers = () => {
      if (observers.size > 0 && delay !== MIN_DELAY) {
        delay = MIN_DELAY;
        schedule();
      }
    };

    class MutationObserver {
      constructor(callback) {
        if (typeof callback !== 'function') {
          throw new TypeError('MutationObserver callback must be a function');
        }
        defineProperty(this, '_callback', { value: callback });
        defineProperty(this, '_entries', { value: new SafeMap() });
      }
      observe(target, init) {
        const options = assign({}, init);
        if (options.attributes === undefined && (options.attributeOldValue || options.attributeFilter)) {
          options.attributes = true;
        }
        if (options.characterData === undefined && options.characterDataOldValue) {
          options.characterData = true;
        }
        if (!options.childList && !options.attributes && !options.characterData) {
          throw new TypeError(
            "Failed to execute 'observe' on 'MutationObserver': The options object must set at least one of 'attributes', 'characterData', or 'childList' to true.",
          );
        }
        this._entries.set(target, {
          target, options, sig: signature(target), before: snapshot(target, options.subtree),
        });
        observers.add(this);
        delay = MIN_DELAY;
        schedule();
      }
      disconnect() {
        this._entries.clear();
        observers.delete(this);
        stopIfIdle();
      }
      takeRecords() {
        return collect(this);
      }
    }
    defineProperty(global, 'MutationObserver', {
      value: MutationObserver,
      writable: true,
      configurable: true,
    });
  }

  // ---- text editing -----------------------------------------------------

  function selectionOf(el) {
    const length = String(el.value).length;
    let sel = selections.get(el);
    if (!sel) sel = { anchor: length, focus: length };
    return {
      anchor: Math.min(sel.anchor, length),
      focus: Math.min(sel.focus, length),
    };
  }

  function setSelection(el, anchor, focus) {
    selections.set(el, { anchor, focus });
  }

  function fireInput(el, inputType, data) {
    let event;
    if (typeof global.InputEvent === 'function') {
      event = new global.InputEvent('input', {
        bubbles: true, composed: true, inputType, data,
      });
    } else {
      event = new global.Event('input', { bubbles: true, composed: true });
      defineProperty(event, 'inputType', { value: inputType });
      defineProperty(event, 'data', { value: data });
    }
    el.dispatchEvent(event);
  }

  function editable(el) {
    return isTextControl(el) && !has(el, 'readonly') && !has(el, 'disabled');
  }

  function replaceRange(el, start, end, text, inputType) {
    const value = String(el.value);
    const next = value.slice(0, start) + text + value.slice(end);
    const max = parseInt(el.getAttribute('maxlength'), 10);
    if (!isNaN(max) && next.length > max && text.length > 0) return false;
    el.value = next;
    const caret = start + text.length;
    setSelection(el, caret, caret);
    fireInput(el, inputType, text.length > 0 ? text : null);
    return true;
  }

  function isContentEditable(el) {
    if (!el || el.nodeType !== 1 || isTextControl(el)) return false;
    const value = el.getAttribute('contenteditable');
    return value !== null && value.toLowerCase() !== 'false';
  }

  function insertText(text) {
    const el = current();
    if (isContentEditable(el) && text) {
      unsupported = 'contenteditable';
      return false;
    }
    if (!el || !editable(el) || !text) return false;
    const sel = selectionOf(el);
    return replaceRange(el, Math.min(sel.anchor, sel.focus), Math.max(sel.anchor, sel.focus),
      el.localName === 'textarea' ? text : text.replace(/[\r\n]+/g, ' '), 'insertText');
  }

  function deleteContent(el, forward) {
    const sel = selectionOf(el);
    let start = Math.min(sel.anchor, sel.focus);
    let end = Math.max(sel.anchor, sel.focus);
    const value = String(el.value);
    if (start === end) {
      if (forward) {
        if (end >= value.length) return;
        end += /[\ud800-\udbff]/.test(value[end]) ? 2 : 1;
      } else {
        if (start <= 0) return;
        start -= /[\udc00-\udfff]/.test(value[start - 1]) && start > 1 ? 2 : 1;
      }
    }
    replaceRange(el, start, end, '', forward ? 'deleteContentForward' : 'deleteContentBackward');
  }

  function moveCaret(el, key, extend) {
    const sel = selectionOf(el);
    const length = String(el.value).length;
    let focus = sel.focus;
    if (key === 'ArrowLeft') {
      focus = !extend && sel.anchor !== sel.focus ? Math.min(sel.anchor, sel.focus) : Math.max(0, focus - 1);
    } else if (key === 'ArrowRight') {
      focus = !extend && sel.anchor !== sel.focus ? Math.max(sel.anchor, sel.focus) : Math.min(length, focus + 1);
    } else if (key === 'Home' || key === 'ArrowUp') {
      focus = 0;
    } else {
      focus = length;
    }
    setSelection(el, extend ? sel.anchor : focus, focus);
  }

  // ---- mouse ------------------------------------------------------------

  let hovered = null;
  let pressed = null;

  function mouseInit(init, extra) {
    return assign({
      bubbles: true,
      cancelable: true,
      composed: true,
      view: global,
      detail: init.detail || 0,
      screenX: init.x,
      screenY: init.y,
      clientX: init.x,
      clientY: init.y,
      button: init.button || 0,
      buttons: init.buttons || 0,
      ctrlKey: !!init.ctrlKey,
      shiftKey: !!init.shiftKey,
      altKey: !!init.altKey,
      metaKey: !!init.metaKey,
    }, extra);
  }

  function fireMouse(target, type, init, extra) {
    return target.dispatchEvent(new global.MouseEvent(type, mouseInit(init, extra)));
  }

  function commonAncestor(a, b) {
    for (let node = a; node; node = node.parentNode) {
      if (node.contains(b)) return node;
    }
    return null;
  }

  function closestElement(node) {
    while (node && node.nodeType !== 1) node = node.parentNode;
    return node;
  }

  function labelControl(label) {
    const id = label.getAttribute('for');
    if (id !== null) return doc.getElementById(id);
    return label.querySelector('input, select, textarea, button');
  }

  // What a default action asks the host to do, and what it cannot do. The
  // exported `mouse`, `key` and `insertText` return them as
  // `{ navigate, unsupported }` and reset them.
  let navigation = null;
  let unsupported = null;

  // Only http(s) documents are navigated to: a refused navigation would
  // replace the current document with an error page, and `javascript:` or
  // `file:` targets are not this host's to follow.
  function absoluteUrl(reference) {
    try {
      const href = new global.URL(reference, doc.URL).href;
      return /^https?:/i.test(href) ? href : null;
    } catch (error) {
      return null;
    }
  }

  function sameDocument(url) {
    const hash = url.indexOf('#');
    const base = String(doc.URL);
    const baseHash = base.indexOf('#');
    return hash !== -1 && url.slice(0, hash) === (baseHash === -1 ? base : base.slice(0, baseHash));
  }

  const selfTargets = ['', '_self', '_top', '_parent'];

  // The application/x-www-form-urlencoded serializer: USVString conversion,
  // CRLF newlines, UTF-8 percent-encoding that leaves only `*-._` and
  // alphanumerics, space as `+`.
  function encodeComponent(text) {
    const wellFormed = String(text)
      .replace(/\r\n|\r|\n/g, '\r\n')
      .replace(/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g, '�');
    return encodeURIComponent(wellFormed)
      .replace(/[!'()~]/g, (c) => '%' + c.charCodeAt(0).toString(16).toUpperCase())
      .replace(/%20/g, '+');
  }

  // The controls of `form`: its descendants, minus those that name another
  // form, plus controls elsewhere that name this one with `form=`.
  function controlsOf(form) {
    const id = form.getAttribute('id');
    const out = [];
    const inside = form.querySelectorAll('input, textarea, select, button');
    for (let i = 0; i < inside.length; i += 1) {
      const owner = inside[i].getAttribute('form');
      if (owner === null || owner === id) out.push(inside[i]);
    }
    if (id) {
      const outside = doc.querySelectorAll('input[form], textarea[form], select[form], button[form]');
      for (let i = 0; i < outside.length; i += 1) {
        if (outside[i].getAttribute('form') === id && !form.contains(outside[i])) out.push(outside[i]);
      }
    }
    return out;
  }

  function selectedOptions(select) {
    const options = select.querySelectorAll('option');
    const chosen = [];
    for (let i = 0; i < options.length; i += 1) {
      const option = options[i];
      const selected = option.selected === undefined ? has(option, 'selected') : option.selected;
      if (selected && !has(option, 'disabled')) chosen.push(option);
    }
    if (chosen.length === 0 && !has(select, 'multiple') && options.length > 0 && !has(options[0], 'disabled')) {
      chosen.push(options[0]);
    }
    return chosen.map((option) => (has(option, 'value') ? option.getAttribute('value') : option.textContent));
  }

  // The URL of a GET submission of `form`: the action (or the submitter's
  // `formaction`) with the successful controls as its query. POST and
  // `dialog` forms have no navigation here, there is no constraint
  // validation, and the encoding is UTF-8 whatever `accept-charset` says.
  function formUrl(form, submitter) {
    const own = (name) => (submitter && has(submitter, name) ? submitter.getAttribute(name) : null);
    const method = (own('formmethod') || form.getAttribute('method') || '').toLowerCase();
    if (method === 'post' || method === 'dialog') return null;
    const target = (own('formtarget') || form.getAttribute('target') || '').toLowerCase();
    if (selfTargets.indexOf(target) === -1) return null;
    const action = absoluteUrl(own('formaction') || form.getAttribute('action') || '');
    if (!action) return null;
    const pairs = [];
    const controls = controlsOf(form);
    for (let i = 0; i < controls.length; i += 1) {
      const control = controls[i];
      const name = control.getAttribute('name');
      if (!name || has(control, 'disabled')) continue;
      const tag = control.localName;
      const kind = tag === 'input' ? attr(control, 'type') : tag === 'button' ? attr(control, 'type') || 'submit' : '';
      if (tag === 'button' || kind === 'submit' || kind === 'image') {
        if (control !== submitter) continue;
      } else if (kind === 'button' || kind === 'reset' || kind === 'file') {
        continue;
      } else if ((kind === 'checkbox' || kind === 'radio') && !control.checked) {
        continue;
      }
      let values;
      if (tag === 'select') {
        values = selectedOptions(control);
      } else if (kind === 'checkbox' || kind === 'radio') {
        values = [has(control, 'value') ? control.getAttribute('value') : 'on'];
      } else if (tag === 'button' || kind === 'submit') {
        values = [control.getAttribute('value') || ''];
      } else {
        values = [control.value === undefined ? control.getAttribute('value') || '' : String(control.value)];
      }
      for (const value of values) pairs.push(encodeComponent(name) + '=' + encodeComponent(value));
    }
    const hash = action.indexOf('#');
    const base = (hash === -1 ? action : action.slice(0, hash)).replace(/\?.*$/, '');
    return base + '?' + pairs.join('&') + (hash === -1 ? '' : action.slice(hash));
  }

  function submit(form, submitter) {
    if (!form) return;
    const Submit = typeof global.SubmitEvent === 'function' ? global.SubmitEvent : global.Event;
    const event = new Submit('submit', { bubbles: true, cancelable: true, submitter });
    if (form.dispatchEvent(event)) navigation = formUrl(form, submitter);
  }

  function formOf(el) {
    const id = el.getAttribute('form');
    return id ? doc.getElementById(id) : el.closest('form');
  }

  // The click event of a mouse press, with the activation behaviour browsers
  // give it: checkbox and radio state, labels, submit buttons and <summary>.
  function click(target, init, extra) {
    const stop = target.closest(
      'a[href], button, input, label, select, textarea, summary',
    );
    const kind = stop && stop.localName === 'input' ? attr(stop, 'type') : '';
    const toggles = stop && (kind === 'checkbox' || kind === 'radio') && !has(stop, 'disabled');
    let before = null;
    if (toggles) {
      before = stop.checked;
      stop.checked = kind === 'checkbox' ? !before : true;
    }
    const proceed = fireMouse(target, 'click', init, extra);
    if (!stop) return;
    if (!proceed) {
      if (toggles) stop.checked = before;
      return;
    }
    if (toggles) {
      if (kind === 'checkbox' || !before) {
        fireInputAndChange(stop);
      }
    } else if (stop.localName === 'label') {
      const control = labelControl(stop);
      if (control && !stop.contains(target.closest('input, select, textarea, button'))) {
        focusElement(control);
        click(control, init, { detail: 0 });
      }
    } else if (stop.localName === 'a') {
      const target = attr(stop, 'target');
      const opens = init.ctrlKey || init.metaKey || init.shiftKey || init.altKey;
      const url = absoluteUrl(stop.getAttribute('href'));
      if (url && !opens && selfTargets.indexOf(target) !== -1 && !has(stop, 'download')
          && !sameDocument(url)) {
        navigation = url;
      }
    } else if (stop.localName === 'summary') {
      const details = stop.parentElement;
      if (details && details.localName === 'details') details.toggleAttribute('open');
    } else if (
      (stop.localName === 'button' && attr(stop, 'type') !== 'button' && attr(stop, 'type') !== 'reset') ||
      (stop.localName === 'input' && kind === 'submit')
    ) {
      if (!has(stop, 'disabled')) submit(formOf(stop), stop);
    }
  }

  function fireInputAndChange(el) {
    el.dispatchEvent(new global.Event('input', { bubbles: true, composed: true }));
    el.dispatchEvent(new global.Event('change', { bubbles: true }));
  }

  function mouse(target, type, init) {
    if (!target || target.nodeType !== 1) return false;
    if (type === 'mouseMoved') {
      if (hovered !== target) {
        const previous = hovered && hovered.isConnected ? hovered : null;
        hovered = target;
        if (previous) fireMouse(previous, 'mouseout', init, { cancelable: true, relatedTarget: target });
        fireMouse(target, 'mouseover', init, { relatedTarget: previous });
      }
      fireMouse(target, 'mousemove', init, {});
      return true;
    }
    if (type === 'mousePressed') {
      pressed = { target, button: init.button || 0 };
      const proceed = fireMouse(target, 'mousedown', init, {});
      if (proceed && (init.button || 0) === 0) {
        let focusTarget = target;
        while (focusTarget && !focusable(focusTarget)) focusTarget = focusTarget.parentElement;
        if (focusTarget) focusElement(focusTarget);
        else if (current()) blurElement(current());
      }
      if ((init.button || 0) === 2) fireMouse(target, 'contextmenu', init, {});
      return proceed;
    }
    if (type === 'mouseReleased') {
      const down = pressed;
      pressed = null;
      fireMouse(target, 'mouseup', init, {});
      if (!down || down.button !== (init.button || 0)) return true;
      const clicked = down.target === target ? target : closestElement(commonAncestor(down.target, target));
      if (!clicked) return true;
      if ((init.button || 0) === 0) click(clicked, init, {});
      else if ((init.button || 0) === 1) fireMouse(clicked, 'auxclick', init, {});
      return true;
    }
    return false;
  }

  // ---- keyboard ---------------------------------------------------------

  function keyTarget() {
    return current() || doc.body || doc.documentElement;
  }

  function fireKey(target, type, init) {
    const event = new global.KeyboardEvent(type, {
      bubbles: true,
      cancelable: true,
      composed: true,
      view: global,
      key: init.key,
      code: init.code,
      location: init.location || 0,
      repeat: !!init.repeat,
      ctrlKey: !!init.ctrlKey,
      shiftKey: !!init.shiftKey,
      altKey: !!init.altKey,
      metaKey: !!init.metaKey,
    });
    if (init.keyCode) {
      // keyCode and charCode are legacy members scripts still read.
      defineProperty(event, 'keyCode', { value: init.keyCode });
      defineProperty(event, 'which', { value: init.keyCode });
    }
    return target.dispatchEvent(event);
  }

  function focusOrder() {
    const all = apply(querySelectorAllOf, doc, ['*']);
    const out = [];
    for (let i = 0; i < all.length; i += 1) {
      const el = all[i];
      if (!focusable(el)) continue;
      const index = el.getAttribute('tabindex');
      if (index !== null && parseInt(index, 10) < 0) continue;
      out.push(el);
    }
    return out;
  }

  function tab(backwards) {
    const order = focusOrder();
    if (order.length === 0) return;
    const index = order.indexOf(current());
    let next;
    if (index === -1) next = backwards ? order[order.length - 1] : order[0];
    else next = order[index + (backwards ? -1 : 1)];
    if (next) focusElement(next);
    else if (current()) blurElement(current());
  }

  function editKey(el, init) {
    const key = init.key;
    const shortcut = init.ctrlKey || init.metaKey;
    if (isContentEditable(el) && !shortcut
        && (key === 'Backspace' || key === 'Delete' || key === 'Enter')) {
      unsupported = 'contenteditable';
      return;
    }
    if (isTextControl(el)) {
      if (shortcut && (key === 'a' || key === 'A' || (init.commands || []).indexOf('selectAll') !== -1)) {
        setSelection(el, 0, String(el.value).length);
        return;
      }
      if (shortcut) return;
      if (key === 'Backspace' && editable(el)) deleteContent(el, false);
      else if (key === 'Delete' && editable(el)) deleteContent(el, true);
      else if (['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].indexOf(key) !== -1) {
        moveCaret(el, key, !!init.shiftKey);
      } else if (key === 'Enter') {
        if (el.localName === 'textarea') {
          if (editable(el)) insertText('\n');
        } else {
          const form = formOf(el);
          if (form) {
            const button = form.querySelector(
              'button:not([type]), button[type="submit"], input[type="submit"]',
            );
            if (button) click(button, { x: 0, y: 0 }, { detail: 0 });
            else submit(form, null);
          }
        }
      }
    }
    if (key === 'Tab') tab(!!init.shiftKey);
  }

  function activateOnKey(el, type, init) {
    const tag = el.localName;
    const kind = tag === 'input' ? attr(el, 'type') : '';
    const button = tag === 'button' || kind === 'button' || kind === 'submit' || kind === 'reset';
    if (init.key === 'Enter' && type === 'keydown' && (button || (tag === 'a' && has(el, 'href')))) {
      click(el, { x: 0, y: 0 }, { detail: 0 });
    } else if (init.key === ' ' && type === 'keyup' && (button || kind === 'checkbox' || kind === 'radio')) {
      click(el, { x: 0, y: 0 }, { detail: 0 });
    }
  }

  // Chrome's key event types: keyDown (keydown, then keypress and text when
  // the key produces text), rawKeyDown (keydown only), char (keypress and
  // text), keyUp.
  function key(type, init) {
    const target = keyTarget();
    if (type === 'keyUp') {
      fireKey(target, 'keyup', init);
      activateOnKey(target, 'keyup', init);
      return true;
    }
    if (type === 'char') {
      if (fireKey(target, 'keypress', init) && init.text) insertText(init.text);
      return true;
    }
    const proceed = fireKey(target, 'keydown', init);
    if (!proceed) return false;
    activateOnKey(target, 'keydown', init);
    if (type === 'keyDown' && init.text) {
      if (fireKey(target, 'keypress', init)) {
        // Enter arrives as a carriage return: a command, not text.
        if (init.text === '\r' || init.text === '\n') editKey(target, assign({}, init, { key: 'Enter' }));
        else insertText(init.text);
      }
    } else {
      editKey(target, init);
    }
    return true;
  }

  // ---- geometry from the host's layout snapshot ----------------------------

  // The host pushes the layout (see domains/snapshot.rs) before it runs a
  // client's function that reads geometry. Until native geometry exists these
  // members answer from the last push and as a box-less element before one.
  let layout = null;

  function rectOf(x, y, width, height) {
    return {
      x, y, width, height,
      top: y, right: x + width, bottom: y + height, left: x,
      toJSON() {
        return { x, y, width, height, top: y, right: x + width, bottom: y + height, left: x };
      },
    };
  }

  // Element -> index in `querySelectorAll('*')` order, built when the host
  // pushes a layout: the boxes describe that moment's tree, and so does this.
  let indexes = new SafeMap();

  function boxesOf(el) {
    if (!layout || !el.isConnected) return null;
    const index = indexes.get(el);
    return index === undefined ? null : layout.boxes[index] || null;
  }

  function defineGeometry(proto, name, descriptor) {
    if (proto && !Object.prototype.hasOwnProperty.call(proto, name)) {
      defineProperty(proto, name, assign({ configurable: true }, descriptor));
    }
  }

  defineGeometry(baseProto, 'getClientRects', {
    value: function getClientRects() {
      const boxes = boxesOf(this);
      return boxes ? boxes.r.map((r) => rectOf(r[0], r[1], r[2], r[3])) : [];
    },
    writable: true,
  });
  defineGeometry(baseProto, 'getBoundingClientRect', {
    value: function getBoundingClientRect() {
      const boxes = boxesOf(this);
      if (!boxes || boxes.r.length === 0) return rectOf(0, 0, 0, 0);
      let left = Infinity, top = Infinity, right = -Infinity, bottom = -Infinity;
      for (const r of boxes.r) {
        left = Math.min(left, r[0]);
        top = Math.min(top, r[1]);
        right = Math.max(right, r[0] + r[2]);
        bottom = Math.max(bottom, r[1] + r[3]);
      }
      return rectOf(left, top, right - left, bottom - top);
    },
    writable: true,
  });
  defineGeometry(baseProto, 'clientWidth', {
    get: function clientWidth() {
      if (this === doc.documentElement && layout) return layout.viewport[0];
      const boxes = boxesOf(this);
      return boxes ? Math.round(boxes.c[0]) : 0;
    },
  });
  defineGeometry(baseProto, 'clientHeight', {
    get: function clientHeight() {
      if (this === doc.documentElement && layout) return layout.viewport[1];
      const boxes = boxesOf(this);
      return boxes ? Math.round(boxes.c[1]) : 0;
    },
  });
  defineGeometry(elementProto, 'offsetWidth', {
    get: function offsetWidth() {
      const boxes = boxesOf(this);
      return boxes && boxes.r.length ? Math.round(boxes.r[0][2]) : 0;
    },
  });
  defineGeometry(elementProto, 'offsetHeight', {
    get: function offsetHeight() {
      const boxes = boxesOf(this);
      return boxes && boxes.r.length ? Math.round(boxes.r[0][3]) : 0;
    },
  });

  // ---- addressing nodes ---------------------------------------------------

  // A node is named to the host by its child indexes from the document.
  function pathOf(node) {
    if (!node || typeof node.nodeType !== 'number') return false;
    const path = [];
    for (let cursor = node; cursor && cursor !== doc; cursor = cursor.parentNode) {
      const parent = apply(parentNodeOf, cursor, []);
      if (!parent) return null;
      path.push(apply(indexOf, childrenOf(parent), [cursor]));
    }
    return path.reverse();
  }

  function nodeAt(path) {
    let node = doc;
    for (let i = 0; i < path.length; i += 1) {
      node = node && node.childNodes[path[i]];
    }
    return node || null;
  }

  function reporting(action) {
    return function () {
      wakeObservers();
      navigation = null;
      unsupported = null;
      apply(action, null, arguments);
      const report = { navigate: navigation, unsupported };
      navigation = null;
      unsupported = null;
      return report;
    };
  }

  defineProperty(global, '__tentaCdp', {
    value: Object.freeze({
      focus: focusElement,
      mouse: reporting(mouse),
      key: reporting(key),
      insertText: reporting(insertText),
      pathOf,
      nodeAt,
      setLayout(next) {
        wakeObservers();
        const all = apply(querySelectorAllOf, doc, ['*']);
        const fresh = new SafeMap();
        for (let i = 0; i < all.length; i += 1) fresh.set(all[i], i);
        indexes = fresh;
        layout = next;
      },
    }),
  });
})(globalThis);
