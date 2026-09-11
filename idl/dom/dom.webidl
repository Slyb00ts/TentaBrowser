// Minimal DOM WebIDL for the P2 package (c) binding phases
// (`docs/spec/05-javascript.md`). This is the first real DOM IDL wired through
// `tenta-idl` in a consumer crate; it carries the read-only surface the tree
// navigation slice needs and grows into the full DOM in the later phases.
//
// Binding phase 1a adds read-only tree navigation (`nodeName`, the parent,
// child and sibling links, `childNodes`/`NodeList`, `tagName`, the reflected
// `id` and `firstElementChild`). Binding phase 1b adds node-derived query/create
// methods, text nodes (Text, Comment, CharacterData), and live HTMLCollection.
// Writable members, `EventTarget`, attributes, `classList` and the
// querySelector[All]/create[Element/TextNode] variants are deliberately later phases.

[Exposed=Window]
interface Node : EventTarget {
  const unsigned short ELEMENT_NODE = 1;
  const unsigned short ATTRIBUTE_NODE = 2;
  const unsigned short TEXT_NODE = 3;
  const unsigned short CDATA_SECTION_NODE = 4;
  const unsigned short ENTITY_REFERENCE_NODE = 5;
  const unsigned short ENTITY_NODE = 6;
  const unsigned short PROCESSING_INSTRUCTION_NODE = 7;
  const unsigned short COMMENT_NODE = 8;
  const unsigned short DOCUMENT_NODE = 9;
  const unsigned short DOCUMENT_TYPE_NODE = 10;
  const unsigned short DOCUMENT_FRAGMENT_NODE = 11;
  const unsigned short NOTATION_NODE = 12;

  readonly attribute unsigned short nodeType;
  readonly attribute DOMString nodeName;

  // Phase 4a: document association and connectivity.
  readonly attribute Document? ownerDocument;
  readonly attribute boolean isConnected;

  readonly attribute Node? parentNode;
  readonly attribute Element? parentElement;
  readonly attribute Node? firstChild;
  readonly attribute Node? lastChild;
  readonly attribute Node? previousSibling;
  readonly attribute Node? nextSibling;

  // Phase 2: textContent and nodeValue are writable
  attribute DOMString? textContent;
  attribute DOMString? nodeValue;

  [SameObject] readonly attribute NodeList childNodes;

  boolean hasChildNodes();
  boolean contains(Node? other);

  // Phase 2a: mutation operations
  Node appendChild(Node node);
  Node insertBefore(Node node, Node? child);
  Node removeChild(Node child);
  Node replaceChild(Node node, Node child);
  Node cloneNode(optional boolean deep = false);
};

[Exposed=Window]
interface CharacterData : Node {
  // Phase 2: data is writable
  attribute DOMString data;
  readonly attribute unsigned long length;

  // Phase 2b-2: substring/mutation ops. Offsets are UTF-16 code units;
  // out-of-range offsets throw IndexSizeError. appendData cannot fail, so it
  // stays fast-API eligible; the rest are [Throws] and take the fallible path.
  [Throws] DOMString substringData(unsigned long offset, unsigned long count);
  undefined appendData(DOMString data);
  [Throws] undefined insertData(unsigned long offset, DOMString data);
  [Throws] undefined deleteData(unsigned long offset, unsigned long count);
  [Throws] undefined replaceData(unsigned long offset, unsigned long count, DOMString data);
};

[Exposed=Window]
interface Text : CharacterData {};

[Exposed=Window]
interface Comment : CharacterData {};

[Exposed=Window]
interface Element : Node {
  readonly attribute DOMString tagName;
  // Phase 4b-1: reflected name/namespace and a writable id/className.
  readonly attribute DOMString? namespaceURI;
  readonly attribute DOMString localName;
  attribute DOMString id;
  attribute DOMString className;
  readonly attribute Element? firstElementChild;
  readonly attribute Element? lastElementChild;
  readonly attribute unsigned long childElementCount;

  [SameObject] readonly attribute HTMLCollection children;

  // Phase 4c-1: classList reflects the `class` attribute as an ordered set of
  // tokens; frameworks toggle classes through it constantly.
  [SameObject] readonly attribute DOMTokenList classList;

  Element? querySelector(DOMString selectors);
  [NewObject] NodeList querySelectorAll(DOMString selectors);
  boolean matches(DOMString selectors);
  Element? closest(DOMString selectors);

