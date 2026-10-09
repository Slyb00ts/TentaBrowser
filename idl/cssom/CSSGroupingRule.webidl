// CSSStyleRule inherits this prototype in current CSSOM. Nested rule
// mutation is deliberately outside the first supported rule subset.
[Exposed=Window]
interface CSSGroupingRule : CSSRule {
  [SameObject] readonly attribute CSSRuleList cssRules;
  [Throws] unsigned long insertRule(DOMString rule, optional unsigned long index = 0);
  [Throws] undefined deleteRule(unsigned long index);
};
