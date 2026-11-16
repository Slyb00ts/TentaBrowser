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

// `CDATASection : Text` with no members of its own, exactly as the DOM
// Standard declares it. It exists because `Document.createCDATASection`
// returns one and `dom/common.js` calls that member, and because a
// `NodeKind::CData` node had no interface name of its own before it — a CDATA
// section wrapped as a bare `Node`.
[Exposed=Window]
interface CDATASection : Text {};

[Exposed=Window]
interface Comment : CharacterData {};

[Exposed=Window]
interface Element : Node {
  readonly attribute DOMString tagName;
  // Phase 4b-1: reflected name/namespace and a writable id/className.
  readonly attribute DOMString? namespaceURI;
  readonly attribute DOMString localName;
  readonly attribute DOMString? prefix;
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
  [NewObject] HTMLCollection getElementsByTagNameNS(DOMString? namespace, DOMString localName);
  [NewObject] HTMLCollection getElementsByClassName(DOMString classNames);

  // Phase 2a: attribute mutation
  undefined setAttribute(DOMString name, DOMString value);
  DOMString? getAttribute(DOMString name);
  undefined removeAttribute(DOMString name);
  boolean hasAttribute(DOMString name);

  // Namespace-aware attribute operations normalize the null and empty
  // namespaces alike. Local names use ordinary DOMString conversion.
  undefined setAttributeNS(DOMString? namespace, DOMString qualifiedName, DOMString value);
  DOMString? getAttributeNS(DOMString? namespace, DOMString localName);
  undefined removeAttributeNS(DOMString? namespace, DOMString localName);
  boolean hasAttributeNS(DOMString? namespace, DOMString localName);

  // Phase 4b-1: attribute presence and toggling.
  boolean hasAttributes();
  [Throws] boolean toggleAttribute(DOMString name, optional boolean force);

  // Phase 2b-3: attribute-name enumeration (sequence return).
  sequence<DOMString> getAttributeNames();

  // d-10: the `Attr`/`NamedNodeMap` surface. `attributes` is `[SameObject]`
  // because the specification's attribute list is: the object is derived from
  // the element's attribute storage on every read, and two reads answer with
  // the same object.
  [SameObject] readonly attribute NamedNodeMap attributes;
  Attr? getAttributeNode(DOMString name);
  Attr? getAttributeNodeNS(DOMString? namespace, DOMString localName);
  [Throws] Attr? setAttributeNode(Attr attr);
  [Throws] Attr? setAttributeNodeNS(Attr attr);
  [Throws] Attr removeAttributeNode(Attr attr);

  // Phase 2b-1: innerHTML (getter + setter - note: setter is fast-API eligible so non-throwing)
  attribute DOMString innerHTML;

  // Phase 2b-5: outerHTML (getter + [Throws] setter forces the fallible path)
  // and insertAdjacentHTML.
  [Throws] attribute DOMString outerHTML;
  [Throws] undefined insertAdjacentHTML(DOMString position, DOMString text);

  // CSS1: CSSOM View geometry, answered by the page's layout (a page below the
  // render tier answers as an element with no box). Nothing scrolls, so the
  // scroll offsets are 0 and writing them is ignored.
  DOMRect getBoundingClientRect();
  [NewObject] DOMRectList getClientRects();
  readonly attribute long clientTop;
  readonly attribute long clientLeft;
  readonly attribute long clientWidth;
  readonly attribute long clientHeight;
  attribute unrestricted double scrollTop;
  attribute unrestricted double scrollLeft;
  readonly attribute long scrollWidth;
  readonly attribute long scrollHeight;
};

[Exposed=Window]
interface Document : Node {
  // The DOM Standard's own constructor: "create a new Document with its
  // content type set to `application/xml`, URL `about:blank`, and no
  // browsing context" (<https://dom.spec.whatwg.org/#dom-document-document>).
  // It is the bootstrap `dom/common.js` needs — line 59 of that file is
  // `const xmlDocument = new Document();`, and the throw there is what made
  // all 24 `dom/ranges` files that include the file report a harness `ERROR`
  // with no subtests at all.
  constructor();

  readonly attribute Element? documentElement;

