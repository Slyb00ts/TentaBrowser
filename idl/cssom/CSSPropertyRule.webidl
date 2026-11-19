// https://drafts.css-houdini.org/css-properties-values-api-1/#the-css-property-rule-interface
[Exposed=Window]
interface CSSPropertyRule : CSSRule {
  readonly attribute DOMString name;
  readonly attribute DOMString syntax;
  readonly attribute boolean inherits;
  readonly attribute DOMString? initialValue;
};
