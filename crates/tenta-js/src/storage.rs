//! Context and page ownership for the Web Storage areas.
//!
//! This module does not run script while holding the store lock. StorageEvent
//! delivery is separate because page runtimes do not currently survive a
//! navigation as event targets.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::Mutex;
use tenta_core::{BudgetKind, Budgets, Error, Result};
use tenta_net::policy::cookies::{StorageItem, StorageOrigin};
use url::{Origin, Url};

/// Storage Standard's suggested per-area quota. Accounting below is a user
/// agent policy, not a byte-for-byte specification of an engine string heap.
pub const AREA_QUOTA_BYTES: usize = 5 * 1024 * 1024;
const ENTRY_OVERHEAD_BYTES: usize = 64;
const AREA_OVERHEAD_BYTES: usize = 128;
const SESSION_OVERHEAD_BYTES: usize = 64;

/// A failure that a binding maps to a DOMException before script resumes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageError {
    /// The document has no supported, nonopaque storage key, or its page closed.
    Security,
    /// A write would exceed either the area or the context storage budget.
    QuotaExceeded,
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Security => formatter.write_str("storage is unavailable for this document"),
            Self::QuotaExceeded => formatter.write_str("storage quota exceeded"),
        }
    }
}

impl std::error::Error for StorageError {}

/// A single context's local areas and all of its page-session areas.
///
/// The same lock protects item changes and both quotas, so concurrent page
/// threads cannot each accept a write that collectively exceeds the budget.
#[derive(Clone, Debug)]
pub struct ContextStorage {
    inner: Arc<StorageInner>,
}

#[derive(Debug)]
struct StorageInner {
    state: Mutex<Store>,
    max_bytes: usize,
}

#[derive(Debug, Default)]
struct Store {
    local: BTreeMap<String, AreaData>,
    sessions: HashMap<usize, BTreeMap<String, AreaData>>,
    total_bytes: usize,
    closed: bool,
}

#[derive(Clone, Debug, Default)]
struct AreaData {
    // Lexicographic order is an implementation-defined but stable key order.
    entries: BTreeMap<String, String>,
    // Charged items plus the origin string and the area's native metadata.
    bytes: usize,
}

/// One page's session-storage namespace. Clones refer to the same namespace.
#[derive(Clone, Debug)]
pub struct PageStorage {
    context: ContextStorage,
    session: Arc<SessionLease>,
}

#[derive(Debug)]
struct SessionLease {
    id: usize,
    context: Weak<StorageInner>,
    closed: AtomicBool,
}

/// Whether an area is shared by its context or isolated to one page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageKind {
    Local,
    Session,
}

/// A live view of one origin's area. A new view sees concurrent page writes.
#[derive(Clone, Debug)]
pub struct StorageArea {
    page: PageStorage,
    origin: String,
    kind: StorageKind,
}

