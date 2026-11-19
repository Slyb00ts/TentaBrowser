// A style rule at any depth. The declaration block stays mutable through the
// same Stylo rule allocation.
[Exposed=Window]
interface CSSStyleRule : CSSGroupingRule {
  attribute DOMString selectorText;
  [SameObject, PutForwards=cssText] readonly attribute CSSStyleProperties style;
};
