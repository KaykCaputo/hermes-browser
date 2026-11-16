use std::sync::atomic::{AtomicUsize, Ordering};

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

/// Every `commit` gets a session-history position no other commit has, so that none of them
/// looks like a `replaceState`.
static POSITION: AtomicUsize = AtomicUsize::new(1);

/// The engine's report of a session-history change, as `talaria-servo` sends it.
fn commit(controller: &mut BrowserController, tab_id: TabId, url: &str) {
    let position = POSITION.fetch_add(1, Ordering::Relaxed);
    commit_at(controller, tab_id, url, position + 1, position);
}

fn commit_at(
    controller: &mut BrowserController,
    tab_id: TabId,
    url: &str,
    history_length: usize,
    history_index: usize,
) {
    controller.handle_event(
        tab_id,
        BrowserEvent::AddressChanged {
            url: url.to_owned(),
        },
    );
    controller.handle_event(
        tab_id,
        BrowserEvent::PageCommitted {
            url: url.to_owned(),
            history_length,
            history_index,
        },
    );
}

/// A title as `talaria-servo` reports it: with the web view's URL at that moment.
fn title_at(controller: &mut BrowserController, tab_id: TabId, url: &str, title: Option<&str>) {
    controller.handle_event(
        tab_id,
        BrowserEvent::PageTitleChanged {
            title: title.map(str::to_owned),
        },
    );
    controller.handle_event(
        tab_id,
        BrowserEvent::PageTitleAt {
            url: url.to_owned(),
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

fn title_update(url: &str, title: &str) -> VisitUpdate {
    VisitUpdate::Title {
        url: url.to_owned(),
        title: title.to_owned(),
    }
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
fn only_the_engines_commit_report_counts() {
    let (mut controller, tab_id) = controller_with_ready_tab();

    controller.handle_event(
        tab_id,
        BrowserEvent::AddressChanged {
            url: "https://a.test/".to_owned(),
        },
    );

    assert!(controller.take_visit_updates().is_empty());
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
    commit(&mut controller, tab_id, "https://start.test/");
    controller.take_visit_updates();
    controller.queue_navigation("http://a.test/").unwrap();

    commit(&mut controller, tab_id, "https://www.a.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://www.a.test/", NavigationOrigin::Typed)]
    );
}

#[test]
fn a_subframe_commit_during_a_typed_navigation_does_not_take_its_origin() {
    // A subframe navigating reports the top-level URL again.
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    controller.take_visit_updates();
    controller.queue_navigation("https://b.test/").unwrap();

    commit(&mut controller, tab_id, "https://a.test/");
    commit(&mut controller, tab_id, "https://b.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://b.test/", NavigationOrigin::Typed)]
    );
}

#[test]
fn a_blank_page_committing_first_does_not_take_a_typed_origin() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    controller.queue_navigation("https://a.test/").unwrap();
    // The new tab's web view reports its initial blank page (shown as "") late.
    commit(&mut controller, tab_id, "");
    commit(&mut controller, tab_id, "about:blank");
    commit(&mut controller, tab_id, "https://a.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/", NavigationOrigin::Typed)]
    );
}

#[test]
fn typing_an_address_that_is_not_recorded_leaves_no_origin_behind() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    controller.queue_navigation("about:blank").unwrap();

    commit(&mut controller, tab_id, "");
    commit(&mut controller, tab_id, "https://a.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/", NavigationOrigin::Link)]
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
fn a_reload_that_never_commits_is_forgotten_at_the_next_page() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    controller.take_visit_updates();

    controller.queue_command(BrowserCommand::Reload);
    commit(&mut controller, tab_id, "https://b.test/");
    commit(&mut controller, tab_id, "https://b.test/");
    commit(&mut controller, tab_id, "https://c.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![
            ("https://b.test/", NavigationOrigin::Reload),
            ("https://c.test/", NavigationOrigin::Link),
        ]
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
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    controller.take_visit_updates();

    controller.queue_command(BrowserCommand::GoBack);
    commit(&mut controller, tab_id, "https://a.test/");

    assert!(controller.take_visit_updates().is_empty());
}

