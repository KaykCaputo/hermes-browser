use std::collections::VecDeque;
use std::time::Duration;

use crate::address::{AddressResolutionError, SearchUrlBuilder, resolve_address};
use crate::browser::LoadState;
use crate::browser::{BrowserCommand, BrowserEvent, BrowserState, placeholder_title};
use crate::page_request::decision_to_remember;
use crate::{
    ColorScheme, ControlAnswer, ControlId, ControlRequest, DEFAULT_ZOOM_PERCENT, Tab,
    TabActivation, TabCommand, TabId,
};
use crate::{
    PageRequestAnswer, PageRequestId, PageRequestReply, PendingPageRequest, PermissionDecisions,
    SessionPermissionDecisions,
};

const NO_SEARCH: &str = "no search engine is set";

/// What `close_tab` did.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseTabOutcome {
    Closed,
    NotFound,
    /// The tab is the only one and stays open; closing it means quitting.
    LastTab,
}

#[derive(Debug)]
pub struct BrowserController {
    tabs: Vec<Tab>,
    active_tab_id: TabId,
    next_tab_id: u64,
    command_queue: VecDeque<TabCommand>,
    color_scheme: ColorScheme,
    // settings
    /// The system's scheme, as last reported; `color_scheme` is what pages see.
    system_color_scheme: ColorScheme,
    forced_color_scheme: Option<ColorScheme>,
    default_zoom_percent: u16,
    search: Option<SearchUrlBuilder>,
    // dialogs
    permission_decisions: Box<dyn PermissionDecisions>,
}

impl BrowserController {
    pub fn new(state: BrowserState) -> Self {
        let initial_tab_id = TabId::new(1);

        Self {
            tabs: vec![Tab::new(initial_tab_id, state)],
            active_tab_id: initial_tab_id,
            next_tab_id: 2,
            command_queue: VecDeque::new(),
            color_scheme: ColorScheme::default(),
            system_color_scheme: ColorScheme::default(),
            forced_color_scheme: None,
            default_zoom_percent: DEFAULT_ZOOM_PERCENT,
            search: None,
            permission_decisions: Box::new(SessionPermissionDecisions::default()),
        }
    }

    /// Navigates the active tab. If the tab's first page has not committed yet, the engine
    /// cannot navigate it, so the pending page is replaced instead: the URL becomes the tab's
    /// initial URL again and the engine recreates the tab's web view with it.
    pub fn queue_navigation(&mut self, input: &str) -> Result<(), AddressResolutionError> {
        let url = self.resolve_address(input)?.to_string();
        let tab_id = self.active_tab_id;
        let tab = self
            .tab_mut(tab_id)
            .expect("the controller always owns an active tab");

        if tab.is_engine_ready() {
            tab.state_mut()
                .apply(BrowserEvent::AddressChanged { url: url.clone() });
            self.queue_command(BrowserCommand::Navigate { url });
        } else {
            tab.replace_initial_url(url.clone());
            show_loading(tab, url);
            self.command_queue.retain(|queued| {
                queued.tab_id() != tab_id || !queued.command().targets_the_current_page()
            });
        }
        Ok(())
    }

    pub fn state(&self) -> &BrowserState {
        self.active_state()
    }

    pub fn active_state(&self) -> &BrowserState {
        self.active_tab()
            .expect("the controller always owns an active tab")
            .state()
    }

    pub const fn active_tab_id(&self) -> TabId {
        self.active_tab_id
    }

    pub fn tabs(&self) -> impl ExactSizeIterator<Item = &Tab> {
        self.tabs.iter()
    }

    pub fn open_tab(&mut self) -> TabId {
        self.open_blank_tab(TabActivation::Foreground)
    }

    /// Opens a tab. With `Some(address)` the address is resolved like `queue_navigation`
    /// and becomes the tab's initial URL; the tab starts in `LoadState::Loading`.
    pub fn open_tab_with(
        &mut self,
        initial_address: Option<&str>,
        activation: TabActivation,
    ) -> Result<TabId, AddressResolutionError> {
        let initial_url = initial_address
            .map(|address| self.resolve_address(address).map(|url| url.to_string()))
            .transpose()?;
        let tab_id = self.open_blank_tab(activation);

        if let Some(initial_url) = initial_url {
            let tab = self
                .tab_mut(tab_id)
                .expect("the tab was opened by this call");
            set_tab_initial_url(tab, initial_url);
        }

        Ok(tab_id)
    }

