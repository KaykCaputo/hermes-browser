// User activation per tab (tabs lane).
//
// Servo 0.8.0 does not tell the embedder whether a page request follows a user gesture: it
// asks for every page-opened tab without activation information, and it opens the file panel
// and colour picker from a script `click()`. The browser therefore tracks activation itself,
// from the input it forwards to each tab's page.
//
// Rules: a pointer *press* activates (its release does not, so one click is one activation);
// a key press activates unless it is an auto-repeat, synthetic (winit's `is_synthetic`, sent
// for keys already held when the window gains focus), Escape, or a modifier-only key; keys
// consumed as browser shortcuts never reach the page and never activate. An activation lasts
// `ACTIVATION_WINDOW`, is used up by the first request it allows, and does not survive the tab
// starting to load a new document.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use slint::winit_030::winit::event::ElementState;
use slint::winit_030::winit::keyboard::{Key, NamedKey};
use talaria_core::TabId;

pub(crate) const ACTIVATION_WINDOW: Duration = Duration::from_secs(1);

thread_local! {
    static ACTIVATIONS: RefCell<Activations> = RefCell::new(Activations::default());
}

/// Called where a trusted pointer press/release or a key press is forwarded to this tab's web view
/// (not Escape, not modifier-only keys, not keys consumed as browser shortcuts).
pub(crate) fn note_user_activation(tab: TabId, now: Instant) {
    ACTIVATIONS.with(|activations| activations.borrow_mut().note(tab, now));
}

/// True, and the activation is used up, if the tab had one within ACTIVATION_WINDOW.
pub(crate) fn consume_user_activation(tab: TabId, now: Instant) -> bool {
    ACTIVATIONS.with(|activations| activations.borrow_mut().consume(tab, now))
}

/// Drops a closed tab's entry.
pub(crate) fn forget_tab(tab: TabId) {
    ACTIVATIONS.with(|activations| activations.borrow_mut().forget(tab));
}

/// Whether a key event forwarded to a page counts as an activation: a first, real press of
/// anything but Escape or a modifier-only key.
pub(crate) fn is_activating_key(
    key: &Key,
    state: ElementState,
    repeat: bool,
    synthetic: bool,
) -> bool {
    state == ElementState::Pressed
        && !repeat
        && !synthetic
        && !matches!(
            key,
            Key::Named(
                NamedKey::Escape
                    | NamedKey::Shift
                    | NamedKey::Control
                    | NamedKey::Alt
                    | NamedKey::AltGraph
                    | NamedKey::Super
                    | NamedKey::Meta
                    | NamedKey::Hyper
                    | NamedKey::Fn
                    | NamedKey::FnLock
                    | NamedKey::CapsLock
                    | NamedKey::NumLock
                    | NamedKey::ScrollLock
                    | NamedKey::Symbol
                    | NamedKey::SymbolLock
            )
        )
}

/// Whether a pointer button event forwarded to a page counts as an activation: presses only.
pub(crate) fn is_activating_pointer_event(pressed: bool) -> bool {
    pressed
}

/// The latest activation of each tab.
#[derive(Debug, Default)]
pub(crate) struct Activations(HashMap<TabId, Instant>);

impl Activations {
    pub(crate) fn note(&mut self, tab: TabId, now: Instant) {
        self.0.insert(tab, now);
    }

    pub(crate) fn consume(&mut self, tab: TabId, now: Instant) -> bool {
        self.0
            .remove(&tab)
            .is_some_and(|activated| now.saturating_duration_since(activated) < ACTIVATION_WINDOW)
    }

    pub(crate) fn forget(&mut self, tab: TabId) {
        self.0.remove(&tab);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAB: TabId = TabId::new(1);

    #[test]
    fn an_activation_is_used_up_once() {
        let mut activations = Activations::default();
        let now = Instant::now();
        activations.note(TAB, now);

        assert!(activations.consume(TAB, now + Duration::from_millis(999)));
        assert!(!activations.consume(TAB, now + Duration::from_millis(999)));
    }

    #[test]
    fn an_activation_expires_after_the_window() {
        let mut activations = Activations::default();
        let now = Instant::now();
        activations.note(TAB, now);

        assert!(!activations.consume(TAB, now + ACTIVATION_WINDOW));
    }

    #[test]
    fn activations_belong_to_their_tab() {
        let mut activations = Activations::default();
        let now = Instant::now();
        activations.note(TAB, now);

        assert!(!activations.consume(TabId::new(2), now));
        assert!(activations.consume(TAB, now));
    }

    #[test]
    fn a_forgotten_tab_has_no_activation() {
        let mut activations = Activations::default();
        let now = Instant::now();
        activations.note(TAB, now);
        activations.forget(TAB);

        assert!(!activations.consume(TAB, now));
    }

    #[test]
    fn the_thread_api_notes_consumes_and_forgets() {
        let tab = TabId::new(4242);
        let now = Instant::now();
        note_user_activation(tab, now);
        assert!(consume_user_activation(tab, now));
        assert!(!consume_user_activation(tab, now));

        note_user_activation(tab, now);
        forget_tab(tab);
        assert!(!consume_user_activation(tab, now));
    }

    #[test]
    fn escape_modifiers_and_key_releases_are_not_activations() {
        assert!(is_activating_key(
            &Key::Character("a".into()),
            ElementState::Pressed,
            false,
            false
        ));
        assert!(is_activating_key(
            &Key::Named(NamedKey::Enter),
            ElementState::Pressed,
            false,
            false
        ));
        assert!(!is_activating_key(
            &Key::Character("a".into()),
            ElementState::Released,
            false,
            false
        ));
        assert!(!is_activating_key(
            &Key::Named(NamedKey::Escape),
            ElementState::Pressed,
            false,
            false
        ));
        for modifier in [
            NamedKey::Shift,
            NamedKey::Control,
            NamedKey::Alt,
            NamedKey::Super,
            NamedKey::Meta,
            NamedKey::Fn,
            NamedKey::CapsLock,
            NamedKey::AltGraph,
        ] {
            assert!(
                !is_activating_key(&Key::Named(modifier), ElementState::Pressed, false, false),
                "{modifier:?}"
            );
        }
        // Auto-repeat while a key is held, and synthetic presses, never activate.
        assert!(!is_activating_key(
            &Key::Named(NamedKey::ArrowDown),
            ElementState::Pressed,
            true,
            false
        ));
        assert!(!is_activating_key(
            &Key::Character(" ".into()),
            ElementState::Pressed,
            false,
            true
        ));
    }

    #[test]
    fn one_click_is_one_activation() {
        let mut activations = Activations::default();
        let now = Instant::now();
        for pressed in [true, false] {
            if is_activating_pointer_event(pressed) {
                activations.note(TAB, now);
            }
            // The page's mousedown handler uses the activation...
            if pressed {
                assert!(activations.consume(TAB, now));
            }
        }
        // ...and its click handler, after the release, gets none.
        assert!(!activations.consume(TAB, now));
    }
}
