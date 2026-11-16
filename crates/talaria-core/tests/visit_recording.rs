use talaria_core::{
    BrowserCommand, BrowserController, BrowserEvent, LoadState, NavigationOrigin, TabActivation,
    TabId, VisitUpdate,
};

const ERROR_TITLE: &str = "Error loading page";

/// A controller whose first tab's web view exists and shows a blank page.
fn controller_with_ready_tab() -> (BrowserController, TabId) {
    let mut controller = BrowserController::default();
    controller.set_error_page_titles([ERROR_TITLE.to_owned()]);
    let tab_id = controller.active_tab_id();
    commit(&mut controller, tab_id, "");
    assert!(controller.take_visit_updates().is_empty());
    (controller, tab_id)
}

fn commit(controller: &mut BrowserController, tab_id: TabId, url: &str) {
    controller.handle_event(
        tab_id,
        BrowserEvent::AddressChanged {
            url: url.to_owned(),
        },
    );
}

fn title(controller: &mut BrowserController, tab_id: TabId, title: Option<&str>) {
    controller.handle_event(
        tab_id,
        BrowserEvent::PageTitleChanged {
            title: title.map(str::to_owned),
        },
    );
}

fn load(controller: &mut BrowserController, tab_id: TabId, state: LoadState) {
    controller.handle_event(tab_id, BrowserEvent::LoadStateChanged { state });
}

/// The visits in `updates` as (URL, origin).
fn visits(updates: &[VisitUpdate]) -> Vec<(&str, NavigationOrigin)> {
    updates
        .iter()
        .filter_map(|update| match update {
            VisitUpdate::Visit { url, origin, .. } => Some((url.as_str(), *origin)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_typed_address_is_recorded_as_a_typed_visit_when_it_commits() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    controller.queue_navigation("example.com").unwrap();

    assert!(controller.take_visit_updates().is_empty());
    commit(&mut controller, tab_id, "https://example.com/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://example.com/", NavigationOrigin::Typed)]
    );
}

#[test]
fn a_search_is_a_typed_visit_to_the_results_page() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    controller.queue_navigation("servo engine").unwrap();
    let url = controller.state().url().to_owned();

    commit(&mut controller, tab_id, &url);

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![(url.as_str(), NavigationOrigin::Typed)]
    );
}

#[test]
fn a_navigation_the_page_started_is_a_link_visit() {
    let (mut controller, tab_id) = controller_with_ready_tab();

    commit(&mut controller, tab_id, "https://a.test/");
    commit(&mut controller, tab_id, "https://b.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![
            ("https://a.test/", NavigationOrigin::Link),
            ("https://b.test/", NavigationOrigin::Link),
        ]
    );
}

#[test]
fn an_origin_applies_to_the_next_commit_only() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    controller.queue_navigation("https://a.test/").unwrap();

    commit(&mut controller, tab_id, "https://a.test/");
    commit(&mut controller, tab_id, "https://a.test/next");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![
            ("https://a.test/", NavigationOrigin::Typed),
            ("https://a.test/next", NavigationOrigin::Link),
        ]
    );
}

#[test]
fn a_redirected_typed_address_records_the_page_it_ended_on_as_typed() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    controller.queue_navigation("http://a.test/").unwrap();

    commit(&mut controller, tab_id, "https://www.a.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://www.a.test/", NavigationOrigin::Typed)]
    );
}

#[test]
fn a_reload_is_recorded_as_a_reload_visit() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    controller.take_visit_updates();

    controller.queue_command(BrowserCommand::Reload);
    commit(&mut controller, tab_id, "https://a.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/", NavigationOrigin::Reload)]
    );
}

#[test]
fn typing_the_current_address_again_records_a_typed_visit() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    controller.take_visit_updates();

    controller.queue_navigation("https://a.test/").unwrap();
    commit(&mut controller, tab_id, "https://a.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/", NavigationOrigin::Typed)]
    );
}

#[test]
fn the_same_address_reported_again_without_a_user_command_is_not_a_visit() {
    // Subframe navigations and `replaceState` to the same URL report the top-level URL again.
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    controller.take_visit_updates();

    commit(&mut controller, tab_id, "https://a.test/");

    assert!(controller.take_visit_updates().is_empty());
}