  // d-9: the document-side surface the `dom/nodes` gate reads. `doctype` is the
  // document's first child doctype (`DocumentType`); `implementation` is the
  // document's own `DOMImplementation` (`[SameObject]`, one per document);
  // `defaultView` is the window of the browsing context the document is the
  // active document of, and null for a document with none — which is every
  // document `DOMImplementation` mints in this tree.
  readonly attribute DocumentType? doctype;
  [SameObject] readonly attribute DOMImplementation implementation;
  readonly attribute WindowProxy? defaultView;

  // The three members the HTML Standard defines beside `characterSet`
  // (HTML §4.2.3): `charset` and `inputEncoding` are two more spellings of the
  // same encoding name, and `contentType` is the MIME type the document was
  // created with — which is what distinguishes an HTML document from the XML
  // one `DOMImplementation.createDocument` mints.
  readonly attribute DOMString charset;
  readonly attribute DOMString inputEncoding;
  readonly attribute DOMString contentType;

  // Phase 4a: document conveniences the frameworks read on load.
  readonly attribute HTMLElement? body;
  readonly attribute HTMLElement? head;
  readonly attribute DOMString readyState;
  readonly attribute Element? activeElement;
  boolean hasFocus();

  // d-7: the document metadata members. `title` is not a reflected attribute:
  // it reads and writes the document's `<title>` element (see the host impl).
  // `URL`/`documentURI` report the document's own URL, `characterSet` its
  // encoding and `compatMode` its quirks mode. `referrer` has no navigation
  // state behind it yet and reports the empty string.
  attribute DOMString title;
  readonly attribute USVString URL;
  readonly attribute USVString documentURI;
  readonly attribute DOMString characterSet;
  readonly attribute DOMString compatMode;
  readonly attribute USVString referrer;

  // d-4b: `document.cookie` — the last framework-visible IDL gap. The
  // getter and setter are the HTML Standard's cookie getter/setter
  // (https://html.spec.whatwg.org/multipage/dom.html#dom-document-cookie),
  // which delegate to RFC 6265bis's "cookie-string" and "receiving a
  // set-cookie-string" with the non-HTTP API flag set.
  attribute USVString cookie;

  readonly attribute Element? firstElementChild;
  readonly attribute Element? lastElementChild;
  readonly attribute unsigned long childElementCount;

  [SameObject] readonly attribute HTMLCollection children;

  Element? getElementById(DOMString elementId);
  Element? querySelector(DOMString selectors);
  [NewObject] NodeList querySelectorAll(DOMString selectors);

  // Phase 1c: static snapshots (live collections in later phases)
  [NewObject] HTMLCollection getElementsByTagName(DOMString qualifiedName);
  [NewObject] HTMLCollection getElementsByTagNameNS(DOMString? namespace, DOMString localName);
  [NewObject] HTMLCollection getElementsByClassName(DOMString classNames);

  [NewObject] Element createElement(DOMString localName);
  [NewObject] Text createTextNode(DOMString data);
  [NewObject] Comment createComment(DOMString data);

  // d-10: the detached-`Attr` factories. `createAttribute` lowercases in an
  // HTML document exactly as `createElement` does; `createAttributeNS` runs the
  // same *validate and extract* as `createElementNS`.
  [NewObject] Attr createAttribute(DOMString localName);
  [Throws, NewObject] Attr createAttributeNS(DOMString? namespace, DOMString qualifiedName);

  // d-9: the processing-instruction factory. `target` is validated against
  // XML's `Name` production and `data` against the `?>` sequence, both with an
  // `InvalidCharacterError`; see the host implementation for the two rules and
  // the WPT file that pins them.
  [Throws, NewObject] ProcessingInstruction createProcessingInstruction(DOMString target, DOMString data);

  // The CDATA-section factory. It is XML-only: in an HTML document the
  // standard's step is "throw a `NotSupportedError`"
  // (<https://dom.spec.whatwg.org/#dom-document-createcdatasection>), which is
  // why the constructor above and this member travel together — the document
  // `new Document()` makes is the only one `dom/common.js` can call it on.
  [Throws, NewObject] CDATASection createCDATASection(DOMString data);

  // The range factory. A new range is collapsed at the document's own
  // (0, 0) — the document node with offset 0.
  [NewObject] Range createRange();