#[test]
fn same_document_navigations_that_add_an_entry_are_link_visits() {
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
fn a_replaced_entry_moves_the_current_visit_to_the_new_address() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    controller.queue_navigation("https://app.test/").unwrap();
    commit_at(&mut controller, tab_id, "https://app.test/", 2, 1);
    title_at(&mut controller, tab_id, "https://app.test/", Some("App"));
    let first = controller.take_visit_updates();
    let VisitUpdate::Visit { key: old, .. } = first[0] else {
        panic!("the commit is recorded first: {first:?}");
    };

    commit_at(&mut controller, tab_id, "https://app.test/?q=1", 2, 1);
    let updates = controller.take_visit_updates();

    let VisitUpdate::Visit { key: new, .. } = updates[1] else {
        panic!("the new address is recorded after the old one is forgotten: {updates:?}");
    };
    assert_ne!(old, new);
    // The old title is only carried provisionally: written if the page reports none before
    // the next commit.
    assert_eq!(
        updates,
        vec![
            VisitUpdate::Forget { key: old },
            VisitUpdate::Visit {
                key: new,
                url: "https://app.test/?q=1".to_owned(),
                origin: NavigationOrigin::Typed,
            },
        ]
    );
    commit(&mut controller, tab_id, "https://app.test/next");
    assert_eq!(
        controller.take_visit_updates()[..1],
        [title_update("https://app.test/?q=1", "App")]
    );
}

#[test]
fn a_user_navigation_that_replaces_the_entry_is_a_new_visit() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit_at(&mut controller, tab_id, "https://a.test/", 2, 1);
    controller.take_visit_updates();
    controller.queue_navigation("https://b.test/").unwrap();

    commit_at(&mut controller, tab_id, "https://b.test/", 2, 1);

    let updates = controller.take_visit_updates();
    assert!(
        !updates
            .iter()
            .any(|u| matches!(u, VisitUpdate::Forget { .. }))
    );
    assert_eq!(
        visits(&updates),
        vec![("https://b.test/", NavigationOrigin::Typed)]
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
fn credentials_in_an_address_are_not_recorded() {
    let (mut controller, tab_id) = controller_with_ready_tab();

    commit(&mut controller, tab_id, "https://user:secret@a.test/p");
    title_at(
        &mut controller,
        tab_id,
        "https://user:secret@a.test/p",
        Some("Private"),
    );

    assert_eq!(
        controller.take_visit_updates()[1..],
        [title_update("https://a.test/p", "Private")]
    );
    commit(&mut controller, tab_id, "https://user@a.test/q");
    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/q", NavigationOrigin::Link)]
    );
}

#[test]
fn a_typed_address_with_credentials_keeps_its_origin() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    controller.take_visit_updates();
    controller
        .queue_navigation("https://user:pw@b.test/")
        .unwrap();

    commit(&mut controller, tab_id, "https://a.test/");
    commit(&mut controller, tab_id, "https://user:pw@b.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://b.test/", NavigationOrigin::Typed)]
    );
}

#[test]
fn titles_follow_the_current_visit() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    let url = "https://a.test/";

    commit(&mut controller, tab_id, url);
    title_at(&mut controller, tab_id, url, Some(""));
    title_at(&mut controller, tab_id, url, Some("   "));
    title_at(&mut controller, tab_id, url, Some("Page A"));
    assert_eq!(
        controller.take_visit_updates()[1..],
        [title_update(url, "Page A")]
    );

    title_at(&mut controller, tab_id, url, Some("Page A"));
    assert!(controller.take_visit_updates().is_empty());
    // Within one pump only the last title counts.
    title_at(&mut controller, tab_id, url, Some("Page A, typing"));
    title_at(&mut controller, tab_id, url, Some("Page A, updated"));
    assert_eq!(
        controller.take_visit_updates(),
        [title_update(url, "Page A, updated")]
    );
}

