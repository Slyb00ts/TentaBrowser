// The live list behind `document.styleSheets`: every `<style>` element and
// every `<link rel=stylesheet>` whose sheet has loaded, in tree order. Indexed
// access goes through the same getter as `item()`.
[Exposed=Window]
interface StyleSheetList {
  getter CSSStyleSheet? item(unsigned long index);
  readonly attribute unsigned long length;
};
