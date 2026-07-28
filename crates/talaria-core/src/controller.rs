use std::collections::VecDeque;

use crate::address::{AddressResolutionError, resolve_address};
use crate::browser::{BrowserCommand, BrowserEvent, BrowserState};

#[derive(Debug)]
pub struct BrowserController {
    state: BrowserState,
    command_queue: VecDeque<BrowserCommand>,
}

impl BrowserController {
    pub fn new(state: BrowserState) -> Self {
        Self {
            state,
            command_queue: VecDeque::new(),
        }
    }

    pub fn queue_navigation(&mut self, input: &str) -> Result<(), AddressResolutionError> {
        let url = resolve_address(input)?.to_string();
        self.state
            .apply(BrowserEvent::AddressChanged { url: url.clone() });
        self.command_queue
            .push_back(BrowserCommand::Navigate { url });
        Ok(())
    }

    pub fn state(&self) -> &BrowserState {
        &self.state
    }

    pub fn queue_command(&mut self, command: BrowserCommand) {
        if let Some(last_command) = self.command_queue.back_mut() {
            match (last_command, &command) {
                (
                    BrowserCommand::PointerMoved { x, y },
                    BrowserCommand::PointerMoved {
                        x: latest_x,
                        y: latest_y,
                    },
                ) => {
                    *x = *latest_x;
                    *y = *latest_y;
                    return;
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
                    return;
                }
                (
                    BrowserCommand::FocusChanged { focused },
                    BrowserCommand::FocusChanged {
                        focused: latest_focused,
                    },
                ) => {
                    *focused = *latest_focused;
                    return;
                }
                _ => {}
            }
        }

        self.command_queue.push_back(command);
    }

    pub fn next_command(&mut self) -> Option<BrowserCommand> {
        self.command_queue.pop_front()
    }

    pub fn has_pending_commands(&self) -> bool {
        !self.command_queue.is_empty()
    }

    pub fn handle_event(&mut self, event: BrowserEvent) {
        self.state.apply(event);
    }
}

impl Default for BrowserController {
    fn default() -> Self {
        Self::new(BrowserState::default())
    }
}