#[test]
fn a_title_reported_for_another_address_is_dropped() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    controller.take_visit_updates();

    title_at(&mut controller, tab_id, "https://b.test/", Some("B"));
    controller.handle_event(
        tab_id,
        BrowserEvent::PageTitleChanged {
            title: Some("Without an address".to_owned()),
        },
    );

    assert!(controller.take_visit_updates().is_empty());
}

#[test]
fn a_title_set_after_a_same_document_navigation_belongs_to_the_new_address() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://app.test/");
    title_at(&mut controller, tab_id, "https://app.test/", Some("Inbox"));
    controller.take_visit_updates();

    commit(&mut controller, tab_id, "https://app.test/settings");
    title_at(
        &mut controller,
        tab_id,
        "https://app.test/settings",
        Some("Settings"),
    );

    assert_eq!(
        controller.take_visit_updates()[1..],
        [title_update("https://app.test/settings", "Settings")]
    );
}

#[test]
fn titles_of_pages_that_are_not_recorded_are_ignored() {
    let (mut controller, tab_id) = controller_with_ready_tab();

    title_at(&mut controller, tab_id, "", Some("New Tab"));
    commit(&mut controller, tab_id, "about:memory");
    title_at(
        &mut controller,
        tab_id,
        "about:memory",
        Some("about:memory"),
    );

    assert!(controller.take_visit_updates().is_empty());
}

#[test]
fn a_reload_offers_the_title_again() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    let url = "https://a.test/";
    commit(&mut controller, tab_id, url);
    title_at(&mut controller, tab_id, url, Some("A"));
    controller.take_visit_updates();

    controller.queue_command(BrowserCommand::Reload);
    commit(&mut controller, tab_id, url);
    title_at(&mut controller, tab_id, url, Some("A"));

    assert_eq!(
        controller.take_visit_updates()[1..],
        [title_update(url, "A")]
    );
}

#[test]
fn an_engine_error_page_takes_its_visit_back() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    let url = "https://down.test/";
    controller.queue_navigation(url).unwrap();

    commit(&mut controller, tab_id, url);
    title_at(&mut controller, tab_id, url, Some(""));
    title_at(&mut controller, tab_id, url, Some(ERROR_TITLE));
    let updates = controller.take_visit_updates();

    let VisitUpdate::Visit { key, .. } = updates[0] else {
        panic!("the commit is recorded first: {updates:?}");
    };
    assert_eq!(updates[1..], [VisitUpdate::Forget { key }]);
}

#[test]
fn an_error_title_for_another_address_never_takes_a_visit_back() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    controller.take_visit_updates();

    title_at(
        &mut controller,
        tab_id,
        "https://down.test/",
        Some(ERROR_TITLE),
    );

    assert!(controller.take_visit_updates().is_empty());
}

#[test]
fn loading_the_same_address_after_an_error_page_is_a_visit() {
    // For example the certificate error page's "proceed" link.
    let (mut controller, tab_id) = controller_with_ready_tab();
    let url = "https://down.test/";
    commit(&mut controller, tab_id, url);
    title_at(&mut controller, tab_id, url, Some(ERROR_TITLE));
    controller.take_visit_updates();

    commit(&mut controller, tab_id, url);

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![(url, NavigationOrigin::Link)]
    );
}

#[test]
fn the_error_title_on_a_page_that_already_had_a_title_is_just_a_title() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    let url = "https://a.test/";
    commit(&mut controller, tab_id, url);
    title_at(&mut controller, tab_id, url, Some("A"));
    controller.take_visit_updates();

    title_at(&mut controller, tab_id, url, Some(ERROR_TITLE));

    assert_eq!(
        controller.take_visit_updates(),
        vec![title_update(url, ERROR_TITLE)]
    );
}

