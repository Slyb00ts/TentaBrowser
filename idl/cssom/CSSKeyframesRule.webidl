// https://drafts.csswg.org/css-animations-1/#interface-csskeyframesrule
// `cssRules` and the indexed getter give the rule's keyframes as
// CSSKeyframeRule objects; appendRule() and deleteRule() ignore text that is
// not a keyframe or a selector no keyframe has.
[Exposed=Window]
interface CSSKeyframesRule : CSSRule {
  attribute DOMString name;
  [SameObject] readonly attribute CSSRuleList cssRules;
  readonly attribute unsigned long length;
  getter CSSKeyframeRule (unsigned long index);
  undefined appendRule(DOMString rule);
  undefined deleteRule(DOMString select);
  CSSKeyframeRule? findRule(DOMString select);
};
