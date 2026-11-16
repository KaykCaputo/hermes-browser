mod common;

use common::test_search;
use talaria_core::{
    AddressResolutionError, BrowserCommand, BrowserController, BrowserEvent, BrowserState,
    ColorScheme, LoadState, PointerButton, PointerButtonAction, TabActivation, TabCommand, TabId,
};

fn command(queued: Option<TabCommand>) -> Option<BrowserCommand> {
    queued.map(TabCommand::into_command)
}

/// Simulates the engine reporting the tab's first document, which makes it dispatchable.
fn mark_engine_ready(controller: &mut BrowserController, tab_id: TabId) {
    let handled =
        controller.handle_event(tab_id, BrowserEvent::AddressChanged { url: String::new() });
    assert!(handled);
}

fn tab_is_engine_ready(controller: &BrowserController, tab_id: TabId) -> bool {
    controller
        .tabs()
        .find(|tab| tab.id() == tab_id)
        .is_some_and(|tab| tab.is_engine_ready())
}

fn controller_with_ready_tab() -> BrowserController {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    mark_engine_ready(&mut controller, tab_id);
    controller
}

#[test]
fn default_controller_starts_without_pending_commands() {
    let mut controller = BrowserController::default();

    assert!(!controller.has_pending_commands());
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn queued_commands_are_returned_in_fifo_order() {
    let mut controller = controller_with_ready_tab();

    controller.queue_command(BrowserCommand::Navigate {
        url: "https://example.com".to_owned(),
    });
    controller.queue_command(BrowserCommand::Reload);
    controller.queue_command(BrowserCommand::GoBack);

    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::Navigate {
            url: "https://example.com".to_owned(),
        })
    );
    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::Reload)
    );
    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::GoBack)
    );
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn new_controller_owns_the_provided_state() {
    let mut state = BrowserState::default();
    state.apply(BrowserEvent::AddressChanged {
        url: "https://example.com".to_owned(),
    });

    let controller = BrowserController::new(state);

    assert_eq!(controller.state().url(), "https://example.com");
}

#[test]
fn handled_events_update_the_owned_browser_state() {
    let mut controller = BrowserController::default();

    let tab_id = controller.active_tab_id();
    controller.handle_event(
        tab_id,
        BrowserEvent::LoadStateChanged {
            state: LoadState::Loading,
        },
    );
    controller.handle_event(
        tab_id,
        BrowserEvent::HistoryChanged {
            can_go_back: true,
            can_go_forward: false,
        },
    );

    assert_eq!(controller.state().load_state(), &LoadState::Loading);
    assert!(controller.state().can_go_back());
    assert!(!controller.state().can_go_forward());
}