#[test]
fn without_error_page_titles_no_title_takes_a_visit_back() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    let url = "https://down.test/";

    commit(&mut controller, tab_id, url);
    title_at(&mut controller, tab_id, url, Some(ERROR_TITLE));

    assert!(matches!(
        controller.take_visit_updates()[..],
        [VisitUpdate::Visit { .. }, VisitUpdate::Title { .. }]
    ));
}

#[test]
fn a_slow_page_that_fails_is_still_a_visit() {
    // Every `Failed` is synthetic (an address that does not resolve, a web view that could not
    // be created, a crash); network failures arrive as error pages.
    let (mut controller, tab_id) = controller_with_ready_tab();

    commit(&mut controller, tab_id, "https://slow.test/");
    load(&mut controller, tab_id, LoadState::Loading);
    load(
        &mut controller,
        tab_id,
        LoadState::Failed {
            message: "invalid URL".to_owned(),
        },
    );

    let updates = controller.take_visit_updates();
    assert_eq!(updates.len(), 1);
    assert!(matches!(updates[0], VisitUpdate::Visit { .. }));
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
    commit(&mut controller, tab_id, "https://start.test/");
    controller.take_visit_updates();
    controller.queue_navigation("https://a.test/").unwrap();

    assert!(controller.set_navigation_origin(tab_id, NavigationOrigin::Bookmark));
    commit(&mut controller, tab_id, "https://start.test/");
    commit(&mut controller, tab_id, "https://a.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/", NavigationOrigin::Bookmark)]
    );
    assert!(!controller.set_navigation_origin(TabId::new(99), NavigationOrigin::Bookmark));
}

#[test]
fn a_new_tab_opened_from_a_bookmark_is_a_bookmark_visit() {
    let mut controller = BrowserController::default();
    let tab_id = controller
        .open_tab_with(Some("https://a.test/"), TabActivation::Foreground)
        .unwrap();
    assert!(controller.set_navigation_origin(tab_id, NavigationOrigin::Bookmark));
    controller.take_initial_url(tab_id);

    commit(&mut controller, tab_id, "https://a.test/");

    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/", NavigationOrigin::Bookmark)]
    );
}

#[test]
fn a_restored_tab_records_nothing_for_its_restored_page() {
    let mut controller = BrowserController::default();
    let tab_id = controller
        .open_tab_with(Some("https://a.test/"), TabActivation::Background)
        .unwrap();
    assert!(controller.set_navigation_origin(tab_id, NavigationOrigin::Restored));
    controller.take_initial_url(tab_id);

    commit(&mut controller, tab_id, "https://a.test/");
    title_at(&mut controller, tab_id, "https://a.test/", Some("A"));
    commit(&mut controller, tab_id, "https://a.test/");
    assert!(controller.take_visit_updates().is_empty());

    commit(&mut controller, tab_id, "https://b.test/");
    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://b.test/", NavigationOrigin::Link)]
    );
}

#[test]
fn a_restored_page_that_redirects_is_not_recorded_either() {
    let mut controller = BrowserController::default();
    let tab_id = controller
        .open_tab_with(Some("http://a.test/"), TabActivation::Background)
        .unwrap();
    controller.set_navigation_origin(tab_id, NavigationOrigin::Restored);
    controller.take_initial_url(tab_id);

    commit(&mut controller, tab_id, "https://a.test/");

    assert!(controller.take_visit_updates().is_empty());
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

    assert_eq!(
        visits(&controller.take_visit_updates()),
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
    title_at(&mut controller, second, "https://b.test/", Some("B"));

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
    keys.sort();
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

#[test]
fn a_title_reported_before_the_first_commit_belongs_to_the_first_page() {
    // Servo reports a new web view's first titles before its first commit, while the web view
    // shows no page (an empty URL); the blank page has no title of its own.
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    title_at(&mut controller, tab_id, "", Some(""));
    title_at(&mut controller, tab_id, "", Some("Single page"));
    commit(&mut controller, tab_id, "https://a.test/");
    let updates = controller.take_visit_updates();

    assert_eq!(updates.len(), 2, "{updates:?}");
    assert!(matches!(updates[0], VisitUpdate::Visit { .. }));
    assert_eq!(updates[1], title_update("https://a.test/", "Single page"));
}

#[test]
fn an_error_title_reported_before_the_commit_records_nothing() {
    let mut controller = BrowserController::default();
    controller.set_error_page_titles([ERROR_TITLE.to_owned()]);
    let tab_id = controller.active_tab_id();

    title_at(&mut controller, tab_id, "", Some(ERROR_TITLE));
    commit(&mut controller, tab_id, "https://down.test/");

    assert!(controller.take_visit_updates().is_empty());
    commit(&mut controller, tab_id, "https://down.test/");
    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://down.test/", NavigationOrigin::Link)]
    );
}

#[test]
fn an_early_title_is_dropped_if_the_next_page_is_not_recorded() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    title_at(&mut controller, tab_id, "", Some("Memory"));
    commit(&mut controller, tab_id, "about:memory");
    commit(&mut controller, tab_id, "https://a.test/");

    let updates = controller.take_visit_updates();
    assert_eq!(updates.len(), 1, "{updates:?}");
}

