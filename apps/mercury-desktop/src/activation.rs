// Owned by the tabs lane; this copy is replaced at merge.

//! Recent user activation per tab: a pointer press or release, or a key press, forwarded to the
//! tab's page. Features a page could otherwise trigger without the user (opening a tab, a file
//! panel, a colour picker) consume it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use talaria_core::TabId;

/// How long a gesture lets the page ask for one such feature.
pub(crate) const ACTIVATION_WINDOW: Duration = Duration::from_secs(1);

thread_local! {
    static LAST_ACTIVATION: RefCell<HashMap<TabId, Instant>> = RefCell::new(HashMap::new());
}

pub(crate) fn note_user_activation(tab: TabId, now: Instant) {
    LAST_ACTIVATION.with(|last| {
        last.borrow_mut().insert(tab, now);
    });
}

/// True, once, if the tab had a gesture within `ACTIVATION_WINDOW` before `now`.
pub(crate) fn consume_user_activation(tab: TabId, now: Instant) -> bool {
    LAST_ACTIVATION.with(|last| {
        let Some(at) = last.borrow_mut().remove(&tab) else {
            return false;
        };
        now.checked_duration_since(at)
            .is_some_and(|elapsed| elapsed <= ACTIVATION_WINDOW)
    })
}

#[expect(dead_code, reason = "the tabs lane calls it when a tab closes")]
pub(crate) fn forget_tab(tab: TabId) {
    LAST_ACTIVATION.with(|last| {
        last.borrow_mut().remove(&tab);
    });
}
