// CSS.supports is backed by Stylo's condition and declaration parsers, and
// CSS.escape by cssparser's identifier serializer. The wider CSSOM object
// model is separate P2 work.

[Exposed=Window]
namespace CSS {
  DOMString escape(DOMString ident);
  boolean supports(DOMString conditionText);
  boolean supports(DOMString property, DOMString value);
};
