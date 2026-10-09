// XMLSerializer's P2 surface from the HTML Standard's dynamic markup
// insertion section
// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html>).
// tenta-dom's XML serializer writes the arena tree and namespace bindings.
// Attr is a Node backed by a host record; XML serialization of an Attr returns
// the empty string without entering the arena serializer.

[Exposed=Window]
interface XMLSerializer {
  constructor();

  DOMString serializeToString(Node root);
};
