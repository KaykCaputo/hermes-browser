use talaria_core::{
    BrowserEvent, BrowserState, LoadState, PageIcon, PointerCursor, display_origin,
    placeholder_title,
};

#[test]
fn default_state_describes_the_initial_page() {
    let state = BrowserState::default();

    assert_eq!(state.page_title(), "New Tab");
    assert_eq!(state.url(), "");
    assert_eq!(state.page_icon(), None);
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
fn page_icon_changed_uses_the_received_icon() {
    let mut state = BrowserState::default();
    let icon =
        PageIcon::from_rgba(1, 1, vec![10, 20, 30, 255]).expect("the test icon should be valid");

    state.apply(BrowserEvent::PageIconChanged {
        icon: Some(icon.clone()),
    });

    assert_eq!(state.page_icon(), Some(&icon));
}

#[test]
fn page_icon_can_be_cleared() {
    let mut state = BrowserState::default();
    let icon =
        PageIcon::from_rgba(1, 1, vec![10, 20, 30, 255]).expect("the test icon should be valid");

    state.apply(BrowserEvent::PageIconChanged { icon: Some(icon) });
    state.apply(BrowserEvent::PageIconChanged { icon: None });

    assert_eq!(state.page_icon(), None);
}

#[test]
fn navigating_to_a_different_address_clears_the_previous_icon() {
    let mut state = BrowserState::default();
    let icon =
        PageIcon::from_rgba(1, 1, vec![10, 20, 30, 255]).expect("the test icon should be valid");

    state.apply(BrowserEvent::PageIconChanged { icon: Some(icon) });
    state.apply(BrowserEvent::AddressChanged {
        url: "https://example.com".to_owned(),
    });

    assert_eq!(state.page_icon(), None);
}

#[test]
fn receiving_the_same_address_preserves_the_current_icon() {
    let mut state = BrowserState::default();
    let icon =
        PageIcon::from_rgba(1, 1, vec![10, 20, 30, 255]).expect("the test icon should be valid");

    state.apply(BrowserEvent::AddressChanged {
        url: "https://example.com".to_owned(),
    });
    state.apply(BrowserEvent::PageIconChanged {
        icon: Some(icon.clone()),
    });
    state.apply(BrowserEvent::AddressChanged {
        url: "https://example.com".to_owned(),
    });

    assert_eq!(state.page_icon(), Some(&icon));
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

#[test]
fn page_feedback_starts_neutral() {
    let state = BrowserState::default();

    assert_eq!(state.cursor(), PointerCursor::Default);
    assert_eq!(state.status_text(), None);
    assert!(!state.is_fullscreen());
}

#[test]
fn cursor_change_is_stored() {
    let mut state = BrowserState::default();

    state.apply(BrowserEvent::CursorChanged {
        cursor: PointerCursor::Pointer,
    });

    assert_eq!(state.cursor(), PointerCursor::Pointer);
}

#[test]
fn status_text_is_stored_and_cleared() {
    let mut state = BrowserState::default();

    state.apply(BrowserEvent::StatusTextChanged {
        text: Some("https://example.com/link".to_owned()),
    });
    assert_eq!(state.status_text(), Some("https://example.com/link"));

    state.apply(BrowserEvent::StatusTextChanged { text: None });
    assert_eq!(state.status_text(), None);
}

#[test]
fn fullscreen_flag_follows_events() {
    let mut state = BrowserState::default();

    state.apply(BrowserEvent::FullscreenChanged { fullscreen: true });
    assert!(state.is_fullscreen());

    state.apply(BrowserEvent::FullscreenChanged { fullscreen: false });
    assert!(!state.is_fullscreen());
}

/// The engine only reports cursor and status-text changes against its own cached values, so
/// the browser must never reset its copies on navigation: a reset that the engine does not see
/// would leave them out of step (for example an arrow shown over links after an in-page
/// navigation, because the engine still holds the pointer cursor and never sends it again).
#[test]
fn an_address_change_leaves_cursor_and_status_text_to_the_engine() {
    let mut state = BrowserState::default();
    state.apply(BrowserEvent::AddressChanged {
        url: "https://a.test/".to_owned(),
    });
    state.apply(BrowserEvent::CursorChanged {
        cursor: PointerCursor::Pointer,
    });
    state.apply(BrowserEvent::StatusTextChanged {
        text: Some("https://a.test/next".to_owned()),
    });

    state.apply(BrowserEvent::AddressChanged {
        url: "https://a.test/#section".to_owned(),
    });
    state.apply(BrowserEvent::AddressChanged {
        url: "https://b.test/".to_owned(),
    });

    assert_eq!(state.cursor(), PointerCursor::Pointer);
    assert_eq!(state.status_text(), Some("https://a.test/next"));
}

/// After a crash the engine still holds the pre-crash cursor, and talaria-servo reports it
/// right after the failure; the browser must not invent a cursor of its own.
#[test]
fn a_failed_page_clears_fullscreen_and_status_text_but_leaves_the_cursor() {
    let mut state = BrowserState::default();
    state.apply(BrowserEvent::FullscreenChanged { fullscreen: true });
    state.apply(BrowserEvent::CursorChanged {
        cursor: PointerCursor::Grabbing,
    });
    state.apply(BrowserEvent::StatusTextChanged {
        text: Some("https://a.test/".to_owned()),
    });

    state.apply(BrowserEvent::LoadStateChanged {
        state: LoadState::Failed {
            message: "crashed".to_owned(),
        },
    });

    assert!(!state.is_fullscreen());
    assert_eq!(state.status_text(), None);
    assert_eq!(state.cursor(), PointerCursor::Grabbing);
}

#[test]
fn a_normal_load_state_change_keeps_page_feedback() {
    let mut state = BrowserState::default();
    state.apply(BrowserEvent::FullscreenChanged { fullscreen: true });
    state.apply(BrowserEvent::CursorChanged {
        cursor: PointerCursor::Text,
    });

    state.apply(BrowserEvent::LoadStateChanged {
        state: LoadState::Loaded,
    });

    assert!(state.is_fullscreen());
    assert_eq!(state.cursor(), PointerCursor::Text);
}

#[test]
fn the_same_address_keeps_cursor_and_status_text() {
    let mut state = BrowserState::default();
    state.apply(BrowserEvent::AddressChanged {
        url: "https://a.test/".to_owned(),
    });
    state.apply(BrowserEvent::CursorChanged {
        cursor: PointerCursor::Text,
    });
    state.apply(BrowserEvent::StatusTextChanged {
        text: Some("x".to_owned()),
    });

    state.apply(BrowserEvent::AddressChanged {
        url: "https://a.test/".to_owned(),
    });

    assert_eq!(state.cursor(), PointerCursor::Text);
    assert_eq!(state.status_text(), Some("x"));
}

fn titled_page(url: &str, title: &str) -> BrowserState {
    let mut state = BrowserState::default();
    state.apply(BrowserEvent::AddressChanged {
        url: url.to_owned(),
    });
    state.apply(BrowserEvent::PageTitleChanged {
        title: Some(title.to_owned()),
    });
    state.apply(BrowserEvent::PageIconChanged {
        icon: Some(test_icon()),
    });
    state
}

fn test_icon() -> PageIcon {
    PageIcon::from_rgba(1, 1, vec![10, 20, 30, 255]).expect("the test icon should be valid")
}

#[test]
fn navigating_to_another_document_replaces_the_title_with_its_host() {
    let mut state = titled_page("https://a.test/x", "A");

    state.apply(BrowserEvent::AddressChanged {
        url: "https://b.test/y".to_owned(),
    });

    assert_eq!(state.page_title(), "b.test");
    assert_eq!(state.page_icon(), None);
}

#[test]
fn fragment_navigation_keeps_title_and_icon() {
    let mut state = titled_page("https://a.test/x", "A");

    state.apply(BrowserEvent::AddressChanged {
        url: "https://a.test/x#part".to_owned(),
    });
    state.apply(BrowserEvent::AddressChanged {
        url: "https://a.test/x#other".to_owned(),
    });

    assert_eq!(state.url(), "https://a.test/x#other");
    assert_eq!(state.page_title(), "A");
    assert_eq!(state.page_icon(), Some(&test_icon()));
}

#[test]
fn same_url_keeps_title() {
    let mut state = titled_page("https://a.test/x", "A");

    state.apply(BrowserEvent::AddressChanged {
        url: "https://a.test/x".to_owned(),
    });

    assert_eq!(state.page_title(), "A");
    assert_eq!(state.page_icon(), Some(&test_icon()));
}

#[test]
fn a_query_change_is_another_document() {
    let mut state = titled_page("https://a.test/x?page=1", "A");

    state.apply(BrowserEvent::AddressChanged {
        url: "https://a.test/x?page=2".to_owned(),
    });

    assert_eq!(state.page_title(), "a.test");
    assert_eq!(state.page_icon(), None);
}

#[test]
fn later_title_replaces_the_placeholder() {
    let mut state = titled_page("https://a.test/x", "A");
    state.apply(BrowserEvent::AddressChanged {
        url: "https://b.test/y".to_owned(),
    });

    state.apply(BrowserEvent::PageTitleChanged {
        title: Some("B".to_owned()),
    });

    assert_eq!(state.page_title(), "B");
}

#[test]
fn empty_title_falls_back_to_the_host() {
    let mut state = titled_page("https://a.test/x", "A");

    state.apply(BrowserEvent::PageTitleChanged {
        title: Some("  ".to_owned()),
    });
    assert_eq!(state.page_title(), "a.test");

    state.apply(BrowserEvent::PageTitleChanged {
        title: Some("A".to_owned()),
    });
    state.apply(BrowserEvent::PageTitleChanged { title: None });
    assert_eq!(state.page_title(), "a.test");
}

#[test]
fn blank_tab_is_titled_new_tab() {
    let mut state = titled_page("https://a.test/x", "A");

    // The engine reports the blank page as an empty address.
    state.apply(BrowserEvent::AddressChanged { url: String::new() });
    assert_eq!(state.page_title(), "New Tab");

    state.apply(BrowserEvent::PageTitleChanged { title: None });
    assert_eq!(state.page_title(), "New Tab");
}

#[test]
fn placeholder_title_is_the_host_else_the_url_else_new_tab() {
    assert_eq!(placeholder_title("https://example.com/a"), "example.com");
    assert_eq!(placeholder_title("about:blank"), "about:blank");
    assert_eq!(placeholder_title(""), "New Tab");
    assert_eq!(
        placeholder_title("file:///tmp/page.html"),
        "file:///tmp/page.html"
    );
    assert_eq!(placeholder_title("not a url"), "not a url");
}

#[test]
fn a_data_url_is_titled_by_its_scheme_only() {
    assert_eq!(placeholder_title("data:text/html,<p>secret</p>"), "data:");
}

#[test]
fn a_blob_url_is_titled_by_its_inner_host() {
    assert_eq!(
        placeholder_title("blob:https://a.test/4f8c1a52-7c1e-4c1a-9c55-0d3d3b9e2a10"),
        "a.test"
    );
}

#[test]
fn a_long_url_fallback_is_shortened() {
    let long = format!("file:///{}", "x".repeat(300));

    let title = placeholder_title(&long);

    assert_eq!(title.chars().count(), 80);
    assert!(title.ends_with('…'));
    assert!(long.starts_with(title.trim_end_matches('…')));
}

#[test]
fn display_origins_name_the_scheme_host_and_port_or_local_files() {
    assert_eq!(
        display_origin("https://a.test/path?q#f"),
        Some("https://a.test".to_owned())
    );
    assert_eq!(
        display_origin("http://a.test:8080/"),
        Some("http://a.test:8080".to_owned())
    );
    assert_eq!(
        display_origin("file:///Users/me/page.html"),
        Some("file://".to_owned())
    );
    assert_eq!(display_origin("data:text/html,x"), None);
    assert_eq!(display_origin("about:blank"), None);
    assert_eq!(display_origin(""), None);
}
