//! Browsing history: recording what the controller decided were visits.

use mercury_store::VisitTransition;
use talaria_core::{BrowserController, NavigationOrigin, VisitKey, VisitUpdate};

use super::{Area, Message, ProfileData, RECENT_VISITS, handle, now_ms};

/// Hands the controller's history updates to the service, stamped with the current time. Call
/// after applying engine events; with no service the updates are dropped.
pub(crate) fn record_visits(controller: &mut BrowserController) {
    let updates = controller.take_visit_updates();
    if updates.is_empty() {
        return;
    }
    if let Some(data) = handle() {
        data.record_updates(updates, now_ms());
    }
}

/// The stored transition for a navigation's origin. Back and forward count as following a link
/// to the page again: the store has no transition of its own for them. `Restored` pages are
/// never recorded; should one reach here, it counts as a link.
pub(super) fn transition(origin: NavigationOrigin) -> VisitTransition {
    match origin {
        NavigationOrigin::Typed => VisitTransition::Typed,
        NavigationOrigin::Link | NavigationOrigin::History | NavigationOrigin::Restored => {
            VisitTransition::Link
        }
        NavigationOrigin::Bookmark => VisitTransition::Bookmark,
        NavigationOrigin::Reload => VisitTransition::Reload,
    }
}

impl ProfileData {
    /// Records `updates` in order, as having happened at `at_ms`. No reply.
    pub(crate) fn record_updates(&self, updates: Vec<VisitUpdate>, at_ms: i64) {
        for update in updates {
            match update {
                VisitUpdate::Visit { key, url, origin } => {
                    self.record_visit(key, url, transition(origin), at_ms)
                }
                VisitUpdate::Title { url, title } => self.set_page_title(url, title),
                VisitUpdate::Forget { key } => self.forget_visit(key),
            }
        }
    }

    /// Records a visit to `url` at `at_ms`; `key` lets [`forget_visit`](Self::forget_visit)
    /// take it back. No reply.
    fn record_visit(&self, key: VisitKey, url: String, transition: VisitTransition, at_ms: i64) {
        self.submit(
            Some(Area::History),
            "recording a visit",
            move |database, _now| {
                let id = database.store.record_visit(&url, None, transition, at_ms)?;
                if database.recent_visits.len() == RECENT_VISITS {
                    database.recent_visits.pop_front();
                }
                database.recent_visits.push_back((key, id));
                Ok(())
            },
        );
    }

    /// Removes a visit recorded with `key` moments ago (an error page, a replaced entry), and
    /// its page if that has no other visit and no bookmark. Older visits are kept. No reply.
    fn forget_visit(&self, key: VisitKey) {
        self.submit(
            Some(Area::History),
            "forgetting a visit",
            move |database, now| {
                let index = database.recent_visits.iter().position(|(k, _)| *k == key);
                let Some((_, id)) = index.and_then(|index| database.recent_visits.remove(index))
                else {
                    log::debug!("visit {} is no longer recent; kept", key.value());
                    return Ok(());
                };
                database.store.delete_visit(id, now)?;
                Ok(())
            },
        );
    }

    /// Sets the title of the page at `url`, if it is known; debounced per URL. No reply.
    pub(super) fn set_page_title(&self, url: String, title: String) {
        self.send(Message::Title { url, title });
    }
}
