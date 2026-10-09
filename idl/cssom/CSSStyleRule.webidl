// Top-level, childless style rules in constructed sheets. The declaration
// block remains mutable through the same Stylo rule allocation.
[Exposed=Window]
interface CSSStyleRule : CSSGroupingRule {
  attribute DOMString selectorText;
  [SameObject, PutForwards=cssText] readonly attribute CSSStyleProperties style;
};