  // Phase 4b-3: fragment and namespace-aware creation. Frameworks batch node
  // inserts into a DocumentFragment (React range insertion, Vue fragment
  // blocks) and reach for createElementNS to build SVG/MathML subtrees.
  [NewObject] DocumentFragment createDocumentFragment();
  [Throws, NewObject] Element createElementNS(DOMString? namespace, DOMString qualifiedName);

  // Phase 3a: event factory (createEvent + initEvent path, no constructors yet).
  [Throws, NewObject] Event createEvent(DOMString interface);

  // CSS1: hit testing against the page's layout, and the element the document
  // scrolls. A page below the render tier answers `null` and an empty list.
  Element? elementFromPoint(double x, double y);
  sequence<Element> elementsFromPoint(double x, double y);
  readonly attribute Element? scrollingElement;

  // CSS1: the document's style sheets, one per `<style>` and per loaded
  // `<link rel=stylesheet>`, in tree order.
  [SameObject] readonly attribute StyleSheetList styleSheets;
};

// Phase 4d-3: `iterable<>` makes the collection usable with `for...of` and
// spread. Both are array-like (a `length` and an indexed getter), so the
// generated `@@iterator` is `%Array.prototype.values%`; application code -
// and every framework - iterates query results far more often than it indexes
// them.
[Exposed=Window, LegacyUnenumerableNamedProperties]
interface HTMLCollection {
  readonly attribute unsigned long length;
  getter Element? item(unsigned long index);
  getter Element? namedItem(DOMString name);
  iterable<Element>;
};

[Exposed=Window]
interface NodeList {
  readonly attribute unsigned long length;
  getter Node? item(unsigned long index);
  iterable<Node>;
};

// d-10: `Element.attributes`. The DOM Standard's `NamedNodeMap` is the
// element's attribute list itself — one live object per element, which is what
// `[SameObject]` on `Element.attributes` and this interface's own identity
// rule rest on. Every member reads the element's attribute storage, so a write
// through `setAttribute` is visible through a map obtained before it.
//
// The indexed and named getters follow the DOM Standard's legacy collection
// property rules; getNamedItemNS remains an ordinary operation.
[Exposed=Window, LegacyUnenumerableNamedProperties]
interface NamedNodeMap {
  readonly attribute unsigned long length;
  getter Attr? item(unsigned long index);
  getter Attr? getNamedItem(DOMString name);
  Attr? getNamedItemNS(DOMString? namespace, DOMString localName);
  [Throws] Attr? setNamedItem(Attr attr);
  [Throws] Attr? setNamedItemNS(Attr attr);
  [Throws] Attr removeNamedItem(DOMString name);
  [Throws] Attr removeNamedItemNS(DOMString? namespace, DOMString localName);
  iterable<Attr>;
};

// Attr is a Node with no tree position or children. Its TAG_HOST record names
// an entry in the element's packed attribute list without allocating an arena
// node. Inherited Node operations dispatch on that record explicitly.
[Exposed=Window]
interface Attr : Node {
  readonly attribute DOMString? namespaceURI;
  readonly attribute DOMString? prefix;
  readonly attribute DOMString localName;
  readonly attribute DOMString name;
  attribute DOMString value;
  readonly attribute Element? ownerElement;
  readonly attribute boolean specified;
};

// Phase 4c-1: DOMTokenList backs Element.classList. Backed by the `class`
// attribute as an ordered set of tokens (whitespace-split, de-duplicated).
// Phase 4d-3 makes `item` an indexed getter and the list iterable, so
// `list[0]` and `for (const c of el.classList)` behave as the spec requires.
[Exposed=Window]
interface DOMTokenList {
  readonly attribute unsigned long length;
  getter DOMString? item(unsigned long index);
  iterable<DOMString>;
  boolean contains(DOMString token);
  [Throws] undefined add(DOMString... tokens);
  [Throws] undefined remove(DOMString... tokens);
  [Throws] boolean toggle(DOMString token, optional boolean force);
  [Throws] boolean replace(DOMString token, DOMString newToken);
  attribute DOMString value;
};

