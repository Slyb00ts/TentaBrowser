// https://drafts.csswg.org/cssom/#the-medialist-interface
// The live media query list of an `@media` rule.
[Exposed=Window]
interface MediaList {
  stringifier attribute [LegacyNullToEmptyString] DOMString mediaText;
  readonly attribute unsigned long length;
  getter DOMString? item(unsigned long index);
  [Throws] undefined appendMedium(DOMString medium);
  [Throws] undefined deleteMedium(DOMString medium);
};
