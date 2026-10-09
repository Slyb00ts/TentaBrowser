use std::sync::{Arc, Barrier};
use std::thread;

use tenta_js::storage::{AREA_QUOTA_BYTES, ContextStorage, StorageError};
use tenta_net::policy::cookies::{StorageItem, StorageOrigin};
use url::Url;

fn url(value: &str) -> Url {
    Url::parse(value).unwrap()
}

#[test]
fn origin_context_and_page_session_partitioning() {
    let context = ContextStorage::new(64 * 1024 * 1024);
    let first = context.new_page();
    let second = context.new_page();
    let other_context = ContextStorage::new(64 * 1024 * 1024).new_page();
    let a = url("https://EXAMPLE.test:443/path");
    let same_a = url("https://example.test/elsewhere");
    let b = url("https://example.test:444/");
    let http_a = url("http://example.test/");

    let first_local = first.local_area(Some(&a)).unwrap();
    let first_session = first.session_area(Some(&a)).unwrap();
    assert_eq!(first_local.origin(), "https://example.test");
    assert!(first_local.set_item("key", "local").unwrap());
    assert!(first_session.set_item("key", "session").unwrap());
    assert_eq!(
        first.local_area(Some(&b)).unwrap().get_item("key").unwrap(),
        None
    );
    assert_eq!(
        first
            .local_area(Some(&http_a))
            .unwrap()
            .get_item("key")
            .unwrap(),
        None
    );
    assert_eq!(
        second
            .local_area(Some(&same_a))
            .unwrap()
            .get_item("key")
            .unwrap()
            .as_deref(),
        Some("local")
    );
    assert_eq!(
        second
            .session_area(Some(&same_a))
            .unwrap()
            .get_item("key")
            .unwrap(),
        None
    );
    assert_eq!(
        other_context
            .local_area(Some(&same_a))
            .unwrap()
            .get_item("key")
            .unwrap(),
        None
    );

    // A -> B -> A in one page retains the first origin's session area.
    first
        .session_area(Some(&b))
        .unwrap()
        .set_item("key", "other")
        .unwrap();
    assert_eq!(
        first
            .session_area(Some(&same_a))
            .unwrap()
            .get_item("key")
            .unwrap()
            .as_deref(),
        Some("session")
    );
    first.close_session();
    assert_eq!(first_session.get_item("key"), Err(StorageError::Security));
    assert_eq!(first_local.get_item("key"), Err(StorageError::Security));
    assert_eq!(
        second
            .local_area(Some(&a))
            .unwrap()
            .get_item("key")
            .unwrap()
            .as_deref(),
        Some("local")
    );
    assert_eq!(
        second
            .session_area(Some(&a))
            .unwrap()
            .get_item("key")
            .unwrap(),
        None
    );
}

#[test]
fn opaque_and_missing_final_urls_fail_closed() {
    let page = ContextStorage::new(1024).new_page();
    assert!(matches!(page.local_area(None), Err(StorageError::Security)));
    assert!(matches!(
        page.session_area(Some(&url("data:text/plain,x"))),
        Err(StorageError::Security)
    ));
    assert!(matches!(
        page.local_area(Some(&url("about:blank"))),
        Err(StorageError::Security)
    ));
    assert!(page.local_area(Some(&url("https://example.test/"))).is_ok());
}

#[test]
fn quotas_and_no_ops_are_atomic() {
    let context = ContextStorage::new(AREA_QUOTA_BYTES + 1024);
    let page = context.new_page();
    let area = page.local_area(Some(&url("https://example.test"))).unwrap();
    assert!(area.set_item("a", "v").unwrap());
    let used = context.usage_bytes();
    assert!(!area.set_item("a", "v").unwrap());
    assert_eq!(context.usage_bytes(), used);
    assert_eq!(
        area.set_item("huge", &"x".repeat(AREA_QUOTA_BYTES)),
        Err(StorageError::QuotaExceeded)
    );
    assert_eq!(area.len().unwrap(), 1);
    assert_eq!(area.get_item("huge").unwrap(), None);
    assert_eq!(context.usage_bytes(), used);
    assert!(!area.remove_item("missing").unwrap());
    assert!(area.remove_item("a").unwrap());
    assert!(!area.clear().unwrap());
    assert_eq!(context.usage_bytes(), 0);
}

