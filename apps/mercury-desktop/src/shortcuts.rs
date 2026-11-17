use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use mercury_ui::MainWindow;
use slint::ComponentHandle;
use slint::winit_030::winit;
use talaria_core::{BrowserCommand, BrowserController};
use winit::event::ElementState;
use winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};

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
    /// Escape while the active tab's page is fullscreen.
    ExitFullscreen,
    // settings-ui
    ShowSettings,
}

impl ShortcutAction {
    /// Whether holding the keys down repeats the action. Repeats of other shortcuts are still
    /// consumed, so the page never sees them, but do nothing.
    pub(crate) const fn repeats(self) -> bool {
        matches!(
            self,
            Self::ZoomIn | Self::ZoomOut | Self::NextTab | Self::PreviousTab
        )
    }
}

/// What to do with one key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyDisposition {
    /// Not a browser shortcut: give it to whoever has focus.
    Forward,
    /// Part of a shortcut that has already run: swallow it.
    Consume,
    /// Run this shortcut and swallow the event.
    Run(ShortcutAction),
}

/// Pairs key releases (and auto-repeats) with presses: a release is swallowed exactly when its
/// press ran a shortcut, whatever modifiers are held by then, and every other release is
/// forwarded. Shortcuts only start on a first press, never on a repeat.
#[derive(Debug, Default)]
pub(crate) struct ShortcutKeys {
    consumed: HashSet<PhysicalKey>,
}

impl ShortcutKeys {
    /// `action` is the shortcut the key would trigger with the modifiers held right now.
    pub(crate) fn press(
        &mut self,
        key: PhysicalKey,
        repeat: bool,
        action: Option<ShortcutAction>,
    ) -> KeyDisposition {
        if repeat {
            if !self.consumed.contains(&key) {
                return KeyDisposition::Forward;
            }
            return match action {
                Some(action) if action.repeats() => KeyDisposition::Run(action),
                _ => KeyDisposition::Consume,
            };
        }

        match action {
            Some(action) => {
                self.consumed.insert(key);
                KeyDisposition::Run(action)
            }
            None => {
                self.consumed.remove(&key);
                KeyDisposition::Forward
            }
        }
    }

    pub(crate) fn release(&mut self, key: PhysicalKey) -> KeyDisposition {
        if self.consumed.remove(&key) {
            KeyDisposition::Consume
        } else {
            KeyDisposition::Forward
        }
    }

    /// Forgets every pressed shortcut key, for when key-ups can no longer be relied on.
    pub(crate) fn clear(&mut self) {
        self.consumed.clear();
    }