  // Phase 1c: static snapshots (live collections in later phases)
  [NewObject] HTMLCollection getElementsByTagName(DOMString qualifiedName);
  [NewObject] HTMLCollection getElementsByClassName(DOMString classNames);

  // Phase 2a: attribute mutation
  undefined setAttribute(DOMString name, DOMString value);
  DOMString? getAttribute(DOMString name);
  undefined removeAttribute(DOMString name);
  boolean hasAttribute(DOMString name);

  // Phase 4b-1: attribute presence and toggling.
  boolean hasAttributes();
  [Throws] boolean toggleAttribute(DOMString name, optional boolean force);

  // Phase 2b-3: attribute-name enumeration (sequence return).
  sequence<DOMString> getAttributeNames();

  // Phase 2b-1: innerHTML (getter + setter - note: setter is fast-API eligible so non-throwing)
  attribute DOMString innerHTML;

  // Phase 2b-5: outerHTML (getter + [Throws] setter forces the fallible path)
  // and insertAdjacentHTML.
  [Throws] attribute DOMString outerHTML;
  [Throws] undefined insertAdjacentHTML(DOMString position, DOMString text);
};

[Exposed=Window]
interface Document : Node {
  readonly attribute Element? documentElement;

  // Phase 4a: document conveniences the frameworks read on load.
  readonly attribute HTMLElement? body;
  readonly attribute HTMLElement? head;
  readonly attribute DOMString readyState;
  readonly attribute Element? activeElement;

  readonly attribute Element? firstElementChild;
  readonly attribute Element? lastElementChild;
  readonly attribute unsigned long childElementCount;

  [SameObject] readonly attribute HTMLCollection children;

  Element? getElementById(DOMString elementId);
  Element? querySelector(DOMString selectors);
  [NewObject] NodeList querySelectorAll(DOMString selectors);

  // Phase 1c: static snapshots (live collections in later phases)
  [NewObject] HTMLCollection getElementsByTagName(DOMString qualifiedName);
  [NewObject] HTMLCollection getElementsByClassName(DOMString classNames);

  [NewObject] Element createElement(DOMString localName);
  [NewObject] Text createTextNode(DOMString data);
  [NewObject] Comment createComment(DOMString data);

  // Phase 4b-3: fragment and namespace-aware creation. Frameworks batch node
  // inserts into a DocumentFragment (React range insertion, Vue fragment
  // blocks) and reach for createElementNS to build SVG/MathML subtrees.
  [NewObject] DocumentFragment createDocumentFragment();
  [Throws, NewObject] Element createElementNS(DOMString? namespace, DOMString qualifiedName);

  // Phase 3a: event factory (createEvent + initEvent path, no constructors yet).
  [Throws, NewObject] Event createEvent(DOMString interface);
};

[Exposed=Window]
interface HTMLCollection {
  readonly attribute unsigned long length;
  getter Element? item(unsigned long index);
  Element? namedItem(DOMString name);
};

[Exposed=Window]
interface NodeList {
  readonly attribute unsigned long length;
  getter Node? item(unsigned long index);
};

// Phase 4c-1: DOMTokenList backs Element.classList. Backed by the `class`
// attribute as an ordered set of tokens (whitespace-split, de-duplicated).
// `item` is a plain operation (not an indexed getter) and iteration is
// deferred; frameworks reach for contains/add/remove/toggle, not `list[i]`.
[Exposed=Window]
interface DOMTokenList {
  readonly attribute unsigned long length;
  DOMString? item(unsigned long index);
  boolean contains(DOMString token);
  [Throws] undefined add(DOMString... tokens);
  [Throws] undefined remove(DOMString... tokens);
  [Throws] boolean toggle(DOMString token, optional boolean force);
  [Throws] boolean replace(DOMString token, DOMString newToken);
  attribute DOMString value;
};

// Phase 3d: HTMLElement — the interface HTML-namespace element wrappers get.
// `click()` is [Throws] so it takes the fallible (non-fast-API) path and may
// reenter JS to dispatch a synthetic, untrusted `click` MouseEvent.
[Exposed=Window]
interface HTMLElement : Element {
  [Throws] undefined click();
};

// Phase 4a-2: interface shells so `instanceof` and the global constructor exist.
// A framework reads `element instanceof win.HTMLIFrameElement` (React's
// `getActiveElementDeep`) and `container instanceof SVGElement` (Vue's root
// namespace resolution); both throw if the right-hand side is undefined.
[Exposed=Window]
interface HTMLIFrameElement : HTMLElement {};

