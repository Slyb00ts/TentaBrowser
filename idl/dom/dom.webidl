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
interface Node {
  const unsigned short ELEMENT_NODE = 1;
  const unsigned short TEXT_NODE = 3;
  const unsigned short COMMENT_NODE = 8;
  const unsigned short DOCUMENT_NODE = 9;

  readonly attribute unsigned short nodeType;
  readonly attribute DOMString nodeName;

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
  readonly attribute DOMString id;
  readonly attribute Element? firstElementChild;
  readonly attribute Element? lastElementChild;
  readonly attribute unsigned long childElementCount;

  [SameObject] readonly attribute HTMLCollection children;

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

  // Phase 2b-1: innerHTML (getter + setter - note: setter is fast-API eligible so non-throwing)
  attribute DOMString innerHTML;
};

[Exposed=Window]
interface Document : Node {
  readonly attribute Element? documentElement;
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
