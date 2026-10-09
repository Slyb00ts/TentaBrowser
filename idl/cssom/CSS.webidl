// CSS.supports is backed by Stylo's condition and declaration parsers.
// CSS.escape and the wider CSSOM object model are separate P2 work.

[Exposed=Window]
namespace CSS {
  boolean supports(DOMString conditionText);
  boolean supports(DOMString property, DOMString value);
};