    /// Sets the initial URL of a tab whose web view has not been created yet.
    /// Returns Ok(false) if the tab does not exist or its initial URL was already taken.
    pub fn set_initial_address(
        &mut self,
        tab_id: TabId,
        address: &str,
    ) -> Result<bool, AddressResolutionError> {
        let url = self.resolve_address(address)?.to_string();
        Ok(self
            .tab_mut(tab_id)
            .is_some_and(|tab| set_tab_initial_url(tab, url)))
    }

    /// Returns the tab's initial URL exactly once (None afterwards, and None for a blank tab),
    /// unless a navigation before the first commit made a new one pending. When it returns a
    /// URL the tab is no longer engine-ready and shows that URL as loading.
    pub fn take_initial_url(&mut self, tab_id: TabId) -> Option<String> {
        let tab = self.tab_mut(tab_id)?;
        let url = tab.take_initial_url()?;
        // controls: the new web view has asked for nothing yet.
        tab.state_mut().forget_controls();
        show_loading(tab, url.clone());
        Some(url)
    }

    pub fn select_tab(&mut self, tab_id: TabId) -> bool {
        if self.tabs.iter().any(|tab| tab.id() == tab_id) {
            self.active_tab_id = tab_id;
            true
        } else {
            false
        }
    }

    pub fn select_tab_at(&mut self, index: usize) -> bool {
        let Some(tab) = self.tabs.get(index) else {
            return false;
        };

        self.active_tab_id = tab.id();
        true
    }

    pub fn select_next_tab(&mut self, backwards: bool) {
        let active_index = self
            .tabs
            .iter()
            .position(|tab| tab.id() == self.active_tab_id)
            .expect("the active tab is always present");
        let next_index = if backwards {
            active_index.checked_sub(1).unwrap_or(self.tabs.len() - 1)
        } else {
            (active_index + 1) % self.tabs.len()
        };

        self.active_tab_id = self.tabs[next_index].id();
    }

    pub fn select_last_tab(&mut self) {
        if let Some(tab) = self.tabs.last() {
            self.active_tab_id = tab.id();
        }
    }

    /// Zooms the active tab in by one step; see `zoom_in`.
    pub fn zoom_in(&mut self) {
        self.set_active_zoom(crate::zoom_in(self.active_state().zoom_percent()));
    }

    /// Zooms the active tab out by one step; see `zoom_out`.
    pub fn zoom_out(&mut self) {
        self.set_active_zoom(crate::zoom_out(self.active_state().zoom_percent()));
    }

    pub fn reset_zoom(&mut self) {
        self.set_active_zoom(self.default_zoom_percent);
    }

    /// The colour scheme pages should see; the engine gives it to every new web view.
    pub const fn color_scheme(&self) -> ColorScheme {
        self.color_scheme
    }

    /// Records the scheme and queues it for every tab. Tabs that are not engine-ready hold it
    /// until they are, like other non-pointer commands.
    /// While a scheme is forced (`set_forced_color_scheme`), the system's is only remembered.
    pub fn set_color_scheme(&mut self, scheme: ColorScheme) {
        self.system_color_scheme = scheme;
        if self.forced_color_scheme.is_some() {
            return;
        }
        self.send_color_scheme(scheme);
    }

    fn send_color_scheme(&mut self, scheme: ColorScheme) {
        self.color_scheme = scheme;
        let tab_ids = self.tabs.iter().map(Tab::id).collect::<Vec<_>>();
        for tab_id in tab_ids {
            self.queue_command_for(tab_id, BrowserCommand::SetColorScheme(scheme));
        }
    }

