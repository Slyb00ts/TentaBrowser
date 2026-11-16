// Geometry interfaces for the CSSOM View members (`getBoundingClientRect`,
// `getClientRects`). Member text follows the Geometry Interfaces module
// (<https://drafts.fxtf.org/geometry/>) and CSSOM View.
//
// Not declared: `static fromRect()` and `toJSON()`. The generator has no
// static-operation installation path yet, and `toJSON` returns an `object` the
// host cannot build; `crates/tenta-js/src/geometry.rs` installs both from a
// small script when an engine is created (`GEOMETRY_PRELUDE_SOURCE`).
//
// Deviation: the generator emits an inherited member's accessor in both the
// base and the derived `Ops` trait, which makes every call of it ambiguous, so
// `x`, `y`, `width` and `height` are declared once, on `DOMRectReadOnly`, with
// setters, and a read-only rectangle ignores a write. A page can see the
// difference only by reading the property descriptor of `DOMRectReadOnly`.
//
// `DOMRectList.item()` mints a new `DOMRect` on every call, so two reads of
// `list[0]` are equal rectangles and not the same object.

dictionary DOMRectInit {
  unrestricted double x = 0;
  unrestricted double y = 0;
  unrestricted double width = 0;
  unrestricted double height = 0;
};

[Exposed=Window]
interface DOMRectReadOnly {
  constructor(optional unrestricted double x = 0, optional unrestricted double y = 0,
              optional unrestricted double width = 0, optional unrestricted double height = 0);

  attribute unrestricted double x;
  attribute unrestricted double y;
  attribute unrestricted double width;
  attribute unrestricted double height;
  readonly attribute unrestricted double top;
  readonly attribute unrestricted double right;
  readonly attribute unrestricted double bottom;
  readonly attribute unrestricted double left;
};

[Exposed=Window]
interface DOMRect : DOMRectReadOnly {
  constructor(optional unrestricted double x = 0, optional unrestricted double y = 0,
              optional unrestricted double width = 0, optional unrestricted double height = 0);

};

[Exposed=Window]
interface DOMRectList {
  readonly attribute unsigned long length;
  getter DOMRect? item(unsigned long index);
};
