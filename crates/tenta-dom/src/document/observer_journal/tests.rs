use super::*;
use tenta_core::NodeRef;
use tenta_core::atom::atoms;

struct Fixture {
    document: crate::Document,
    pins: super::super::observer_pins::ObserverPins,
    budget: tenta_core::NativeMemoryBudget,
    nodes: Vec<ObserverNode>,
}

impl Fixture {
    fn new() -> Self {
        let mut document = crate::Document::new();
        let mut nodes = vec![ObserverNode::new(
            document.document_id(),
            document.node_ref(document.document_node()),
        )];
        for _ in 1..=32 {
            let id = document.create_html_element(atoms::DIV);
            nodes.push(ObserverNode::new(
                document.document_id(),
                document.node_ref(id),
            ));
        }
        let budget = tenta_core::NativeMemoryBudget::with_limits(
            32 * 1024 * 1024,
            32 * 1024 * 1024,
            32 * 1024 * 1024,
            0,
        );
        Self {
            document,
            pins: super::super::observer_pins::ObserverPins::new(budget.clone()),
            budget,
            nodes,
        }
    }

    fn node(&self, id: u32) -> ObserverNode {
        self.nodes[id as usize]
    }

    fn queue(&mut self, journal: &mut ObserverJournal, facts: MutationFacts) {
        let prepared = journal.prepare(&facts).unwrap();
        self.commit_prepared(journal, vec![prepared]);
    }

    fn commit_prepared(
        &mut self,
        journal: &mut ObserverJournal,
        mut mutations: Vec<PreparedMutation>,
    ) {
        journal.reserve_for_commits(&mut mutations).unwrap();
        let owned = super::super::observer_ownership::acquire_prepared_operation(
            &self.budget,
            &mut self.pins,
            &mut self.document,
            mutations,
        )
        .unwrap();
        for mutation in owned {
            journal.commit(mutation);
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.pins.release_all(&mut self.document);
    }
}

fn child_options(subtree: bool) -> ObserveOptions {
    ObserveOptions::normalize(ObserveOptionsInput {
        child_list: true,
        subtree,
        ..ObserveOptionsInput::default()
    })
    .unwrap()
}

fn attribute_options(subtree: bool, old_value: bool) -> ObserveOptions {
    ObserveOptions::normalize(ObserveOptionsInput {
        attributes: Some(true),
        subtree,
        attribute_old_value: Some(old_value),
        ..ObserveOptionsInput::default()
    })
    .unwrap()
}

#[test]
fn option_presence_infers_types_and_explicit_false_is_checked() {
    let options = ObserveOptions::normalize(ObserveOptionsInput {
        attribute_old_value: Some(false),
        ..ObserveOptionsInput::default()
    })
    .unwrap();
    assert!(options.attributes);
    assert!(!options.attribute_old_value);
    assert!(
        ObserveOptions::normalize(ObserveOptionsInput {
            attribute_filter: Some(Vec::new()),
            ..ObserveOptionsInput::default()
        })
        .unwrap()
        .attributes
    );
    assert!(
        ObserveOptions::normalize(ObserveOptionsInput {
            character_data_old_value: Some(false),
            ..ObserveOptionsInput::default()
        })
        .unwrap()
        .character_data
    );
    assert_eq!(
        ObserveOptions::normalize(ObserveOptionsInput::default()),
        Err(OptionsError::NoMutationType)
    );
    assert_eq!(
        ObserveOptions::normalize(ObserveOptionsInput {
            attributes: Some(false),
            attribute_filter: Some(Vec::new()),
            child_list: true,
            ..ObserveOptionsInput::default()
        }),
        Err(OptionsError::AttributesDisabled)
    );
    assert_eq!(
        ObserveOptions::normalize(ObserveOptionsInput {
            character_data: Some(false),
            character_data_old_value: Some(true),
            child_list: true,
            ..ObserveOptionsInput::default()
        }),
        Err(OptionsError::CharacterDataDisabled)
    );
}

#[test]
fn registration_filters_retain_and_release_their_page_charge() {
    let fixture = Fixture::new();
    let budget = fixture.budget.clone();
    let root = fixture.node(1);
    let removed = fixture.node(2);
    let observer = ObserverId(1);
    let mut journal = ObserverJournal::new(budget.clone());
    journal
        .observe(observer, root, attribute_options(true, false))
        .unwrap();
    let unfiltered = budget.used(tenta_core::NativeMemoryComponent::MutationObserver);

    let mut filtered = attribute_options(true, false);
    filtered.attribute_filter = Some(vec![atoms::ID; 128]);
    journal.observe(observer, root, filtered).unwrap();
    let filter_bytes = 128 * std::mem::size_of::<tenta_core::Atom>();
    let registered = budget.used(tenta_core::NativeMemoryComponent::MutationObserver);
    assert!(registered >= unfiltered + filter_bytes);

    let transients = journal.prepare_transients(removed, &[root]).unwrap();
    assert!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver)
            >= registered + filter_bytes
    );
    drop(transients);
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        registered
    );

    journal
        .observe(observer, root, attribute_options(true, false))
        .unwrap();
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        unfiltered
    );
    drop(journal);
    drop(fixture);
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
}