    /// Records the zoom and queues it for the engine. A zoom requested before the tab is
    /// engine-ready is held like other non-pointer commands; the engine also applies the
    /// recorded zoom whenever it creates the tab's web view.
    fn set_active_zoom(&mut self, percent: u16) {
        let tab_id = self.active_tab_id;
        let tab = self
            .tab_mut(tab_id)
            .expect("the controller always owns an active tab");
        if tab.state().zoom_percent() == percent {
            return;
        }
        tab.state_mut().set_zoom_percent(percent);
        self.queue_command(BrowserCommand::SetPageZoom { percent });
    }

    /// Closes a tab. The only tab is never closed; `LastTab` tells the caller to quit instead.
    pub fn close_tab(&mut self, tab_id: TabId) -> CloseTabOutcome {
        let Some(index) = self.tabs.iter().position(|tab| tab.id() == tab_id) else {
            return CloseTabOutcome::NotFound;
        };
        if self.tabs.len() == 1 {
            return CloseTabOutcome::LastTab;
        }

        let was_active = self.active_tab_id == tab_id;
        self.tabs.remove(index);
        self.command_queue
            .retain(|queued| queued.tab_id() != tab_id);

        if was_active {
            let replacement_index = index.saturating_sub(1).min(self.tabs.len() - 1);
            self.active_tab_id = self.tabs[replacement_index].id();
        }

        CloseTabOutcome::Closed
    }

    pub fn close_tab_at(&mut self, index: usize) -> CloseTabOutcome {
        let Some(tab_id) = self.tabs.get(index).map(Tab::id) else {
            return CloseTabOutcome::NotFound;
        };

        self.close_tab(tab_id)
    }

    pub fn queue_command(&mut self, command: BrowserCommand) {
        let tab_id = self.active_tab_id;
        let queued = self.queue_command_for(tab_id, command);
        debug_assert!(queued, "the active tab must accept commands");
    }

    pub fn queue_command_for(&mut self, tab_id: TabId, command: BrowserCommand) -> bool {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id() == tab_id) else {
            return false;
        };

        if !tab.is_engine_ready() && command.is_discarded_before_engine_ready() {
            return true;
        }
        // dialogs
        self.observe_command_for_page_requests(tab_id, &command);

        if let Some(last_command) = self.command_queue.back_mut() {
            if last_command.tab_id() != tab_id {
                self.command_queue
                    .push_back(TabCommand::new(tab_id, command));
                return true;
            }

            match (last_command.command_mut(), &command) {
                (
                    BrowserCommand::PointerMoved { x, y },
                    BrowserCommand::PointerMoved {
                        x: latest_x,
                        y: latest_y,
                    },
                ) => {
                    *x = *latest_x;
                    *y = *latest_y;
                    return true;
                }
                (
                    BrowserCommand::Scrolled {
                        delta_x,
                        delta_y,
                        x,
                        y,
                    },
                    BrowserCommand::Scrolled {
                        delta_x: latest_delta_x,
                        delta_y: latest_delta_y,
                        x: latest_x,
                        y: latest_y,
                    },
                ) => {
                    *delta_x += latest_delta_x;
                    *delta_y += latest_delta_y;
                    *x = *latest_x;
                    *y = *latest_y;
                    return true;
                }
                (
                    BrowserCommand::PinchZoom { delta, x, y },
                    BrowserCommand::PinchZoom {
                        delta: latest_delta,
                        x: latest_x,
                        y: latest_y,
                    },
                ) => {
                    *delta *= latest_delta;
                    *x = *latest_x;
                    *y = *latest_y;
                    return true;
                }
                (
                    BrowserCommand::FocusChanged { focused },
                    BrowserCommand::FocusChanged {
                        focused: latest_focused,
                    },
                ) => {
                    *focused = *latest_focused;
                    return true;
                }
                _ => {}
            }
        }

