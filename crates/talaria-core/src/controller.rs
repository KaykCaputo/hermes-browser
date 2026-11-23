use std::collections::VecDeque;

use crate::address::{AddressResolutionError, resolve_address};
use crate::browser::{BrowserCommand, BrowserEvent, BrowserState};
use crate::{Tab, TabCommand, TabId};

#[derive(Debug)]
pub struct BrowserController {
    tabs: Vec<Tab>,
    active_tab_id: TabId,
    next_tab_id: u64,
    tab_revision: u64,
    closed_tabs: VecDeque<(usize, String)>,
    command_queue: VecDeque<TabCommand>,
}

impl BrowserController {
    pub fn new(state: BrowserState) -> Self {
        let initial_tab_id = TabId::new(1);

        Self {
            tabs: vec![Tab::new(initial_tab_id, state)],
            active_tab_id: initial_tab_id,
            next_tab_id: 2,
            tab_revision: 0,
            closed_tabs: VecDeque::new(),
            command_queue: VecDeque::new(),
        }
    }

    /// Restore the tab strip without scheduling any network requests. The desktop
    /// opens a restored address when that tab first needs a WebView.
    pub fn from_session(addresses: &[String], selected: usize) -> Self {
        let mut controller = Self::default();
        if addresses.is_empty() {
            return controller;
        }
        controller.tabs = addresses
            .iter()
            .enumerate()
            .map(|(index, address)| {
                let mut state = BrowserState::default();
                if let Ok(url) = url::Url::parse(address)
                    && matches!(url.scheme(), "http" | "https" | "about")
                    && url.as_str() != "about:blank"
                {
                    let title = url.host_str().unwrap_or(url.as_str()).to_owned();
                    state.apply(BrowserEvent::AddressChanged { url: url.into() });
                    state.apply(BrowserEvent::PageTitleChanged { title: Some(title) });
                }
                Tab::new(TabId::new(index as u64 + 1), state)
            })
            .collect();
        controller.next_tab_id = addresses.len() as u64 + 1;
        controller.select_tab_at(selected.min(addresses.len() - 1));
        controller
    }

    pub fn tab_revision(&self) -> u64 {
        self.tab_revision
    }

    pub fn pending_navigations(&self) -> impl Iterator<Item = TabId> + '_ {
        self.command_queue
            .iter()
            .filter(|queued| matches!(queued.command(), BrowserCommand::Navigate { .. }))
            .map(TabCommand::tab_id)
    }

    /// A newly constructed WebView starts its first navigation through its builder.
    /// Loading about:blank first races Servo's initial pipeline registration.
    pub fn take_initial_navigation(&mut self, tab_id: TabId) -> Option<String> {
        let index = self.command_queue.iter().position(|queued| {
            queued.tab_id() == tab_id && matches!(queued.command(), BrowserCommand::Navigate { .. })
        })?;
        match self.command_queue.remove(index)?.into_command() {
            BrowserCommand::Navigate { url } => Some(url),
            _ => unreachable!("located a navigation command"),
        }
    }

    pub fn queue_navigation(&mut self, input: &str) -> Result<(), AddressResolutionError> {
        let url = resolve_address(input)?.to_string();
        self.active_state_mut()
            .apply(BrowserEvent::AddressChanged { url: url.clone() });
        self.queue_command(BrowserCommand::Navigate { url });
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
        let tab_id = TabId::new(self.next_tab_id);
        self.next_tab_id += 1;
        self.tab_revision += 1;
        self.tabs.push(Tab::new(tab_id, BrowserState::default()));
        self.active_tab_id = tab_id;
        tab_id
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

    pub fn close_tab(&mut self, tab_id: TabId) -> bool {
        if self.tabs.len() == 1 {
            return false;
        }

        let Some(index) = self.tabs.iter().position(|tab| tab.id() == tab_id) else {
            return false;
        };

        let was_active = self.active_tab_id == tab_id;
        let removed = self.tabs.remove(index);
        self.tab_revision += 1;
        self.closed_tabs
            .push_back((index, removed.state().url().to_owned()));
        if self.closed_tabs.len() > 25 {
            self.closed_tabs.pop_front();
        }
        self.command_queue
            .retain(|queued| queued.tab_id() != tab_id);

        if was_active {
            let replacement_index = index.saturating_sub(1).min(self.tabs.len() - 1);
            self.active_tab_id = self.tabs[replacement_index].id();
        }

        true
    }

    pub fn reopen_closed_tab(&mut self) -> bool {
        let Some((index, address)) = self.closed_tabs.pop_back() else {
            return false;
        };
        let id = TabId::new(self.next_tab_id);
        self.next_tab_id += 1;
        self.tab_revision += 1;
        self.tabs.insert(
            index.min(self.tabs.len()),
            Tab::new(id, BrowserState::default()),
        );
        self.active_tab_id = id;
        if !address.is_empty() {
            self.active_state_mut().apply(BrowserEvent::AddressChanged {
                url: address.clone(),
            });
            self.queue_command(BrowserCommand::Navigate { url: address });
        }
        true
    }

    pub fn close_tab_at(&mut self, index: usize) -> bool {
        let Some(tab_id) = self.tabs.get(index).map(Tab::id) else {
            return false;
        };

        self.close_tab(tab_id)
    }

    pub fn queue_command(&mut self, command: BrowserCommand) {
        let tab_id = self.active_tab_id;
        let queued = self.queue_command_for(tab_id, command);
        debug_assert!(queued, "the active tab must accept commands");
    }

    pub fn queue_command_for(&mut self, tab_id: TabId, command: BrowserCommand) -> bool {
        if !self.tabs.iter().any(|tab| tab.id() == tab_id) {
            return false;
        }

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

    pub fn next_command(&mut self) -> Option<TabCommand> {
        self.command_queue.pop_front()
    }

    pub fn has_pending_commands(&self) -> bool {
        !self.command_queue.is_empty()
    }

    pub fn handle_event(&mut self, tab_id: TabId, event: BrowserEvent) -> bool {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id() == tab_id) else {
            return false;
        };

        tab.state_mut().apply(event);
        true
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id() == self.active_tab_id)
    }

    fn active_state_mut(&mut self) -> &mut BrowserState {
        let active_tab_id = self.active_tab_id;
        self.tabs
            .iter_mut()
            .find(|tab| tab.id() == active_tab_id)
            .expect("the controller always owns an active tab")
            .state_mut()
    }
}

impl Default for BrowserController {
    fn default() -> Self {
        Self::new(BrowserState::default())
    }
}