#[test]
fn back_and_forward_are_history_visits() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    commit(&mut controller, tab_id, "https://b.test/");
    controller.take_visit_updates();

    controller.queue_command(BrowserCommand::GoBack);
    commit(&mut controller, tab_id, "https://a.test/");
    controller.queue_command(BrowserCommand::GoForward);
    commit(&mut controller, tab_id, "https://b.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![
            ("https://a.test/", NavigationOrigin::History),
            ("https://b.test/", NavigationOrigin::History),
        ]
    );
}

#[test]
fn going_back_to_the_same_address_is_not_a_visit() {
    // Back across a subframe navigation keeps the top-level URL.
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    controller.take_visit_updates();

    controller.queue_command(BrowserCommand::GoBack);
    commit(&mut controller, tab_id, "https://a.test/");

    assert!(controller.take_visit_updates().is_empty());
}

#[test]
fn same_document_navigations_are_link_visits() {
    let (mut controller, tab_id) = controller_with_ready_tab();

    commit(&mut controller, tab_id, "https://a.test/");
    commit(&mut controller, tab_id, "https://a.test/#part");
    commit(&mut controller, tab_id, "https://a.test/pushed");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![
            ("https://a.test/", NavigationOrigin::Link),
            ("https://a.test/#part", NavigationOrigin::Link),
            ("https://a.test/pushed", NavigationOrigin::Link),
        ]
    );
}

#[test]
fn only_web_and_file_pages_are_recorded() {
    let (mut controller, tab_id) = controller_with_ready_tab();

    for url in [
        "about:blank",
        "about:memory",
        "data:text/html,hello",
        "blob:https://a.test/0b6e",
        "javascript:void(0)",
        "not a url",
        "http://a.test/",
        "https://b.test/",
        "file:///tmp/page.html",
    ] {
        commit(&mut controller, tab_id, url);
    }

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![
            ("http://a.test/", NavigationOrigin::Link),
            ("https://b.test/", NavigationOrigin::Link),
            ("file:///tmp/page.html", NavigationOrigin::Link),
        ]
    );
}

#[test]
fn a_page_that_is_not_recorded_still_takes_the_origin() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    controller.queue_navigation("about:blank").unwrap();

    commit(&mut controller, tab_id, "about:blank");
    commit(&mut controller, tab_id, "https://a.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/", NavigationOrigin::Link)]
    );
}

#[test]
fn credentials_in_an_address_are_not_recorded() {
    let (mut controller, tab_id) = controller_with_ready_tab();

    commit(&mut controller, tab_id, "https://user:secret@a.test/p");
    title(&mut controller, tab_id, Some("Private"));

    assert_eq!(
        controller.take_visit_updates()[1..],
        [VisitUpdate::Title {
            url: "https://a.test/p".to_owned(),
            title: "Private".to_owned(),
        }]
    );
    commit(&mut controller, tab_id, "https://user@a.test/q");
    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/q", NavigationOrigin::Link)]
    );
}

#[test]
fn titles_follow_the_current_visit() {
    let (mut controller, tab_id) = controller_with_ready_tab();

    commit(&mut controller, tab_id, "https://a.test/");
    title(&mut controller, tab_id, Some(""));
    title(&mut controller, tab_id, None);
    title(&mut controller, tab_id, Some("   "));
    title(&mut controller, tab_id, Some("Page A"));
    title(&mut controller, tab_id, Some("Page A"));
    title(&mut controller, tab_id, Some("Page A, updated"));
    let updates = controller.take_visit_updates();

    let VisitUpdate::Visit { key, .. } = updates[0] else {
        panic!("the commit is recorded first: {updates:?}");
    };
    assert_eq!(
        updates,
        vec![
            VisitUpdate::Visit {
                key,
                url: "https://a.test/".to_owned(),
                origin: NavigationOrigin::Link,
            },
            VisitUpdate::Title {
                url: "https://a.test/".to_owned(),
                title: "Page A".to_owned(),
            },
            VisitUpdate::Title {
                url: "https://a.test/".to_owned(),
                title: "Page A, updated".to_owned(),
            },
        ]
    );
}

