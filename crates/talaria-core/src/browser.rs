const DEFAULT_PAGE_TITLE: &str = "New Tab";
const INITIAL_ADDRESS: &str = "https://servo.org";

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserEvent {
    AddressChanged {
        url: String,
    },
    PageTitleChanged {
        title: Option<String>,
    },
    LoadStateChanged {
        state: LoadState,
    },
    HistoryChanged {
        can_go_back: bool,
        can_go_forward: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserState {
    url: String,
    page_title: String,
    load_state: LoadState,
    can_go_back: bool,
    can_go_forward: bool,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self {
            url: INITIAL_ADDRESS.to_owned(),
            page_title: DEFAULT_PAGE_TITLE.to_owned(),
            load_state: LoadState::Idle,
            can_go_back: false,
            can_go_forward: false,
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
    pub fn load_state(&self) -> &LoadState {
        &self.load_state
    }
    pub fn can_go_back(&self) -> bool {
        self.can_go_back
    }
    pub fn can_go_forward(&self) -> bool {
        self.can_go_forward
    }

    pub fn apply(&mut self, event: BrowserEvent) {
        match event {
            BrowserEvent::AddressChanged { url } => {
                self.url = url;
            }
            BrowserEvent::PageTitleChanged { title } => {
                self.page_title = title
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| DEFAULT_PAGE_TITLE.to_owned());
            }
            BrowserEvent::LoadStateChanged { state } => {
                self.load_state = state;
            }
            BrowserEvent::HistoryChanged {
                can_go_back,
                can_go_forward,
            } => {
                self.can_go_back = can_go_back;
                self.can_go_forward = can_go_forward;
            }
        }
    }
}
