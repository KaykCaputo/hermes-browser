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

#[derive(Debug)]
pub struct Tab {
    id: TabId,
    state: BrowserState,
}

impl Tab {
    pub fn new(id: TabId, state: BrowserState) -> Self {
        Self { id, state }
    }

    pub const fn id(&self) -> TabId {
        self.id
    }

    pub const fn state(&self) -> &BrowserState {
        &self.state
    }

    pub(crate) fn state_mut(&mut self) -> &mut BrowserState {
        &mut self.state
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