        self.command_queue
            .push_back(TabCommand::new(tab_id, command));
        true
    }

    /// Returns the oldest queued command whose tab is engine-ready. Commands for tabs whose
    /// browsing context does not exist yet stay queued, in order, until it does.
    pub fn next_command(&mut self) -> Option<TabCommand> {
        let index = self
            .command_queue
            .iter()
            .position(|queued| self.is_dispatchable(queued))?;
        self.command_queue.remove(index)
    }

    /// Reports whether `next_command` would return a command.
    pub fn has_pending_commands(&self) -> bool {
        self.command_queue
            .iter()
            .any(|queued| self.is_dispatchable(queued))
    }

    /// Applies an engine event to its tab. `CloseRequested` changes nothing here: whether and
    /// how the tab closes is the embedder's decision.
    pub fn handle_event(&mut self, tab_id: TabId, event: BrowserEvent) -> bool {
        // dialogs
        self.observe_event_for_page_requests(tab_id, &event);
        // controls
        let event = match self.route_control_request(tab_id, event) {
            Some(event) => event,
            None => return true,
        };
        let Some(tab) = self.tab_mut(tab_id) else {
            return false;
        };

        if event.shows_engine_ready() {
            tab.mark_engine_ready();
        }
        tab.state_mut().apply(event);
        true
    }

    /// Reserves a tab for a web view the engine is creating itself. The tab is inserted
    /// directly after `opener`, is selected if `activation` says so, has no initial URL, and is
    /// titled after the opener's host until its page reports. Like any other tab it becomes
    /// engine-ready on its first address or history event.
    /// Returns None if `opener` does not exist.
    pub fn adopt_tab(&mut self, opener: TabId, activation: TabActivation) -> Option<TabId> {
        let opener_index = self.tabs.iter().position(|tab| tab.id() == opener)?;
        let title = placeholder_title(self.tabs[opener_index].state().url());
        let tab_id = TabId::new(self.next_tab_id);
        self.next_tab_id += 1;
        let mut state = BrowserState::default();
        state.set_page_title(title);
        self.tabs
            .insert(opener_index + 1, Tab::opened_by_page(tab_id, state));
        if activation == TabActivation::Foreground {
            self.active_tab_id = tab_id;
        }
        Some(tab_id)
    }

    // controls
    /// Answers the active tab's pending control. Returns false, and sends nothing, unless `id`
    /// is that control: every request is answered at most once.
    pub fn answer_control(&mut self, id: ControlId, answer: ControlAnswer) -> bool {
        let tab_id = self.active_tab_id;
        let Some(tab) = self.tab_mut(tab_id) else {
            return false;
        };
        if tab
            .state()
            .pending_control()
            .map(|(pending_id, _)| pending_id)
            != Some(id)
        {
            return false;
        }
        tab.state_mut().take_pending_control();
        self.queue_command_for(tab_id, BrowserCommand::AnswerControl { id, answer })
    }

    /// Drops the tab's pending control, if any, with the engine's default answer. Called when
    /// the tab stops being shown: pickers are interactive, and a stale one makes no sense.
    pub fn dismiss_control(&mut self, tab_id: TabId) {
        let Some(id) = self
            .tab_mut(tab_id)
            .and_then(|tab| tab.state_mut().take_pending_control())
        else {
            return;
        };
        self.queue_command_for(
            tab_id,
            BrowserCommand::AnswerControl {
                id,
                answer: ControlAnswer::Dismiss,
            },
        );
    }

    /// A control for a background tab, or one asked for while the tab's file chooser is up, is
    /// dropped at once; otherwise one that replaces a pending control drops the older one. Returns the event to apply, or None if it was consumed.
    fn route_control_request(
        &mut self,
        tab_id: TabId,
        event: BrowserEvent,
    ) -> Option<BrowserEvent> {
        let BrowserEvent::ControlShown { id, .. } = &event else {
            return Some(event);
        };
        let id = *id;
        if !self.tabs.iter().any(|tab| tab.id() == tab_id) {
            return Some(event);
        }
        // The file chooser is a sheet on the whole window: while it is up, newer requests of
        // the tab are dropped rather than the chooser.
        let file_chooser_up = self.tabs.iter().any(|tab| {
            tab.id() == tab_id
                && matches!(
                    tab.state().pending_control(),
                    Some((_, ControlRequest::File { .. }))
                )
        });
        if tab_id != self.active_tab_id || file_chooser_up {
            self.queue_command_for(
                tab_id,
                BrowserCommand::AnswerControl {
                    id,
                    answer: ControlAnswer::Dismiss,
                },
            );
            return None;
        }
        self.dismiss_control(tab_id);
        Some(event)
    }

    fn open_blank_tab(&mut self, activation: TabActivation) -> TabId {
        let tab_id = TabId::new(self.next_tab_id);
        self.next_tab_id += 1;
        let mut state = BrowserState::default();
        state.set_zoom_percent(self.default_zoom_percent);
        self.tabs.push(Tab::new(tab_id, state));
        if activation == TabActivation::Foreground {
            self.active_tab_id = tab_id;
        }
        tab_id
    }

    // settings

    /// The URL address-bar input stands for, with searches built by this controller's search
    /// builder (keywords included), without navigating anywhere. Until a builder is set, text
    /// that is not an address is refused rather than searched for anywhere.
    pub fn resolve_address(&self, input: &str) -> Result<url::Url, AddressResolutionError> {
        match &self.search {
            Some(search) => resolve_address(input, search),
            None => resolve_address(input, &SearchUrlBuilder::new(|_| Err(NO_SEARCH.to_owned()))),
        }
    }

    /// Searches typed in the address bar are built by `search` from now on.
    pub fn set_search_url_builder(&mut self, search: SearchUrlBuilder) {
        self.search = Some(search);
    }

    /// The zoom of a tab that has no zoom of its own.
    pub const fn default_zoom_percent(&self) -> u16 {
        self.default_zoom_percent
    }

    /// New tabs open at `percent`, and every tab still at the old default moves to it.
    pub fn set_default_zoom_percent(&mut self, percent: u16) {
        let old = std::mem::replace(&mut self.default_zoom_percent, percent);
        if old == percent {
            return;
        }
        let at_old_default = self
            .tabs
            .iter()
            .filter(|tab| tab.state().zoom_percent() == old)
            .map(Tab::id)
            .collect::<Vec<_>>();
        for tab_id in at_old_default {
            if let Some(tab) = self.tab_mut(tab_id) {
                tab.state_mut().set_zoom_percent(percent);
            }
            self.queue_command_for(tab_id, BrowserCommand::SetPageZoom { percent });
        }
    }

    /// Makes pages see `scheme` whatever the system's is, or follow the system again with
    /// `None`. Tabs are told only when what they see changes.
    pub fn set_forced_color_scheme(&mut self, scheme: Option<ColorScheme>) {
        self.forced_color_scheme = scheme;
        let effective = scheme.unwrap_or(self.system_color_scheme);
        if effective != self.color_scheme {
            self.send_color_scheme(effective);
        }
    }

    fn is_engine_ready(&self, tab_id: TabId) -> bool {
        self.tabs
            .iter()
            .any(|tab| tab.id() == tab_id && tab.is_engine_ready())
    }

    fn tab_mut(&mut self, tab_id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id() == tab_id)
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id() == self.active_tab_id)
    }

    fn is_dispatchable(&self, queued: &TabCommand) -> bool {
        self.is_engine_ready(queued.tab_id())
            || queued.command().is_dispatched_before_engine_ready()
    }
}

