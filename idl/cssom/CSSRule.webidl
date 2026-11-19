// The base of every rule object. A rule read from `cssRules` is an object of
// the interface for its kind (CSSStyleRule, CSSMediaRule, ...); `type` is the
// legacy number of that kind, and 0 for the kinds that never had one.
[Exposed=Window]
interface CSSRule {
  attribute DOMString cssText;
  readonly attribute CSSRule? parentRule;
  readonly attribute CSSStyleSheet? parentStyleSheet;
  readonly attribute unsigned short type;
  const unsigned short STYLE_RULE = 1;
  const unsigned short CHARSET_RULE = 2;
  const unsigned short IMPORT_RULE = 3;
  const unsigned short MEDIA_RULE = 4;
  const unsigned short FONT_FACE_RULE = 5;
  const unsigned short PAGE_RULE = 6;
  const unsigned short KEYFRAMES_RULE = 7;
  const unsigned short KEYFRAME_RULE = 8;
  const unsigned short MARGIN_RULE = 9;
  const unsigned short NAMESPACE_RULE = 10;
  const unsigned short COUNTER_STYLE_RULE = 11;
  const unsigned short SUPPORTS_RULE = 12;
  const unsigned short FONT_FEATURE_VALUES_RULE = 14;
};
