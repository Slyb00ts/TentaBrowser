//! One module per protocol domain. A module owns its handler and its
//! `register` function; this file only lists them, so a domain package edits
//! its own file and nothing shared.

use crate::domain::DomainRegistry;

mod audits;
mod browser;
mod css;
mod dom;
mod emulation;
mod fetch;
mod geometry;
mod input;
mod inspector;
mod log;
pub(crate) mod network;
mod nodes;
mod page;
mod performance;
mod runtime;
mod security;
mod storage;
mod target;

pub(crate) fn register_all(registry: &mut DomainRegistry) {
    audits::register(registry);
    browser::register(registry);
    css::register(registry);
    dom::register(registry);
    emulation::register(registry);
    fetch::register(registry);
    input::register(registry);
    inspector::register(registry);
    log::register(registry);
    network::register(registry);
    page::register(registry);
    performance::register(registry);
    runtime::register(registry);
    security::register(registry);
    storage::register(registry);
    target::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Commands the clients do not send but the transport needs.
    const OUTSIDE_THE_CHECKLIST: &[&str] = &[
        "Target.getTargets",
        "Target.activateTarget",
        "Inspector.enable",
        "Inspector.disable",
        "Page.disable",
        "Page.stopLoading",
        "Log.disable",
        "Runtime.releaseObjectGroup",
        "Runtime.discardConsoleEntries",
        "Audits.enable",
        "Audits.disable",
        "DOM.getDocument",
        "DOM.querySelector",
        "DOM.querySelectorAll",
        "DOM.requestNode",
        "DOM.getOuterHTML",
        "DOM.getNodeForLocation",
        "DOM.focus",
        "Network.disable",
        "Network.getAllCookies",
        "Network.setCookie",
        "Network.clearBrowserCookies",
        "Performance.disable",
        "Security.enable",
        "Security.disable",
    ];

    #[test]
    fn every_registered_command_is_one_a_client_uses() {
        let registry = DomainRegistry::with_defaults();
        for (domain, method) in registry.methods() {
            let name = format!("{domain}.{method}");
            assert!(
                crate::METHODS.contains(&name.as_str())
                    || OUTSIDE_THE_CHECKLIST.contains(&name.as_str()),
                "{name} is not in the client checklist"
            );
        }
    }
}
