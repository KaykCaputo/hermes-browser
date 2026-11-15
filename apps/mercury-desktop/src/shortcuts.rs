use std::cell::{Cell, RefCell};

use mercury_ui::MainWindow;
use slint::ComponentHandle;
use slint::winit_030::winit;
use talaria_core::{BrowserCommand, BrowserController};
use winit::event::ElementState;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::ui_sync::sync_ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    MacOs,
    Other,
}

impl Platform {
    pub(crate) const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShortcutAction {
    NewTab,
    CloseTab,
    NextTab,
    PreviousTab,
    /// Selects the tab at this zero-based index.
    SelectTab(usize),
    SelectLastTab,
    Back,
    Forward,
    Reload,
    FocusAddressBar,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Quit,
}

/// Maps a key press to a browser shortcut. "Primary" is Cmd on macOS and Ctrl elsewhere; the
/// other of the two only takes part in Ctrl+Tab, so macOS Ctrl combinations reach the page.
pub(crate) fn shortcut_action(
    key: &Key,
    modifiers: ModifiersState,
    platform: Platform,
) -> Option<ShortcutAction> {
    use ShortcutAction::*;

    let control = modifiers.control_key();
    let shift = modifiers.shift_key();
    let alt = modifiers.alt_key();
    let command = modifiers.super_key();

    if matches!(key, Key::Named(NamedKey::Tab)) && control && !alt && !command {
        return Some(if shift { PreviousTab } else { NextTab });
    }

    let (primary, secondary) = match platform {
        Platform::MacOs => (command, control),
        Platform::Other => (control, command),
    };
    if secondary {
        return None;
    }

    let character = match key {
        Key::Character(character) => Some(character.to_lowercase()),
        _ => None,
    };
    let character = character.as_deref();

    if primary && !alt {
        let common = match (character, shift) {
            (Some("t"), false) => Some(NewTab),
            (Some("w"), false) => Some(CloseTab),
            (Some("r"), false) => Some(Reload),
            (Some("l"), false) => Some(FocusAddressBar),
            (Some("q"), false) => Some(Quit),
            (Some("=" | "+"), _) => Some(ZoomIn),
            (Some("-"), false) => Some(ZoomOut),
            (Some("0"), false) => Some(ZoomReset),
            (Some("9"), false) => Some(SelectLastTab),
            (Some(digit), false) => match digit.parse::<usize>() {
                Ok(number @ 1..=8) => Some(SelectTab(number - 1)),
                _ => None,
            },
            _ => None,
        };
        if common.is_some() {
            return common;
        }

        return match (platform, key, character, shift) {
            (Platform::MacOs, _, Some("["), false) => Some(Back),
            (Platform::MacOs, _, Some("]"), false) => Some(Forward),
            (Platform::MacOs, _, Some("[" | "{"), true) => Some(PreviousTab),
            (Platform::MacOs, _, Some("]" | "}"), true) => Some(NextTab),
            (Platform::MacOs, Key::Named(NamedKey::ArrowLeft), _, false) => Some(Back),
            (Platform::MacOs, Key::Named(NamedKey::ArrowRight), _, false) => Some(Forward),
            (Platform::Other, Key::Named(NamedKey::PageDown), _, false) => Some(NextTab),
            (Platform::Other, Key::Named(NamedKey::PageUp), _, false) => Some(PreviousTab),
            _ => None,
        };
    }

    if platform != Platform::Other || primary || shift {
        return None;
    }
    match (key, character, alt) {
        (Key::Named(NamedKey::ArrowLeft), _, true) => Some(Back),
        (Key::Named(NamedKey::ArrowRight), _, true) => Some(Forward),
        (_, Some("d"), true) => Some(FocusAddressBar),
        (Key::Named(NamedKey::F5), _, false) => Some(Reload),
        _ => None,
    }
}

/// Cmd+Left/Right moves to the start or end of a line in a text field (the address bar, or a
/// field in the page, which Servo handles). Focus inside the page cannot be known, so on macOS
/// these keys are only browser shortcuts while neither the browser UI nor the page has focus.
pub(crate) fn yields_to_focused_text(key: &Key, platform: Platform) -> bool {
    platform == Platform::MacOs
        && matches!(key, Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight))
}

