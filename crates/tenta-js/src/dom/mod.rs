//! Engine-independent DOM behaviour layered over `tenta_dom` and the generated
//! bindings. Today this is the event subsystem (`events`); more DOM algorithms
//! that must call back into script will join it.

pub mod events;