#[test]
fn queued_navigation_resolves_hostname_and_updates_state() {
    let mut controller = controller_with_ready_tab();

    let result = controller.queue_navigation("example.com/docs");

    assert!(result.is_ok());
    assert_eq!(controller.state().url(), "https://example.com/docs");
    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::Navigate {
            url: "https://example.com/docs".to_owned(),
        })
    );
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn queued_navigation_resolves_search_query_with_the_search_builder() {
    let mut controller = controller_with_ready_tab();
    controller.set_search_url_builder(test_search());

    let result = controller.queue_navigation("ownership in rust");

    assert!(result.is_ok());
    assert_eq!(
        controller.state().url(),
        "https://search.test/?q=ownership+in+rust"
    );
    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::Navigate {
            url: "https://search.test/?q=ownership+in+rust".to_owned(),
        })
    );
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn invalid_navigation_does_not_change_state_or_queue_command() {
    let mut controller = controller_with_ready_tab();
    let original_url = controller.state().url().to_owned();

    let result = controller.queue_navigation("   ");

    assert!(matches!(result, Err(AddressResolutionError::EmptyInput)));
    assert_eq!(controller.state().url(), original_url);
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn consecutive_pointer_moves_keep_only_the_latest_position() {
    let mut controller = controller_with_ready_tab();

    controller.queue_command(BrowserCommand::PointerMoved { x: 10.0, y: 20.0 });
    controller.queue_command(BrowserCommand::PointerMoved { x: 30.0, y: 40.0 });

    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::PointerMoved { x: 30.0, y: 40.0 })
    );
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn consecutive_scrolls_accumulate_delta_at_the_latest_position() {
    let mut controller = controller_with_ready_tab();

    controller.queue_command(BrowserCommand::Scrolled {
        delta_x: 1.0,
        delta_y: 2.0,
        x: 10.0,
        y: 20.0,
    });
    controller.queue_command(BrowserCommand::Scrolled {
        delta_x: 3.0,
        delta_y: 4.0,
        x: 30.0,
        y: 40.0,
    });

    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::Scrolled {
            delta_x: 4.0,
            delta_y: 6.0,
            x: 30.0,
            y: 40.0,
        })
    );
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn input_commands_are_not_coalesced_across_an_ordering_boundary() {
    let mut controller = controller_with_ready_tab();

    controller.queue_command(BrowserCommand::PointerMoved { x: 10.0, y: 20.0 });
    controller.queue_command(BrowserCommand::Reload);
    controller.queue_command(BrowserCommand::PointerMoved { x: 30.0, y: 40.0 });

    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::PointerMoved { x: 10.0, y: 20.0 })
    );
    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::Reload)
    );
    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::PointerMoved { x: 30.0, y: 40.0 })
    );
}

#[test]
fn consecutive_focus_changes_keep_only_the_latest_state() {
    let mut controller = controller_with_ready_tab();

    controller.queue_command(BrowserCommand::FocusChanged { focused: true });
    controller.queue_command(BrowserCommand::FocusChanged { focused: false });

    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::FocusChanged { focused: false })
    );
    assert!(!controller.has_pending_commands());
}

#[test]
fn opening_a_tab_selects_it_and_preserves_the_previous_tab() {
    let mut controller = BrowserController::default();
    let first_tab_id = controller.active_tab_id();

    let second_tab_id = controller.open_tab();

    assert_ne!(second_tab_id, first_tab_id);
    assert_eq!(controller.active_tab_id(), second_tab_id);
    assert_eq!(controller.tabs().len(), 2);
    assert_eq!(controller.state().url(), "");
}

#[test]
fn commands_remain_associated_with_the_tab_that_created_them() {
    let mut controller = BrowserController::default();
    let first_tab_id = controller.active_tab_id();
    mark_engine_ready(&mut controller, first_tab_id);
    controller.queue_command(BrowserCommand::Reload);
    let second_tab_id = controller.open_tab();
    mark_engine_ready(&mut controller, second_tab_id);
    controller.queue_command(BrowserCommand::GoBack);

    let first = controller
        .next_command()
        .expect("first command should exist");
    let second = controller
        .next_command()
        .expect("second command should exist");

    assert_eq!(first.tab_id(), first_tab_id);
    assert_eq!(first.command(), &BrowserCommand::Reload);
    assert_eq!(second.tab_id(), second_tab_id);
    assert_eq!(second.command(), &BrowserCommand::GoBack);
}

#[test]
fn selecting_a_tab_restores_its_own_state() {
    let mut controller = BrowserController::default();
    let first_tab_id = controller.active_tab_id();
    controller
        .queue_navigation("example.com")
        .expect("navigation should be valid");
    let second_tab_id = controller.open_tab();

    assert_eq!(controller.state().url(), "");
    assert!(controller.select_tab(first_tab_id));
    assert_eq!(controller.state().url(), "https://example.com/");
    assert!(controller.select_tab(second_tab_id));
    assert_eq!(controller.state().url(), "");
}