#[test]
fn a_page_that_leaves_for_an_unreachable_host_keeps_its_title_and_records_nothing() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    let page = "https://a.test/";
    commit(&mut controller, tab_id, page);
    title_at(&mut controller, tab_id, page, Some("Page that leaves"));
    controller.take_visit_updates();

    title_at(&mut controller, tab_id, page, None);
    title_at(&mut controller, tab_id, page, Some(""));
    title_at(&mut controller, tab_id, page, Some(ERROR_TITLE));
    commit(&mut controller, tab_id, "https://down.test/");
    load(&mut controller, tab_id, LoadState::Loading);
    load(&mut controller, tab_id, LoadState::Loaded);

    assert!(controller.take_visit_updates().is_empty());
}

#[test]
fn a_new_documents_title_reported_before_its_commit_is_its_own() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://a.test/");
    title_at(&mut controller, tab_id, "https://a.test/", Some("A"));
    controller.take_visit_updates();

    title_at(&mut controller, tab_id, "https://a.test/", None);
    title_at(&mut controller, tab_id, "https://a.test/", Some("B"));
    commit(&mut controller, tab_id, "https://b.test/");

    assert_eq!(
        controller.take_visit_updates()[1..],
        [title_update("https://b.test/", "B")]
    );
}

#[test]
fn location_replace_to_an_unreachable_host_records_nothing_and_keeps_the_page() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    let page = "https://redirector.test/";
    commit_at(&mut controller, tab_id, page, 2, 1);
    title_at(&mut controller, tab_id, page, Some("Redirector"));
    controller.take_visit_updates();

    title_at(&mut controller, tab_id, page, None);
    title_at(&mut controller, tab_id, page, Some(""));
    title_at(&mut controller, tab_id, page, Some(ERROR_TITLE));
    commit_at(&mut controller, tab_id, "https://down.test/", 2, 1);

    assert!(controller.take_visit_updates().is_empty());
}

#[test]
fn location_replace_to_a_page_without_a_title_does_not_carry_the_old_title() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    let page = "https://login.test/";
    commit_at(&mut controller, tab_id, page, 2, 1);
    title_at(&mut controller, tab_id, page, Some("Login"));
    controller.take_visit_updates();

    title_at(&mut controller, tab_id, page, None);
    commit_at(&mut controller, tab_id, "https://app.test/untitled", 2, 1);
    load(&mut controller, tab_id, LoadState::Loaded);
    commit(&mut controller, tab_id, "https://app.test/next");

    let updates = controller.take_visit_updates();
    assert!(
        !updates
            .iter()
            .any(|u| matches!(u, VisitUpdate::Title { .. })),
        "{updates:?}"
    );
}

