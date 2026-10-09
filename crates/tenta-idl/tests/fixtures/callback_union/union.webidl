// Callback functions and callback interfaces are distinguishable when the
// function lacks [LegacyTreatNonObjectAsNull]. Declaration order intentionally
// puts the broad object arm first to exercise WebIDL's callable precedence.
callback interface Listener {
  undefined handleEvent();
};
callback Handler = undefined ();

[Exposed=Window]
interface CallbackUnionProbe {
  undefined choose((Listener or Handler) value);
  undefined chooseIterable((Listener or sequence<DOMString>) value);
  undefined choosePlatform((Listener or CallbackUnionProbe) value);
};
