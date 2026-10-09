// Constructed-sheet subset: attached sheets, MediaList, and title follow with
// HTML style/link loading. Keep the abstract parent prototype visible.
[Exposed=Window]
interface StyleSheet {
  readonly attribute DOMString type;
  readonly attribute USVString? href;
  readonly attribute Element? ownerNode;
  readonly attribute CSSStyleSheet? parentStyleSheet;
  readonly attribute DOMString? title;
  attribute boolean disabled;
};