#[test]
fn concurrent_writers_cannot_exceed_context_budget() {
    // Each one-character key/value pair is charged 64 + 2 * (1 + 1) bytes.
    // The sole area's origin and metadata add 20 + 128 bytes.
    let context = ContextStorage::new(20 + 128 + 4 * 68);
    let area = context
        .new_page()
        .local_area(Some(&url("https://example.test")))
        .unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|index| {
            let area = area.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let name = char::from(b'a' + index as u8).to_string();
                area.set_item(&name, "v")
            })
        })
        .collect();
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| result == &&Ok(true))
            .count(),
        4
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| result == &&Err(StorageError::QuotaExceeded))
            .count(),
        4
    );
    assert_eq!(area.len().unwrap(), 4);
    assert_eq!(context.usage_bytes(), 20 + 128 + 4 * 68);
}

#[test]
fn competing_origins_share_one_atomic_context_budget() {
    let origin_a = url("https://a.test");
    let origin_b = url("https://b.test");
    // Each origin costs 128 + 14 bytes; each item costs 68 bytes.
    let context = ContextStorage::new(2 * (128 + 14) + 4 * 68);
    let page = context.new_page();
    let first = page.local_area(Some(&origin_a)).unwrap();
    let second = page.local_area(Some(&origin_b)).unwrap();
    first.set_item("x", "v").unwrap();
    second.set_item("x", "v").unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|index| {
            let area = if index % 2 == 0 {
                first.clone()
            } else {
                second.clone()
            };
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let name = char::from(b'a' + index as u8).to_string();
                area.set_item(&name, "v")
            })
        })
        .collect();
    let successes = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .filter(Result::is_ok)
        .count();
    assert_eq!(successes, 2);
    assert_eq!(first.len().unwrap() + second.len().unwrap(), 4);
    assert_eq!(context.usage_bytes(), 2 * (128 + 14) + 4 * 68);
}

#[test]
fn storage_state_import_is_canonical_atomic_and_local_only() {
    let context = ContextStorage::new(1024);
    let page = context.new_page();
    let origin = url("https://example.test/");
    page.local_area(Some(&origin))
        .unwrap()
        .set_item("old", "value")
        .unwrap();
    page.session_area(Some(&origin))
        .unwrap()
        .set_item("session", "private")
        .unwrap();
    let original_usage = context.usage_bytes();

    let good = StorageOrigin {
        origin: "https://EXAMPLE.test:443".into(),
        local_storage: vec![StorageItem {
            name: "new".into(),
            value: "ok".into(),
        }],
    };
    let duplicate = StorageOrigin {
        origin: "https://example.test".into(),
        local_storage: vec![],
    };
    assert!(context.import_origins(&[good.clone(), duplicate]).is_err());
    assert_eq!(context.usage_bytes(), original_usage);
    assert_eq!(
        page.local_area(Some(&origin))
            .unwrap()
            .get_item("new")
            .unwrap(),
        None
    );

    let invalid = StorageOrigin {
        origin: "https://other.test/path".into(),
        local_storage: vec![],
    };
    assert!(context.import_origins(&[good.clone(), invalid]).is_err());
    assert_eq!(context.usage_bytes(), original_usage);

    let over_quota = StorageOrigin {
        origin: "https://other.test".into(),
        local_storage: vec![StorageItem {
            name: "large".into(),
            value: "x".repeat(AREA_QUOTA_BYTES),
        }],
    };
    assert!(context.import_origins(&[good.clone(), over_quota]).is_err());
    assert_eq!(context.usage_bytes(), original_usage);
    assert_eq!(
        page.local_area(Some(&origin))
            .unwrap()
            .get_item("new")
            .unwrap(),
        None
    );

    assert_eq!(context.import_origins(&[good]).unwrap(), 1);
    let exported = context.export_origins();
    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].origin, "https://example.test");
    assert_eq!(exported[0].local_storage.len(), 2);
    assert!(
        exported[0]
            .local_storage
            .iter()
            .all(|item| item.name != "session")
    );
    assert_eq!(
        page.session_area(Some(&origin))
            .unwrap()
            .get_item("session")
            .unwrap()
            .as_deref(),
        Some("private")
    );
}

#[test]
fn page_and_context_close_release_items_and_invalidate_views() {
    let context = ContextStorage::new(1024);
    let first = context.new_page();
    let second = context.new_page();
    let origin = url("https://example.test");
    let first_session = first.session_area(Some(&origin)).unwrap();
    let second_session = second.session_area(Some(&origin)).unwrap();
    first_session.set_item("a", "v").unwrap();
    second_session.set_item("b", "v").unwrap();
    let both_usage = context.usage_bytes();
    drop(first);
    drop(first_session);
    assert!(context.usage_bytes() < both_usage);
    assert_eq!(second_session.get_item("b").unwrap().as_deref(), Some("v"));
    context.close();
    assert_eq!(context.usage_bytes(), 0);
    assert_eq!(second_session.get_item("b"), Err(StorageError::Security));
}