#[test]
fn prepared_record_buffers_keep_their_charge_until_preflight_ends() {
    let fixture = Fixture::new();
    let budget = fixture.budget.clone();
    let target = fixture.node(1);
    let mut journal = ObserverJournal::new(budget.clone());
    journal
        .observe(ObserverId(75), target, attribute_options(false, true))
        .unwrap();
    let before = budget.used(tenta_core::NativeMemoryComponent::MutationObserver);
    let facts = MutationFacts::attributes(
        target,
        vec![target],
        atoms::CLASS,
        None,
        Some("old-value".repeat(64)),
    );
    let prepared = journal.prepare(&facts).unwrap();
    assert_eq!(prepared.records().count(), 1);
    assert!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver)
            > before + "old-value".len() * 64
    );
    drop(prepared);
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        before
    );
}

#[test]
fn refused_prepared_record_drops_scratch_without_queuing() {
    let fixture = Fixture::new();
    let budget = tenta_core::NativeMemoryBudget::with_limits(4096, 4096, 4096, 0);
    let target = fixture.node(1);
    let observer = ObserverId(76);
    let mut journal = ObserverJournal::new(budget.clone());
    journal
        .observe(observer, target, attribute_options(false, true))
        .unwrap();
    let before = budget.used(tenta_core::NativeMemoryComponent::MutationObserver);
    let facts = MutationFacts::attributes(
        target,
        vec![target],
        atoms::CLASS,
        None,
        Some("x".repeat(8192)),
    );
    assert!(matches!(journal.prepare(&facts), Err(JournalError::Budget)));
    assert_eq!(journal.queued_count(observer), 0);
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        before
    );
}

#[test]
fn rejected_filter_update_keeps_the_previous_registration() {
    let fixture = Fixture::new();
    let budget = tenta_core::NativeMemoryBudget::with_limits(4096, 4096, 4096, 0);
    let mut journal = ObserverJournal::new(budget.clone());
    let observer = ObserverId(1);
    let target = fixture.node(1);
    journal
        .observe(observer, target, attribute_options(false, false))
        .unwrap();
    let used = budget.used(tenta_core::NativeMemoryComponent::MutationObserver);

    let mut filtered = attribute_options(false, false);
    filtered.attribute_filter = Some(vec![atoms::ID; 4096]);
    assert_eq!(
        journal.observe(observer, target, filtered),
        Err(JournalError::Budget)
    );
    assert!(journal.may_observe_path([target], RecordType::Attributes, Some(atoms::CLASS), None,));
    assert_eq!(journal.registration_count, 1);
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        used
    );
}

#[test]
fn uninterested_path_is_rejected_without_materializing_ancestry_or_old_value() {
    let fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let root = fixture.node(1);
    let target = fixture.node(2);
    let name = tenta_core::atom::atoms::ID;
    journal
        .observe(ObserverId(1), root, attribute_options(false, true))
        .unwrap();
    assert!(!journal.may_observe_path([target, root], RecordType::Attributes, Some(name), None,));
    journal
        .observe(ObserverId(1), root, attribute_options(true, true))
        .unwrap();
    assert!(journal.may_observe_path([target, root], RecordType::Attributes, Some(name), None,));
    assert!(!journal.may_observe_path([target, root], RecordType::ChildList, None, None,));
}

