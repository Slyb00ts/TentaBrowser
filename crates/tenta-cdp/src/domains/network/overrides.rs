//! The page-wide network settings sessions have set on one page.
//!
//! Each setting keeps one entry per session, the newest entry is the one in
//! force, and a session that leaves removes only its own entries: what another
//! session set stays, and the last one out puts the default back. It is the
//! pattern of `crate::overrides` for the viewport, per setting.

use tenta_browser::network::NetworkHub;

#[derive(Default)]
struct Stack<T> {
    entries: Vec<(usize, T)>,
}

impl<T: Clone + Default> Stack<T> {
    fn set(&mut self, owner: usize, value: T) {
        self.entries.retain(|(known, _)| *known != owner);
        self.entries.push((owner, value));
    }

    fn clear(&mut self, owner: usize) {
        self.entries.retain(|(known, _)| *known != owner);
    }

    fn current(&self) -> T {
        self.entries
            .last()
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }
}

#[derive(Default)]
pub(crate) struct NetOverrides {
    headers: Stack<Vec<(String, String)>>,
    agent: Stack<(Option<String>, Option<String>)>,
    cache_disabled: Stack<bool>,
    offline: Stack<bool>,
}

impl NetOverrides {
    pub(crate) fn set_headers(&mut self, owner: usize, headers: Vec<(String, String)>) {
        self.headers.set(owner, headers);
    }

    pub(crate) fn set_agent(&mut self, owner: usize, agent: (Option<String>, Option<String>)) {
        self.agent.set(owner, agent);
    }

    pub(crate) fn set_cache_disabled(&mut self, owner: usize, disabled: bool) {
        self.cache_disabled.set(owner, disabled);
    }

    pub(crate) fn set_offline(&mut self, owner: usize, offline: bool) {
        self.offline.set(owner, offline);
    }

    /// Removes everything `owner` set.
    pub(crate) fn clear(&mut self, owner: usize) {
        self.headers.clear(owner);
        self.agent.clear(owner);
        self.cache_disabled.clear(owner);
        self.offline.clear(owner);
    }

    /// Puts the settings in force on the page.
    pub(crate) fn apply(&self, hub: &NetworkHub) {
        hub.set_extra_headers(self.headers.current());
        let (agent, language) = self.agent.current();
        hub.set_user_agent(agent, language);
        hub.set_cache_disabled(self.cache_disabled.current());
        hub.set_offline(self.offline.current());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leaving_session_only_undoes_its_own_settings() {
        let mut overrides = NetOverrides::default();
        overrides.set_offline(1, true);
        overrides.set_headers(2, vec![("x".to_owned(), "1".to_owned())]);
        overrides.clear(2);
        assert!(overrides.offline.current(), "session 1's setting stays");
        assert!(overrides.headers.current().is_empty());
        overrides.set_offline(2, false);
        assert!(!overrides.offline.current(), "the newest entry wins");
        overrides.clear(2);
        assert!(overrides.offline.current(), "and hands over when it leaves");
        overrides.clear(1);
        assert!(!overrides.offline.current());
    }
}