// d-9: `DocumentType` and `ProcessingInstruction` — the two node kinds the
// `tenta-dom` arena already had (`NodeKind::DocType`, `NodeKind::ProcessingInstruction`)
// and the JS surface was missing entirely. Both are real node types: their
// wrappers are node wrappers, their handles point into the arena, and their
// members read the arena's own per-kind payloads.
[Exposed=Window]
interface DocumentType : Node {
  readonly attribute DOMString name;
  readonly attribute DOMString publicId;
  readonly attribute DOMString systemId;
};

// `ProcessingInstruction` is a `CharacterData` subclass in the DOM Standard, so
// `data`, `length` and the six splice operations come from the parent and the
// target is the only member it adds.
[Exposed=Window]
interface ProcessingInstruction : CharacterData {
  readonly attribute DOMString target;
};

// d-9: `DOMImplementation`. `document.implementation` was the single largest
// missing member of the `dom/nodes` gate (488 + 136 + 117 of 3215 failing
// subtests reach it through `documentElement`, `hasFeature` and
// `createHTMLDocument` respectively).
//
// An implementation is a per-document object with no state of its own, and its
// three factories build *real* nodes and documents in `tenta_dom` arenas rather
// than stand-ins: `createDocumentType` mints a doctype in the implementation's
// own document, and `createHTMLDocument` mints a whole second document with its
// own doctype, `html`/`head`/`title`/`body` and URL (`about:blank`).
//
// Three declarations here differ from the DOM Standard's text, and all are
// recorded deviations rather than oversights:
//
//  - `createDocument` returns `Document` where the standard says `XMLDocument`.
//    This tree has no `XMLDocument` interface and no XML-specific document
//    prototype, so the standard's `Object.getPrototypeOf(doc) ===
//    XMLDocument.prototype` cannot hold here; every document is an
//    HTML-document-shaped `tenta_dom::Document` registered in the same arena
//    registry.
//  - `createDocument`'s `namespace` and `qualifiedName` retain nullable
//    compatibility signatures instead of the standard's `optional DOMString?`
//    and `[LegacyNullToEmptyString] DOMString`. The generator now supports the
//    annotation; aligning these signatures and host conversions remains a
//    separate change. In particular, explicitly passing `undefined` for
//    `qualifiedName` is not equivalent to the standard conversion.
//  - There is no *adoption*: a node cannot move between documents. This is the
//    one deviation that reaches beyond `DOMImplementation`, and it is why the
//    two above are shaped the way they are. A node's document here *is* the
//    arena the node lives in, and a handle names that arena by its slot, so
//    adopting would mean moving a subtree into another arena — assigning it new
//    node ids there — and re-keying every wrapper over the moved nodes. Two
//    consequences, both measured against `dom/nodes`:
//
//    * a `doctype` argument to `createDocument` is *reproduced* in the new
//      document's arena rather than moved. The new document gets a doctype with
//      the same name/publicId/systemId owned by itself, but
//      `doc.doctype === doctype` does not hold and the argument doctype stays
//      where it was, whether or not it already had a parent.
//      `DOMImplementation-createDocument.html` asserts the identity, so the rows
//      that pass a doctype fail one assertion each; the alternative — throwing
//      when the argument is already in a tree — would fail every assertion in
//      the row, for a step the standard makes an *append* (which adopts) and not
//      fallible.
//    * an insertion of a node from another document — `appendChild`,
//      `insertBefore` and `replaceChild`, whose standard algorithms adopt the
//      node into the parent's document as their first step — is refused with a
//      `NotSupportedError` instead of writing a foreign node id into this
//      arena, where it would name an unrelated node or none at all. The same
//      refusal the `FormData` constructor already makes for a `form` argument.
//      `Node-mutation-adoptNode.html`'s "simple append of foreign div with
//      text", which asserts `div.ownerDocument === document` afterwards, is the
//      subtest this records as failing. `Node.contains` needs no refusal: a node
//      of another tree is not an inclusive ancestor of anything here, and the
//      answer is `false`. See `HostObject::same_document_node`.
[Exposed=Window]
interface DOMImplementation {
  // The DOM Standard declares this with no parameters at all ("useless; always
  // returns true"), which is what lets
  // `dom/nodes/DOMImplementation-hasFeature.html` call it with zero, one and
  // two arguments and expect `true` every time. Declaring two required
  // parameters would turn six of its rows into `TypeError`s.
  boolean hasFeature();
  [NewObject] DocumentType createDocumentType(DOMString qualifiedName, DOMString publicId, DOMString systemId);
  [NewObject] Document createDocument(DOMString? namespace, DOMString? qualifiedName, optional DocumentType? doctype);
  [NewObject] Document createHTMLDocument(optional DOMString title);
};

