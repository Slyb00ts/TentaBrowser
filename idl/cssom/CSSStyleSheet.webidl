// Constructed sheets and the sheets of `<style>` and `<link>`. The
// CSSStyleSheetInit dictionary and asynchronous replace() are not here yet.
// insertRule() takes any rule Stylo parses, except `@import` (the sheet has
// no loader to fetch one with) and `@namespace`.
[Exposed=Window]
interface CSSStyleSheet : StyleSheet {
  constructor();
  readonly attribute CSSRule? ownerRule;
  [SameObject] readonly attribute CSSRuleList cssRules;
  [Throws] unsigned long insertRule(DOMString rule, optional unsigned long index = 0);
  [Throws] undefined deleteRule(unsigned long index);
  [Throws] undefined replaceSync(USVString text);
};
