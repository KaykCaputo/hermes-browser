use talaria_core::{
    AddressResolutionError, BrowserCommand, BrowserController, BrowserEvent, BrowserState,
    LoadState, TabCommand,
};

fn command(queued: Option<TabCommand>) -> Option<BrowserCommand> {
    queued.map(TabCommand::into_command)
}

#[test]
fn default_controller_starts_without_pending_commands() {
    let mut controller = BrowserController::default();

    assert!(!controller.has_pending_commands());
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn queued_commands_are_returned_in_fifo_order() {
    let mut controller = BrowserController::default();

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
    let mut controller = BrowserController::default();

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
fn queued_navigation_resolves_search_query_with_duckduckgo() {
    let mut controller = BrowserController::default();

    let result = controller.queue_navigation("ownership in rust");

    assert!(result.is_ok());
    assert_eq!(
        controller.state().url(),
        "https://duckduckgo.com/?q=ownership+in+rust"
    );
    assert_eq!(
        command(controller.next_command()),
        Some(BrowserCommand::Navigate {
            url: "https://duckduckgo.com/?q=ownership+in+rust".to_owned(),
        })
    );
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn invalid_navigation_does_not_change_state_or_queue_command() {
    let mut controller = BrowserController::default();
    let original_url = controller.state().url().to_owned();

    let result = controller.queue_navigation("   ");

    assert!(matches!(result, Err(AddressResolutionError::EmptyInput)));
    assert_eq!(controller.state().url(), original_url);
    assert_eq!(command(controller.next_command()), None);
}

#[test]
fn consecutive_pointer_moves_keep_only_the_latest_position() {
    let mut controller = BrowserController::default();

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
    let mut controller = BrowserController::default();

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
    let mut controller = BrowserController::default();

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
    let mut controller = BrowserController::default();

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
    controller.queue_command(BrowserCommand::Reload);
    let second_tab_id = controller.open_tab();
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
    controller.queue_command(BrowserCommand::Reload);

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
