//! The page-side script behind the `DOM` and `Input` domains.
//!
//! `interaction.js` documents what it supplies and why. Every page the server
//! creates runs it before the document's own scripts, so the domains can call
//! `__tentaCdp` in the main world at any time.

/// Source of the init script.
pub(crate) const SOURCE: &str = include_str!("interaction.js");
