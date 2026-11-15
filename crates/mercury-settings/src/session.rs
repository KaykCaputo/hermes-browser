//! The session snapshot that restores windows and tabs, and the throttle that decides when to
//! write it.

use serde::{Deserialize, Serialize};

use crate::bounded;
use crate::settings::{MAX_ZOOM_PERCENT, MIN_ZOOM_PERCENT, session_url, truncated};

/// How many recently closed tabs are remembered (as Chrome's and Firefox's defaults).
pub const MAX_CLOSED_TABS: usize = 25;
/// How many tabs are restored, across all windows.
pub const MAX_RESTORED_TABS: usize = 500;
/// The longest tab title kept, in bytes.
pub const MAX_TITLE_BYTES: usize = 1024;
/// The smallest restored window side, in logical pixels.
pub const MIN_WINDOW_SIDE: u32 = 100;
/// The largest restored window side, in logical pixels.
pub const MAX_WINDOW_SIDE: u32 = 32_768;
/// The furthest a restored window's corner may be from the origin on either axis, in logical
/// pixels.
pub const MAX_WINDOW_COORDINATE: u32 = 65_536;

/// Everything needed to put the windows and tabs back.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSnapshot {
    /// The open windows, front to back as the caller chooses.
    #[serde(deserialize_with = "bounded::windows")]
    pub windows: Vec<WindowSnapshot>,
    /// Recently closed tabs, newest first.
    #[serde(deserialize_with = "bounded::closed_tabs")]
    pub closed_tabs: Vec<ClosedTab>,
    /// When the snapshot was taken, in milliseconds since the Unix epoch.
    pub saved_at_ms: i64,
}

/// One window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSnapshot {
    /// The tabs, in strip order.
    #[serde(deserialize_with = "bounded::tabs")]
    pub tabs: Vec<TabSnapshot>,
    /// The index of the selected tab in `tabs`.
    pub active_tab: usize,
    /// Where the window was, if known and sensible.
    pub bounds: Option<WindowBounds>,
    /// Whether it was maximised (zoomed, on macOS).
    pub maximized: bool,
    /// Whether it was full screen.
    pub fullscreen: bool,
}

/// A window's position and size, in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowBounds {
    /// The left edge.
    pub x: i32,
    /// The top edge.
    pub y: i32,
    /// The width.
    pub width: u32,
    /// The height.
    pub height: u32,
}

/// One tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabSnapshot {
    /// The page's URL. A new-tab page must be an `about:` URL (for example `about:newtab`) to
    /// survive a restore.
    #[serde(deserialize_with = "bounded::url")]
    pub url: String,
    /// The page's title.
    #[serde(deserialize_with = "bounded::title")]
    pub title: String,
    /// The tab's zoom, in percent.
    pub zoom_percent: u16,
    /// Whether the tab is pinned.
    pub pinned: bool,
}

impl Default for TabSnapshot {
    fn default() -> Self {
        Self {
            url: String::new(),
            title: String::new(),
            zoom_percent: 100,
            pinned: false,
        }
    }
}

/// A tab that was closed, and where it was.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClosedTab {
    /// The tab.
    pub tab: TabSnapshot,
    /// The index of the window it was in, at the time. A hint: the window may be gone.
    pub window: usize,
    /// Its index in that window's strip, at the time. A hint: clamp before use.
    pub index: usize,
    /// When it was closed, in milliseconds since the Unix epoch.
    pub closed_at_ms: i64,
}