#[test]
fn closing_the_active_tab_selects_its_left_neighbor() {
    let mut controller = BrowserController::default();
    let first_tab_id = controller.active_tab_id();
    let second_tab_id = controller.open_tab();

    assert!(controller.close_tab(second_tab_id));
    assert_eq!(controller.active_tab_id(), first_tab_id);
    assert_eq!(controller.tabs().len(), 1);
}

#[test]
fn the_last_tab_cannot_be_closed() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    assert!(!controller.close_tab(tab_id));
    assert_eq!(controller.tabs().len(), 1);
}

#[test]
fn closed_tabs_discard_their_pending_commands_and_late_events() {
    let mut controller = BrowserController::default();
    let first_tab_id = controller.active_tab_id();
    let closed_tab_id = controller.open_tab();
    mark_engine_ready(&mut controller, closed_tab_id);
    controller.queue_command(BrowserCommand::Reload);
    assert!(controller.has_pending_commands());

    assert!(controller.close_tab(closed_tab_id));
    assert!(!controller.has_pending_commands());
    assert!(!controller.handle_event(
        closed_tab_id,
        BrowserEvent::PageTitleChanged {
            title: Some("Too late".to_owned()),
        },
    ));
    assert_eq!(controller.active_tab_id(), first_tab_id);
}

#[test]
fn next_tab_selection_wraps_in_both_directions() {
    let mut controller = BrowserController::default();
    let first_tab_id = controller.active_tab_id();
    let second_tab_id = controller.open_tab();

    controller.select_next_tab(false);
    assert_eq!(controller.active_tab_id(), first_tab_id);
    controller.select_next_tab(true);
    assert_eq!(controller.active_tab_id(), second_tab_id);
}

#[test]
fn open_tab_with_address_sets_initial_url_and_loading_state() {
    let mut controller = BrowserController::default();

    let tab_id = controller
        .open_tab_with(Some("example.com"), TabActivation::Foreground)
        .expect("the address should resolve");

    assert_eq!(controller.active_tab_id(), tab_id);
    assert_eq!(controller.state().url(), "https://example.com/");
    assert_eq!(controller.state().load_state(), &LoadState::Loading);
    assert_eq!(
        controller.take_initial_url(tab_id).as_deref(),
        Some("https://example.com/")
    );
    assert_eq!(controller.take_initial_url(tab_id), None);
}

#[test]
fn open_tab_with_search_terms_uses_the_search_url() {
    let mut controller = BrowserController::default();
    controller.set_search_url_builder(test_search());

    let tab_id = controller
        .open_tab_with(Some("ownership in rust"), TabActivation::Foreground)
        .expect("search terms should resolve");

    let initial_url = controller
        .take_initial_url(tab_id)
        .expect("the tab should have an initial URL");
    assert!(initial_url.starts_with("https://search.test/?q="));
    assert_eq!(controller.state().url(), initial_url);
}

#[test]
fn background_tab_does_not_change_the_active_tab() {
    let mut controller = BrowserController::default();
    let first_tab_id = controller.active_tab_id();

    let background_tab_id = controller
        .open_tab_with(Some("https://example.com/"), TabActivation::Background)
        .expect("the address should resolve");

    assert_ne!(background_tab_id, first_tab_id);
    assert_eq!(controller.active_tab_id(), first_tab_id);
    assert_eq!(controller.tabs().len(), 2);
    assert_eq!(controller.state().url(), "");
    let background_tab = controller
        .tabs()
        .find(|tab| tab.id() == background_tab_id)
        .expect("the background tab should exist");
    assert_eq!(background_tab.state().url(), "https://example.com/");
    assert_eq!(background_tab.state().load_state(), &LoadState::Loading);
}

#[test]
fn open_tab_with_invalid_address_creates_no_tab() {
    let mut controller = BrowserController::default();
    let first_tab_id = controller.active_tab_id();

    let result = controller.open_tab_with(Some("ftp://x"), TabActivation::Foreground);

    assert!(matches!(
        result,
        Err(AddressResolutionError::UnsupportedScheme(ref scheme)) if scheme == "ftp"
    ));
    assert_eq!(controller.tabs().len(), 1);
    assert_eq!(controller.active_tab_id(), first_tab_id);
}