#[test]
fn a_title_set_after_a_same_document_navigation_belongs_to_the_new_address() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://app.test/");
    title(&mut controller, tab_id, Some("Inbox"));
    controller.take_visit_updates();

    commit(&mut controller, tab_id, "https://app.test/settings");
    title(&mut controller, tab_id, Some("Settings"));

    assert_eq!(
        controller.take_visit_updates()[1..],
        [VisitUpdate::Title {
            url: "https://app.test/settings".to_owned(),
            title: "Settings".to_owned(),
        }]
    );
}

#[test]
fn titles_of_pages_that_are_not_recorded_are_ignored() {
    let (mut controller, tab_id) = controller_with_ready_tab();

    title(&mut controller, tab_id, Some("New Tab"));
    commit(&mut controller, tab_id, "about:memory");
    title(&mut controller, tab_id, Some("about:memory"));

    assert!(controller.take_visit_updates().is_empty());
}

#[test]
fn a_reload_offers_the_title_again() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    title(&mut controller, tab_id, Some("A"));
    controller.take_visit_updates();

    controller.queue_command(BrowserCommand::Reload);
    commit(&mut controller, tab_id, "https://a.test/");
    title(&mut controller, tab_id, Some("A"));

    assert_eq!(
        controller.take_visit_updates()[1..],
        [VisitUpdate::Title {
            url: "https://a.test/".to_owned(),
            title: "A".to_owned(),
        }]
    );
}

#[test]
fn an_engine_error_page_takes_its_visit_back() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    controller.queue_navigation("https://down.test/").unwrap();

    commit(&mut controller, tab_id, "https://down.test/");
    title(&mut controller, tab_id, Some(""));
    title(&mut controller, tab_id, Some(ERROR_TITLE));
    load(&mut controller, tab_id, LoadState::Loading);
    load(&mut controller, tab_id, LoadState::Loaded);
    let updates = controller.take_visit_updates();

    let VisitUpdate::Visit { key, .. } = updates[0] else {
        panic!("the commit is recorded first: {updates:?}");
    };
    assert_eq!(updates[1..], [VisitUpdate::Forget { key }]);
}

#[test]
fn loading_the_same_address_after_an_error_page_is_a_visit() {
    // For example the certificate error page's "proceed" link.
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://down.test/");
    title(&mut controller, tab_id, Some(ERROR_TITLE));
    controller.take_visit_updates();

    commit(&mut controller, tab_id, "https://down.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://down.test/", NavigationOrigin::Link)]
    );
}

#[test]
fn the_error_title_on_a_page_that_already_had_a_title_is_just_a_title() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    title(&mut controller, tab_id, Some("A"));
    controller.take_visit_updates();

    title(&mut controller, tab_id, Some(ERROR_TITLE));

    assert_eq!(
        controller.take_visit_updates(),
        vec![VisitUpdate::Title {
            url: "https://a.test/".to_owned(),
            title: ERROR_TITLE.to_owned(),
        }]
    );
}

#[test]
fn without_error_page_titles_no_title_takes_a_visit_back() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    commit(&mut controller, tab_id, "https://down.test/");
    title(&mut controller, tab_id, Some(ERROR_TITLE));

    assert!(matches!(
        controller.take_visit_updates()[..],
        [VisitUpdate::Visit { .. }, VisitUpdate::Title { .. }]
    ));
}

#[test]
fn a_load_that_fails_before_it_finishes_takes_its_visit_back() {
    let (mut controller, tab_id) = controller_with_ready_tab();

    commit(&mut controller, tab_id, "https://crash.test/");
    load(&mut controller, tab_id, LoadState::Loading);
    load(
        &mut controller,
        tab_id,
        LoadState::Failed {
            message: "the content process crashed".to_owned(),
        },
    );
    let updates = controller.take_visit_updates();

    let VisitUpdate::Visit { key, .. } = updates[0] else {
        panic!("the commit is recorded first: {updates:?}");
    };
    assert_eq!(updates[1..], [VisitUpdate::Forget { key }]);
}

#[test]
fn a_failure_after_the_page_loaded_keeps_its_visit() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    load(&mut controller, tab_id, LoadState::Loading);
    load(&mut controller, tab_id, LoadState::Loaded);
    controller.take_visit_updates();

    load(
        &mut controller,
        tab_id,
        LoadState::Failed {
            message: "the content process crashed".to_owned(),
        },
    );

    assert!(controller.take_visit_updates().is_empty());
}