impl ContextStorage {
    /// The context cap is an independent ceiling on all local and session
    /// items together; the caller derives it from its context-memory budget.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            inner: Arc::new(StorageInner {
                state: Mutex::new(Store::default()),
                max_bytes,
            }),
        }
    }

    /// Builds a context store from the declared storage-plus-cache ceiling.
    /// Cache sharing is tracked separately until one combined meter exists.
    pub fn from_context_budget(budgets: &Budgets) -> Self {
        let bytes = u64::from(budgets.context_memory_mb) * 1024 * 1024;
        Self::new(usize::try_from(bytes).unwrap_or(usize::MAX))
    }

    /// Creates a page-owned session namespace. A cloned page handle shares it;
    /// a separate call creates a new namespace even in the same context.
    pub fn new_page(&self) -> PageStorage {
        let session = Arc::new_cyclic(|weak: &Weak<SessionLease>| SessionLease {
            id: weak.as_ptr() as usize,
            context: Arc::downgrade(&self.inner),
            closed: AtomicBool::new(false),
        });
        PageStorage {
            context: self.clone(),
            session,
        }
    }

    /// Charged bytes across local and live session areas in this context.
    pub fn usage_bytes(&self) -> usize {
        self.inner.state.lock().total_bytes
    }

    /// Exports only local areas, as Playwright's storageState does.
    pub fn export_origins(&self) -> Vec<StorageOrigin> {
        let store = self.inner.state.lock();
        store
            .local
            .iter()
            .filter(|(_, area)| !area.entries.is_empty())
            .map(|(origin, area)| StorageOrigin {
                origin: origin.clone(),
                local_storage: area
                    .entries
                    .iter()
                    .map(|(name, value)| StorageItem {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect(),
            })
            .collect()
    }

    /// Merges a Playwright local-storage payload atomically. The caller must
    /// validate its cookies before applying this import and then load those
    /// already-validated cookies; the two external stores have no shared lock.
    pub fn import_origins(&self, origins: &[StorageOrigin]) -> Result<usize> {
        let mut staged = Vec::with_capacity(origins.len());
        let mut seen_origins = HashSet::with_capacity(origins.len());
        let mut imported_items = 0usize;
        for stored in origins {
            let origin = parse_import_origin(&stored.origin)?;
            if !seen_origins.insert(origin.clone()) {
                return Err(Error::invalid(format!(
                    "duplicate storage origin: {}",
                    stored.origin
                )));
            }
            let mut names = HashSet::with_capacity(stored.local_storage.len());
            for item in &stored.local_storage {
                if !names.insert(item.name.as_str()) {
                    return Err(Error::invalid(format!(
                        "duplicate localStorage key for {origin}: {}",
                        item.name
                    )));
                }
            }
            imported_items = imported_items
                .checked_add(stored.local_storage.len())
                .ok_or_else(|| Error::invalid("too many storage items"))?;
            staged.push((origin, &stored.local_storage));
        }

        let mut store = self.inner.state.lock();
        if store.closed {
            return Err(Error::Closed);
        }
        let mut next_local = store.local.clone();
        let mut next_total = store.total_bytes;
        for (origin, items) in staged {
            for item in items {
                let existing = next_local.get(&origin);
                if existing.and_then(|area| area.entries.get(&item.name)) == Some(&item.value) {
                    continue;
                }
                let previous = existing
                    .and_then(|area| area.entries.get(&item.name))
                    .map(|value| entry_bytes(&item.name, value))
                    .transpose()
                    .map_err(|_| storage_budget_error())?
                    .unwrap_or(0);
                let incoming =
                    entry_bytes(&item.name, &item.value).map_err(|_| storage_budget_error())?;
                let metadata = if existing.is_none() {
                    area_metadata_bytes(&origin).map_err(|_| storage_budget_error())?
                } else {
                    0
                };
                let next_area = existing
                    .map_or(0, |area| area.bytes)
                    .checked_sub(previous)
                    .and_then(|used| used.checked_add(incoming))
                    .and_then(|used| used.checked_add(metadata))
                    .ok_or_else(storage_budget_error)?;
                next_total = next_total
                    .checked_sub(previous)
                    .and_then(|used| used.checked_add(incoming))
                    .and_then(|used| used.checked_add(metadata))
                    .ok_or_else(storage_budget_error)?;
                if next_area > AREA_QUOTA_BYTES || next_total > self.inner.max_bytes {
                    return Err(storage_budget_error());
                }
                let area = next_local.entry(origin.clone()).or_default();
                area.entries.insert(item.name.clone(), item.value.clone());
                area.bytes = next_area;
            }
        }
        store.local = next_local;
        store.total_bytes = next_total;
        Ok(imported_items)
    }

    /// Closes the context and invalidates every area view retained by a page.
    pub fn close(&self) {
        let mut store = self.inner.state.lock();
        store.closed = true;
        store.local.clear();
        store.sessions.clear();
        store.total_bytes = 0;
    }
}

impl PageStorage {
    /// An independent store for a standalone engine without a BrowserContext.
    /// Its empty document has no origin and storage access remains denied
    /// until the caller assigns a final document URL.
    pub fn isolated() -> Self {
        ContextStorage::from_context_budget(&Budgets::platform_default()).new_page()
    }

    /// Returns the current document's local area, keyed by its final URL's
    /// origin. A mutable base URL never participates in this lookup.
    pub fn local_area(
        &self,
        final_url: Option<&Url>,
    ) -> std::result::Result<StorageArea, StorageError> {
        self.area(StorageKind::Local, final_url)
    }

    /// Returns this page's separate session area for the document origin.
    pub fn session_area(
        &self,
        final_url: Option<&Url>,
    ) -> std::result::Result<StorageArea, StorageError> {
        self.area(StorageKind::Session, final_url)
    }

    fn area(
        &self,
        kind: StorageKind,
        final_url: Option<&Url>,
    ) -> std::result::Result<StorageArea, StorageError> {
        if self.session.closed.load(Ordering::Acquire) || self.context.inner.state.lock().closed {
            return Err(StorageError::Security);
        }
        let origin = final_url
            .and_then(storage_origin)
            .ok_or(StorageError::Security)?;
        Ok(StorageArea {
            page: self.clone(),
            origin,
            kind,
        })
    }

    /// Invalidates the page's area handles and drops its session items.
    pub fn close_session(&self) {
        self.session.closed.store(true, Ordering::Release);
        if let Some(context) = self.session.context.upgrade() {
            context.state.lock().remove_session(self.session.id);
        }
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            context.state.lock().remove_session(self.id);
        }
    }
}

