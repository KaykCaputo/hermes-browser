use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use mercury_ui::MainWindow;
use slint::{ComponentHandle, Image, Timer};

/// How long the previous tab's frame may stay on screen after a tab switch while the new tab's
/// first frame is on its way. Then the page area shows the window background instead.
pub(crate) const PREVIOUS_FRAME_GRACE: Duration = Duration::from_millis(150);

/// Bounds how long the previous tab's frame is shown after a switch. Each switch starts a
/// single-shot Slint timer; when it fires, the texture is cleared unless a frame of the new tab
/// was shown since, or a later switch started its own timer.
#[derive(Debug, Default)]
pub(crate) struct TabSwitchFrame {
    state: Rc<SwitchState>,
}

#[derive(Debug, Default)]
struct SwitchState {
    /// Counts switches, so an older timer does nothing.
    generation: Cell<u64>,
    /// The switch whose first frame has not been shown yet.
    waiting_for: Cell<Option<u64>>,
}

impl SwitchState {
    fn begin(&self) -> u64 {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        self.waiting_for.set(Some(generation));
        generation
    }

    fn frame_shown(&self) {
        self.waiting_for.set(None);
    }

    /// The timer of switch `generation` fired: returns whether to clear the texture.
    fn expire(&self, generation: u64) -> bool {
        if self.waiting_for.get() != Some(generation) {
            return false;
        }
        self.waiting_for.set(None);
        true
    }
}

impl TabSwitchFrame {
    /// The active tab changed: keep the current frame for at most `PREVIOUS_FRAME_GRACE`.
    pub(crate) fn begin(&self, window: &MainWindow) {
        let generation = self.state.begin();
        let state = Rc::clone(&self.state);
        let window = window.as_weak();
        Timer::single_shot(PREVIOUS_FRAME_GRACE, move || {
            let _slint = crate::callback_scope::InSlintCallback::enter();
            if state.expire(generation)
                && let Some(window) = window.upgrade()
            {
                window.set_web_view_texture(Image::default());
            }
        });
    }

    /// A frame of the active tab was put on screen.
    pub(crate) fn frame_shown(&self) {
        self.state.frame_shown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_previous_frame_is_cleared_if_no_new_frame_came_in_time() {
        let state = SwitchState::default();
        let generation = state.begin();

        assert!(state.expire(generation));
        assert!(!state.expire(generation));
    }

    #[test]
    fn a_new_frame_in_time_keeps_the_texture() {
        let state = SwitchState::default();
        let generation = state.begin();

        state.frame_shown();

        assert!(!state.expire(generation));
    }

    #[test]
    fn an_earlier_switchs_timer_does_nothing() {
        let state = SwitchState::default();
        let first = state.begin();
        let second = state.begin();

        assert!(!state.expire(first));
        assert!(state.expire(second));
    }
}