#[test]
fn a_failure_without_a_load_since_the_commit_keeps_its_visit() {
    // A same-document navigation starts no load; an invalid address typed afterwards reports
    // a failure on the tab, which says nothing about the page shown.
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://app.test/");
    load(&mut controller, tab_id, LoadState::Loading);
    load(&mut controller, tab_id, LoadState::Loaded);
    commit(&mut controller, tab_id, "https://app.test/settings");
    controller.take_visit_updates();

    load(
        &mut controller,
        tab_id,
        LoadState::Failed {
            message: "invalid URL".to_owned(),
        },
    );

    assert!(controller.take_visit_updates().is_empty());
}

#[test]
fn a_typed_address_for_a_tab_whose_first_page_has_not_committed_keeps_its_origin() {
    let mut controller = BrowserController::default();
    let tab_id = controller
        .open_tab_with(Some("https://slow.test/"), TabActivation::Foreground)
        .unwrap();
    controller.take_initial_url(tab_id);

    controller.queue_navigation("https://fast.test/").unwrap();
    controller.take_initial_url(tab_id);
    commit(&mut controller, tab_id, "https://fast.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://fast.test/", NavigationOrigin::Typed)]
    );
}

#[test]
fn a_tab_opened_at_an_address_records_a_link_visit() {
    let mut controller = BrowserController::default();
    let tab_id = controller
        .open_tab_with(Some("https://a.test/"), TabActivation::Background)
        .unwrap();
    controller.take_initial_url(tab_id);

    commit(&mut controller, tab_id, "https://a.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/", NavigationOrigin::Link)]
    );
}

#[test]
fn a_caller_can_name_the_origin_of_a_navigation() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    controller.queue_navigation("https://a.test/").unwrap();

    assert!(controller.set_navigation_origin(tab_id, NavigationOrigin::Bookmark));
    commit(&mut controller, tab_id, "https://a.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/", NavigationOrigin::Bookmark)]
    );
    assert!(!controller.set_navigation_origin(TabId::new(99), NavigationOrigin::Bookmark));
}

#[test]
fn origins_and_addresses_are_kept_per_tab() {
    let (mut controller, first) = controller_with_ready_tab();
    commit(&mut controller, first, "https://a.test/");
    let second = controller.open_tab();
    commit(&mut controller, second, "https://a.test/");
    controller.take_visit_updates();

    controller.select_tab(first);
    controller.queue_command(BrowserCommand::Reload);
    commit(&mut controller, second, "https://a.test/");
    commit(&mut controller, first, "https://a.test/");

    let updates = controller.take_visit_updates();
    assert_eq!(
        visits(&updates),
        vec![("https://a.test/", NavigationOrigin::Reload)]
    );
}

#[test]
fn a_closed_tab_records_nothing_more() {
    let (mut controller, first) = controller_with_ready_tab();
    let second = controller.open_tab();
    commit(&mut controller, second, "https://a.test/");
    controller.take_visit_updates();

    assert!(controller.close_tab(second));
    commit(&mut controller, second, "https://b.test/");
    title(&mut controller, second, Some("B"));

    assert!(controller.take_visit_updates().is_empty());
    commit(&mut controller, first, "https://c.test/");
    assert_eq!(controller.take_visit_updates().len(), 1);
}

#[test]
fn every_visit_has_its_own_key() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    let second = controller.open_tab();

    commit(&mut controller, tab_id, "https://a.test/");
    commit(&mut controller, second, "https://a.test/");
    commit(&mut controller, tab_id, "https://b.test/");

    let mut keys = controller
        .take_visit_updates()
        .into_iter()
        .filter_map(|update| match update {
            VisitUpdate::Visit { key, .. } => Some(key),
            _ => None,
        })
        .collect::<Vec<_>>();
    keys.dedup();
    assert_eq!(keys.len(), 3);
}

#[test]
fn taking_the_updates_empties_the_queue() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");

    assert_eq!(controller.take_visit_updates().len(), 1);
    assert!(controller.take_visit_updates().is_empty());
}
