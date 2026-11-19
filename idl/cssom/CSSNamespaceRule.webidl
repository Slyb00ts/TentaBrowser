// https://drafts.csswg.org/cssom/#the-cssnamespacerule-interface
[Exposed=Window]
interface CSSNamespaceRule : CSSRule {
  readonly attribute DOMString namespaceURI;
  readonly attribute DOMString prefix;
};
