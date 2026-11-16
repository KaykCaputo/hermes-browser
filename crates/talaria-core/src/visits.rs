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
//!   `Visit` the new URL with the old origin). The old title is carried only *provisionally*,
//!   and only when no new document announced itself: the page's own first title replaces it
//!   (and still goes through the error-page check); it is written only if none came before
//!   the next commit, or when the tab closes.
//! - **Not recorded:** anything but `http`, `https` and `file`; a commit consumed by `Restored`,
//!   and the replaced entries after it, until a user origin or a commit that pushes a new entry
//!   (the same flag will serve private tabs). User names and passwords are removed from
//!   recorded URLs.
//! - **Titles** come with the URL the web view showed when the title was reported, which is not
//!   always the title's own document: Servo sends titles and commits on different channels.
//!   Measured on Servo 0.8: a new document announces itself with a `None` title, and its titles
//!   can arrive *before* its commit, carrying the previous page's URL (a page navigating to an
//!   unreachable host reports `None`, `""`, "Error loading page", then commits). So:
//!   - after a `None` title, the titles are the next document's: they are placed on the next
//!     recorded commit, or, if the commit came first (a reload), on the current visit when the
//!     page has loaded;
//!   - before a web view's first commit every title is the first page's;
//!   - a title reported on the blank page after that (an opened window titled by its opener)
//!     belongs to nobody, and so does one for another URL than the current visit's;
//!   - the order of titles and commits within one engine pump means nothing, so a tab's titles
//!     are placed when the pump ends (`take_updates`), and only the last one counts; a
//!     same-document commit later in the same pump takes the title (`pushState` followed by a
//!     new `document.title` reaches the embedder title first). Across pumps this cannot be
//!     corrected.
//! - **An error page that replaced an entry** (a `location.replace` that commits before its
//!   error title) takes its own visit back and records the replaced page again, as of now.
//!
//!   The first title of a visit's own page is emitted at once; if it is an engine error page's
//!   title, the visit is taken back (`Forget`), or never recorded if the title came first.

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
    /// The history's length and index at the last commit; `None` before the first commit.
    position: Option<(usize, usize)>,
    current: Option<CurrentVisit>,
    /// `Some` once a new document has announced itself (a `None` title, or any title before
    /// the web view's first commit) and until its titles are placed: its latest non-blank
    /// title, if any.
    incoming: Option<Option<String>>,
    /// Commits are not recorded: set by a `Restored` commit, until a user origin or a commit
    /// that pushes a new entry. A per-tab "do not record" (private tabs) would set it for good.
    not_recorded: bool,
    /// The last title reported for the current visit's URL in this pump, placed when the pump
    /// ends (`take_updates`): a same-document commit arriving later in the pump overtook it,
    /// and takes it.
    unplaced: Option<String>,
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
    /// The last title reported for this visit's own page, if any.
    title: Option<String>,
    /// A title carried from the entry this one replaced: written only if the page reports none
    /// of its own before the next commit.
    provisional: Option<String>,
    /// The entry this visit replaced, recorded again if this one turns out to be an error page.
    replaced: Option<Box<CurrentVisit>>,
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
        if origin != NavigationOrigin::Restored {
            tab.not_recorded = false;
        }
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

    /// The tab closed: its last titles are placed, as no others will come.
    pub(crate) fn forget_tab(&mut self, tab_id: TabId) {
        self.place_titles(tab_id);
        if let Some(mut tab) = self.tabs.remove(&tab_id) {
            Self::settle_provisional(&mut self.updates, tab.current.as_mut());
        }
    }

    /// The updates so far; called once per engine pump, which ends it: the titles reported in
    /// the pump are placed first.
    pub(crate) fn take_updates(&mut self) -> Vec<VisitUpdate> {
        let tabs = self
            .tabs
            .iter()
            .filter(|(_, tab)| tab.unplaced.is_some())
            .map(|(tab_id, _)| *tab_id)
            .collect::<Vec<_>>();
        for tab_id in tabs {
            self.place_titles(tab_id);
        }
        std::mem::take(&mut self.updates)
    }

    fn place_titles(&mut self, tab_id: TabId) {
        if let Some(title) = self
            .tabs
            .get_mut(&tab_id)
            .and_then(|tab| tab.unplaced.take())
        {
            self.apply_title(tab_id, &title);
        }
    }

    pub(crate) fn observe(&mut self, tab_id: TabId, event: &BrowserEvent) {
        match event {
            BrowserEvent::PageCommitted {
                url,
                history_length,
                history_index,
            } => self.committed(tab_id, url, (*history_length, *history_index)),
            BrowserEvent::PageTitleAt { url, title } => self.titled(tab_id, url, title.as_deref()),
            BrowserEvent::LoadStateChanged {
                state: LoadState::Loaded,
            } => self.loaded(tab_id),
            _ => {}
        }
    }

    fn new_key(&mut self) -> VisitKey {
        let key = VisitKey(self.next_key);
        self.next_key += 1;
        key
    }

    fn is_error_title(&self, title: &str) -> bool {
        self.error_page_titles.iter().any(|t| t == title)
    }

    /// Writes a visit's provisional title, if it still has one.
    fn settle_provisional(updates: &mut Vec<VisitUpdate>, current: Option<&mut CurrentVisit>) {
        if let Some(current) = current
            && let Some(title) = current.provisional.take()
        {
            updates.push(VisitUpdate::Title {
                url: current.url.clone(),
                title,
            });
        }
    }

    fn committed(&mut self, tab_id: TabId, url: &str, position: (usize, usize)) {
        let key = self.new_key();
        let tab = self.tabs.entry(tab_id).or_default();
        let recordable = recordable_url(url);
        let same_url = tab.committed_url.as_deref() == Some(url);
        let same_position = tab.position == Some(position);

        let origin = match &tab.pending {
            Some(pending)
                if pending.target.is_some() && pending.target == recordable
                    || recordable.is_some() && !same_url =>
            {
                tab.pending.take().map(|pending| pending.origin)
            }
            _ => None,
        };
        tab.committed_url = Some(url.to_owned());
        tab.position = Some(position);
        if same_url && !origin.is_some_and(NavigationOrigin::loads_the_same_url) {
            // Subframe or `replaceState` noise: an announced document is still to come.
            return;
        }
        let incoming = tab.incoming.take();
        // A title reported before this commit: the old page's if a new document was announced
        // since, otherwise it overtook this same-document commit and is the new page's.
        let unplaced = tab.unplaced.take();
        let carried_title = match (&incoming, unplaced) {
            (None, title) => title,
            (Some(_), Some(title)) => {
                self.apply_title(tab_id, &title);
                None
            }
            (Some(_), None) => None,
        };
        let tab = self.tabs.entry(tab_id).or_default();
        let replaced = match (origin, same_position) {
            (None, true) => tab.current.take(),
            _ => {
                Self::settle_provisional(&mut self.updates, tab.current.as_mut());
                tab.current = None;
                None
            }
        };
        if origin.is_some() || !same_position {
            tab.not_recorded = false;
        }
        if origin == Some(NavigationOrigin::Restored) {
            tab.not_recorded = true;
        }
        let Some(url) = recordable.filter(|_| !tab.not_recorded) else {
            return;
        };
        let own_title = incoming.clone().flatten();
        if own_title
            .as_deref()
            .is_some_and(|title| self.error_page_titles.iter().any(|t| t == title))
        {
            // An error page, known before its commit: not recorded, and the entry it replaced
            // (if any) stays as it was. The same URL loaded again is a visit.
            tab.current = replaced;
            tab.committed_url = None;
            return;
        }
        let origin = replaced
            .as_ref()
            .map_or(origin.unwrap_or(NavigationOrigin::Link), |old| old.origin);
        if let Some(old) = &replaced {
            self.updates.push(VisitUpdate::Forget { key: old.key });
        }
        let replaced = replaced.map(|mut old| {
            // Only one entry back is kept.
            old.replaced = None;
            Box::new(old)
        });
        self.updates.push(VisitUpdate::Visit {
            key,
            url: url.clone(),
            origin,
        });
        if let Some(title) = &own_title {
            self.updates.push(VisitUpdate::Title {
                url: url.clone(),
                title: title.clone(),
            });
        }
        // A title is carried only across a replace that announced no new document.
        let provisional = match (&incoming, &replaced) {
            (None, Some(old)) => old.title.clone().or_else(|| old.provisional.clone()),
            _ => None,
        };
        tab.current = Some(CurrentVisit {
            key,
            url,
            origin,
            title: own_title,
            provisional,
            replaced,
        });
        if let Some(title) = carried_title {
            self.apply_title(tab_id, &title);
        }
    }

    fn titled(&mut self, tab_id: TabId, url: &str, title: Option<&str>) {
        let tab = self.tabs.entry(tab_id).or_default();
        if tab.position.is_none() {
            // Before the web view's first commit every title is the first page's.
            let held = tab.incoming.get_or_insert(None);
            if let Some(title) = title.filter(|title| !title.trim().is_empty()) {
                *held = Some(title.to_owned());
            }
            return;
        }
        if url.is_empty() {
            // The blank page has no title; one set on it (an opened window) belongs to nobody.
            return;
        }
        let Some(title) = title else {
            // A new document has started; its titles may come before its commit.
            tab.incoming = Some(None);
            return;
        };
        if title.trim().is_empty() {
            return;
        }
        if let Some(held) = tab.incoming.as_mut() {
            *held = Some(title.to_owned());
            return;
        }
        if tab
            .current
            .as_ref()
            .is_some_and(|current| recordable_url(url).as_deref() == Some(current.url.as_str()))
        {
            tab.unplaced = Some(title.to_owned());
        }
    }

    /// The tab's page finished loading: titles of a document announced after its commit (a
    /// reload) belong to the current visit.
    fn loaded(&mut self, tab_id: TabId) {
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        if tab.position.is_none() {
            return;
        }
        match tab.incoming.take() {
            Some(Some(title)) => self.apply_title(tab_id, &title),
            Some(None) => {
                if let Some(current) = tab.current.as_mut() {
                    // The page reported no title of its own.
                    current.provisional = None;
                }
            }
            None => {}
        }
    }

    /// A non-blank title of the current visit's own page.
    fn apply_title(&mut self, tab_id: TabId, title: &str) {
        let is_error = self.is_error_title(title);
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        let Some(current) = tab.current.as_mut() else {
            return;
        };
        current.provisional = None;
        if current.title.is_none() && is_error {
            self.updates.push(VisitUpdate::Forget { key: current.key });
            // The same URL loaded again (a "proceed anyway", a retry) is a visit.
            tab.committed_url = None;
            // An entry this error page replaced was seen: it is recorded again (as of now).
            let restored = tab.current.take().and_then(|current| current.replaced);
            if let Some(mut old) = restored {
                let key = VisitKey(self.next_key);
                self.next_key += 1;
                self.updates.push(VisitUpdate::Visit {
                    key,
                    url: old.url.clone(),
                    origin: old.origin,
                });
                if let Some(title) = old.title.clone().or_else(|| old.provisional.take()) {
                    self.updates.push(VisitUpdate::Title {
                        url: old.url.clone(),
                        title,
                    });
                }
                old.key = key;
                tab.current = Some(*old);
            }
            return;
        }
        current.replaced = None;
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
