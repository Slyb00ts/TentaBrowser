// https://drafts.csswg.org/css-conditional-3/#the-cssmediarule-interface
// `matches` is not here: evaluating the list takes the page's device, which
// this object does not reach.
[Exposed=Window]
interface CSSMediaRule : CSSConditionRule {
  [SameObject, PutForwards=mediaText] readonly attribute MediaList media;
};
