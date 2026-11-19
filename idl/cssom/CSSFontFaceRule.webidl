// https://drafts.csswg.org/css-fonts-4/#om-fontface
// `style` reads the rule's descriptors (`font-family`, `src`, ...). It is read
// only for now: assigning through it changes nothing.
[Exposed=Window]
interface CSSFontFaceRule : CSSRule {
  [SameObject] readonly attribute CSSStyleDeclaration style;
};