#[test]
fn open_tab_without_address_matches_open_tab() {
    let mut controller = BrowserController::default();

    let tab_id = controller
        .open_tab_with(None, TabActivation::Foreground)
        .expect("a blank tab needs no address");

    assert_eq!(controller.active_tab_id(), tab_id);
    assert_eq!(controller.state(), &BrowserState::default());
    assert_eq!(controller.take_initial_url(tab_id), None);
}

#[test]
fn set_initial_address_is_rejected_after_the_url_was_taken() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    assert!(matches!(
        controller.set_initial_address(tab_id, "example.com"),
        Ok(true)
    ));
    assert_eq!(controller.state().url(), "https://example.com/");
    assert_eq!(controller.state().load_state(), &LoadState::Loading);
    assert_eq!(
        controller.take_initial_url(tab_id).as_deref(),
        Some("https://example.com/")
    );

    assert!(matches!(
        controller.set_initial_address(tab_id, "example.org"),
        Ok(false)
    ));
    assert_eq!(controller.take_initial_url(tab_id), None);
    assert_eq!(controller.state().url(), "https://example.com/");
}

#[test]
fn set_initial_address_is_rejected_for_a_blank_tab_whose_web_view_exists() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    assert_eq!(controller.take_initial_url(tab_id), None);

    assert!(matches!(
        controller.set_initial_address(tab_id, "example.com"),
        Ok(false)
    ));
    assert_eq!(controller.state().url(), "");
}

#[test]
fn set_initial_address_reports_unknown_tabs_and_invalid_addresses() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    assert!(matches!(
        controller.set_initial_address(TabId::new(99), "example.com"),
        Ok(false)
    ));
    assert!(matches!(
        controller.set_initial_address(tab_id, "ftp://x"),
        Err(AddressResolutionError::UnsupportedScheme(_))
    ));
    assert_eq!(controller.take_initial_url(tab_id), None);
}

