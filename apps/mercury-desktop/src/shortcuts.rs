use std::cell::{Cell, RefCell};

use mercury_ui::MainWindow;
use slint::ComponentHandle;
use slint::winit_030::winit;
use talaria_core::{BrowserCommand, BrowserController};
use winit::event::ElementState;
use winit::keyboard::{Key, ModifiersState};

use crate::ui_sync::sync_ui;

pub(crate) fn handle_browser_shortcut(
    window: &MainWindow,
    controller: &RefCell<BrowserController>,
    web_view_focused: &Cell<bool>,
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
) -> bool {
    let primary = modifiers.control_key() || modifiers.super_key();
    let character = match &event.logical_key {
        Key::Character(character) => Some(character.to_ascii_lowercase()),
        _ => None,
    };
    let is_pressed = event.state == ElementState::Pressed;

    let shortcut = if primary && character.as_deref() == Some("l") {
        if is_pressed {
            if web_view_focused.replace(false) {
                controller
                    .borrow_mut()
                    .queue_command(BrowserCommand::FocusChanged { focused: false });
            }
            window.invoke_focus_address_bar();
        }
        true
    } else if primary && character.as_deref() == Some("t") {
        if is_pressed {
            if web_view_focused.replace(false) {
                controller
                    .borrow_mut()
                    .queue_command(BrowserCommand::FocusChanged { focused: false });
            }
            controller.borrow_mut().open_tab();
            sync_ui(window, &controller.borrow());
            window.invoke_focus_address_bar();
        }
        true
    } else if primary && character.as_deref() == Some("w") {
        if is_pressed {
            let active_tab_id = controller.borrow().active_tab_id();
            controller.borrow_mut().close_tab(active_tab_id);
            sync_ui(window, &controller.borrow());
        }
        true
    } else if primary
        && matches!(
            event.logical_key,
            Key::Named(winit::keyboard::NamedKey::Tab)
        )
    {
        if is_pressed {
            controller
                .borrow_mut()
                .select_next_tab(modifiers.shift_key());
            sync_ui(window, &controller.borrow());
        }
        true
    } else if (primary && character.as_deref() == Some("r"))
        || matches!(event.logical_key, Key::Named(winit::keyboard::NamedKey::F5))
    {
        if is_pressed {
            controller
                .borrow_mut()
                .queue_command(BrowserCommand::Reload);
        }
        true
    } else if modifiers.alt_key()
        && matches!(
            event.logical_key,
            Key::Named(winit::keyboard::NamedKey::ArrowLeft)
        )
    {
        if is_pressed {
            controller
                .borrow_mut()
                .queue_command(BrowserCommand::GoBack);
        }
        true
    } else if modifiers.alt_key()
        && matches!(
            event.logical_key,
            Key::Named(winit::keyboard::NamedKey::ArrowRight)
        )
    {
        if is_pressed {
            controller
                .borrow_mut()
                .queue_command(BrowserCommand::GoForward);
        }
        true
    } else {
        false
    };

    if shortcut && is_pressed {
        window.window().request_redraw();
    }
    shortcut
}
