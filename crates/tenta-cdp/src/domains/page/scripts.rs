//! The scripts a session registered for new documents.

use std::sync::Mutex;

use tenta_browser::Page;

use crate::backend::lock;

/// What one session registered. The scripts are the session's, so they go
/// when it does: a client that reconnects does not run its old scripts twice.
#[derive(Default)]
pub(super) struct InitScripts {
    owned: Mutex<Vec<(u64, Page)>>,
}

impl InitScripts {
    pub(super) fn add(&self, page: Page, id: u64) {
        lock(&self.owned).push((id, page));
    }

    /// Unregisters the script `identifier` names, if this session added it.
    pub(super) fn remove(&self, identifier: &str) -> bool {
        let Ok(id) = identifier.parse::<u64>() else {
            return false;
        };
        let mut owned = lock(&self.owned);
        let Some(index) = owned.iter().position(|(owned_id, _)| *owned_id == id) else {
            return false;
        };
        let (id, page) = owned.remove(index);
        page.remove_init_script(id)
    }
}

impl Drop for InitScripts {
    fn drop(&mut self) {
        for (id, page) in lock(&self.owned).drain(..) {
            page.remove_init_script(id);
        }
    }
}
