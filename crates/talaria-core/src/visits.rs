//! Which committed navigations become history visits, and how the user reached each one.
//!
//! The engine reports every session-history change of a tab as `PageCommitted` (a new
//! document, a fragment change, `pushState`/`replaceState`, a reload, back or forward, and also
//! a subframe navigating, which repeats the top-level URL), with the history's length and the
//! current index. The rules:
//!
//! - **Visits.** A commit to a URL other than the tab's last commit is a visit. A commit to the
//!   same URL is a visit only when it is the target of a user's load (typed again, a bookmark,
//!   a reload); otherwise it is subframe or `replaceState` noise.
//! - **Origins** are set by the user's command for the tab, with the URL it targets: a typed
//!   address or search (`Typed`, the resolved URL), a reload (`Reload`, the URL shown), back or
//!   forward (`History`, any target), or one a caller names (`Bookmark`, `Restored`, keeping the
//!   target of the navigation already started). A commit consumes the origin when its URL is the
//!   target, or when it is a recordable URL other than the one shown (the navigation ended
//!   somewhere else: a redirect, or the traversal). Same-URL commits and unrecorded commits
//!   (`about:blank`, shown as "") with another target leave the origin waiting, so the next
//!   top-level page change always consumes it: an origin never outlives one page change. An
//!   origin whose target is not recordable is not kept at all. Without an origin, the page
//!   navigated itself: a link.
//! - **Replaced entries.** A commit to another URL that leaves the history's length and index
//!   unchanged, with no user origin, replaced the current entry (`replaceState`,
//!   `location.replace`): the current visit moves to the new URL (`Forget` the old visit,
//!   `Visit` the new URL with the old origin, and the old title, if any, as `Title`).
//! - **Not recorded:** anything but `http`, `https` and `file`; a commit consumed by `Restored`
//!   (its titles are ignored too). User names and passwords are removed from recorded URLs.
//! - **Titles** come with the URL the web view showed when the title was reported, which is not
//!   always the title's own document: Servo sends titles and commits on different channels, and
//!   a page's title can arrive before its commit. A title reported while the web view showed no
//!   page (an empty URL: before the first commit, or on the blank page, which has no title)
//!   belongs to the next page and is applied when that page is recorded. Otherwise a title for
//!   another URL than the current visit's is dropped. The first non-blank title of a visit is
//!   emitted at once; if it is an engine error page's title, the visit is taken back
//!   (`Forget`), or never recorded if the title came first.

use std::collections::HashMap;

use url::Url;

use crate::{BrowserEvent, TabId};

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
    /// A tab restored from a saved session: its restored page is not recorded.
    Restored,
}

impl NavigationOrigin {
    /// Whether this origin means the user asked for a load even of the URL already shown.
    const fn loads_the_same_url(self) -> bool {
        matches!(
            self,
            Self::Typed | Self::Bookmark | Self::Reload | Self::Restored
        )
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
    /// The visit turned out to be an error page, or its entry was replaced.
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
    pending: Option<PendingOrigin>,
    /// The URL of the tab's last commit, as the engine reported it.
    committed_url: Option<String>,
    /// The history's length and index at the last commit.
    position: Option<(usize, usize)>,
    current: Option<CurrentVisit>,
    /// The last title reported while the web view showed no page (an empty URL), which can
    /// only belong to the next page.
    early_title: Option<String>,
}

#[derive(Debug)]
struct PendingOrigin {
    origin: NavigationOrigin,
    /// The recordable URL the navigation goes to; `None` for any (back and forward).
    target: Option<String>,
}

#[derive(Debug)]
struct CurrentVisit {
    key: VisitKey,
    url: String,
    origin: NavigationOrigin,
    /// The last title reported for this visit, if any.
    title: Option<String>,
}

impl VisitTracker {
    pub(crate) fn set_error_page_titles(&mut self, titles: Vec<String>) {
        self.error_page_titles = titles;
    }

    /// The user started a navigation of the tab to `target` (`None`: wherever back or forward
    /// leads). A target that is never recorded leaves no origin.
    pub(crate) fn set_origin(
        &mut self,
        tab_id: TabId,
        origin: NavigationOrigin,
        target: Option<&str>,
    ) {
        let tab = self.tabs.entry(tab_id).or_default();
        tab.pending = match target {
            None => Some(PendingOrigin {
                origin,
                target: None,
            }),
            Some(target) => recordable_url(target).map(|target| PendingOrigin {
                origin,
                target: Some(target),
            }),
        };
    }

