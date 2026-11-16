//! Marks the code Mercury runs from Slint's callbacks, so that code reached from outside Slint
//! (an AppKit callback dispatched from a nested run loop, such as a modal panel's, opened by a
//! Slint callback) can tell that Slint is in the middle of a callback and may hold its state
//! borrowed.
//!
//! Every closure Mercury hands to Slint (window and UI callbacks, the rendering notifier, the
//! winit event hook, timers, `invoke_from_event_loop` closures) starts with
//! `let _slint = InSlintCallback::enter();`.

use std::cell::Cell;

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Held for the duration of one Slint callback.
pub(crate) struct InSlintCallback(());

impl InSlintCallback {
    pub(crate) fn enter() -> Self {
        DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self(())
    }
}

impl Drop for InSlintCallback {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Whether a Slint callback of Mercury's is running on this thread.
pub(crate) fn inside_slint_callback() -> bool {
    DEPTH.with(|depth| depth.get() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_is_marked_while_a_callback_runs_including_nested_ones() {
        assert!(!inside_slint_callback());
        let outer = InSlintCallback::enter();
        {
            let _inner = InSlintCallback::enter();
            assert!(inside_slint_callback());
        }
        assert!(inside_slint_callback());
        drop(outer);
        assert!(!inside_slint_callback());
    }

    #[test]
    fn the_mark_is_removed_when_a_callback_unwinds() {
        let unwound = std::panic::catch_unwind(|| {
            let _scope = InSlintCallback::enter();
            panic!("deliberate panic inside a callback");
        });

        assert!(unwound.is_err());
        assert!(!inside_slint_callback());
    }
}
