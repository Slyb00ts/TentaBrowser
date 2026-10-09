// Constructed sheets and live top-level style rules. The CSSStyleSheetInit
// dictionary, asynchronous replace(), and attached-sheet loading follow in a
// separate CSSOM package. Valid unsupported rule kinds are rejected before a
// mutation rather than exposed through an incorrect generic CSSRule wrapper.
[Exposed=Window]
interface CSSStyleSheet : StyleSheet {
  constructor();
  readonly attribute CSSRule? ownerRule;
  [SameObject] readonly attribute CSSRuleList cssRules;
  [Throws] unsigned long insertRule(DOMString rule, optional unsigned long index = 0);
  [Throws] undefined deleteRule(unsigned long index);
  [Throws] undefined replaceSync(USVString text);
};