impl SessionSnapshot {
    /// Drops what must not or cannot be restored and clamps the rest:
    ///
    /// - Tabs are kept only if their URL is `http`, `https`, `file` or `about` and at most
    ///   [`crate::MAX_URL_BYTES`] long before and after parsing; `javascript:`, `data:`, `blob:`,
    ///   other schemes and unparsable URLs are dropped. The URL kept is the parsed URL's text,
    ///   with any user name and password removed: a session file never holds a password. Titles are cut to [`MAX_TITLE_BYTES`] and zoom clamped to
    ///   [`MIN_ZOOM_PERCENT`]..=[`MAX_ZOOM_PERCENT`].
    /// - Windows left without tabs are dropped. `active_tab` is clamped into range; if the
    ///   active tab itself was dropped, the nearest surviving tab to its right becomes active,
    ///   or failing that the nearest to its left.
    /// - Window bounds with a side outside [`MIN_WINDOW_SIDE`]..=[`MAX_WINDOW_SIDE`] or a
    ///   corner further than [`MAX_WINDOW_COORDINATE`] from the origin become `None`.
    /// - At most [`MAX_RESTORED_TABS`] tabs survive across all windows: first every pinned
    ///   tab (in strip order), then each window's active tab, then the rest by their distance
    ///   from the end of their window's strip — where new tabs open, so the most recently
    ///   opened go first — taking the windows in turn. Kept tabs stay in their original order.
    /// - At most [`MAX_CLOSED_TABS`] closed tabs survive, the newest; closed tabs with a URL
    ///   that would be dropped are dropped first.
    ///
    /// Sanitizing the result again changes nothing.
    pub fn sanitized(self) -> Self {
        let windows: Vec<WindowSnapshot> = self
            .windows
            .into_iter()
            .filter_map(WindowSnapshot::sanitized)
            .collect();
        let closed_tabs = self
            .closed_tabs
            .into_iter()
            .filter_map(|closed| {
                Some(ClosedTab {
                    tab: closed.tab.sanitized()?,
                    ..closed
                })
            })
            .take(MAX_CLOSED_TABS)
            .collect();
        Self {
            windows: cap_tabs(windows, MAX_RESTORED_TABS),
            closed_tabs,
            saved_at_ms: self.saved_at_ms,
        }
    }

    /// Records a closed tab, newest first, bounded by `MAX_CLOSED_TABS`.
    pub fn push_closed(&mut self, tab: ClosedTab) {
        self.closed_tabs.insert(0, tab);
        self.closed_tabs.truncate(MAX_CLOSED_TABS);
    }

    /// The most recently closed tab, removed from the list ("reopen closed tab").
    ///
    /// Its `window` and `index` say where it was; the caller clamps them to what exists now.
    pub fn pop_closed(&mut self) -> Option<ClosedTab> {
        (!self.closed_tabs.is_empty()).then(|| self.closed_tabs.remove(0))
    }
}

impl TabSnapshot {
    fn sanitized(self) -> Option<Self> {
        let url = session_url(&self.url)?;
        let title = truncated(&self.title, MAX_TITLE_BYTES).to_owned();
        Some(Self {
            url,
            title,
            zoom_percent: self.zoom_percent.clamp(MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT),
            ..self
        })
    }
}

impl WindowBounds {
    fn is_sensible(&self) -> bool {
        let sides = MIN_WINDOW_SIDE..=MAX_WINDOW_SIDE;
        sides.contains(&self.width)
            && sides.contains(&self.height)
            && self.x.unsigned_abs() <= MAX_WINDOW_COORDINATE
            && self.y.unsigned_abs() <= MAX_WINDOW_COORDINATE
    }
}

impl WindowSnapshot {
    fn sanitized(self) -> Option<Self> {
        let tabs: Vec<Option<TabSnapshot>> =
            self.tabs.into_iter().map(TabSnapshot::sanitized).collect();
        let keep: Vec<bool> = tabs.iter().map(Option::is_some).collect();
        // `active_tab` still refers to the strip before the drops; `retain_tabs` remaps it.
        let window = Self {
            tabs: tabs.into_iter().flatten().collect(),
            bounds: self.bounds.filter(WindowBounds::is_sensible),
            ..self
        };
        retain_tabs(window, &keep, true)
    }
}

/// Keeps the tabs whose flag in `keep` is set and moves `active_tab` along with them. When
/// `already_filtered`, `window.tabs` holds only the kept tabs and `keep` describes the strip
/// `active_tab` refers to. `None` if no tab is left.
fn retain_tabs(
    window: WindowSnapshot,
    keep: &[bool],
    already_filtered: bool,
) -> Option<WindowSnapshot> {
    let last = keep.len().checked_sub(1)?;
    let active = window.active_tab.min(last);
    let kept_before = |index: usize| keep[..index].iter().filter(|kept| **kept).count();
    let new_active = if keep[active] {
        active
    } else {
        (active + 1..keep.len())
            .find(|&index| keep[index])
            .or_else(|| (0..active).rev().find(|&index| keep[index]))?
    };
    let tabs = if already_filtered {
        window.tabs
    } else {
        window
            .tabs
            .into_iter()
            .zip(keep)
            .filter_map(|(tab, kept)| kept.then_some(tab))
            .collect()
    };
    Some(WindowSnapshot {
        tabs,
        active_tab: kept_before(new_active),
        ..window
    })
}

