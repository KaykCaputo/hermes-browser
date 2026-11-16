use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mercury_ui::MainWindow;
use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use talaria_core::{
    BrowserCommand, BrowserController, BrowserEvent, LoadState, PointerButtonAction,
};

use crate::ui_sync::{pointer_button_from_ui, sync_ui};

pub(crate) fn connect_ui_callbacks(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
) {
    crate::page_requests::connect_page_request_callbacks(window, Rc::clone(&controller)); // dialogs
    let navigate_controller = Rc::clone(&controller);
    let navigate_window = window.as_weak();
    window.on_navigate(move |address| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let result = navigate_controller
            .borrow_mut()
            .queue_navigation(address.as_str());
        let Some(window) = navigate_window.upgrade() else {
            return;
        };

        if let Err(error) = result {
            let tab_id = navigate_controller.borrow().active_tab_id();
            navigate_controller.borrow_mut().handle_event(
                tab_id,
                BrowserEvent::LoadStateChanged {
                    state: LoadState::Failed {
                        message: error.to_string(),
                    },
                },
            );
        }
        sync_ui(&window, &navigate_controller.borrow());
        window.window().request_redraw();
    });

    let back_controller = Rc::clone(&controller);
    let back_window = window.as_weak();
    window.on_go_back(move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        back_controller
            .borrow_mut()
            .queue_command(BrowserCommand::GoBack);
        if let Some(window) = back_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let forward_controller = Rc::clone(&controller);
    let forward_window = window.as_weak();
    window.on_go_forward(move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        forward_controller
            .borrow_mut()
            .queue_command(BrowserCommand::GoForward);
        if let Some(window) = forward_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let reload_controller = Rc::clone(&controller);
    let reload_window = window.as_weak();
    window.on_reload(move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        reload_controller
            .borrow_mut()
            .queue_command(BrowserCommand::Reload);
        if let Some(window) = reload_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let new_tab_controller = Rc::clone(&controller);
    let new_tab_window = window.as_weak();
    window.on_new_tab(move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        new_tab_controller.borrow_mut().open_tab();
        if let Some(window) = new_tab_window.upgrade() {
            sync_ui(&window, &new_tab_controller.borrow());
            window.window().request_redraw();
        }
    });

    let select_tab_controller = Rc::clone(&controller);
    let select_tab_window = window.as_weak();
    window.on_select_tab(move |index| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if select_tab_controller.borrow_mut().select_tab_at(index)
            && let Some(window) = select_tab_window.upgrade()
        {
            sync_ui(&window, &select_tab_controller.borrow());
            window.window().request_redraw();
        }
    });

    let close_tab_controller = Rc::clone(&controller);
    let close_tab_window = window.as_weak();
    window.on_close_tab(move |index| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if close_tab_controller.borrow_mut().close_tab_at(index)
            && let Some(window) = close_tab_window.upgrade()
        {
            sync_ui(&window, &close_tab_controller.borrow());
            window.window().request_redraw();
        }
    });

    let pointer_moved_controller = Rc::clone(&controller);
    let pointer_moved_window = window.as_weak();
    window.on_web_view_pointer_moved(move |x, y| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        pointer_moved_controller
            .borrow_mut()
            .queue_command(BrowserCommand::PointerMoved { x, y });
        if let Some(window) = pointer_moved_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let pointer_button_controller = Rc::clone(&controller);
    let pointer_button_window = window.as_weak();
    window.on_web_view_pointer_button(move |button, pressed, x, y| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let action = if pressed {
            PointerButtonAction::Down
        } else {
            PointerButtonAction::Up
        };
        let tab_id = pointer_button_controller.borrow().active_tab_id();
        if is_activation_press(action) {
            crate::activation::note_user_activation(tab_id, std::time::Instant::now()); // activation (tabs lane)
        }
        pointer_button_controller
            .borrow_mut()
            .queue_command(BrowserCommand::PointerButton {
                action,
                // controls: a macOS Control-click becomes a secondary click.
                button: crate::controls::pointer_button(pointer_button_from_ui(button), action),
                x,
                y,
            });
        if let Some(window) = pointer_button_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let scroll_controller = Rc::clone(&controller);
    let scroll_window = window.as_weak();
    window.on_web_view_scrolled(move |delta_x, delta_y, x, y| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        scroll_controller
            .borrow_mut()
            .queue_command(BrowserCommand::Scrolled {
                delta_x,
                delta_y,
                x,
                y,
            });
        if let Some(window) = scroll_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let pointer_left_controller = Rc::clone(&controller);
    let pointer_left_window = window.as_weak();
    window.on_web_view_pointer_left(move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        pointer_left_controller
            .borrow_mut()
            .queue_command(BrowserCommand::PointerLeft);
        if let Some(window) = pointer_left_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let focus_controller = controller;
    let focus_window = window.as_weak();
    window.on_web_view_focus_changed(move |focused| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        web_view_focused.set(focused);
        focus_controller
            .borrow_mut()
            .queue_command(BrowserCommand::FocusChanged { focused });
        if let Some(window) = focus_window.upgrade() {
            let _native_window = window
                .window()
                .with_winit_window(|native_window| native_window.set_ime_allowed(focused));
            window.window().request_redraw();
        }
    });
}

/// Only a press counts as a gesture (HTML's activation-triggering events are `mousedown` and
/// `pointerdown`), so one click is one activation.
fn is_activation_press(action: PointerButtonAction) -> bool {
    action == PointerButtonAction::Down
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use talaria_core::TabId;

    use super::*;
    use crate::activation::{consume_user_activation, note_user_activation};

    /// The page's `mousedown` handler uses the press's activation; the release must not give
    /// its `click` handler another one.
    #[test]
    fn one_click_is_one_activation() {
        let tab = TabId::new(77);
        let now = Instant::now();
        let mut consumed = Vec::new();
        for action in [PointerButtonAction::Down, PointerButtonAction::Up] {
            if is_activation_press(action) {
                note_user_activation(tab, now);
            }
            consumed.push(consume_user_activation(tab, now));
        }

        assert_eq!(consumed, vec![true, false]);
    }
}