#[test]
fn a_cross_document_replace_announced_after_its_commit_drops_the_carried_title() {
    // The commit came first, so the replace looked like `replaceState`; the `None` title that
    // follows shows a new document without a title.
    let (mut controller, tab_id) = controller_with_ready_tab();
    let page = "https://login.test/";
    commit_at(&mut controller, tab_id, page, 2, 1);
    title_at(&mut controller, tab_id, page, Some("Login"));
    controller.take_visit_updates();

    commit_at(&mut controller, tab_id, "https://app.test/untitled", 2, 1);
    title_at(&mut controller, tab_id, "https://app.test/untitled", None);
    load(&mut controller, tab_id, LoadState::Loaded);
    commit(&mut controller, tab_id, "https://app.test/next");

    let updates = controller.take_visit_updates();
    assert!(
        !updates
            .iter()
            .any(|u| matches!(u, VisitUpdate::Title { .. })),
        "{updates:?}"
    );
}

#[test]
fn a_replaced_entrys_own_title_replaces_the_carried_one() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit_at(&mut controller, tab_id, "https://app.test/", 2, 1);
    title_at(&mut controller, tab_id, "https://app.test/", Some("App"));
    controller.take_visit_updates();

    commit_at(&mut controller, tab_id, "https://app.test/?q=1", 2, 1);
    title_at(
        &mut controller,
        tab_id,
        "https://app.test/?q=1",
        Some("Results"),
    );
    // A pump ends between the title and the next page.
    let mut updates = controller.take_visit_updates();
    commit(&mut controller, tab_id, "https://app.test/next");
    updates.extend(controller.take_visit_updates());

    let titles = updates
        .into_iter()
        .filter(|u| matches!(u, VisitUpdate::Title { .. }))
        .collect::<Vec<_>>();
    assert_eq!(
        titles,
        vec![title_update("https://app.test/?q=1", "Results")]
    );
}

#[test]
fn a_carried_title_does_not_switch_off_error_page_detection() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit_at(&mut controller, tab_id, "https://app.test/", 2, 1);
    title_at(&mut controller, tab_id, "https://app.test/", Some("App"));
    controller.take_visit_updates();

    commit_at(&mut controller, tab_id, "https://down.test/", 2, 1);
    title_at(
        &mut controller,
        tab_id,
        "https://down.test/",
        Some(ERROR_TITLE),
    );

    let updates = controller.take_visit_updates();
    let VisitUpdate::Visit { key, .. } = updates[1] else {
        panic!("{updates:?}");
    };
    assert_eq!(updates[2], VisitUpdate::Forget { key });
    // The entry it replaced is recorded again.
    assert_eq!(
        visits(&updates[3..]),
        vec![("https://app.test/", NavigationOrigin::Link)]
    );
    assert_eq!(updates[4..], [title_update("https://app.test/", "App")]);
}

#[test]
fn titles_after_a_reload_announced_after_its_commit_apply_when_it_has_loaded() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    let url = "https://a.test/";
    commit(&mut controller, tab_id, url);
    title_at(&mut controller, tab_id, url, Some("A"));
    controller.take_visit_updates();

    controller.queue_command(BrowserCommand::Reload);
    commit(&mut controller, tab_id, url);
    title_at(&mut controller, tab_id, url, None);
    load(&mut controller, tab_id, LoadState::Loading);
    title_at(&mut controller, tab_id, url, Some(""));
    title_at(&mut controller, tab_id, url, Some("A, reloaded"));
    load(&mut controller, tab_id, LoadState::Loaded);

    assert_eq!(
        controller.take_visit_updates()[1..],
        [title_update(url, "A, reloaded")]
    );
}

#[test]
fn a_reload_that_ends_on_an_error_page_is_taken_back() {
    let (mut controller, tab_id) = controller_with_ready_tab();
    let url = "https://a.test/";
    commit(&mut controller, tab_id, url);
    title_at(&mut controller, tab_id, url, Some("A"));
    controller.take_visit_updates();

    controller.queue_command(BrowserCommand::Reload);
    commit(&mut controller, tab_id, url);
    title_at(&mut controller, tab_id, url, None);
    title_at(&mut controller, tab_id, url, Some(ERROR_TITLE));
    load(&mut controller, tab_id, LoadState::Loaded);

    let updates = controller.take_visit_updates();
    let VisitUpdate::Visit { key, .. } = updates[0] else {
        panic!("{updates:?}");
    };
    assert_eq!(updates[1..], [VisitUpdate::Forget { key }]);
}