/// Cuts the windows down to `max` tabs in all; see [`SessionSnapshot::sanitized`].
fn cap_tabs(windows: Vec<WindowSnapshot>, max: usize) -> Vec<WindowSnapshot> {
    let total: usize = windows.iter().map(|window| window.tabs.len()).sum();
    if total <= max {
        return windows;
    }
    // (class, order within the class, window, index): pinned, then active, then by distance
    // from the end of the strip, windows taking turns.
    let mut ranked: Vec<(u8, usize, usize, usize)> = Vec::with_capacity(total);
    for (window_index, window) in windows.iter().enumerate() {
        let length = window.tabs.len();
        for (index, tab) in window.tabs.iter().enumerate() {
            let (class, order) = if tab.pinned {
                (0, index)
            } else if index == window.active_tab {
                (1, 0)
            } else {
                (2, length - 1 - index)
            };
            ranked.push((class, order, window_index, index));
        }
    }
    ranked.sort_unstable();
    let mut keep: Vec<Vec<bool>> = windows
        .iter()
        .map(|window| vec![false; window.tabs.len()])
        .collect();
    for &(_, _, window_index, index) in ranked.iter().take(max) {
        keep[window_index][index] = true;
    }
    windows
        .into_iter()
        .zip(keep)
        .filter_map(|(window, keep)| retain_tabs(window, &keep, false))
        .collect()
}

/// Decides when a changed session is worth writing: at most once per interval, and at once
/// when asked to flush. Pure; the caller supplies the time.
///
/// Call [`mark_dirty`](Self::mark_dirty) on every change, ask
/// [`should_save`](Self::should_save) from a timer, and call [`saved`](Self::saved) once the
/// write lands. To flush (on quit, or before something risky), save without asking and then
/// call `saved`. A save that fails is simply not reported; it is offered again once the
/// interval has passed.
#[derive(Clone, Debug)]
pub struct SaveThrottle {
    min_interval_ms: i64,
    /// Counts changes; `saved_changes` is the count already on disk.
    changes: u64,
    saved_changes: u64,
    /// The count when `should_save` last said yes, if no save has been reported since.
    offered_changes: Option<u64>,
    last_offer_ms: Option<i64>,
    last_saved_ms: Option<i64>,
}

impl SaveThrottle {
    /// A throttle that offers a save at most once per `min_interval_ms` (a negative interval
    /// counts as zero).
    pub fn new(min_interval_ms: i64) -> Self {
        Self {
            min_interval_ms: min_interval_ms.max(0),
            changes: 0,
            saved_changes: 0,
            offered_changes: None,
            last_offer_ms: None,
            last_saved_ms: None,
        }
    }

    /// Records that the session changed.
    pub fn mark_dirty(&mut self) {
        self.changes = self.changes.wrapping_add(1);
    }

    /// Whether there are changes not yet reported as saved.
    pub fn is_dirty(&self) -> bool {
        self.changes != self.saved_changes
    }

    /// True at most once per interval while dirty: the interval runs from the last save and
    /// from the last time this said yes. A time earlier than either (the clock went back)
    /// counts as the interval having passed.
    pub fn should_save(&mut self, now_ms: i64) -> bool {
        let interval = self.min_interval_ms;
        let passed = |since: Option<i64>| {
            since.is_none_or(|since| {
                let elapsed = now_ms.saturating_sub(since);
                elapsed < 0 || elapsed >= interval
            })
        };
        if !self.is_dirty() || !passed(self.last_saved_ms) || !passed(self.last_offer_ms) {
            return false;
        }
        self.offered_changes = Some(self.changes);
        self.last_offer_ms = Some(now_ms);
        true
    }

    /// Records that a save landed at `now_ms`: the state as of the last
    /// [`should_save`](Self::should_save) that said yes, or, when the caller saved without
    /// asking, the state now. Changes marked after that yes stay dirty.
    pub fn saved(&mut self, now_ms: i64) {
        self.saved_changes = self.offered_changes.take().unwrap_or(self.changes);
        self.last_saved_ms = Some(now_ms);
        self.last_offer_ms = None;
    }
}
