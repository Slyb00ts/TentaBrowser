//! The page-side script behind click, type, fill and key presses.
//!
//! It is the CDP host's `interaction.js`, included from that crate's source
//! so both hosts drive one implementation of focus, pointer activation and
//! text editing. Hosts install it as an init script, so `__tentaCdp` exists
//! in the main world before the page's own scripts run. Native focus and
//! editing support replace it inside the script; the entry points used here
//! (`nodeAt`, `focus`, `mouse`, `key`, `insertText`) are the contract.

/// Source of the init script.
pub(crate) const SOURCE: &str = include_str!("../../tenta-cdp/src/interaction.js");

/// The `__tentaCdp` members the MCP tools call, and what each must return.
/// The page-side script belongs to the CDP host and is being reworked; this
/// fails the build of this crate's tests when a member this host relies on
/// goes away, before an agent finds out.
#[cfg(test)]
mod tests {
    use super::SOURCE;

    #[test]
    fn the_script_still_exports_what_the_tools_call() {
        let exported = SOURCE
            .split("defineProperty(global, '__tentaCdp'")
            .nth(1)
            .expect("the script defines __tentaCdp");
        for member in ["focus:", "mouse:", "key:", "insertText:", "nodeAt"] {
            assert!(exported.contains(member), "__tentaCdp lost `{member}`");
        }
    }
}
