use mercury_ui::{BrowserTab, MainWindow, WebViewPointerButton};
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use talaria_core::{BrowserController, LoadState, PageIcon, PointerButton};

pub(crate) fn sync_ui(window: &MainWindow, controller: &BrowserController) {
    let state = controller.active_state();
    window.set_page_title(state.page_title().into());
    window.set_address(state.url().into());
    window.set_is_new_tab(state.url().is_empty());
    window.set_can_go_back(state.can_go_back());
    window.set_can_go_forward(state.can_go_forward());
    window.set_is_loading(matches!(state.load_state(), LoadState::Loading));
    window.set_zoom_percent(i32::from(state.zoom_percent()));
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

    let content_message = match state.load_state() {
        LoadState::Failed { message } => message.as_str(),
        _ if state.url().is_empty() => "Search privately with DuckDuckGo.",
        LoadState::Idle => "Servo runtime initialized. Waiting for WebView.",
        LoadState::Loading => "The page is loading.",
        LoadState::Loaded => "",
    };
    window.set_content_message(content_message.into());
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
