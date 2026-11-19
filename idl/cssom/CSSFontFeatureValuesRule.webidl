// https://drafts.csswg.org/css-fonts-4/#om-fontfeaturevalues
// The feature value maps are not exposed; `cssText` has them.
[Exposed=Window]
interface CSSFontFeatureValuesRule : CSSRule {
  readonly attribute DOMString fontFamily;
};
