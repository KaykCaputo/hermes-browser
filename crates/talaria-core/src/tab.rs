use crate::page_request::PageRequests;
use crate::{BrowserCommand, BrowserEvent, BrowserState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(u64);

impl TabId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Whether a newly opened tab is selected or opened behind the active tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabActivation {
    Foreground,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InitialUrl {
    /// The tab's web view has not been created; it will start at this URL, or blank.
    Pending(Option<String>),
    Taken,
}

#[derive(Debug)]
pub struct Tab {
    id: TabId,
    state: BrowserState,
    initial_url: InitialUrl,
    engine_ready: bool,
    // dialogs
    page_requests: PageRequests,
}

impl Tab {
    pub fn new(id: TabId, state: BrowserState) -> Self {
        Self {
            id,
            state,
            initial_url: InitialUrl::Pending(None),
            engine_ready: false,
            page_requests: PageRequests::default(),
        }
    }

    pub const fn id(&self) -> TabId {
        self.id
    }

    pub const fn state(&self) -> &BrowserState {
        &self.state
    }

    /// True once the engine has reported the tab's first address or history change, which
    /// means its browsing context exists and commands for it can be dispatched. A tab whose
    /// web view is about to be (re)created with a pending initial URL is never ready.
    pub const fn is_engine_ready(&self) -> bool {
        self.engine_ready && !matches!(self.initial_url, InitialUrl::Pending(Some(_)))
    }

    pub(crate) fn state_mut(&mut self) -> &mut BrowserState {
        &mut self.state
    }

    pub(crate) fn mark_engine_ready(&mut self) {
        self.engine_ready = true;
    }

    /// Replaces the initial URL; returns false once the URL has been taken.
    pub(crate) fn set_initial_url(&mut self, url: String) -> bool {
        match &mut self.initial_url {
            InitialUrl::Pending(initial_url) => {
                *initial_url = Some(url);
                true
            }
            InitialUrl::Taken => false,
        }
    }

    /// Makes `url` the pending initial URL even if the previous one was taken, so the engine
    /// replaces the tab's web view with a new one.
    pub(crate) fn replace_initial_url(&mut self, url: String) {
        self.initial_url = InitialUrl::Pending(Some(url));
        // dialogs: the web view that asked is about to be replaced.
        self.page_requests.forget_all();
    }

    /// Hands the initial URL to the engine, which creates a fresh web view for it; that web
    /// view has reported nothing yet, so the tab is no longer engine-ready.
    pub(crate) fn take_initial_url(&mut self) -> Option<String> {
        let initial_url = match std::mem::replace(&mut self.initial_url, InitialUrl::Taken) {
            InitialUrl::Pending(initial_url) => initial_url,
            InitialUrl::Taken => None,
        }?;
        self.engine_ready = false;
        // dialogs: requests belonged to the web view being replaced.
        self.page_requests.forget_all();
        Some(initial_url)
    }

    // dialogs
    /// The tab's web view is about to be replaced; requests still arriving come from the old one.
    pub(crate) const fn is_awaiting_replacement(&self) -> bool {
        matches!(self.initial_url, InitialUrl::Pending(Some(_)))
    }

    pub(crate) const fn page_requests(&self) -> &PageRequests {
        &self.page_requests
    }

    pub(crate) fn page_requests_mut(&mut self) -> &mut PageRequests {
        &mut self.page_requests
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabCommand {
    tab_id: TabId,
    command: BrowserCommand,
}

impl TabCommand {
    pub const fn new(tab_id: TabId, command: BrowserCommand) -> Self {
        Self { tab_id, command }
    }

    pub const fn tab_id(&self) -> TabId {
        self.tab_id
    }

    pub fn into_command(self) -> BrowserCommand {
        self.command
    }

    pub const fn command(&self) -> &BrowserCommand {
        &self.command
    }

    pub(crate) fn command_mut(&mut self) -> &mut BrowserCommand {
        &mut self.command
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabEvent {
    tab_id: TabId,
    event: BrowserEvent,
}

impl TabEvent {
    pub const fn new(tab_id: TabId, event: BrowserEvent) -> Self {
        Self { tab_id, event }
    }

    pub const fn tab_id(&self) -> TabId {
        self.tab_id
    }

    pub fn into_event(self) -> BrowserEvent {
        self.event
    }
}