    /// Renames the origin of the navigation already started, keeping its target; with none
    /// pending, the navigation to `shown` (the URL the tab shows as loading) is the one named.
    pub(crate) fn rename_origin(&mut self, tab_id: TabId, origin: NavigationOrigin, shown: &str) {
        match self
            .tabs
            .get_mut(&tab_id)
            .and_then(|tab| tab.pending.as_mut())
        {
            Some(pending) => pending.origin = origin,
            None => self.set_origin(tab_id, origin, Some(shown)),
        }
    }

    pub(crate) fn forget_tab(&mut self, tab_id: TabId) {
        self.tabs.remove(&tab_id);
    }

    pub(crate) fn take_updates(&mut self) -> Vec<VisitUpdate> {
        std::mem::take(&mut self.updates)
    }

    pub(crate) fn observe(&mut self, tab_id: TabId, event: &BrowserEvent) {
        match event {
            BrowserEvent::PageCommitted {
                url,
                history_length,
                history_index,
            } => self.committed(tab_id, url, (*history_length, *history_index)),
            BrowserEvent::PageTitleAt {
                url,
                title: Some(title),
            } => self.titled(tab_id, url, title),
            _ => {}
        }
    }

    fn new_key(&mut self) -> VisitKey {
        let key = VisitKey(self.next_key);
        self.next_key += 1;
        key
    }

    fn committed(&mut self, tab_id: TabId, url: &str, position: (usize, usize)) {
        let key = self.new_key();
        let tab = self.tabs.entry(tab_id).or_default();
        let recordable = recordable_url(url);
        let same_url = tab.committed_url.as_deref() == Some(url);
        let same_position = tab.position == Some(position);
        tab.committed_url = Some(url.to_owned());
        tab.position = Some(position);
        let early_title = tab.early_title.take();

        let origin = match &tab.pending {
            Some(pending)
                if pending.target.is_some() && pending.target == recordable
                    || recordable.is_some() && !same_url =>
            {
                tab.pending.take().map(|pending| pending.origin)
            }
            _ => None,
        };
        if same_url && !origin.is_some_and(NavigationOrigin::loads_the_same_url) {
            return;
        }
        let replaced = match (origin, same_position) {
            (None, true) => tab.current.take(),
            _ => None,
        };
        tab.current = None;
        // A future per-tab "do not record" flag (private tabs) belongs here, with `Restored`.
        let (Some(url), false) = (recordable, origin == Some(NavigationOrigin::Restored)) else {
            return;
        };
        if early_title
            .as_ref()
            .is_some_and(|title| self.error_page_titles.contains(title))
        {
            // An error page, known before its commit: not recorded at all.
            tab.committed_url = None;
            return;
        }
        let origin = replaced
            .as_ref()
            .map_or(origin.unwrap_or(NavigationOrigin::Link), |old| old.origin);
        if let Some(old) = &replaced {
            self.updates.push(VisitUpdate::Forget { key: old.key });
        }
        self.updates.push(VisitUpdate::Visit {
            key,
            url: url.clone(),
            origin,
        });
        let title = early_title.or_else(|| replaced.and_then(|old| old.title));
        if let Some(title) = &title {
            self.updates.push(VisitUpdate::Title {
                url: url.clone(),
                title: title.clone(),
            });
        }
        tab.current = Some(CurrentVisit {
            key,
            url,
            origin,
            title,
        });
    }

    fn titled(&mut self, tab_id: TabId, url: &str, title: &str) {
        if title.trim().is_empty() {
            return;
        }
        if url.is_empty() {
            self.tabs.entry(tab_id).or_default().early_title = Some(title.to_owned());
            return;
        }
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        let Some(current) = tab.current.as_mut() else {
            return;
        };
        if recordable_url(url).as_deref() != Some(current.url.as_str()) {
            return;
        }
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
}

/// The URL to record for `url`, if it is recorded at all: `http`, `https` and `file` pages
/// only, without user name and password.
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
