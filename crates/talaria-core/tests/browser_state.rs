use talaria_core::{BrowserEvent, BrowserState, LoadState};

#[test]
fn default_state_describes_the_initial_page() {
    let state = BrowserState::default();

    assert_eq!(state.page_title(), "New Tab");
    assert_eq!(state.url(), "https://servo.org");
    assert_eq!(state.load_state(), &LoadState::Idle);
    assert!(!state.can_go_back());
    assert!(!state.can_go_forward());
}

#[test]
fn address_changed_replaces_the_current_url() {
    let mut state = BrowserState::default();

    state.apply(BrowserEvent::AddressChanged {
        url: "https://example.com".to_owned(),
    });

    assert_eq!(state.url(), "https://example.com");
}

#[test]
fn page_title_changed_uses_the_received_title() {
    let mut state = BrowserState::default();

    state.apply(BrowserEvent::PageTitleChanged {
        title: Some("Example Domain".to_owned()),
    });

    assert_eq!(state.page_title(), "Example Domain");
}

#[test]
fn missing_page_title_restores_the_default_title() {
    let mut state = BrowserState::default();

    state.apply(BrowserEvent::PageTitleChanged {
        title: Some("Example Domain".to_owned()),
    });
    state.apply(BrowserEvent::PageTitleChanged { title: None });

    assert_eq!(state.page_title(), "New Tab");
}

#[test]
fn blank_page_title_restores_the_default_title() {
    let mut state = BrowserState::default();

    state.apply(BrowserEvent::PageTitleChanged {
        title: Some("   ".to_owned()),
    });

    assert_eq!(state.page_title(), "New Tab");
}

#[test]
fn load_state_changed_replaces_the_current_load_state() {
    let mut state = BrowserState::default();

    state.apply(BrowserEvent::LoadStateChanged {
        state: LoadState::Loading,
    });

    assert_eq!(state.load_state(), &LoadState::Loading);
}

#[test]
fn navigation_failure_preserves_its_error_message() {
    let mut state = BrowserState::default();

    state.apply(BrowserEvent::LoadStateChanged {
        state: LoadState::Failed {
            message: "The connection was refused.".to_owned(),
        },
    });

    assert_eq!(
        state.load_state(),
        &LoadState::Failed {
            message: "The connection was refused.".to_owned(),
        }
    );
}

#[test]
fn history_changed_updates_both_navigation_directions() {
    let mut state = BrowserState::default();

    state.apply(BrowserEvent::HistoryChanged {
        can_go_back: true,
        can_go_forward: false,
    });

    assert!(state.can_go_back());
    assert!(!state.can_go_forward());
}
