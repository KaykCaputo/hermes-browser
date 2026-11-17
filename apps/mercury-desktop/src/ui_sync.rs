use std::cell::Cell;

use mercury_ui::{BrowserTab, MainWindow, WebViewCursor, WebViewPointerButton};
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use talaria_core::{BrowserController, LoadState, PageIcon, PointerButton, PointerCursor, TabId};

thread_local! {
    // tabs: the active tab at the previous sync, so a tab switch replaces the address draft.
    static SYNCED_TAB: SyncedTab = const { SyncedTab(Cell::new(None)) };
}

/// The active tab as of the previous sync.
#[derive(Debug, Default)]
pub(crate) struct SyncedTab(Cell<Option<TabId>>);

impl SyncedTab {
    /// Records `active` and reports whether it differs from the previous sync's tab.
    pub(crate) fn tab_changed(&self, active: TabId) -> bool {
        self.0.replace(Some(active)) != Some(active)
    }

    /// `tab_id` was selected without the user switching (a page opened it): the address
    /// field keeps the user's draft.
    pub(crate) fn treat_as_synced(&self, tab_id: TabId) {
        self.0.set(Some(tab_id));
    }
}

/// A page-opened tab became active: keep the address draft. See `SyncedTab::treat_as_synced`.
pub(crate) fn keep_address_draft_for(tab_id: TabId) {
    SYNCED_TAB.with(|synced| synced.treat_as_synced(tab_id));
}

/// What the page area shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PageArea {
    pub(crate) is_new_tab: bool,
    pub(crate) web_view_visible: bool,
}

/// A tab a page opened never shows the new-tab page, whose search field would look like the
/// user's own tab: until its page reports, the area stays blank.
pub(crate) fn page_area(
    url_is_empty: bool,
    failed: bool,
    opened_by_page: bool,
    engine_ready: bool,
) -> PageArea {
    PageArea {
        is_new_tab: url_is_empty && !opened_by_page,
        web_view_visible: (!url_is_empty || (opened_by_page && engine_ready)) && !failed,
    }
}

pub(crate) fn sync_ui(window: &MainWindow, controller: &BrowserController) {
    let state = controller.active_state();
    window.set_page_title(state.page_title().into());
    // tabs: keep what the user is typing unless the active tab changed.
    let tab_changed = SYNCED_TAB.with(|synced| synced.tab_changed(controller.active_tab_id()));
    window.set_committed_address(state.url().into());
    if let Some(address) =
        address_bar_update(state.url(), window.get_address_has_focus(), tab_changed)
    {
        window.invoke_reset_address(address.into());
    }
    // tabs: a tab opened by a page never shows the new-tab page
    let area = active_page_area(controller);
    window.set_is_new_tab(area.is_new_tab);
    window.set_can_go_back(state.can_go_back());
    window.set_can_go_forward(state.can_go_forward());
    window.set_is_loading(matches!(state.load_state(), LoadState::Loading));
    window.set_zoom_percent(i32::from(state.zoom_percent()));
    window.set_web_view_cursor(web_view_cursor(state.cursor()));
    window.set_status_text(single_line(state.status_text().unwrap_or_default()).into());
    sync_fullscreen(window, state.is_fullscreen());
    window.set_web_view_visible(area.web_view_visible);
    // tabs: the "Pop-up blocked" indicator
    window.set_pop_up_blocked_origin(
        crate::page_tabs::blocked_pop_up_origin(controller.active_tab_id(), state.url())
            .unwrap_or_default()
            .into(),
    );

    let active_tab_id = controller.active_tab_id();
    let mut active_tab_index = 0;
    let tabs = controller
        .tabs()
        .enumerate()
        .map(|(index, tab)| {
            let state = tab.state();
            if tab.id() == active_tab_id {
                active_tab_index = index;
            }
            let (icon, has_icon) = match state.page_icon() {
                Some(icon) => (slint_image_from_page_icon(icon), true),
                None => (Image::default(), false),
            };

            BrowserTab {
                title: state.page_title().into(),
                icon,
                has_icon,
                active: tab.id() == active_tab_id,
                loading: matches!(state.load_state(), LoadState::Loading),
            }
        })
        .collect::<Vec<_>>();
    window.set_tabs(ModelRc::new(VecModel::from(tabs)));
    window.set_active_tab_index(i32::try_from(active_tab_index).unwrap_or(i32::MAX));
    window.invoke_reveal_active_tab();

    // settings-ui: the new-tab hint names the chosen search engine.
    let new_tab_hint = crate::settings::new_tab_hint(&window.get_search_engine_name());
    let content_message = match state.load_state() {
        LoadState::Failed { message } => message.as_str(),
        _ if area.is_new_tab => new_tab_hint.as_str(),
        _ if state.url().is_empty() => "",
        LoadState::Idle => "Servo runtime initialized. Waiting for WebView.",
        LoadState::Loading => "The page is loading.",
        LoadState::Loaded => "",
    };
    window.set_content_message(content_message.into());
    crate::page_requests::sync_page_request(window, controller); // dialogs
    // controls
    crate::controls::sync(window, controller);
    // menus lane: what the native menus show (Back/Forward, the tab list).
    crate::menus::sync(controller);
}