pub(crate) fn handle_browser_shortcut(
    window: &MainWindow,
    controller: &RefCell<BrowserController>,
    web_view_focused: &Cell<bool>,
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
) -> bool {
    let platform = Platform::current();
    let Some(action) = shortcut_action(&event.logical_key, modifiers, platform) else {
        return false;
    };
    if yields_to_focused_text(&event.logical_key, platform)
        && (web_view_focused.get() || window.get_text_input_focused())
    {
        return false;
    }

    if event.state == ElementState::Pressed {
        run_shortcut(window, controller, web_view_focused, action);
        window.window().request_redraw();
    }
    true
}

fn run_shortcut(
    window: &MainWindow,
    controller: &RefCell<BrowserController>,
    web_view_focused: &Cell<bool>,
    action: ShortcutAction,
) {
    match action {
        ShortcutAction::NewTab => {
            unfocus_web_view(controller, web_view_focused);
            controller.borrow_mut().open_tab();
            sync_ui(window, &controller.borrow());
            window.invoke_focus_address_bar();
        }
        ShortcutAction::CloseTab => {
            let active_tab_id = controller.borrow().active_tab_id();
            controller.borrow_mut().close_tab(active_tab_id);
            sync_ui(window, &controller.borrow());
        }
        ShortcutAction::NextTab | ShortcutAction::PreviousTab => {
            controller
                .borrow_mut()
                .select_next_tab(action == ShortcutAction::PreviousTab);
            sync_ui(window, &controller.borrow());
        }
        ShortcutAction::SelectTab(index) => {
            if controller.borrow_mut().select_tab_at(index) {
                sync_ui(window, &controller.borrow());
            }
        }
        ShortcutAction::SelectLastTab => {
            controller.borrow_mut().select_last_tab();
            sync_ui(window, &controller.borrow());
        }
        ShortcutAction::Back => controller
            .borrow_mut()
            .queue_command(BrowserCommand::GoBack),
        ShortcutAction::Forward => controller
            .borrow_mut()
            .queue_command(BrowserCommand::GoForward),
        ShortcutAction::Reload => controller
            .borrow_mut()
            .queue_command(BrowserCommand::Reload),
        ShortcutAction::FocusAddressBar => {
            unfocus_web_view(controller, web_view_focused);
            window.invoke_focus_address_bar();
        }
        ShortcutAction::ZoomIn | ShortcutAction::ZoomOut | ShortcutAction::ZoomReset => {
            {
                let mut controller = controller.borrow_mut();
                match action {
                    ShortcutAction::ZoomIn => controller.zoom_in(),
                    ShortcutAction::ZoomOut => controller.zoom_out(),
                    _ => controller.reset_zoom(),
                }
            }
            sync_ui(window, &controller.borrow());
        }
        ShortcutAction::Quit => {
            if let Err(error) = slint::quit_event_loop() {
                log::warn!("could not quit: {error}");
            }
        }
    }
}