impl StorageArea {
    pub fn kind(&self) -> StorageKind {
        self.kind
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn len(&self) -> std::result::Result<usize, StorageError> {
        let store = self.lock_open()?;
        Ok(store
            .area(self.kind, self.page.session.id, &self.origin)
            .map_or(0, |area| area.entries.len()))
    }

    pub fn is_empty(&self) -> std::result::Result<bool, StorageError> {
        Ok(self.len()? == 0)
    }

    pub fn key(&self, index: usize) -> std::result::Result<Option<String>, StorageError> {
        let store = self.lock_open()?;
        Ok(store
            .area(self.kind, self.page.session.id, &self.origin)
            .and_then(|area| area.entries.keys().nth(index).cloned()))
    }

    /// A detached copy for the named-property enumerator; no JavaScript runs
    /// while the context lock is held.
    pub fn keys(&self) -> std::result::Result<Vec<String>, StorageError> {
        let store = self.lock_open()?;
        Ok(store
            .area(self.kind, self.page.session.id, &self.origin)
            .map_or_else(Vec::new, |area| area.entries.keys().cloned().collect()))
    }

    pub fn get_item(&self, name: &str) -> std::result::Result<Option<String>, StorageError> {
        let store = self.lock_open()?;
        Ok(store
            .area(self.kind, self.page.session.id, &self.origin)
            .and_then(|area| area.entries.get(name).cloned()))
    }

    /// Checks named-property support without cloning the stored value.
    pub fn contains_key(&self, name: &str) -> std::result::Result<bool, StorageError> {
        let store = self.lock_open()?;
        Ok(store
            .area(self.kind, self.page.session.id, &self.origin)
            .is_some_and(|area| area.entries.contains_key(name)))
    }

    /// Returns whether the value changed. Identical writes do no work and do
    /// not consume additional quota.
    pub fn set_item(&self, name: &str, value: &str) -> std::result::Result<bool, StorageError> {
        let mut store = self.lock_open()?;
        let existing = store.area(self.kind, self.page.session.id, &self.origin);
        if existing
            .and_then(|area| area.entries.get(name))
            .is_some_and(|old| old == value)
        {
            return Ok(false);
        }
        let previous = existing
            .and_then(|area| area.entries.get(name))
            .map(|old| entry_bytes(name, old))
            .transpose()?
            .unwrap_or(0);
        let incoming = entry_bytes(name, value)?;
        let area_bytes = existing.map_or(0, |area| area.bytes);
        let area_metadata = if existing.is_none() {
            area_metadata_bytes(&self.origin)?
        } else {
            0
        };
        let session_metadata = if self.kind == StorageKind::Session
            && !store.sessions.contains_key(&self.page.session.id)
        {
            SESSION_OVERHEAD_BYTES
        } else {
            0
        };
        let next_area = area_bytes
            .checked_sub(previous)
            .and_then(|used| used.checked_add(incoming))
            .and_then(|used| used.checked_add(area_metadata))
            .ok_or(StorageError::QuotaExceeded)?;
        let next_total = store
            .total_bytes
            .checked_sub(previous)
            .and_then(|used| used.checked_add(incoming))
            .and_then(|used| used.checked_add(area_metadata))
            .and_then(|used| used.checked_add(session_metadata))
            .ok_or(StorageError::QuotaExceeded)?;
        if next_area > AREA_QUOTA_BYTES || next_total > self.page.context.inner.max_bytes {
            return Err(StorageError::QuotaExceeded);
        }
        let new_name = name.to_owned();
        let new_value = value.to_owned();
        let area = store.area_mut(self.kind, self.page.session.id, &self.origin);
        area.entries.insert(new_name, new_value);
        area.bytes = next_area;
        store.total_bytes = next_total;
        Ok(true)
    }

    /// Returns whether a key existed; a missing key is a no-op.
    pub fn remove_item(&self, name: &str) -> std::result::Result<bool, StorageError> {
        let mut store = self.lock_open()?;
        let Some(area) = store.area_mut_existing(self.kind, self.page.session.id, &self.origin)
        else {
            return Ok(false);
        };
        let Some(value) = area.entries.get(name) else {
            return Ok(false);
        };
        let released = entry_bytes(name, value)?;
        if area.entries.len() == 1 {
            let removed = store
                .remove_area(self.kind, self.page.session.id, &self.origin)
                .expect("the area still contains its final item");
            store.total_bytes -= removed.bytes;
            store.remove_empty_session(self.page.session.id);
            return Ok(true);
        }
        area.entries.remove(name);
        area.bytes -= released;
        store.total_bytes -= released;
        Ok(true)
    }

    /// Returns whether any keys existed; clearing an empty area is a no-op.
    pub fn clear(&self) -> std::result::Result<bool, StorageError> {
        let mut store = self.lock_open()?;
        let Some(area) = store.remove_area(self.kind, self.page.session.id, &self.origin) else {
            return Ok(false);
        };
        store.total_bytes -= area.bytes;
        store.remove_empty_session(self.page.session.id);
        Ok(!area.entries.is_empty())
    }

    fn lock_open(&self) -> std::result::Result<parking_lot::MutexGuard<'_, Store>, StorageError> {
        let store = self.page.context.inner.state.lock();
        if store.closed || self.page.session.closed.load(Ordering::Acquire) {
            return Err(StorageError::Security);
        }
        Ok(store)
    }
}

