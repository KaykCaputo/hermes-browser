use crate::{ColorScheme, DEFAULT_ZOOM_PERCENT, ImeInput, KeyboardInput, PageIcon, PointerCursor};
use crate::{PageRequest, PageRequestAnswer, PageRequestId};

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
    // dialogs
    /// Answers a page request (see `page_request`); `Discard` gives the engine's default.
    AnswerPageRequest {
        id: PageRequestId,
        answer: PageRequestAnswer,
    },
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

    /// Answers go to the web view that asked even before its first page has committed: a
    /// sign-in for that page holds the commit until it is answered.
    pub(crate) const fn is_dispatched_before_engine_ready(&self) -> bool {
        matches!(self, Self::AnswerPageRequest { .. })
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
    // dialogs
    /// The page asks the user something; see `page_request`.
    PageRequested(PageRequest),
    /// A new document started loading in the tab (not a same-document navigation).
    PageLoadStarted,
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
                // dialogs: Servo writes credentials typed into a sign-in into the request URL,
                // and so into the page's address; they are never shown or kept.
                let url = crate::strip_user_info(&url);
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
            // dialogs: kept by the tab's page requests, not here.
            BrowserEvent::PageRequested(_) | BrowserEvent::PageLoadStarted => {}
        }
    }
}