[Exposed=Window]
interface SVGElement : Element {};

// Phase 4b-3: DocumentFragment — a lightweight parentless container. Appending
// a fragment moves its children into the target and empties the fragment.
[Exposed=Window]
interface DocumentFragment : Node {};

// Phase 4b-3: <template> keeps its parsed children inert inside a separate
// DocumentFragment reachable only through `.content`; Vue's compiler and other
// templating paths read it. The contents fragment is owned by the template
// element, so `content` is [SameObject].
[Exposed=Window]
interface HTMLTemplateElement : HTMLElement {
  [SameObject] readonly attribute DocumentFragment content;
};

// Phase 4d: per-tag HTML element interfaces so `instanceof HTMLInputElement`
// (and friends) works and each has a global constructor. Frameworks branch on
// these — React's controlled-input logic, event delegation and focus handling
// test `el instanceof HTMLInputElement`/`HTMLSelectElement`/`HTMLTextAreaElement`,
// and layout/query code checks the common block/inline interfaces. Only the
// unambiguously-safe reflected attributes are wired here: `name`, `placeholder`
// (verbatim string reflections) and `disabled` (boolean). The dirty-value model
// (`value`/`checked`/`selected`), the enumerated `type`/`method` attributes and
// the URL-valued `href`/`src`/`action` are deliberately deferred.
[Exposed=Window]
interface HTMLDivElement : HTMLElement {};

[Exposed=Window]
interface HTMLSpanElement : HTMLElement {};

[Exposed=Window]
interface HTMLParagraphElement : HTMLElement {};

[Exposed=Window]
interface HTMLAnchorElement : HTMLElement {};

[Exposed=Window]
interface HTMLImageElement : HTMLElement {};

[Exposed=Window]
interface HTMLLabelElement : HTMLElement {
  attribute DOMString htmlFor;
};

[Exposed=Window]
interface HTMLFormElement : HTMLElement {
  attribute DOMString name;
};

[Exposed=Window]
interface HTMLInputElement : HTMLElement {
  attribute DOMString name;
  attribute DOMString placeholder;
  attribute boolean disabled;
  // The dirty value model (05 §9a): `value` reads the dirty value once one has
  // been set and the `value` content attribute until then; assigning always
  // raises the dirty value flag. `checked` works the same way over the
  // `checked` content attribute.
  attribute DOMString value;
  attribute boolean checked;
};

[Exposed=Window]
interface HTMLButtonElement : HTMLElement {
  attribute DOMString name;
  attribute boolean disabled;
};

[Exposed=Window]
interface HTMLSelectElement : HTMLElement {
  attribute DOMString name;
  attribute boolean disabled;
};

[Exposed=Window]
interface HTMLTextAreaElement : HTMLElement {
  attribute DOMString name;
  attribute DOMString placeholder;
  attribute boolean disabled;
  // A textarea's raw value is its child text, so `value` falls back to the
  // text content rather than to an attribute until the dirty flag is raised.
  attribute DOMString value;
};

[Exposed=Window]
interface HTMLOptionElement : HTMLElement {
  attribute boolean disabled;
};

// Phase 4b-2: the ChildNode mixin (the first `includes` in idl/dom). Frameworks
// call `node.remove()` to detach a node from its parent; it is shared by Element
// and CharacterData (Text/Comment).
interface mixin ChildNode {
  undefined remove();
};

Element includes ChildNode;
CharacterData includes ChildNode;

// Phase 4c-2: inline style. CSSStyleDeclaration exposes the `style` attribute's
// declaration block as a CSSOM object. Camel-case named access (`style.color`)
// needs a named-property handler the generator does not install yet, so only
// the object model (cssText, item, getPropertyValue, setProperty, ...) is wired;
// frameworks that set individual properties through `setProperty`/`cssText`
// work, `style.color = ...` does not.
[Exposed=Window]
interface CSSStyleDeclaration {
  attribute DOMString cssText;
  readonly attribute unsigned long length;
  DOMString item(unsigned long index);
  DOMString getPropertyValue(DOMString property);
  DOMString getPropertyPriority(DOMString property);
  undefined setProperty(DOMString property, DOMString value, optional DOMString priority = "");
  DOMString removeProperty(DOMString property);
};

interface mixin ElementCSSInlineStyle {
  [SameObject] readonly attribute CSSStyleDeclaration style;
};

HTMLElement includes ElementCSSInlineStyle;
SVGElement includes ElementCSSInlineStyle;