    // menus
    /// The native menu bar ran an action for this key's press, which therefore never reached
    /// the window: swallow its release as if the press had run a shortcut here.
    pub(crate) fn claim(&mut self, key: PhysicalKey) {
        self.consumed.insert(key);
    }
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
            // settings-ui
            (Some(","), false) => Some(ShowSettings),
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

/// Escape leaves page-requested fullscreen and is then not given to the page.
pub(crate) fn exits_fullscreen(key: &Key, page_fullscreen: bool) -> bool {
    page_fullscreen && matches!(key, Key::Named(NamedKey::Escape))
}

/// The browser action for a key press, including Escape while a page is fullscreen.
pub(crate) fn browser_action(
    key: &Key,
    modifiers: ModifiersState,
    platform: Platform,
    page_fullscreen: bool,
) -> Option<ShortcutAction> {
    if exits_fullscreen(key, page_fullscreen) {
        return Some(ShortcutAction::ExitFullscreen);
    }
    shortcut_action(key, modifiers, platform)
}

/// Returns true if the event belongs to a browser shortcut and must not reach the page.
pub(crate) fn handle_browser_shortcut(
    window: &MainWindow,
    controller: &RefCell<BrowserController>,
    web_view_focused: &Cell<bool>,
    shortcut_keys: &mut ShortcutKeys,
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
) -> bool {
    let disposition = match event.state {
        ElementState::Released => shortcut_keys.release(event.physical_key),
        ElementState::Pressed => {
            let platform = Platform::current();
            let page_fullscreen = controller.borrow().active_state().is_fullscreen();
            let action = browser_action(&event.logical_key, modifiers, platform, page_fullscreen)
                .filter(|_| {
                    !(yields_to_focused_text(&event.logical_key, platform)
                        && (web_view_focused.get() || window.get_text_input_focused()))
                });
            shortcut_keys.press(event.physical_key, event.repeat, action)
        }
    };

    match disposition {
        KeyDisposition::Forward => false,
        KeyDisposition::Consume => true,
        KeyDisposition::Run(action) => {
            run_shortcut(window, controller, web_view_focused, action);
            window.window().request_redraw();
            true
        }
    }
}

pub(crate) fn run_shortcut(
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
            let outcome = controller.borrow_mut().close_tab(active_tab_id);
            // tabs: closing the last tab quits
            crate::tab_close::finish_closing_tab(window, &controller.borrow(), outcome);
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
        ShortcutAction::ExitFullscreen => controller
            .borrow_mut()
            .queue_command(BrowserCommand::ExitFullscreen),
        // settings-ui
        ShortcutAction::ShowSettings => window.invoke_show_settings(),
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
    use winit::keyboard::KeyCode;

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
    fn settings_open_with_primary_comma() {
        assert_eq!(mac(character(","), CMD), Some(ShowSettings));
        assert_eq!(other(character(","), CTRL), Some(ShowSettings));
        assert_eq!(mac(character(","), CTRL), None);
        assert_eq!(mac(character(","), NONE), None);
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
    fn escape_exits_fullscreen_only_while_a_page_is_fullscreen() {
        assert!(exits_fullscreen(&named(NamedKey::Escape), true));
        assert!(!exits_fullscreen(&named(NamedKey::Escape), false));
        assert!(!exits_fullscreen(&character("q"), true));
        assert_eq!(
            browser_action(&named(NamedKey::Escape), NONE, Platform::MacOs, true),
            Some(ExitFullscreen)
        );
        assert_eq!(
            browser_action(&named(NamedKey::Escape), NONE, Platform::MacOs, false),
            None
        );
        assert_eq!(
            browser_action(&character("t"), CMD, Platform::MacOs, true),
            Some(NewTab)
        );
    }

    #[test]
    fn only_zoom_and_tab_cycling_repeat() {
        let expectations = [
            (NewTab, false),
            (CloseTab, false),
            (NextTab, true),
            (PreviousTab, true),
            (SelectTab(0), false),
            (SelectTab(7), false),
            (SelectLastTab, false),
            (Back, false),
            (Forward, false),
            (Reload, false),
            (FocusAddressBar, false),
            (ZoomIn, true),
            (ZoomOut, true),
            (ZoomReset, false),
            (Quit, false),
            (ExitFullscreen, false),
            (ShowSettings, false),
        ];
        for (action, repeats) in expectations {
            // Exhaustive, so a new variant cannot be added without deciding here.
            match action {
                NewTab | CloseTab | NextTab | PreviousTab | SelectTab(_) | SelectLastTab | Back
                | Forward | Reload | FocusAddressBar | ZoomIn | ZoomOut | ZoomReset | Quit
                | ExitFullscreen | ShowSettings => {}
            }
            assert_eq!(action.repeats(), repeats, "{action:?}");
        }
    }

    const KEY_R: PhysicalKey = PhysicalKey::Code(KeyCode::KeyR);
    const KEY_T: PhysicalKey = PhysicalKey::Code(KeyCode::KeyT);
    const KEY_EQUAL: PhysicalKey = PhysicalKey::Code(KeyCode::Equal);
    const COMMAND: PhysicalKey = PhysicalKey::Code(KeyCode::SuperLeft);

    #[test]
    fn a_shortcut_key_released_after_its_modifier_is_still_swallowed() {
        let mut keys = ShortcutKeys::default();

        assert_eq!(keys.press(COMMAND, false, None), KeyDisposition::Forward);
        assert_eq!(
            keys.press(KEY_R, false, Some(Reload)),
            KeyDisposition::Run(Reload)
        );
        assert_eq!(keys.release(COMMAND), KeyDisposition::Forward);
        assert_eq!(keys.release(KEY_R), KeyDisposition::Consume);
    }

    #[test]
    fn a_key_pressed_before_its_modifier_gets_its_release() {
        let mut keys = ShortcutKeys::default();

        assert_eq!(keys.press(KEY_R, false, None), KeyDisposition::Forward);
        assert_eq!(keys.press(COMMAND, false, None), KeyDisposition::Forward);
        assert_eq!(keys.release(KEY_R), KeyDisposition::Forward);
        assert_eq!(keys.release(COMMAND), KeyDisposition::Forward);
    }

    #[test]
    fn a_release_is_swallowed_only_once() {
        let mut keys = ShortcutKeys::default();
        keys.press(KEY_R, false, Some(Reload));

        assert_eq!(keys.release(KEY_R), KeyDisposition::Consume);
        assert_eq!(keys.press(KEY_R, false, None), KeyDisposition::Forward);
        assert_eq!(keys.release(KEY_R), KeyDisposition::Forward);
    }

    #[test]
    fn repeats_run_only_repeating_actions() {
        let mut keys = ShortcutKeys::default();
        keys.press(KEY_T, false, Some(NewTab));
        keys.press(KEY_EQUAL, false, Some(ZoomIn));

        assert_eq!(
            keys.press(KEY_T, true, Some(NewTab)),
            KeyDisposition::Consume
        );
        assert_eq!(
            keys.press(KEY_EQUAL, true, Some(ZoomIn)),
            KeyDisposition::Run(ZoomIn)
        );
    }

    #[test]
    fn repeats_of_a_shortcut_stay_swallowed_after_the_modifier_is_released() {
        let mut keys = ShortcutKeys::default();
        keys.press(KEY_EQUAL, false, Some(ZoomIn));

        assert_eq!(keys.press(KEY_EQUAL, true, None), KeyDisposition::Consume);
        assert_eq!(keys.release(KEY_EQUAL), KeyDisposition::Consume);
    }

    #[test]
    fn a_repeat_never_starts_a_shortcut() {
        let mut keys = ShortcutKeys::default();
        keys.press(KEY_R, false, None);

        assert_eq!(
            keys.press(KEY_R, true, Some(Reload)),
            KeyDisposition::Forward
        );
        assert_eq!(keys.release(KEY_R), KeyDisposition::Forward);
    }

    // menus
    #[test]
    fn a_key_the_native_menu_claimed_has_its_release_swallowed() {
        let mut keys = ShortcutKeys::default();

        keys.claim(KEY_T);

        assert_eq!(keys.release(KEY_T), KeyDisposition::Consume);
        assert_eq!(keys.release(KEY_T), KeyDisposition::Forward);
    }

    #[test]
    fn a_later_press_of_a_claimed_key_is_handled_normally() {
        let mut keys = ShortcutKeys::default();
        keys.claim(KEY_T);

        assert_eq!(keys.press(KEY_T, false, None), KeyDisposition::Forward);
        assert_eq!(keys.release(KEY_T), KeyDisposition::Forward);
    }

    #[test]
    fn clearing_forwards_later_releases() {
        let mut keys = ShortcutKeys::default();
        keys.press(KEY_R, false, Some(Reload));

        keys.clear();

        assert_eq!(keys.release(KEY_R), KeyDisposition::Forward);
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