#[test]
fn commands_for_a_tab_that_is_not_engine_ready_are_held() {
    let mut controller = BrowserController::default();
    let tab_id = controller.open_tab();

    controller.queue_command(BrowserCommand::Reload);

    assert_eq!(command(controller.next_command()), None);
    assert!(!controller.has_pending_commands());

    let handled =
        controller.handle_event(tab_id, BrowserEvent::AddressChanged { url: String::new() });
    assert!(handled);

    assert!(controller.has_pending_commands());
    let queued = controller
        .next_command()
        .expect("the held command should be released");
    assert_eq!(queued.tab_id(), tab_id);
    assert_eq!(queued.command(), &BrowserCommand::Reload);
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn navigation_on_a_tab_that_is_not_engine_ready_replaces_its_initial_url() {
    let mut controller = BrowserController::default();
    let tab_id = controller
        .open_tab_with(Some("https://slow.test"), TabActivation::Foreground)
        .expect("the address should resolve");
    assert_eq!(
        controller.take_initial_url(tab_id).as_deref(),
        Some("https://slow.test/")
    );

    controller
        .queue_navigation("https://other.test")
        .expect("navigation should be valid");

    assert_eq!(command(controller.next_command()), None);
    assert!(!controller.has_pending_commands());
    assert_eq!(controller.state().url(), "https://other.test/");
    assert_eq!(controller.state().load_state(), &LoadState::Loading);
    assert_eq!(
        controller.take_initial_url(tab_id).as_deref(),
        Some("https://other.test/")
    );
    assert_eq!(controller.take_initial_url(tab_id), None);
    assert_eq!(controller.state().url(), "https://other.test/");
    assert_eq!(controller.state().load_state(), &LoadState::Loading);
}

#[test]
fn late_events_from_a_replaced_page_do_not_leave_the_tab_ready() {
    let mut controller = BrowserController::default();
    let tab_id = controller
        .open_tab_with(Some("https://slow.test"), TabActivation::Foreground)
        .expect("the address should resolve");
    controller.take_initial_url(tab_id);
    controller
        .queue_navigation("https://other.test")
        .expect("navigation should be valid");

    controller.handle_event(
        tab_id,
        BrowserEvent::AddressChanged {
            url: "https://slow.test/".to_owned(),
        },
    );
    assert!(!tab_is_engine_ready(&controller, tab_id));
    assert_eq!(
        controller.take_initial_url(tab_id).as_deref(),
        Some("https://other.test/")
    );

    assert!(!tab_is_engine_ready(&controller, tab_id));
    assert_eq!(controller.state().url(), "https://other.test/");
    assert_eq!(controller.state().load_state(), &LoadState::Loading);
}

#[test]
fn held_history_commands_are_dropped_when_the_initial_page_is_replaced() {
    let mut controller = BrowserController::default();
    let tab_id = controller
        .open_tab_with(Some("https://slow.test"), TabActivation::Foreground)
        .expect("the address should resolve");
    controller.take_initial_url(tab_id);
    controller.queue_command(BrowserCommand::Reload);
    controller.queue_command(BrowserCommand::GoBack);
    controller.queue_command(BrowserCommand::GoForward);

    controller
        .queue_navigation("https://other.test")
        .expect("navigation should be valid");
    controller.take_initial_url(tab_id);
    mark_engine_ready(&mut controller, tab_id);

    assert!(!controller.has_pending_commands());
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn replacing_the_initial_page_keeps_other_held_commands() {
    let mut controller = BrowserController::default();
    let first_tab_id = controller.active_tab_id();
    controller.queue_command(BrowserCommand::Reload);
    let tab_id = controller
        .open_tab_with(Some("https://slow.test"), TabActivation::Foreground)
        .expect("the address should resolve");
    controller.take_initial_url(tab_id);
    controller.queue_command(BrowserCommand::FocusChanged { focused: true });

    controller
        .queue_navigation("https://other.test")
        .expect("navigation should be valid");
    controller.take_initial_url(tab_id);
    mark_engine_ready(&mut controller, tab_id);
    mark_engine_ready(&mut controller, first_tab_id);

    let remaining = std::iter::from_fn(|| controller.next_command()).collect::<Vec<_>>();
    assert_eq!(
        remaining,
        vec![
            TabCommand::new(first_tab_id, BrowserCommand::Reload),
            TabCommand::new(tab_id, BrowserCommand::FocusChanged { focused: true }),
        ]
    );
}

#[test]
fn navigation_on_a_ready_tab_still_queues_a_navigate_command() {
    let mut controller = BrowserController::default();
    let tab_id = controller
        .open_tab_with(Some("https://slow.test"), TabActivation::Foreground)
        .expect("the address should resolve");
    controller.take_initial_url(tab_id);
    mark_engine_ready(&mut controller, tab_id);

    controller
        .queue_navigation("https://other.test")
        .expect("navigation should be valid");

    assert_eq!(controller.take_initial_url(tab_id), None);
    assert!(tab_is_engine_ready(&controller, tab_id));
    assert_eq!(controller.state().url(), "https://other.test/");
    assert_eq!(
        controller.next_command(),
        Some(TabCommand::new(
            tab_id,
            BrowserCommand::Navigate {
                url: "https://other.test/".to_owned(),
            }
        ))
    );
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn focus_requested_before_readiness_is_delivered_when_the_tab_becomes_ready() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    controller.queue_command(BrowserCommand::FocusChanged { focused: true });
    assert_eq!(command(controller.next_command()), None);

    mark_engine_ready(&mut controller, tab_id);

    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::FocusChanged { focused: true })
    );
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn history_change_also_makes_a_tab_engine_ready() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    controller.queue_command(BrowserCommand::Reload);
    assert!(!controller.has_pending_commands());

    controller.handle_event(
        tab_id,
        BrowserEvent::HistoryChanged {
            can_go_back: false,
            can_go_forward: false,
        },
    );

    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::Reload)
    );
}