#[test]
fn pending_order_follows_first_record_not_observer_creation() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    journal
        .observe(ObserverId(1), fixture.node(1), child_options(false))
        .unwrap();
    journal
        .observe(ObserverId(2), fixture.node(2), child_options(false))
        .unwrap();
    fixture.queue(
        &mut journal,
        MutationFacts::child_list(
            fixture.node(2),
            vec![fixture.node(2)],
            vec![fixture.node(3)],
            vec![],
            None,
            None,
        ),
    );
    fixture.queue(
        &mut journal,
        MutationFacts::child_list(
            fixture.node(1),
            vec![fixture.node(1)],
            vec![fixture.node(4)],
            vec![],
            None,
            None,
        ),
    );
    fixture.queue(
        &mut journal,
        MutationFacts::child_list(
            fixture.node(2),
            vec![fixture.node(2)],
            vec![fixture.node(5)],
            vec![],
            None,
            None,
        ),
    );
    assert_eq!(
        journal.begin_notify().into_iter().collect::<Vec<_>>(),
        vec![ObserverId(2), ObserverId(1)]
    );
    assert_eq!(journal.drain_for_notify(ObserverId(2)).len(), 2);
    assert_eq!(journal.drain_for_notify(ObserverId(1)).len(), 1);
}

#[test]
fn another_record_for_a_pending_observer_does_not_grow_the_pending_fifo() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::new(fixture.budget.clone());
    for id in 1..=4 {
        journal
            .observe(
                ObserverId(id as u64),
                fixture.node(id),
                child_options(false),
            )
            .unwrap();
        fixture.queue(
            &mut journal,
            MutationFacts::child_list(
                fixture.node(id),
                vec![fixture.node(id)],
                vec![fixture.node(id + 4)],
                vec![],
                None,
                None,
            ),
        );
    }
    assert_eq!(journal.pending.len(), 4);
    let pending_bytes = journal.pending.retained_buffer_bytes();
    fixture.queue(
        &mut journal,
        MutationFacts::child_list(
            fixture.node(1),
            vec![fixture.node(1)],
            vec![fixture.node(9)],
            vec![],
            None,
            None,
        ),
    );
    assert_eq!(journal.pending.len(), 4);
    assert_eq!(journal.pending.retained_buffer_bytes(), pending_bytes);
}

#[test]
fn one_native_operation_reserves_all_records_for_the_same_observer() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(53);
    journal
        .observe(observer, fixture.node(1), child_options(true))
        .unwrap();
    let mutations = [
        journal
            .prepare(&MutationFacts::child_list(
                fixture.node(2),
                vec![fixture.node(2), fixture.node(1)],
                vec![],
                vec![fixture.node(3)],
                None,
                None,
            ))
            .unwrap(),
        journal
            .prepare(&MutationFacts::child_list(
                fixture.node(1),
                vec![fixture.node(1)],
                vec![fixture.node(3)],
                vec![],
                None,
                None,
            ))
            .unwrap(),
    ];
    fixture.commit_prepared(&mut journal, Vec::from(mutations));
    assert_eq!(journal.queued_count(observer), 2);
    assert_eq!(
        journal.begin_notify().into_iter().collect::<Vec<_>>(),
        vec![observer]
    );
}

#[test]
fn teardown_releases_undelivered_records_and_retains_a_failed_owner_for_retry() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(56);
    let target = fixture.node(1);
    journal
        .observe(observer, target, attribute_options(false, true))
        .unwrap();
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            target,
            vec![target],
            atoms::CLASS,
            None,
            Some("before".to_owned()),
        ),
    );
    let used = fixture
        .budget
        .used(tenta_core::NativeMemoryComponent::MutationObserver);
    assert!(used > 0);
    assert_eq!(journal.queued_count(observer), 1);
    assert!(
        journal
            .release_queued_without_delivery(|_| Err(tenta_core::Error::invalid(
                "forced release failure"
            )))
            .is_err()
    );
    assert_eq!(journal.queued_count(observer), 1);
    assert_eq!(
        fixture
            .budget
            .used(tenta_core::NativeMemoryComponent::MutationObserver),
        used
    );
    journal
        .release_queued_without_delivery(|record| {
            record.release(&mut fixture.pins, &mut fixture.document)
        })
        .unwrap();
    assert_eq!(journal.queued_count(observer), 0);
    assert!(fixture.pins.is_empty());
    let budget = fixture.budget.clone();
    drop(fixture);
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
}

