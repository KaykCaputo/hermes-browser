use crate::{ColorScheme, DEFAULT_ZOOM_PERCENT, ImeInput, KeyboardInput, PageIcon, PointerCursor};

const DEFAULT_PAGE_TITLE: &str = "New Tab";
const INITIAL_ADDRESS: &str = "";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    Idle,
    Loading,
    Loaded,
    Failed { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Middle,
    Right,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButtonAction {
    Down,
    Up,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BrowserCommand {
    Navigate {
        url: String,
    },
    GoBack,
    GoForward,
    Reload,
    PointerMoved {
        x: f32,
        y: f32,
    },
    PointerButton {
        action: PointerButtonAction,
        button: PointerButton,
        x: f32,
        y: f32,
    },
    Scrolled {
        delta_x: f32,
        delta_y: f32,
        x: f32,
        y: f32,
    },
    FocusChanged {
        focused: bool,
    },
    KeyboardInput(KeyboardInput),
    ImeInput(ImeInput),
    /// Page zoom as a percentage of the default size.
    SetPageZoom {
        percent: u16,
    },
    /// Multiplies the pinch zoom by `delta`, centred on the given web-view point.
    PinchZoom {
        delta: f32,
        x: f32,
        y: f32,
    },
    SetColorScheme(ColorScheme),
    ExitFullscreen,
    /// The pointer left the web view, or the tab stopped being shown.
    PointerLeft,
}

impl BrowserCommand {
    /// Pointer input only makes sense for the document the user currently sees. It is
    /// discarded, rather than held, while the tab's browsing context does not exist yet.
    pub(crate) const fn is_discarded_before_engine_ready(&self) -> bool {
        matches!(
            self,
            Self::PointerMoved { .. }
                | Self::PointerButton { .. }
                | Self::Scrolled { .. }
                | Self::PinchZoom { .. }
                | Self::PointerLeft
        )
    }

    /// Commands that act on the tab's current page; they are meaningless once that page is
    /// replaced before it ever committed.
    pub(crate) const fn targets_the_current_page(&self) -> bool {
        matches!(
            self,
            Self::Navigate { .. } | Self::Reload | Self::GoBack | Self::GoForward
        )
    }

    /// How a navigation this command starts was reached, when the command alone says so.
    /// `Navigate` does not: `queue_navigation` and its callers name the origin.
    pub(crate) const fn navigation_origin(&self) -> Option<crate::NavigationOrigin> {
        match self {
            Self::Reload => Some(crate::NavigationOrigin::Reload),
            Self::GoBack | Self::GoForward => Some(crate::NavigationOrigin::History),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserEvent {
    AddressChanged {
        url: String,
    },
    PageTitleChanged {
        title: Option<String>,
    },
    PageIconChanged {
        icon: Option<PageIcon>,
    },
    LoadStateChanged {
        state: LoadState,
    },
    HistoryChanged {
        can_go_back: bool,
        can_go_forward: bool,
    },
    CursorChanged {
        cursor: PointerCursor,
    },
    /// Text the page wants shown in the status area, such as a hovered link's URL.
    StatusTextChanged {
        text: Option<String>,
    },
    FullscreenChanged {
        fullscreen: bool,
    },
    // history
    /// The tab's session history changed (a new document, a fragment change, `pushState` or
    /// `replaceState`, a reload, back or forward, or a subframe navigating): `url` is the
    /// top-level URL now shown, at `history_index` of `history_length` entries. Sent after the
    /// matching `AddressChanged`; only history recording reads it.
    PageCommitted {
        url: String,
        history_length: usize,
        history_index: usize,
    },
    /// The page's title, with the URL the web view showed when the title was reported. Sent
    /// after the matching `PageTitleChanged`; only history recording reads it.
    PageTitleAt {
        url: String,
        title: Option<String>,
    },
    // downloads
    /// The top-level page at `url` is the engine's own "Unknown content type" page: the
    /// response was of a type it cannot show (`mime_type`). `posted` when the request that
    /// fetched it was not a GET (a form), which cannot be fetched again. `referrer` is the
    /// page that request came from, if the engine sent one.
    DownloadOffered {
        url: String,
        mime_type: String,
        posted: bool,
        referrer: Option<String>,
    },
}

/// A page the engine could not show, offered to the browser as a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadOffer {
    pub url: String,
    pub mime_type: String,
    pub posted: bool,
    pub referrer: Option<String>,
}

impl BrowserEvent {
    /// Events the engine only sends once the tab's browsing context exists.
    pub(crate) const fn shows_engine_ready(&self) -> bool {
        matches!(
            self,
            Self::AddressChanged { .. } | Self::HistoryChanged { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserState {
    url: String,
    page_title: String,
    page_icon: Option<PageIcon>,
    load_state: LoadState,
    can_go_back: bool,
    can_go_forward: bool,
    zoom_percent: u16,
    cursor: PointerCursor,
    status_text: Option<String>,
    fullscreen: bool,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self {
            url: INITIAL_ADDRESS.to_owned(),
            page_title: DEFAULT_PAGE_TITLE.to_owned(),
            page_icon: None,
            load_state: LoadState::Idle,
            can_go_back: false,
            can_go_forward: false,
            zoom_percent: DEFAULT_ZOOM_PERCENT,
            cursor: PointerCursor::Default,
            status_text: None,
            fullscreen: false,
        }
    }
}

impl BrowserState {
    pub fn page_title(&self) -> &str {
        &self.page_title
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn page_icon(&self) -> Option<&PageIcon> {
        self.page_icon.as_ref()
    }
    pub fn load_state(&self) -> &LoadState {
        &self.load_state
    }
    pub fn can_go_back(&self) -> bool {
        self.can_go_back
    }
    pub fn can_go_forward(&self) -> bool {
        self.can_go_forward
    }
    pub fn zoom_percent(&self) -> u16 {
        self.zoom_percent
    }

    pub fn cursor(&self) -> PointerCursor {
        self.cursor
    }
    pub fn status_text(&self) -> Option<&str> {
        self.status_text.as_deref()
    }
    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    pub(crate) fn set_zoom_percent(&mut self, percent: u16) {
        self.zoom_percent = percent;
    }

    pub fn apply(&mut self, event: BrowserEvent) {
        match event {
            BrowserEvent::AddressChanged { url } => {
                // Cursor and status text are deliberately not reset here: the engine only
                // reports changes against its own cached values, so the engine's notifications
                // must stay the only source of both.
                if self.url != url {
                    self.url = url;
                    self.page_icon = None;
                }
            }
            BrowserEvent::PageTitleChanged { title } => {
                self.page_title = title
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| DEFAULT_PAGE_TITLE.to_owned());
            }
            BrowserEvent::PageIconChanged { icon } => {
                self.page_icon = icon;
            }
            BrowserEvent::LoadStateChanged { state } => {
                if matches!(state, LoadState::Failed { .. }) {
                    // A failed or crashed page can no longer leave fullscreen, so nothing else
                    // would clear these. The cursor is left to the engine: after a crash the
                    // web view still caches its old cursor and reports it again.
                    self.fullscreen = false;
                    self.status_text = None;
                }
                self.load_state = state;
            }
            BrowserEvent::HistoryChanged {
                can_go_back,
                can_go_forward,
            } => {
                self.can_go_back = can_go_back;
                self.can_go_forward = can_go_forward;
            }
            BrowserEvent::CursorChanged { cursor } => {
                self.cursor = cursor;
            }
            BrowserEvent::StatusTextChanged { text } => {
                self.status_text = text;
            }
            BrowserEvent::FullscreenChanged { fullscreen } => {
                self.fullscreen = fullscreen;
            }
            // history: read by the controller's visit tracker, not shown.
            BrowserEvent::PageCommitted { .. } | BrowserEvent::PageTitleAt { .. } => {}
            // downloads: queued by the controller, not shown.
            BrowserEvent::DownloadOffered { .. } => {}
        }
    }
}
