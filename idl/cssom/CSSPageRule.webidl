// https://drafts.csswg.org/cssom/#the-csspagerule-interface
// The interface object only, as for CSSImportRule: Stylo's Servo
// configuration does not parse `@page`, so no rule is one.
[Exposed=Window]
interface CSSPageRule : CSSGroupingRule {
};
