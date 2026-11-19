// https://drafts.csswg.org/css-nesting-1/#the-cssnestrule
// The declarations that follow a nested rule inside a style rule.
[Exposed=Window]
interface CSSNestedDeclarations : CSSRule {
  [SameObject, PutForwards=cssText] readonly attribute CSSStyleProperties style;
};