// Range and its two relatives. The member text is the DOM Standard's own IDL
// index (<https://dom.spec.whatwg.org/#interface-range>), so names, types,
// order and the four `compareBoundaryPoints` constants are the spec's.
//
// `AbstractRange` is the base the other two derive from, and it is *not*
// constructible — the standard gives it no constructor and neither does this
// file, so `new AbstractRange()` is the generator's "Illegal constructor".
//
// Live ranges register their endpoints with `tenta-dom`: insertion, removal
// and character-data replacement update them through the shared mutation path.
// Endpoints retain their nodes until the range is reclaimed. Text splitting,
// normalization and the editing operations listed below remain undeclared.
[Exposed=Window]
interface AbstractRange {
  readonly attribute Node startContainer;
  readonly attribute unsigned long startOffset;
  readonly attribute Node endContainer;
  readonly attribute unsigned long endOffset;
  readonly attribute boolean collapsed;
};

// `StaticRange`'s four members come from a dictionary rather than from four
// parameters, and they are *copied* at construction: the standard's
// `StaticRange` deliberately does not track mutations, which is why its own
// "collapsed" compares the copied boundary points.
dictionary StaticRangeInit {
  required Node startContainer;
  required unsigned long startOffset;
  required Node endContainer;
  required unsigned long endOffset;
};

[Exposed=Window]
interface StaticRange : AbstractRange {
  constructor(StaticRangeInit init);
};

[Exposed=Window]
interface Range : AbstractRange {
  constructor();

  readonly attribute Node commonAncestorContainer;

  undefined setStart(Node node, unsigned long offset);
  undefined setEnd(Node node, unsigned long offset);
  undefined setStartBefore(Node node);
  undefined setStartAfter(Node node);
  undefined setEndBefore(Node node);
  undefined setEndAfter(Node node);
  undefined collapse(optional boolean toStart = false);
  undefined selectNode(Node node);
  undefined selectNodeContents(Node node);

  const unsigned short START_TO_START = 0;
  const unsigned short START_TO_END = 1;
  const unsigned short END_TO_END = 2;
  const unsigned short END_TO_START = 3;

  short compareBoundaryPoints(unsigned short how, Range sourceRange);
  [NewObject] Range cloneRange();
  undefined detach();

  // **The five tree-surgery operations this interface does NOT declare:**
  // `deleteContents()`, `extractContents()`, `cloneContents()`,
  // `insertNode(node)` and `surroundContents(newParent)`. The DOM Standard
  // declares all five here. They are absent rather than stubbed, because each
  // one's algorithm is exact — the "contained", "partially contained" and
  // "partially contained, first/last" classifications decide a DOM shape that
  // `dom/ranges` compares node by node and offset by offset, 63 argument rows
  // deep — and an implementation written to a guess would be an unverifiable
  // member, which is the bug class slice 4a removed. Their absence costs the
  // five files named below, whose remaining subtests fail with "is not a
  // function" *inside* a testharness test (so the harness still reports and the
  // numerators are comparable):
  //
  //   | file | rows |
  //   |---|---|
  //   | `Range-cloneContents.html` | 63 |
  //   | `Range-deleteContents.html` | 63 |
  //   | `Range-extractContents.html` | 63 |
  //   | `Range-insertNode.html` | 63 |
  //   | `Range-surroundContents.html` | 63 |
  //
  // That is 315 subtest rows, all of which need the missing `tenta-dom`
  // primitive none of this engine's tree code has: a *split* of a `Text` node
  // at an offset (`insertNode` step 7, `Text.splitText`) and the adjacent-`Text`
  // merge that goes with it. `extractContents` is also the first step of
  // `surroundContents`, so the five travel as one unit.

  boolean isPointInRange(Node node, unsigned long offset);
  short comparePoint(Node node, unsigned long offset);
  boolean intersectsNode(Node node);