#[test]
fn other_events_do_not_make_a_tab_engine_ready() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    controller.queue_command(BrowserCommand::Reload);

    controller.handle_event(
        tab_id,
        BrowserEvent::LoadStateChanged {
            state: LoadState::Loaded,
        },
    );
    controller.handle_event(
        tab_id,
        BrowserEvent::PageTitleChanged {
            title: Some("Title".to_owned()),
        },
    );

    assert!(!controller.has_pending_commands());
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn held_commands_do_not_block_other_tabs() {
    let mut controller = BrowserController::default();
    let first_tab_id = controller.active_tab_id();
    controller.queue_command(BrowserCommand::Reload);
    controller.queue_command(BrowserCommand::GoBack);
    let second_tab_id = controller.open_tab();
    mark_engine_ready(&mut controller, second_tab_id);
    controller.queue_command(BrowserCommand::GoForward);

    let first = controller
        .next_command()
        .expect("the ready tab's command should be dispatchable");
    assert_eq!(first.tab_id(), second_tab_id);
    assert_eq!(first.command(), &BrowserCommand::GoForward);
    assert_eq!(command(controller.next_command()), None);
    assert!(!controller.has_pending_commands());

    mark_engine_ready(&mut controller, first_tab_id);

    let held = std::iter::from_fn(|| controller.next_command()).collect::<Vec<_>>();
    assert_eq!(
        held,
        vec![
            TabCommand::new(first_tab_id, BrowserCommand::Reload),
            TabCommand::new(first_tab_id, BrowserCommand::GoBack),
        ]
    );
}

#[test]
fn pointer_commands_for_a_not_ready_tab_are_discarded() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    controller.queue_command(BrowserCommand::PointerMoved { x: 1.0, y: 2.0 });
    controller.queue_command(BrowserCommand::PointerButton {
        action: PointerButtonAction::Down,
        button: PointerButton::Left,
        x: 1.0,
        y: 2.0,
    });
    controller.queue_command(BrowserCommand::Scrolled {
        delta_x: 0.0,
        delta_y: 5.0,
        x: 1.0,
        y: 2.0,
    });
    controller.queue_command(BrowserCommand::Reload);

    mark_engine_ready(&mut controller, tab_id);

    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::Reload)
    );
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn zoom_in_queues_a_zoom_command_for_the_active_tab() {
    let mut controller = controller_with_ready_tab();

    controller.zoom_in();

    assert_eq!(controller.active_state().zoom_percent(), 110);
    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::SetPageZoom { percent: 110 })
    );
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn zoom_out_queues_a_zoom_command_for_the_active_tab() {
    let mut controller = controller_with_ready_tab();

    controller.zoom_out();

    assert_eq!(controller.active_state().zoom_percent(), 90);
    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::SetPageZoom { percent: 90 })
    );
}

#[test]
fn reset_zoom_returns_to_100() {
    let mut controller = controller_with_ready_tab();
    controller.zoom_in();
    controller.zoom_in();

    controller.reset_zoom();

    assert_eq!(controller.active_state().zoom_percent(), 100);
    let commands = std::iter::from_fn(|| command(controller.next_command())).collect::<Vec<_>>();
    assert_eq!(
        commands.last(),
        Some(&BrowserCommand::SetPageZoom { percent: 100 })
    );
}

