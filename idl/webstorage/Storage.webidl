// Web Storage is exposed on a Window through localStorage and sessionStorage.
// Every operation is fallible because a page or context can close while script
// still retains a Storage wrapper.
[Exposed=Window]
interface Storage {
  [Throws] readonly attribute unsigned long length;
  [Throws] DOMString? key(unsigned long index);
  [Throws] getter DOMString? getItem(DOMString key);
  [Throws] setter undefined setItem(DOMString key, DOMString value);
  [Throws] deleter undefined removeItem(DOMString key);
  [Throws] undefined clear();
};