impl Store {
    fn area(&self, kind: StorageKind, session: usize, origin: &str) -> Option<&AreaData> {
        match kind {
            StorageKind::Local => self.local.get(origin),
            StorageKind::Session => self.sessions.get(&session)?.get(origin),
        }
    }

    fn area_mut(&mut self, kind: StorageKind, session: usize, origin: &str) -> &mut AreaData {
        match kind {
            StorageKind::Local => self.local.entry(origin.to_owned()).or_default(),
            StorageKind::Session => self
                .sessions
                .entry(session)
                .or_default()
                .entry(origin.to_owned())
                .or_default(),
        }
    }

    fn area_mut_existing(
        &mut self,
        kind: StorageKind,
        session: usize,
        origin: &str,
    ) -> Option<&mut AreaData> {
        match kind {
            StorageKind::Local => self.local.get_mut(origin),
            StorageKind::Session => self.sessions.get_mut(&session)?.get_mut(origin),
        }
    }

    fn remove_area(&mut self, kind: StorageKind, session: usize, origin: &str) -> Option<AreaData> {
        match kind {
            StorageKind::Local => self.local.remove(origin),
            StorageKind::Session => self.sessions.get_mut(&session)?.remove(origin),
        }
    }

    fn remove_session(&mut self, id: usize) {
        if let Some(areas) = self.sessions.remove(&id) {
            self.total_bytes -=
                areas.values().map(|area| area.bytes).sum::<usize>() + SESSION_OVERHEAD_BYTES;
        }
    }

    fn remove_empty_session(&mut self, id: usize) {
        if self.sessions.get(&id).is_some_and(BTreeMap::is_empty) {
            self.sessions.remove(&id);
            self.total_bytes -= SESSION_OVERHEAD_BYTES;
        }
    }
}

fn storage_origin(url: &Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    match url.origin() {
        Origin::Tuple(..) => Some(url.origin().ascii_serialization()),
        Origin::Opaque(_) => None,
    }
}

fn parse_import_origin(raw: &str) -> Result<String> {
    let url = Url::parse(raw)
        .map_err(|error| Error::invalid(format!("invalid storage origin {raw}: {error}")))?;
    if url.username() != ""
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::invalid(format!("not a bare storage origin: {raw}")));
    }
    storage_origin(&url).ok_or_else(|| Error::invalid(format!("opaque storage origin: {raw}")))
}

fn entry_bytes(name: &str, value: &str) -> std::result::Result<usize, StorageError> {
    resident_string_bytes(name)?
        .checked_add(resident_string_bytes(value)?)
        .and_then(|bytes| bytes.checked_add(ENTRY_OVERHEAD_BYTES))
        .ok_or(StorageError::QuotaExceeded)
}

fn area_metadata_bytes(origin: &str) -> std::result::Result<usize, StorageError> {
    origin
        .len()
        .checked_add(AREA_OVERHEAD_BYTES)
        .ok_or(StorageError::QuotaExceeded)
}

fn resident_string_bytes(value: &str) -> std::result::Result<usize, StorageError> {
    value
        .encode_utf16()
        .count()
        .checked_mul(2)
        .map(|utf16_bytes| utf16_bytes.max(value.len()))
        .ok_or(StorageError::QuotaExceeded)
}

fn storage_budget_error() -> Error {
    Error::BudgetExceeded {
        kind: BudgetKind::ContextMemory,
        component: Some("localStorage import".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_areas_and_session_buckets_do_not_accumulate() {
        let context = ContextStorage::new(64 * 1024 * 1024);
        let page = context.new_page();
        for index in 0..1_000 {
            let final_url = Url::parse(&format!("https://site-{index}.test/path")).unwrap();
            for area in [
                page.local_area(Some(&final_url)).unwrap(),
                page.session_area(Some(&final_url)).unwrap(),
            ] {
                area.set_item("key", "value").unwrap();
                assert!(area.remove_item("key").unwrap());
            }
        }
        let store = context.inner.state.lock();
        assert!(store.local.is_empty());
        assert!(store.sessions.is_empty());
        assert_eq!(store.total_bytes, 0);
    }

    #[test]
    fn charge_utf8_residence_when_larger_than_utf16() {
        assert_eq!(resident_string_bytes("漢").unwrap(), 3);
        assert_eq!(resident_string_bytes("a").unwrap(), 2);
    }
}