#[test]
fn taken_record_queue_keeps_its_budget_after_journal_drops() {
    let mut fixture = Fixture::new();
    let observer = ObserverId(57);
    let target = fixture.node(1);
    let mut journal = ObserverJournal::new(fixture.budget.clone());
    journal
        .observe(observer, target, attribute_options(false, true))
        .unwrap();
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(target, vec![target], atoms::CLASS, None, None),
    );
    let records = journal.take_records(observer);
    assert_eq!(records.len(), 1);
    assert!(records.retained_buffer_bytes() > 0);
    drop(journal);
    assert!(
        fixture
            .budget
            .used(tenta_core::NativeMemoryComponent::MutationObserver)
            >= records.retained_buffer_bytes(),
        "the moved queue retains its own native buffer charge"
    );
    for mut record in records {
        record
            .release(&mut fixture.pins, &mut fixture.document)
            .unwrap();
    }
    assert!(fixture.pins.is_empty());
    let budget = fixture.budget.clone();
    drop(fixture);
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
}

#[test]
fn same_observer_gets_one_record_and_later_old_value_registration_promotes_it() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(3);
    journal
        .observe(observer, fixture.node(2), attribute_options(false, false))
        .unwrap();
    journal
        .observe(observer, fixture.node(1), attribute_options(true, true))
        .unwrap();
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(2),
            vec![fixture.node(2), fixture.node(1)],
            atoms::CLASS,
            None,
            Some("old".to_owned()),
        ),
    );
    let records = journal.take_records(observer);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record().old_value.as_deref(), Some("old"));
}

#[test]
fn attribute_filter_rejects_namespaced_attribute_and_empty_filter_rejects_all() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(4);
    journal
        .observe(
            observer,
            fixture.node(1),
            ObserveOptions::normalize(ObserveOptionsInput {
                attributes: Some(true),
                attribute_filter: Some(vec![atoms::CLASS]),
                ..ObserveOptionsInput::default()
            })
            .unwrap(),
        )
        .unwrap();
    for namespace in [None, Some(atoms::NS_XML)] {
        fixture.queue(
            &mut journal,
            MutationFacts::attributes(
                fixture.node(1),
                vec![fixture.node(1)],
                atoms::CLASS,
                namespace,
                None,
            ),
        );
    }
    assert_eq!(journal.queued_count(observer), 1);
    journal
        .observe(
            observer,
            fixture.node(1),
            ObserveOptions::normalize(ObserveOptionsInput {
                attribute_filter: Some(Vec::new()),
                ..ObserveOptionsInput::default()
            })
            .unwrap(),
        )
        .unwrap();
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(1),
            vec![fixture.node(1)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(journal.queued_count(observer), 1);
}

#[test]
fn transient_registration_observes_removed_subtree_until_notify_even_after_take_records() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(5);
    journal
        .observe(observer, fixture.node(1), attribute_options(true, false))
        .unwrap();
    journal.install_transients(fixture.node(2), &[fixture.node(1)]);
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(3),
            vec![fixture.node(3), fixture.node(2)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(journal.take_records(observer).len(), 1);
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(3),
            vec![fixture.node(3), fixture.node(2)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(journal.drain_for_notify(observer).len(), 1);
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(3),
            vec![fixture.node(3), fixture.node(2)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(journal.queued_count(observer), 0);
}

#[test]
fn swept_source_does_not_cancel_a_surviving_removed_subtree_transient() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(50);
    journal
        .observe(observer, fixture.node(1), attribute_options(true, false))
        .unwrap();
    journal.install_transients(fixture.node(2), &[fixture.node(1)]);
    journal.forget_freed_node(fixture.node(1));

    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(3),
            vec![fixture.node(3), fixture.node(2)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(journal.take_records(observer).len(), 1);
    assert!(journal.has_registrations());

    journal.begin_notify();
    assert!(journal.drain_for_notify(observer).is_empty());
    assert!(!journal.has_registrations());
}

#[test]
fn observing_a_transient_target_updates_that_registration_without_making_it_permanent() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(51);
    journal
        .observe(observer, fixture.node(1), attribute_options(true, false))
        .unwrap();
    journal.install_transients(fixture.node(2), &[fixture.node(1)]);
    journal
        .observe(observer, fixture.node(2), attribute_options(true, true))
        .unwrap();

    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(3),
            vec![fixture.node(3), fixture.node(2)],
            atoms::CLASS,
            None,
            Some("before".to_owned()),
        ),
    );
    assert_eq!(
        journal.take_records(observer)[0]
            .record()
            .old_value
            .as_deref(),
        Some("before")
    );

    journal.begin_notify();
    journal.drain_for_notify(observer);
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(3),
            vec![fixture.node(3), fixture.node(2)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(journal.queued_count(observer), 0);
}

