use std::cell::RefCell;
use std::rc::{Rc, Weak};

use talaria_core::{BrowserController, CloseTabOutcome, TabId};
use talaria_servo::NewTabAllocator;

use crate::tab_close::quit_after_last_tab;

/// The decision point for the "block pop-ups" setting: whether a page in `opener` may open a
/// tab (`window.open`, `target=_blank`, Servo's "Open link in new view"). Today every request
/// is allowed.
///
/// Servo 0.8.0 asks the embedder for every such tab and tells it nothing about user activation:
/// `CreateNewWebViewRequest` carries only the responder, and Servo's own pop-up blocker step is a
/// TODO (servo-script 0.8.0, `dom/window/windowproxy.rs`, "Step 8"). A blocking setting can
/// therefore only refuse every page-opened tab, or judge by recent user input to `opener`.
pub(crate) fn pages_may_open_tabs(_opener: TabId) -> bool {
    true
}

/// Gives a page-opened web view its tab. Runs inside Servo's callback, which must answer at
/// once: the opener's script thread waits for it.
pub(crate) fn new_tab_allocator(controller: &Rc<RefCell<BrowserController>>) -> NewTabAllocator {
    let controller = Rc::downgrade(controller);
    Rc::new(move |opener| allocate_page_tab(&controller, opener))
}

fn allocate_page_tab(
    controller: &Weak<RefCell<BrowserController>>,
    opener: TabId,
) -> Option<TabId> {
    if !pages_may_open_tabs(opener) {
        log::info!("tab {}: blocked a tab opened by the page", opener.value());
        return None;
    }
    let controller = controller.upgrade()?;
    let Ok(mut controller) = controller.try_borrow_mut() else {
        log::warn!(
            "tab {}: refused a tab opened by the page: the browser state is busy",
            opener.value()
        );
        return None;
    };
    controller.adopt_tab(opener)
}

/// The page in `tab_id` closed its tab. Returns whether a tab was closed; closing the last tab
/// quits.
pub(crate) fn close_page_tab(controller: &mut BrowserController, tab_id: TabId) -> bool {
    match controller.close_tab(tab_id) {
        CloseTabOutcome::Closed => true,
        CloseTabOutcome::LastTab => {
            quit_after_last_tab();
            false
        }
        CloseTabOutcome::NotFound => false,
    }
}

#[cfg(test)]
mod tests {
    use talaria_core::BrowserEvent;

    use super::*;

    #[test]
    fn pages_may_open_tabs_by_default() {
        assert!(pages_may_open_tabs(TabId::new(1)));
    }

    #[test]
    fn the_allocator_adopts_a_tab_after_the_opener() {
        let controller = Rc::new(RefCell::new(BrowserController::default()));
        let opener = controller.borrow().active_tab_id();
        let allocator = new_tab_allocator(&controller);

        let adopted = allocator(opener).expect("the opener exists");

        assert_eq!(controller.borrow().active_tab_id(), adopted);
        assert_eq!(allocator(TabId::new(999)), None);
    }

    #[test]
    fn the_allocator_refuses_while_the_browser_state_is_borrowed() {
        let controller = Rc::new(RefCell::new(BrowserController::default()));
        let opener = controller.borrow().active_tab_id();
        let allocator = new_tab_allocator(&controller);

        let _borrowed = controller.borrow();
        assert_eq!(allocator(opener), None);
    }

    #[test]
    fn the_allocator_refuses_once_the_browser_state_is_gone() {
        let controller = Rc::new(RefCell::new(BrowserController::default()));
        let opener = controller.borrow().active_tab_id();
        let allocator = new_tab_allocator(&controller);
        drop(controller);

        assert_eq!(allocator(opener), None);
    }

    #[test]
    fn a_page_closing_its_tab_returns_to_the_opener() {
        let mut controller = BrowserController::default();
        let opener = controller.active_tab_id();
        controller.handle_event(opener, BrowserEvent::AddressChanged { url: String::new() });
        let adopted = controller.adopt_tab(opener).expect("the opener exists");

        assert!(close_page_tab(&mut controller, adopted));
        assert_eq!(controller.active_tab_id(), opener);
        assert_eq!(controller.tabs().len(), 1);
    }
}
