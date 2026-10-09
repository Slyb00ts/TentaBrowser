// DOMParser's P2 surface from the HTML Standard's dynamic markup insertion
// section (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html>).
// The source parameter is DOMString rather than (TrustedHTML or DOMString):
// Trusted Types is not available in this binding layer yet. The five MIME
// types, their enum conversion, and the returned Document are otherwise the
// standard's. Parsed documents are inert and owned by the caller's world.

enum DOMParserSupportedType {
  "text/html",
  "text/xml",
  "application/xml",
  "application/xhtml+xml",
  "image/svg+xml"
};

[Exposed=Window]
interface DOMParser {
  constructor();

  [NewObject] Document parseFromString(DOMString string, DOMParserSupportedType type);
};