#[test]
fn nested_transient_keeps_its_immediate_source_when_original_is_reobserved() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(52);
    journal
        .observe(observer, fixture.node(1), attribute_options(true, false))
        .unwrap();
    journal.install_transients(fixture.node(2), &[fixture.node(1)]);
    journal.install_transients(fixture.node(3), &[fixture.node(2)]);

    // Re-observing the original registration removes only transients whose
    // source is that registration. The nested transient's source is B's
    // transient registration, so it survives until notification.
    journal
        .observe(observer, fixture.node(1), child_options(false))
        .unwrap();
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(4),
            vec![fixture.node(4), fixture.node(3)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(journal.queued_count(observer), 1);

    journal.begin_notify();
    assert_eq!(journal.drain_for_notify(observer).len(), 1);
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(4),
            vec![fixture.node(4), fixture.node(3)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(journal.queued_count(observer), 0);
}

#[test]
fn transient_preflight_can_be_discarded_or_rolled_back_without_observation() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(54);
    journal
        .observe(observer, fixture.node(1), attribute_options(true, false))
        .unwrap();

    let mut prepared = journal
        .prepare_transients(fixture.node(2), &[fixture.node(1)])
        .unwrap();
    journal
        .reserve_for_operation(&mut [], std::slice::from_ref(&prepared))
        .unwrap();
    journal
        .reserve_transients(std::slice::from_mut(&mut prepared))
        .unwrap();
    journal.discard_reserved_transients(std::slice::from_ref(&prepared));
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(3),
            vec![fixture.node(3), fixture.node(2)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(journal.queued_count(observer), 0);

    let mut prepared = journal
        .prepare_transients(fixture.node(2), &[fixture.node(1)])
        .unwrap();
    journal
        .reserve_for_operation(&mut [], std::slice::from_ref(&prepared))
        .unwrap();
    journal
        .reserve_transients(std::slice::from_mut(&mut prepared))
        .unwrap();
    let installed = journal.commit_transients(prepared);
    journal.rollback_transients(installed);
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(3),
            vec![fixture.node(3), fixture.node(2)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(journal.queued_count(observer), 0);
}

#[test]
fn child_list_only_removal_has_an_empty_transient_transaction() {
    let fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(77);
    let parent = fixture.node(1);
    let removed = fixture.node(2);
    journal
        .observe(observer, parent, child_options(false))
        .unwrap();
    let before = journal.registration_count;
    let mut prepared = journal.prepare_transients(removed, &[parent]).unwrap();
    assert!(prepared.additions.is_empty());
    journal
        .reserve_transients(std::slice::from_mut(&mut prepared))
        .unwrap();
    let installed = journal.commit_transients(prepared);
    assert!(installed.ids.is_empty());
    journal.rollback_transients(installed);
    assert_eq!(journal.registration_count, before);
    assert!(!journal.registrations.contains_key(&removed));
}

#[test]
fn transient_capacity_is_reserved_cumulatively_for_one_removed_root() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    journal
        .observe(
            ObserverId(55),
            fixture.node(1),
            attribute_options(true, false),
        )
        .unwrap();
    let mut prepared = [
        journal
            .prepare_transients(fixture.node(2), &[fixture.node(1)])
            .unwrap(),
        journal
            .prepare_transients(fixture.node(2), &[fixture.node(1)])
            .unwrap(),
    ];
    journal.reserve_for_operation(&mut [], &prepared).unwrap();
    journal.reserve_transients(&mut prepared).unwrap();
    let [first, second] = prepared;
    let first = journal.commit_transients(first);
    let second = journal.commit_transients(second);
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(3),
            vec![fixture.node(3), fixture.node(2)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(
        journal.queued_count(ObserverId(55)),
        1,
        "two registrations of one observer still select one record"
    );
    journal.rollback_transients(first);
    journal.rollback_transients(second);
}

#[test]
fn reobserve_removes_old_transients_and_replaces_options_without_duplicate_registration() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(6);
    journal
        .observe(observer, fixture.node(1), attribute_options(true, true))
        .unwrap();
    journal.install_transients(fixture.node(2), &[fixture.node(1)]);
    journal
        .observe(observer, fixture.node(1), child_options(false))
        .unwrap();
    fixture.queue(
        &mut journal,
        MutationFacts::attributes(
            fixture.node(3),
            vec![fixture.node(3), fixture.node(2)],
            atoms::CLASS,
            None,
            None,
        ),
    );
    assert_eq!(journal.queued_count(observer), 0);
    fixture.queue(
        &mut journal,
        MutationFacts::child_list(
            fixture.node(1),
            vec![fixture.node(1)],
            vec![fixture.node(4)],
            vec![],
            None,
            None,
        ),
    );
    assert_eq!(journal.queued_count(observer), 1);
}

#[test]
fn semantic_replacement_is_one_record_with_both_nodes_and_original_siblings() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let observer = ObserverId(7);
    journal
        .observe(observer, fixture.node(1), child_options(false))
        .unwrap();
    fixture.queue(
        &mut journal,
        MutationFacts::child_list(
            fixture.node(1),
            vec![fixture.node(1)],
            vec![fixture.node(5)],
            vec![fixture.node(3)],
            Some(fixture.node(2)),
            Some(fixture.node(4)),
        ),
    );
    let records = journal.take_records(observer);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record().added_nodes, vec![fixture.node(5)]);
    assert_eq!(records[0].record().removed_nodes, vec![fixture.node(3)]);
    assert_eq!(records[0].record().previous_sibling, Some(fixture.node(2)));
    assert_eq!(records[0].record().next_sibling, Some(fixture.node(4)));
}

