//! The native ledger belongs to a Page, not one navigation runtime or a
//! reusable PageConfig's settings.

mod support;

use support::Harness;
use tenta_page::Settle;

#[tokio::test(flavor = "current_thread")]
async fn factory_receives_one_ledger_across_navigation_and_distinct_pages() {
    let body = "<!doctype html><html><body><script>first</script></body></html>";
    let first = Harness::new(body);
    first.settle(Settle::Load).await.expect("first navigation");
    first.settle(Settle::Load).await.expect("second navigation");
    let first_budgets = first.record.lock().unwrap().native_memories.clone();
    assert_eq!(first_budgets.len(), 2, "the factory built two runtimes");
    assert!(
        first_budgets[0].same_page(&first_budgets[1]),
        "a navigation must not reset native accounting"
    );

    let second = Harness::new(body);
    second.settle(Settle::Load).await.expect("another page");
    let second_budgets = second.record.lock().unwrap().native_memories.clone();
    assert_eq!(second_budgets.len(), 1);
    assert!(
        !first_budgets[0].same_page(&second_budgets[0]),
        "the same settings must not share accounting across pages"
    );
}