#[test]
fn zoom_that_does_not_change_queues_nothing() {
    let mut controller = controller_with_ready_tab();

    controller.reset_zoom();

    assert_eq!(controller.active_state().zoom_percent(), 100);
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn zoom_is_tracked_per_tab() {
    let mut controller = controller_with_ready_tab();
    let first_tab_id = controller.active_tab_id();
    controller.zoom_in();
    let second_tab_id = controller.open_tab();
    mark_engine_ready(&mut controller, second_tab_id);

    controller.zoom_out();
    controller.zoom_out();

    let zooms = controller
        .tabs()
        .map(|tab| (tab.id(), tab.state().zoom_percent()))
        .collect::<Vec<_>>();
    assert_eq!(zooms, vec![(first_tab_id, 110), (second_tab_id, 80)]);
    assert!(controller.select_tab(first_tab_id));
    assert_eq!(controller.active_state().zoom_percent(), 110);
}

#[test]
fn zoom_requested_before_readiness_is_held_until_the_tab_is_ready() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    controller.zoom_in();

    assert_eq!(controller.active_state().zoom_percent(), 110);
    assert_eq!(command(controller.next_command()), None);
    mark_engine_ready(&mut controller, tab_id);
    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::SetPageZoom { percent: 110 })
    );
}

#[test]
fn pinch_zoom_for_a_not_ready_tab_is_discarded() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    controller.queue_command(BrowserCommand::PinchZoom {
        delta: 1.1,
        x: 1.0,
        y: 2.0,
    });
    mark_engine_ready(&mut controller, tab_id);

    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn consecutive_pinch_zooms_multiply_at_the_latest_position() {
    let mut controller = controller_with_ready_tab();

    controller.queue_command(BrowserCommand::PinchZoom {
        delta: 2.0,
        x: 1.0,
        y: 2.0,
    });
    controller.queue_command(BrowserCommand::PinchZoom {
        delta: 1.5,
        x: 3.0,
        y: 4.0,
    });

    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::PinchZoom {
            delta: 3.0,
            x: 3.0,
            y: 4.0,
        })
    );
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn select_last_tab_activates_the_final_tab() {
    let mut controller = BrowserController::default();
    controller.open_tab();
    let last_tab_id = controller.open_tab();
    assert!(controller.select_tab_at(0));

    controller.select_last_tab();

    assert_eq!(controller.active_tab_id(), last_tab_id);
}

#[test]
fn the_colour_scheme_starts_light() {
    assert_eq!(
        BrowserController::default().color_scheme(),
        ColorScheme::Light
    );
}

#[test]
fn a_colour_scheme_change_is_queued_for_every_tab() {
    let mut controller = controller_with_ready_tab();
    let first_tab_id = controller.active_tab_id();
    let second_tab_id = controller.open_tab();
    mark_engine_ready(&mut controller, second_tab_id);

    controller.set_color_scheme(ColorScheme::Dark);

    assert_eq!(controller.color_scheme(), ColorScheme::Dark);
    let queued = std::iter::from_fn(|| controller.next_command()).collect::<Vec<_>>();
    assert_eq!(
        queued,
        vec![
            TabCommand::new(
                first_tab_id,
                BrowserCommand::SetColorScheme(ColorScheme::Dark)
            ),
            TabCommand::new(
                second_tab_id,
                BrowserCommand::SetColorScheme(ColorScheme::Dark)
            ),
        ]
    );
}

#[test]
fn a_colour_scheme_change_before_readiness_is_held() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    controller.set_color_scheme(ColorScheme::Dark);
    assert_eq!(command(controller.next_command()), None);

    mark_engine_ready(&mut controller, tab_id);
    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::SetColorScheme(ColorScheme::Dark))
    );
}

#[test]
fn pointer_left_for_a_not_ready_tab_is_discarded() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();

    controller.queue_command(BrowserCommand::PointerLeft);
    mark_engine_ready(&mut controller, tab_id);

    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn pointer_left_for_a_ready_tab_is_queued() {
    let mut controller = controller_with_ready_tab();

    controller.queue_command(BrowserCommand::PointerLeft);

    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::PointerLeft)
    );
}
