use mercury_ui::{BrowserTab, MainWindow, WebViewCursor, WebViewPointerButton};
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use talaria_core::{BrowserController, LoadState, PageIcon, PointerButton, PointerCursor};

pub(crate) fn sync_ui(window: &MainWindow, controller: &BrowserController) {
    let state = controller.active_state();
    window.set_page_title(state.page_title().into());
    window.set_address(state.url().into());
    window.set_is_new_tab(state.url().is_empty());
    window.set_can_go_back(state.can_go_back());
    window.set_can_go_forward(state.can_go_forward());
    window.set_is_loading(matches!(state.load_state(), LoadState::Loading));
    window.set_zoom_percent(i32::from(state.zoom_percent()));
    window.set_web_view_cursor(web_view_cursor(state.cursor()));
    window.set_status_text(single_line(state.status_text().unwrap_or_default()).into());
    sync_fullscreen(window, state.is_fullscreen());
    window.set_web_view_visible(
        !state.url().is_empty() && !matches!(state.load_state(), LoadState::Failed { .. }),
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
        _ if state.url().is_empty() => new_tab_hint.as_str(),
        LoadState::Idle => "Servo runtime initialized. Waiting for WebView.",
        LoadState::Loading => "The page is loading.",
        LoadState::Loaded => "",
    };
    window.set_content_message(content_message.into());
    crate::page_requests::sync_page_request(window, controller); // dialogs
    // controls
    crate::controls::sync(window, controller);
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
