// Live, read-only list. Indexed access uses the same getter path as item().
[Exposed=Window]
interface CSSRuleList {
  getter CSSRule? item(unsigned long index);
  readonly attribute unsigned long length;
};