#[test]
fn same_value_writes_are_distinct_records_and_absent_registration_is_a_fast_path() {
    let mut fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let facts =
        MutationFacts::character_data(fixture.node(1), vec![fixture.node(1)], "same".to_owned());
    assert!(!journal.has_registrations());
    assert!(journal.prepare(&facts).unwrap().is_empty());
    let observer = ObserverId(8);
    journal
        .observe(
            observer,
            fixture.node(1),
            ObserveOptions::normalize(ObserveOptionsInput {
                character_data: Some(true),
                character_data_old_value: Some(true),
                ..ObserveOptionsInput::default()
            })
            .unwrap(),
        )
        .unwrap();
    fixture.queue(&mut journal, facts.clone());
    fixture.queue(&mut journal, facts);
    assert_eq!(journal.take_records(observer).len(), 2);
}

#[test]
fn exact_generation_prune_does_not_remove_reused_slot_registration() {
    let fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let old = fixture.node(1);
    let reused = ObserverNode::new(
        old.document_id,
        NodeRef::new(old.node.id, old.node.generation + 1),
    );
    journal
        .observe(ObserverId(9), old, child_options(false))
        .unwrap();
    journal
        .observe(ObserverId(10), reused, child_options(false))
        .unwrap();
    journal.forget_freed_node(old);
    let prepared = journal
        .prepare(&MutationFacts::child_list(
            reused,
            vec![reused],
            vec![fixture.node(3)],
            vec![],
            None,
            None,
        ))
        .unwrap();
    assert_eq!(journal.queued_count(ObserverId(9)), 0);
    assert_eq!(prepared.records().count(), 1);
}

#[test]
fn identical_arena_slot_and_generation_in_another_document_do_not_alias() {
    let fixture = Fixture::new();
    let mut journal = ObserverJournal::default();
    let first = fixture.node(1);
    let mut other_document = crate::Document::new();
    let other_element = other_document.create_html_element(atoms::DIV);
    let second = ObserverNode::new(
        other_document.document_id(),
        other_document.node_ref(other_element),
    );
    assert_eq!(first.node, second.node);
    journal
        .observe(ObserverId(11), first, child_options(false))
        .unwrap();
    journal
        .observe(ObserverId(12), second, child_options(false))
        .unwrap();
    let prepared = journal
        .prepare(&MutationFacts::child_list(
            second,
            vec![second],
            vec![second],
            vec![],
            None,
            None,
        ))
        .unwrap();
    assert_eq!(journal.queued_count(ObserverId(11)), 0);
    assert_eq!(prepared.records().count(), 1);
}