/// Only page-requested fullscreen is mirrored, so a window the user made fullscreen is left
/// alone until a page enters and leaves fullscreen.
fn sync_fullscreen(window: &MainWindow, page_fullscreen: bool) {
    if window.get_chrome_hidden() == page_fullscreen {
        return;
    }
    window.set_chrome_hidden(page_fullscreen);
    // Slint makes the window borderless-fullscreen on its current monitor. Leaving page
    // fullscreen also ends window fullscreen that the user had turned on before; this matches
    // servoshell and is deliberate.
    window.window().set_fullscreen(page_fullscreen);
}

fn active_page_area(controller: &BrowserController) -> PageArea {
    let state = controller.active_state();
    let active = controller
        .tabs()
        .find(|tab| tab.id() == controller.active_tab_id());
    page_area(
        state.url().is_empty(),
        matches!(state.load_state(), LoadState::Failed { .. }),
        active.is_some_and(|tab| tab.is_opened_by_page()),
        active.is_some_and(|tab| tab.is_engine_ready()),
    )
}

/// What the address field should show. `None` means leave the user's text alone.
pub(crate) fn address_bar_update(
    committed: &str,
    field_has_focus: bool,
    tab_changed: bool,
) -> Option<String> {
    (tab_changed || !field_has_focus).then(|| committed.to_owned())
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Slint has no cursor for a few CSS keywords; those use the nearest available shape:
///
/// | CSS cursor      | shown as      |
/// |-----------------|---------------|
/// | `context-menu`  | `default`     |
/// | `cell`          | `crosshair`   |
/// | `vertical-text` | `text`        |
/// | `all-scroll`    | `move`        |
/// | `zoom-in`       | `default`     |
/// | `zoom-out`      | `default`     |
///
/// `none` hides the cursor, which Slint supports.
pub(crate) fn web_view_cursor(cursor: PointerCursor) -> WebViewCursor {
    match cursor {
        PointerCursor::Default | PointerCursor::ContextMenu => WebViewCursor::Default,
        PointerCursor::ZoomIn | PointerCursor::ZoomOut => WebViewCursor::Default,
        PointerCursor::None => WebViewCursor::None,
        PointerCursor::Pointer => WebViewCursor::Pointer,
        PointerCursor::Text | PointerCursor::VerticalText => WebViewCursor::Text,
        PointerCursor::Wait => WebViewCursor::Wait,
        PointerCursor::Progress => WebViewCursor::Progress,
        PointerCursor::Crosshair | PointerCursor::Cell => WebViewCursor::Crosshair,
        PointerCursor::Move | PointerCursor::AllScroll => WebViewCursor::Move,
        PointerCursor::NotAllowed => WebViewCursor::NotAllowed,
        PointerCursor::Grab => WebViewCursor::Grab,
        PointerCursor::Grabbing => WebViewCursor::Grabbing,
        PointerCursor::Help => WebViewCursor::Help,
        PointerCursor::Alias => WebViewCursor::Alias,
        PointerCursor::Copy => WebViewCursor::Copy,
        PointerCursor::NoDrop => WebViewCursor::NoDrop,
        PointerCursor::ColResize => WebViewCursor::ColResize,
        PointerCursor::RowResize => WebViewCursor::RowResize,
        PointerCursor::NResize => WebViewCursor::NResize,
        PointerCursor::EResize => WebViewCursor::EResize,
        PointerCursor::SResize => WebViewCursor::SResize,
        PointerCursor::WResize => WebViewCursor::WResize,
        PointerCursor::NeResize => WebViewCursor::NeResize,
        PointerCursor::NwResize => WebViewCursor::NwResize,
        PointerCursor::SeResize => WebViewCursor::SeResize,
        PointerCursor::SwResize => WebViewCursor::SwResize,
        PointerCursor::EwResize => WebViewCursor::EwResize,
        PointerCursor::NsResize => WebViewCursor::NsResize,
        PointerCursor::NeswResize => WebViewCursor::NeswResize,
        PointerCursor::NwseResize => WebViewCursor::NwseResize,
    }
}

pub(crate) fn slint_image_from_page_icon(icon: &PageIcon) -> Image {
    let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(icon.width(), icon.height());
    pixel_buffer.make_mut_bytes().copy_from_slice(icon.rgba());
    Image::from_rgba8(pixel_buffer)
}

pub(crate) fn pointer_button_from_ui(button: WebViewPointerButton) -> PointerButton {
    match button {
        WebViewPointerButton::Left => PointerButton::Left,
        WebViewPointerButton::Middle => PointerButton::Middle,
        WebViewPointerButton::Right => PointerButton::Right,
        WebViewPointerButton::Other => PointerButton::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_slint_lacks_use_the_nearest_shape() {
        assert_eq!(
            web_view_cursor(PointerCursor::Cell),
            WebViewCursor::Crosshair
        );
        assert_eq!(
            web_view_cursor(PointerCursor::VerticalText),
            WebViewCursor::Text
        );
        assert_eq!(
            web_view_cursor(PointerCursor::AllScroll),
            WebViewCursor::Move
        );
        assert_eq!(
            web_view_cursor(PointerCursor::ContextMenu),
            WebViewCursor::Default
        );
        assert_eq!(
            web_view_cursor(PointerCursor::ZoomIn),
            WebViewCursor::Default
        );
    }

    #[test]
    fn cursors_slint_has_map_one_to_one() {
        assert_eq!(
            web_view_cursor(PointerCursor::Pointer),
            WebViewCursor::Pointer
        );
        assert_eq!(
            web_view_cursor(PointerCursor::Grabbing),
            WebViewCursor::Grabbing
        );
        assert_eq!(
            web_view_cursor(PointerCursor::NwseResize),
            WebViewCursor::NwseResize
        );
        assert_eq!(web_view_cursor(PointerCursor::None), WebViewCursor::None);
    }

    #[test]
    fn address_is_written_when_the_field_is_idle() {
        assert_eq!(
            address_bar_update("https://a.test/", false, false),
            Some("https://a.test/".to_owned())
        );
    }

    #[test]
    fn address_is_left_alone_while_the_user_is_typing() {
        assert_eq!(address_bar_update("https://a.test/", true, false), None);
    }

    #[test]
    fn tab_change_overrides_typing() {
        assert_eq!(
            address_bar_update("https://b.test/", true, true),
            Some("https://b.test/".to_owned())
        );
        assert_eq!(address_bar_update("", false, true), Some(String::new()));
    }

    #[test]
    fn the_address_field_keeps_focus_and_draft_when_a_pop_up_opens() {
        let synced = SyncedTab::default();
        assert!(synced.tab_changed(TabId::new(1)));
        assert!(!synced.tab_changed(TabId::new(1)));

        // The user is typing in the address field when a page opens a tab in front.
        synced.treat_as_synced(TabId::new(2));
        let tab_changed = synced.tab_changed(TabId::new(2));

        assert!(!tab_changed);
        assert_eq!(
            address_bar_update("https://pop.test/", true, tab_changed),
            None
        );
        // A switch the user makes afterwards replaces the draft as usual.
        assert!(synced.tab_changed(TabId::new(1)));
    }

    #[test]
    fn a_tab_opened_by_a_page_never_shows_the_new_tab_page() {
        assert_eq!(
            page_area(true, false, false, true),
            PageArea {
                is_new_tab: true,
                web_view_visible: false
            }
        );
        // Not ready yet: a blank area.
        assert_eq!(
            page_area(true, false, true, false),
            PageArea {
                is_new_tab: false,
                web_view_visible: false
            }
        );
        // Ready at about:blank: the page itself is shown.
        assert_eq!(
            page_area(true, false, true, true),
            PageArea {
                is_new_tab: false,
                web_view_visible: true
            }
        );
        assert_eq!(
            page_area(false, true, true, true),
            PageArea {
                is_new_tab: false,
                web_view_visible: false
            }
        );
        assert_eq!(
            page_area(false, false, false, true),
            PageArea {
                is_new_tab: false,
                web_view_visible: true
            }
        );
    }

    #[test]
    fn status_text_is_shown_on_one_line() {
        assert_eq!(single_line("a\n b\tc "), "a b c");
        assert_eq!(single_line(""), "");
    }

    #[test]
    fn page_icon_pixels_are_preserved_when_converted_for_slint() {
        let icon =
            PageIcon::from_rgba(1, 1, vec![10, 20, 30, 40]).expect("the test icon should be valid");

        let pixels = slint_image_from_page_icon(&icon)
            .to_rgba8()
            .expect("an embedded RGBA image should remain CPU-readable");

        assert_eq!(pixels.width(), 1);
        assert_eq!(pixels.height(), 1);
        assert_eq!(pixels.as_bytes(), &[10, 20, 30, 40]);
    }
}