  stringifier;
};

// Phase 3d: HTMLElement — the interface HTML-namespace element wrappers get.
// `click()` is [Throws] so it takes the fallible (non-fast-API) path and may
// reenter JS to dispatch a synthetic, untrusted `click` MouseEvent.
[Exposed=Window]
interface HTMLElement : Element {
  [Throws] undefined click();

  // CSS1: CSSOM View offset geometry.
  readonly attribute Element? offsetParent;
  readonly attribute long offsetTop;
  readonly attribute long offsetLeft;
  readonly attribute long offsetWidth;
  readonly attribute long offsetHeight;
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
// (`value`/`checked`/`selected`) and the enumerated `type`/`method` attributes
// are deliberately deferred.
//
// d-2c wires the URL-valued `href`/`src`/`action` — the `USVString` reflections
// the HTML Standard defines through the URL rule, which the host implements as
// `reflect_url` (absent -> "", else resolved against the element's node
// document's base URL, falling back to the raw value when it will not parse).
[Exposed=Window]
interface HTMLDivElement : HTMLElement {};

[Exposed=Window]
interface HTMLSpanElement : HTMLElement {};

[Exposed=Window]
interface HTMLParagraphElement : HTMLElement {};

[Exposed=Window]
interface HTMLAnchorElement : HTMLElement {
  attribute USVString href;
};

// IM4: the members script reads to follow an image's loading. The state behind
// `complete`, `naturalWidth`, `naturalHeight` and `currentSrc` belongs to the
// page (`ImageHost`, `crates/tenta-js/src/image.rs`): an image is fetched when
// its element enters the document or its source attributes change, whether or
// not the element is connected, and `load` and `error` are queued as tasks.
// `width` and `height` reflect the content attributes and fall back to the
// natural size; no layout is consulted. `decoding` and `loading` are plain
// reflected strings; `loading=lazy` is honoured by the loader, not by these
// members. `crossOrigin` and `referrerPolicy` are not declared: CORS-mode image
// requests are not implemented.
[Exposed=Window]
interface HTMLImageElement : HTMLElement {
  attribute USVString src;
  attribute USVString srcset;
  attribute DOMString sizes;
  attribute DOMString alt;
  attribute DOMString loading;
  attribute DOMString decoding;
  attribute unsigned long width;
  attribute unsigned long height;
  readonly attribute unsigned long naturalWidth;
  readonly attribute unsigned long naturalHeight;
  readonly attribute boolean complete;
  readonly attribute USVString currentSrc;
  [NewObject] Promise<undefined> decode();
  attribute EventHandler onload;
  attribute EventHandler onerror;
};

[Exposed=Window]
interface HTMLLabelElement : HTMLElement {
  attribute DOMString htmlFor;
};

[Exposed=Window]
interface HTMLFormElement : HTMLElement {
  attribute DOMString name;
  attribute USVString action;
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

// d-2c: the `<script>` element interface, the generated planning manifest's
// most-accessed missing HTML interface (1089 accesses across the golden sites,
// `src` first among its members — `idl/MANIFEST.toml`). Every member here is a
// content-attribute reflection only: `defer`, `async` and `noModule` say
// nothing about *when* a script runs, which stays the parser's and the task
// queue's business.
[Exposed=Window]
interface HTMLScriptElement : HTMLElement {
  attribute USVString src;
  attribute DOMString type;
  attribute DOMString charset;
  attribute DOMString crossOrigin;
  attribute DOMString integrity;
  attribute boolean async;
  attribute boolean defer;
  attribute boolean noModule;
};

// CSS1: the stylesheet-bearing elements. They reflect the content attributes the
// stylesheet loader (`crates/tenta-page/src/page/sheet_loader.rs`) reads, and
// fire `load` / `error` at a `<link>` when its sheet has loaded or failed.
// `sheet` is the CSSOM view of the loaded text; `relList` and `blocking` are not
// declared.
[Exposed=Window]
interface HTMLLinkElement : HTMLElement {
  attribute USVString href;
  attribute DOMString rel;
  attribute DOMString as;
  attribute DOMString media;
  attribute DOMString type;
  attribute DOMString crossOrigin;
  attribute DOMString hreflang;
  attribute boolean disabled;
  attribute EventHandler onload;
  attribute EventHandler onerror;
  readonly attribute CSSStyleSheet? sheet;
};

[Exposed=Window]
interface HTMLStyleElement : HTMLElement {
  attribute DOMString media;
  attribute DOMString type;
  attribute boolean disabled;
  attribute EventHandler onload;
  attribute EventHandler onerror;
  readonly attribute CSSStyleSheet? sheet;
};

// Phase 4b-2: the ChildNode mixin (the first `includes` in idl/dom). Frameworks
// call `node.remove()` to detach a node from its parent; it is shared by Element
// and CharacterData (Text/Comment), and by DocumentType. `before`, `after` and
// `replaceWith` take the same `(Node or DOMString)...` list as `ParentNode.append`.
interface mixin ChildNode {
  undefined before((Node or DOMString)... nodes);
  undefined after((Node or DOMString)... nodes);
  undefined replaceWith((Node or DOMString)... nodes);
  undefined remove();
};

Element includes ChildNode;
CharacterData includes ChildNode;
DocumentType includes ChildNode;

// `ParentNode.append`, `prepend` and `replaceChildren` are shared by Element,
// Document and DocumentFragment. Navigation members already declared directly on
// the interfaces remain there.
//
// Each of them converts each argument with *convert nodes into a node*: a `Node`
// is used as-is and a string becomes a `Text` containing it.
//
// Each string argument creates a separate Text node, including consecutive
// strings. The operation preserves adjacent Text nodes without normalization.
interface mixin ParentNode {
  undefined prepend((Node or DOMString)... nodes);
  undefined append((Node or DOMString)... nodes);
  undefined replaceChildren((Node or DOMString)... nodes);
};

Element includes ParentNode;
Document includes ParentNode;
DocumentFragment includes ParentNode;

// Phase 4c-2: inline style. CSSStyleDeclaration exposes the `style` attribute's
// declaration block as a CSSOM object. The per-property attributes of CSSOM
// ("Extensions to the CSSStyleDeclaration interface": `backgroundColor`,
// `webkitTransform`, `background-color`) cannot be listed here, because the set
// is whatever Stylo supports. They are defined on CSSStyleProperties.prototype
// by `crates/tenta-js/src/bindings/style_properties.rs`, once per context when
// its first CSSStyleProperties wrapper is made, and forward to
// getPropertyValue/setProperty.
[Exposed=Window]
interface CSSStyleDeclaration {
  attribute DOMString cssText;
  readonly attribute unsigned long length;
  getter DOMString item(unsigned long index);
  DOMString getPropertyValue(DOMString property);
  DOMString getPropertyPriority(DOMString property);
  undefined setProperty(DOMString property, [LegacyNullToEmptyString] DOMString value, optional [LegacyNullToEmptyString] DOMString priority = "");
  DOMString removeProperty(DOMString property);
  readonly attribute CSSRule? parentRule;
};

// Both inline element styles and CSSStyleRule.style have the current CSSOM
// subclass prototype. cssFloat forwards to the same Stylo `float` property.
[Exposed=Window]
interface CSSStyleProperties : CSSStyleDeclaration {
  attribute [LegacyNullToEmptyString] DOMString cssFloat;
};

interface mixin ElementCSSInlineStyle {
  [SameObject, PutForwards=cssText] readonly attribute CSSStyleProperties style;
};

HTMLElement includes ElementCSSInlineStyle;
SVGElement includes ElementCSSInlineStyle;

// The focus members of HTML's HTMLOrSVGElement mixin. `focus()` and `blur()`
// run the standard's focusing and unfocusing steps against the document's
// single focused area and fire `focus`/`focusin`/`blur`/`focusout`; they can
// run listeners, so both are [Throws]. `focusVisible` decides
// `:focus-visible`; `preventScroll` is accepted and ignored because nothing
// scrolls.
interface mixin HTMLOrSVGElement {
  [Throws] undefined focus(optional FocusOptions options = {});
  [Throws] undefined blur();
  attribute long tabIndex;
};

dictionary FocusOptions {
  boolean preventScroll = false;
  boolean focusVisible;
};

HTMLElement includes HTMLOrSVGElement;
SVGElement includes HTMLOrSVGElement;
