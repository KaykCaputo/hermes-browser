//! What the browser's settings can change in the controller: the search used for address-bar
//! input that is not an address, the default page zoom, and a colour scheme forced over the
//! system's.

mod common;

use common::test_search;
use talaria_core::{
    AddressResolutionError, BrowserCommand, BrowserController, BrowserEvent, ColorScheme,
    SearchUrlBuilder, TabCommand, TabId, resolve_address,
};

fn mark_engine_ready(controller: &mut BrowserController, tab_id: TabId) {
    assert!(controller.handle_event(tab_id, BrowserEvent::AddressChanged { url: String::new() }));
}

fn queued(controller: &mut BrowserController) -> Vec<TabCommand> {
    std::iter::from_fn(|| controller.next_command()).collect()
}

#[test]
fn a_search_goes_through_the_builder_it_is_given() {
    let url = resolve_address("rust book", &test_search()).expect("a search URL");

    assert_eq!(url.as_str(), "https://search.test/?q=rust+book");
}

#[test]
fn addresses_never_reach_the_builder() {
    let builder = SearchUrlBuilder::new(|_| panic!("an address is not a search"));

    for (input, expected) in [
        ("example.com/docs", "https://example.com/docs"),
        ("https://a.test/", "https://a.test/"),
        ("w.org", "https://w.org/"),
        ("localhost:3000", "https://localhost:3000/"),
    ] {
        let url = resolve_address(input, &builder).expect("an address");
        assert_eq!(url.as_str(), expected, "{input}");
    }
}

#[test]
fn the_whole_trimmed_text_is_handed_to_the_builder() {
    let url = resolve_address("  @wikipedia rust book  ", &test_search()).expect("a search URL");

    assert_eq!(
        url.as_str(),
        "https://search.test/?q=%40wikipedia+rust+book"
    );
}

#[test]
fn without_a_builder_the_controller_refuses_to_search() {
    let mut controller = BrowserController::default();

    let error = controller
        .queue_navigation("rust ownership")
        .expect_err("no search engine is set");

    assert!(matches!(error, AddressResolutionError::Search(_)));
    assert_eq!(controller.active_state().url(), "");
    assert_eq!(
        controller
            .queue_navigation("example.com")
            .map(|()| controller.active_state().url().to_owned())
            .ok()
            .as_deref(),
        Some("https://example.com/")
    );
}

#[test]
fn a_builder_that_refuses_the_text_is_an_address_error() {
    let builder = SearchUrlBuilder::new(|_| Err("no place for this query".to_owned()));

    let error = resolve_address("plain words", &builder).expect_err("refused");

    assert!(matches!(error, AddressResolutionError::Search(_)));
    assert_eq!(
        error.to_string(),
        "cannot search for this text: no place for this query"
    );
}

#[test]
fn the_controller_searches_with_the_builder_it_was_given() {
    let mut controller = BrowserController::default();
    controller.set_search_url_builder(test_search());

    controller
        .queue_navigation("ownership in rust")
        .expect("a search");
    let tab_id = controller
        .open_tab_with(
            Some("second search"),
            talaria_core::TabActivation::Background,
        )
        .expect("a search");
    let first = controller.tabs().next().map(|tab| tab.id()).expect("a tab");

    assert_eq!(
        controller.take_initial_url(first).as_deref(),
        Some("https://search.test/?q=ownership+in+rust")
    );
    assert_eq!(
        controller.take_initial_url(tab_id).as_deref(),
        Some("https://search.test/?q=second+search")
    );
}

#[test]
fn the_default_zoom_starts_at_100() {
    assert_eq!(BrowserController::default().default_zoom_percent(), 100);
}

#[test]
fn a_new_default_zoom_moves_tabs_at_the_old_default_and_new_tabs() {
    let mut controller = BrowserController::default();
    let first = controller.active_tab_id();
    mark_engine_ready(&mut controller, first);
    let zoomed = controller.open_tab();
    mark_engine_ready(&mut controller, zoomed);
    controller.zoom_in();
    let _ = queued(&mut controller);

    controller.set_default_zoom_percent(150);

    let zooms = controller
        .tabs()
        .map(|tab| (tab.id(), tab.state().zoom_percent()))
        .collect::<Vec<_>>();
    assert_eq!(zooms, vec![(first, 150), (zoomed, 110)]);
    assert_eq!(
        queued(&mut controller),
        vec![TabCommand::new(
            first,
            BrowserCommand::SetPageZoom { percent: 150 }
        )]
    );

    let new_tab = controller.open_tab();
    assert_eq!(controller.active_tab_id(), new_tab);
    assert_eq!(controller.active_state().zoom_percent(), 150);
}

#[test]
fn resetting_the_zoom_returns_to_the_default_zoom() {
    let mut controller = BrowserController::default();
    controller.set_default_zoom_percent(90);
    controller.zoom_in();
    controller.zoom_in();

    controller.reset_zoom();

    assert_eq!(controller.active_state().zoom_percent(), 90);
}

#[test]
fn setting_the_same_default_zoom_queues_nothing() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    mark_engine_ready(&mut controller, tab_id);

    controller.set_default_zoom_percent(100);

    assert_eq!(queued(&mut controller), Vec::new());
}

#[test]
fn a_forced_scheme_wins_over_the_system_and_is_queued_for_every_tab() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    mark_engine_ready(&mut controller, tab_id);
    controller.set_color_scheme(ColorScheme::Dark);
    let _ = queued(&mut controller);

    controller.set_forced_color_scheme(Some(ColorScheme::Light));

    assert_eq!(controller.color_scheme(), ColorScheme::Light);
    assert_eq!(
        queued(&mut controller),
        vec![TabCommand::new(
            tab_id,
            BrowserCommand::SetColorScheme(ColorScheme::Light)
        )]
    );
}

#[test]
fn system_changes_while_forced_are_remembered_but_not_sent() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    mark_engine_ready(&mut controller, tab_id);
    controller.set_forced_color_scheme(Some(ColorScheme::Light));
    let _ = queued(&mut controller);

    controller.set_color_scheme(ColorScheme::Dark);

    assert_eq!(controller.color_scheme(), ColorScheme::Light);
    assert_eq!(queued(&mut controller), Vec::new());

    controller.set_forced_color_scheme(None);

    assert_eq!(controller.color_scheme(), ColorScheme::Dark);
    assert_eq!(
        queued(&mut controller),
        vec![TabCommand::new(
            tab_id,
            BrowserCommand::SetColorScheme(ColorScheme::Dark)
        )]
    );
}

#[test]
fn forcing_the_scheme_pages_already_have_queues_nothing() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    mark_engine_ready(&mut controller, tab_id);

    controller.set_forced_color_scheme(Some(ColorScheme::Light));
    controller.set_forced_color_scheme(None);

    assert_eq!(queued(&mut controller), Vec::new());
}

#[test]
fn others_resolve_address_bar_text_through_the_controllers_own_builder() {
    let mut controller = BrowserController::default();
    controller.set_search_url_builder(test_search());

    let search = controller
        .resolve_address("@keyword rust book")
        .expect("a search");
    let address = controller
        .resolve_address("example.org")
        .expect("an address");

    assert_eq!(
        search.as_str(),
        "https://search.test/?q=%40keyword+rust+book"
    );
    assert_eq!(address.as_str(), "https://example.org/");
    assert!(
        controller.active_state().url().is_empty(),
        "nothing navigates"
    );
}