#[test]
fn a_title_set_on_an_opened_blank_window_never_reaches_the_page_it_navigates_to() {
    // `w = window.open(); w.document.title = "X"; w.location = victim`
    let (mut controller, tab_id) = controller_with_ready_tab();

    title_at(&mut controller, tab_id, "", Some("X"));
    commit(&mut controller, tab_id, "https://victim.test/");

    let updates = controller.take_visit_updates();
    assert_eq!(updates.len(), 1, "{updates:?}");
}

#[test]
fn a_restored_page_that_replaces_its_entry_on_load_is_not_recorded() {
    let mut controller = BrowserController::default();
    let tab_id = controller
        .open_tab_with(Some("https://a.test/"), TabActivation::Background)
        .unwrap();
    controller.set_navigation_origin(tab_id, NavigationOrigin::Restored);
    controller.take_initial_url(tab_id);

    commit_at(&mut controller, tab_id, "https://a.test/", 1, 0);
    commit_at(&mut controller, tab_id, "https://a.test/?state=1", 1, 0);
    assert!(controller.take_visit_updates().is_empty());

    commit_at(&mut controller, tab_id, "https://a.test/next", 2, 1);
    assert_eq!(
        visits(&controller.take_visit_updates()),
        vec![("https://a.test/next", NavigationOrigin::Link)]
    );
}

#[test]
fn a_title_that_overtook_its_same_document_commit_goes_to_the_new_address() {
    // `history.pushState(…); document.title = …`: the title reaches the embedder first.
    let (mut controller, tab_id) = controller_with_ready_tab();
    commit(&mut controller, tab_id, "https://app.test/");
    title_at(&mut controller, tab_id, "https://app.test/", Some("Inbox"));
    controller.take_visit_updates();

    title_at(
        &mut controller,
        tab_id,
        "https://app.test/",
        Some("Settings"),
    );
    commit(&mut controller, tab_id, "https://app.test/settings");

    assert_eq!(
        controller.take_visit_updates()[1..],
        [title_update("https://app.test/settings", "Settings")]
    );
}

#[test]
fn a_replace_that_ends_on_an_error_page_records_the_replaced_page_again() {
    // `location.replace(unreachable)` where the commit comes before the error title.
    let (mut controller, tab_id) = controller_with_ready_tab();
    let page = "https://redirector.test/";
    controller.queue_navigation(page).unwrap();
    commit_at(&mut controller, tab_id, page, 2, 1);
    title_at(&mut controller, tab_id, page, Some("Redirector"));
    controller.take_visit_updates();

    title_at(&mut controller, tab_id, page, None);
    title_at(&mut controller, tab_id, page, Some(""));
    commit_at(&mut controller, tab_id, "https://down.test/", 2, 1);
    title_at(
        &mut controller,
        tab_id,
        "https://down.test/",
        Some(ERROR_TITLE),
    );
    let updates = controller.take_visit_updates();

    let VisitUpdate::Visit { key: down, .. } = updates[1] else {
        panic!("{updates:?}");
    };
    let VisitUpdate::Visit { key: again, .. } = updates[3] else {
        panic!("{updates:?}");
    };
    assert!(matches!(updates[0], VisitUpdate::Forget { .. }));
    assert_eq!(updates[2], VisitUpdate::Forget { key: down });
    assert_eq!(
        updates[3],
        VisitUpdate::Visit {
            key: again,
            url: page.to_owned(),
            origin: NavigationOrigin::Typed,
        }
    );
    assert_eq!(updates[4..], [title_update(page, "Redirector")]);
}
