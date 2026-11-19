// https://drafts.csswg.org/css-animations-1/#interface-csskeyframerule
// One keyframe of an `@keyframes` rule. Assigning a `keyText` that is not a
// list of keyframe selectors throws SyntaxError.
[Exposed=Window]
interface CSSKeyframeRule : CSSRule {
  [Throws] attribute DOMString keyText;
  [SameObject, PutForwards=cssText] readonly attribute CSSStyleProperties style;
};