// dialogs
impl BrowserController {
    /// The request the tab shows now: the oldest one it has not answered.
    pub fn pending_page_request(&self, tab_id: TabId) -> Option<PendingPageRequest> {
        self.tabs
            .iter()
            .find(|tab| tab.id() == tab_id)?
            .page_requests()
            .front()
    }

    /// Answers one of the tab's requests and queues the answer for the engine. `now` is on the
    /// clock of `PageRequest::received_at`. Returns false if the tab has no such request (it was
    /// answered or discarded already).
    pub fn answer_page_request(
        &mut self,
        tab_id: TabId,
        id: PageRequestId,
        reply: PageRequestReply,
        now: Duration,
    ) -> bool {
        let Some(tab) = self.tab_mut(tab_id) else {
            return false;
        };
        let Some((answered, discarded)) = tab.page_requests_mut().answer(id, &reply, now) else {
            return false;
        };
        let remembered = decision_to_remember(&answered, &reply);
        self.queue_page_request_answers(tab_id, &discarded, PageRequestAnswer::Discard);
        self.queue_command_for(
            tab_id,
            BrowserCommand::AnswerPageRequest {
                id,
                answer: reply.into_answer(),
            },
        );

        if let Some((origin, permission, decision)) = remembered {
            self.permission_decisions
                .remember(&origin, permission, decision);
            let decided = self
                .tabs
                .iter_mut()
                .map(|tab| {
                    let ids = tab.page_requests_mut().take_decided(&origin, permission);
                    (tab.id(), ids)
                })
                .collect::<Vec<_>>();
            for (tab_id, ids) in decided {
                self.queue_page_request_answers(tab_id, &ids, decision.answer());
            }
        }
        true
    }

