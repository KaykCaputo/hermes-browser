use talaria_core::{BrowserCommand, BrowserController, BrowserEvent, BrowserState, LoadState};

#[test]
fn default_controller_starts_without_pending_commands() {
    let mut controller = BrowserController::default();

    assert_eq!(controller.next_command(), None);
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
        controller.next_command(),
        Some(BrowserCommand::Navigate {
            url: "https://example.com".to_owned(),
        })
    );
    assert_eq!(controller.next_command(), Some(BrowserCommand::Reload));
    assert_eq!(controller.next_command(), Some(BrowserCommand::GoBack));
    assert_eq!(controller.next_command(), None);
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

    controller.handle_event(BrowserEvent::LoadStateChanged {
        state: LoadState::Loading,
    });
    controller.handle_event(BrowserEvent::HistoryChanged {
        can_go_back: true,
        can_go_forward: false,
    });

    assert_eq!(controller.state().load_state(), &LoadState::Loading);
    assert!(controller.state().can_go_back());
    assert!(!controller.state().can_go_forward());
}
