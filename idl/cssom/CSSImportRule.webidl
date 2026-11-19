// https://drafts.csswg.org/cssom/#the-cssimportrule-interface
// The interface object only, so that `rule instanceof CSSImportRule` is an
// expression script can evaluate. No rule is one yet: the sheets script sees
// are parsed without a loader, which leaves `@import` out of `cssRules`.
[Exposed=Window]
interface CSSImportRule : CSSRule {
};