    /// Replaces where remembered permission decisions are kept (by default, for the session).
    pub fn set_permission_decisions(&mut self, decisions: Box<dyn PermissionDecisions>) {
        self.permission_decisions = decisions;
    }

    fn observe_event_for_page_requests(&mut self, tab_id: TabId, event: &BrowserEvent) {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id() == tab_id) else {
            return;
        };
        let awaiting_replacement = tab.is_awaiting_replacement();
        let requests = tab.page_requests_mut();
        match event {
            // Sent by the web view that is being replaced: it never reaches the new page.
            BrowserEvent::PageRequested(request) if awaiting_replacement => {
                self.queue_page_request_answers(tab_id, &[request.id], PageRequestAnswer::Discard);
            }
            BrowserEvent::PageRequested(request) => {
                let id = request.id;
                if let Some(answer) =
                    requests.receive(request.clone(), self.permission_decisions.as_ref())
                {
                    self.queue_page_request_answers(tab_id, &[id], answer);
                }
            }
            BrowserEvent::PageLoadStarted => {
                let discarded = requests.start_new_document();
                self.queue_page_request_answers(tab_id, &discarded, PageRequestAnswer::Discard);
            }
            BrowserEvent::LoadStateChanged {
                state: LoadState::Failed { .. },
            } => {
                let discarded = requests.discard_all();
                self.queue_page_request_answers(tab_id, &discarded, PageRequestAnswer::Discard);
            }
            _ => {}
        }
    }

    fn observe_command_for_page_requests(&mut self, tab_id: TabId, command: &BrowserCommand) {
        let Some(tab) = self.tab_mut(tab_id) else {
            return;
        };
        match command {
            // The user is leaving the page: its questions no longer matter. Answering them first
            // also unblocks the page's script, which the navigation may need.
            command if command.targets_the_current_page() => {
                let reload = matches!(command, BrowserCommand::Reload);
                let discarded = tab.page_requests_mut().note_user_navigation(reload);
                self.queue_page_request_answers(tab_id, &discarded, PageRequestAnswer::Discard);
            }
            BrowserCommand::PointerButton {
                action: crate::PointerButtonAction::Down,
                ..
            } => tab.page_requests_mut().note_user_activation(),
            BrowserCommand::KeyboardInput(input) if input.state == crate::KeyState::Down => {
                tab.page_requests_mut().note_user_activation();
            }
            _ => {}
        }
    }

    fn queue_page_request_answers(
        &mut self,
        tab_id: TabId,
        ids: &[PageRequestId],
        answer: PageRequestAnswer,
    ) {
        for &id in ids {
            self.queue_command_for(
                tab_id,
                BrowserCommand::AnswerPageRequest {
                    id,
                    answer: answer.clone(),
                },
            );
        }
    }
}

fn set_tab_initial_url(tab: &mut Tab, url: String) -> bool {
    if !tab.set_initial_url(url.clone()) {
        return false;
    }

    show_loading(tab, url);
    true
}

fn show_loading(tab: &mut Tab, url: String) {
    let state = tab.state_mut();
    state.apply(BrowserEvent::AddressChanged { url });
    state.apply(BrowserEvent::LoadStateChanged {
        state: LoadState::Loading,
    });
}

impl Default for BrowserController {
    fn default() -> Self {
        Self::new(BrowserState::default())
    }
}
