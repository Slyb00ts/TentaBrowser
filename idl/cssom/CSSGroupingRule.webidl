// A rule with a list of child rules: a style rule (CSS Nesting) and the
// conditional and layering at-rules. insertRule() parses its rule as it would
// be parsed in that place, so `&` is valid under a style rule.
[Exposed=Window]
interface CSSGroupingRule : CSSRule {
  [SameObject] readonly attribute CSSRuleList cssRules;
  [Throws] unsigned long insertRule(DOMString rule, optional unsigned long index = 0);
  [Throws] undefined deleteRule(unsigned long index);
};
