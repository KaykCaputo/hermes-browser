//! Which committed navigations become history visits, and how the user reached each one.
//!
//! The engine reports a committed navigation as `AddressChanged` (for a new document, a
//! fragment change, `pushState`/`replaceState`, a reload, a traversal, and also when a subframe
//! navigates, which repeats the top-level URL). The rules:
//!
//! - A commit to a different URL than the tab's last commit is a visit. So is a commit to the
//!   same URL when the user asked for a load (typed it again, opened it from a bookmark,
//!   reloaded); otherwise a repeated URL is subframe or `replaceState` noise and is ignored.
//! - The origin is the one the user's last command set for the tab (typed address or search,
//!   reload, back or forward, or one a caller names), consumed by the next commit whatever its
//!   URL; without one the page navigated itself, which is a link.
//! - Only `http`, `https` and `file` pages are recorded, without user name or password.
//! - The first non-blank title after a commit is the page's title. If it is an engine error
//!   page's title, the visit is taken back (`Forget`). A load that fails after it started and
//!   before it finished also takes its visit back.

use std::collections::HashMap;

use url::Url;

use crate::{BrowserEvent, LoadState, TabId};

/// How the user reached a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NavigationOrigin {
    /// Typed into the address bar, or searched for from it.
    Typed,
    /// The page navigated itself: a link, a form, a script, a redirect.
    Link,
    /// Opened from a bookmark.
    Bookmark,
    /// Reloaded.
    Reload,
    /// Back or forward.
    History,
}

impl NavigationOrigin {
    /// Whether this origin means the user asked for a load even of the URL already shown.
    const fn loads_the_same_url(self) -> bool {
        matches!(self, Self::Typed | Self::Bookmark | Self::Reload)
    }
}

/// Identifies one recorded visit within this run, so that it can be taken back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VisitKey(u64);

impl VisitKey {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// What the history should learn from the engine's events, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisitUpdate {
    /// A page was visited.
    Visit {
        key: VisitKey,
        url: String,
        origin: NavigationOrigin,
    },
    /// The page at `url`, the tab's current visit, has this title.
    Title { url: String, title: String },
    /// The visit turned out to be an error page or a failed load.
    Forget { key: VisitKey },
}

#[derive(Debug, Default)]
pub(crate) struct VisitTracker {
    tabs: HashMap<TabId, TabVisits>,
    next_key: u64,
    updates: Vec<VisitUpdate>,
    error_page_titles: Vec<String>,
}

#[derive(Debug, Default)]
struct TabVisits {
    pending_origin: Option<NavigationOrigin>,
    /// The URL of the tab's last commit, as the engine reported it.
    committed_url: Option<String>,
    current: Option<CurrentVisit>,
}

#[derive(Debug)]
struct CurrentVisit {
    key: VisitKey,
    url: String,
    /// The last title reported for this visit, if any.
    title: Option<String>,
    /// Whether a load started since the commit, and whether it finished.
    load_started: bool,
    load_finished: bool,
}

impl VisitTracker {
    pub(crate) fn set_error_page_titles(&mut self, titles: Vec<String>) {
        self.error_page_titles = titles;
    }

    pub(crate) fn set_origin(&mut self, tab_id: TabId, origin: NavigationOrigin) {
        self.tabs.entry(tab_id).or_default().pending_origin = Some(origin);
    }

    pub(crate) fn forget_tab(&mut self, tab_id: TabId) {
        self.tabs.remove(&tab_id);
    }

    pub(crate) fn take_updates(&mut self) -> Vec<VisitUpdate> {
        std::mem::take(&mut self.updates)
    }

    pub(crate) fn observe(&mut self, tab_id: TabId, event: &BrowserEvent) {
        match event {
            BrowserEvent::AddressChanged { url } => self.committed(tab_id, url),
            BrowserEvent::PageTitleChanged { title: Some(title) } => self.titled(tab_id, title),
            BrowserEvent::LoadStateChanged { state } => self.load_changed(tab_id, state),
            _ => {}
        }
    }

    fn committed(&mut self, tab_id: TabId, url: &str) {
        let tab = self.tabs.entry(tab_id).or_default();
        let origin = tab.pending_origin.take();
        let same_url = tab.committed_url.as_deref() == Some(url);
        tab.committed_url = Some(url.to_owned());
        if same_url && !origin.is_some_and(NavigationOrigin::loads_the_same_url) {
            return;
        }
        tab.current = None;
        let Some(url) = recordable_url(url) else {
            return;
        };
        let key = VisitKey(self.next_key);
        self.next_key += 1;
        self.updates.push(VisitUpdate::Visit {
            key,
            url: url.clone(),
            origin: origin.unwrap_or(NavigationOrigin::Link),
        });
        tab.current = Some(CurrentVisit {
            key,
            url,
            title: None,
            load_started: false,
            load_finished: false,
        });
    }

    fn titled(&mut self, tab_id: TabId, title: &str) {
        if title.trim().is_empty() {
            return;
        }
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        let Some(current) = tab.current.as_mut() else {
            return;
        };
        if current.title.is_none() && self.error_page_titles.iter().any(|t| t == title) {
            self.updates.push(VisitUpdate::Forget { key: current.key });
            tab.current = None;
            // The same URL loaded again (a "proceed anyway", a retry) is a visit.
            tab.committed_url = None;
            return;
        }
        if current.title.as_deref() == Some(title) {
            return;
        }
        current.title = Some(title.to_owned());
        self.updates.push(VisitUpdate::Title {
            url: current.url.clone(),
            title: title.to_owned(),
        });
    }

    fn load_changed(&mut self, tab_id: TabId, state: &LoadState) {
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        let Some(current) = tab.current.as_mut() else {
            return;
        };
        match state {
            LoadState::Idle => {}
            LoadState::Loading => current.load_started = true,
            LoadState::Loaded => current.load_finished = true,
            LoadState::Failed { .. } => {
                if current.load_started && !current.load_finished {
                    self.updates.push(VisitUpdate::Forget { key: current.key });
                    tab.current = None;
                    tab.committed_url = None;
                }
            }
        }
    }
}

/// The URL to record for a commit to `url`, if it is recorded at all: `http`, `https` and
/// `file` pages only, without user name and password.
fn recordable_url(url: &str) -> Option<String> {
    let mut url = Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "file") {
        return None;
    }
    // Both only fail for URLs that cannot have credentials, which then have none.
    let _ = url.set_password(None);
    let _ = url.set_username("");
    Some(url.into())
}