fn unfocus_web_view(controller: &RefCell<BrowserController>, web_view_focused: &Cell<bool>) {
    if web_view_focused.replace(false) {
        controller
            .borrow_mut()
            .queue_command(BrowserCommand::FocusChanged { focused: false });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ShortcutAction::*;

    const NONE: ModifiersState = ModifiersState::empty();
    const CMD: ModifiersState = ModifiersState::SUPER;
    const CTRL: ModifiersState = ModifiersState::CONTROL;
    const ALT: ModifiersState = ModifiersState::ALT;
    const SHIFT: ModifiersState = ModifiersState::SHIFT;

    fn character(text: &str) -> Key {
        Key::Character(text.into())
    }

    fn named(key: NamedKey) -> Key {
        Key::Named(key)
    }

    fn mac(key: Key, modifiers: ModifiersState) -> Option<ShortcutAction> {
        shortcut_action(&key, modifiers, Platform::MacOs)
    }

    fn other(key: Key, modifiers: ModifiersState) -> Option<ShortcutAction> {
        shortcut_action(&key, modifiers, Platform::Other)
    }

    #[test]
    fn new_and_close_tab_on_macos() {
        assert_eq!(mac(character("t"), CMD), Some(NewTab));
        assert_eq!(mac(character("w"), CMD), Some(CloseTab));
    }

    #[test]
    fn new_and_close_tab_elsewhere() {
        assert_eq!(other(character("t"), CTRL), Some(NewTab));
        assert_eq!(other(character("w"), CTRL), Some(CloseTab));
    }

    #[test]
    fn next_and_previous_tab_on_macos() {
        assert_eq!(mac(named(NamedKey::Tab), CTRL), Some(NextTab));
        assert_eq!(mac(named(NamedKey::Tab), CTRL | SHIFT), Some(PreviousTab));
        assert_eq!(mac(character("]"), CMD | SHIFT), Some(NextTab));
        assert_eq!(mac(character("["), CMD | SHIFT), Some(PreviousTab));
        // macOS reports the shifted character while Cmd is held.
        assert_eq!(mac(character("}"), CMD | SHIFT), Some(NextTab));
        assert_eq!(mac(character("{"), CMD | SHIFT), Some(PreviousTab));
    }

    #[test]
    fn next_and_previous_tab_elsewhere() {
        assert_eq!(other(named(NamedKey::Tab), CTRL), Some(NextTab));
        assert_eq!(other(named(NamedKey::Tab), CTRL | SHIFT), Some(PreviousTab));
        assert_eq!(other(named(NamedKey::PageDown), CTRL), Some(NextTab));
        assert_eq!(other(named(NamedKey::PageUp), CTRL), Some(PreviousTab));
    }

    #[test]
    fn numbered_tabs_on_macos() {
        assert_eq!(mac(character("1"), CMD), Some(SelectTab(0)));
        assert_eq!(mac(character("8"), CMD), Some(SelectTab(7)));
    }

    #[test]
    fn numbered_tabs_elsewhere() {
        assert_eq!(other(character("1"), CTRL), Some(SelectTab(0)));
        assert_eq!(other(character("8"), CTRL), Some(SelectTab(7)));
        assert_eq!(other(character("9"), CTRL), Some(SelectLastTab));
    }

    #[test]
    fn command_9_selects_the_last_tab() {
        assert_eq!(mac(character("9"), CMD), Some(SelectLastTab));
    }

    #[test]
    fn back_and_forward_on_macos() {
        assert_eq!(mac(character("["), CMD), Some(Back));
        assert_eq!(mac(character("]"), CMD), Some(Forward));
        assert_eq!(mac(named(NamedKey::ArrowLeft), CMD), Some(Back));
        assert_eq!(mac(named(NamedKey::ArrowRight), CMD), Some(Forward));
    }

    #[test]
    fn back_and_forward_elsewhere() {
        assert_eq!(other(named(NamedKey::ArrowLeft), ALT), Some(Back));
        assert_eq!(other(named(NamedKey::ArrowRight), ALT), Some(Forward));
    }

    #[test]
    fn option_arrows_are_left_to_the_page_on_macos() {
        assert_eq!(mac(named(NamedKey::ArrowLeft), ALT), None);
        assert_eq!(mac(named(NamedKey::ArrowRight), ALT), None);
    }

    #[test]
    fn command_arrows_yield_to_focused_text_only_on_macos() {
        assert!(yields_to_focused_text(
            &named(NamedKey::ArrowLeft),
            Platform::MacOs
        ));
        assert!(yields_to_focused_text(
            &named(NamedKey::ArrowRight),
            Platform::MacOs
        ));
        assert!(!yields_to_focused_text(&character("["), Platform::MacOs));
        assert!(!yields_to_focused_text(
            &named(NamedKey::ArrowLeft),
            Platform::Other
        ));
    }

    #[test]
    fn reload_on_macos() {
        assert_eq!(mac(character("r"), CMD), Some(Reload));
    }

    #[test]
    fn reload_elsewhere() {
        assert_eq!(other(character("r"), CTRL), Some(Reload));
        assert_eq!(other(named(NamedKey::F5), NONE), Some(Reload));
    }

    #[test]
    fn focus_address_bar_on_macos() {
        assert_eq!(mac(character("l"), CMD), Some(FocusAddressBar));
    }

    #[test]
    fn focus_address_bar_elsewhere() {
        assert_eq!(other(character("l"), CTRL), Some(FocusAddressBar));
        assert_eq!(other(character("d"), ALT), Some(FocusAddressBar));
    }

    #[test]
    fn zoom_on_macos() {
        assert_eq!(mac(character("="), CMD), Some(ZoomIn));
        assert_eq!(mac(character("+"), CMD | SHIFT), Some(ZoomIn));
        assert_eq!(mac(character("-"), CMD), Some(ZoomOut));
        assert_eq!(mac(character("0"), CMD), Some(ZoomReset));
    }

    #[test]
    fn zoom_elsewhere() {
        assert_eq!(other(character("="), CTRL), Some(ZoomIn));
        assert_eq!(other(character("+"), CTRL | SHIFT), Some(ZoomIn));
        assert_eq!(other(character("-"), CTRL), Some(ZoomOut));
        assert_eq!(other(character("0"), CTRL), Some(ZoomReset));
    }

    #[test]
    fn quit_on_macos() {
        assert_eq!(mac(character("q"), CMD), Some(Quit));
    }

    #[test]
    fn quit_elsewhere() {
        assert_eq!(other(character("q"), CTRL), Some(Quit));
    }

    #[test]
    fn control_w_is_not_a_shortcut_on_macos() {
        assert_eq!(mac(character("w"), CTRL), None);
        assert_eq!(mac(character("r"), CTRL), None);
        assert_eq!(mac(character("1"), CTRL), None);
    }

    #[test]
    fn command_letters_are_not_shortcuts_elsewhere() {
        assert_eq!(other(character("w"), CMD), None);
        assert_eq!(other(character("["), CTRL), None);
        assert_eq!(other(named(NamedKey::ArrowLeft), CTRL), None);
    }

    #[test]
    fn macos_does_not_use_the_other_platforms_keys() {
        assert_eq!(mac(named(NamedKey::F5), NONE), None);
        assert_eq!(mac(character("d"), ALT), None);
        assert_eq!(mac(named(NamedKey::PageDown), CMD), None);
    }

    #[test]
    fn extra_modifiers_turn_a_shortcut_off() {
        assert_eq!(mac(character("t"), CMD | SHIFT), None);
        assert_eq!(mac(character("t"), CMD | ALT), None);
        assert_eq!(other(character("w"), CTRL | SHIFT), None);
    }

    #[test]
    fn uppercase_letters_from_caps_lock_still_match() {
        assert_eq!(mac(character("T"), CMD), Some(NewTab));
    }

    #[test]
    fn plain_letters_are_not_shortcuts() {
        for platform in [Platform::MacOs, Platform::Other] {
            for letter in ["t", "w", "r", "l", "q", "d", "1", "9", "=", "-", "0", "["] {
                assert_eq!(
                    shortcut_action(&character(letter), NONE, platform),
                    None,
                    "{letter}"
                );
                assert_eq!(
                    shortcut_action(&character(letter), SHIFT, platform),
                    None,
                    "Shift+{letter}"
                );
            }
            assert_eq!(shortcut_action(&named(NamedKey::Tab), NONE, platform), None);
            assert_eq!(
                shortcut_action(&named(NamedKey::ArrowLeft), NONE, platform),
                None
            );
        }
    }
}
