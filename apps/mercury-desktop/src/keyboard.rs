use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mercury_ui::MainWindow;
use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use talaria_core::{
    BrowserCommand, BrowserController, CompositionState, ImeInput, KeyLocation, KeyState,
    KeyboardInput, KeyboardModifiers, TabId,
};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::keyboard::{Key, KeyLocation as WinitKeyLocation, ModifiersState, PhysicalKey};

use crate::shortcuts::handle_browser_shortcut;

pub(crate) fn connect_native_keyboard_input(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
) {
    let weak_window = window.as_weak();
    let mut modifiers = ModifiersState::empty();
    let mut composition_tab_id: Option<TabId> = None;

    window
        .window()
        .on_winit_window_event(move |_slint_window, event| {
            let Some(window) = weak_window.upgrade() else {
                return EventResult::Propagate;
            };

            match event {
                WindowEvent::ModifiersChanged(changed) => {
                    modifiers = changed.state();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if handle_browser_shortcut(
                        &window,
                        &controller,
                        &web_view_focused,
                        event,
                        modifiers,
                    ) {
                        return EventResult::PreventDefault;
                    }

                    if web_view_focused.get() {
                        let active_tab_id = controller.borrow().active_tab_id();
                        let keyboard_input = keyboard_input_from_winit(
                            event,
                            modifiers,
                            composition_tab_id == Some(active_tab_id),
                        );
                        controller
                            .borrow_mut()
                            .queue_command(BrowserCommand::KeyboardInput(keyboard_input));
                        window.window().request_redraw();
                    }
                }
                WindowEvent::Ime(ime) if web_view_focused.get() || composition_tab_id.is_some() => {
                    queue_ime_input(&controller, &mut composition_tab_id, ime);
                    window.window().request_redraw();
                }
                WindowEvent::Focused(false) => {
                    dismiss_composition(&controller, &mut composition_tab_id);
                }
                _ => {}
            }

            EventResult::Propagate
        });
}

pub(crate) fn keyboard_input_from_winit(
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
    is_composing: bool,
) -> KeyboardInput {
    KeyboardInput {
        state: match event.state {
            ElementState::Pressed => KeyState::Down,
            ElementState::Released => KeyState::Up,
        },
        key: logical_key_string(&event.logical_key),
        code: physical_key_string(event.physical_key),
        location: match event.location {
            WinitKeyLocation::Standard => KeyLocation::Standard,
            WinitKeyLocation::Left => KeyLocation::Left,
            WinitKeyLocation::Right => KeyLocation::Right,
            WinitKeyLocation::Numpad => KeyLocation::Numpad,
        },
        modifiers: KeyboardModifiers {
            alt: modifiers.alt_key(),
            control: modifiers.control_key(),
            meta: modifiers.super_key(),
            shift: modifiers.shift_key(),
        },
        repeat: event.repeat,
        is_composing,
    }
}

pub(crate) fn physical_key_string(key: PhysicalKey) -> String {
    match key {
        PhysicalKey::Code(winit::keyboard::KeyCode::SuperLeft) => "MetaLeft".to_owned(),
        PhysicalKey::Code(winit::keyboard::KeyCode::SuperRight) => "MetaRight".to_owned(),
        PhysicalKey::Code(winit::keyboard::KeyCode::Meta) => "Super".to_owned(),
        PhysicalKey::Code(code) => format!("{code:?}"),
        PhysicalKey::Unidentified(_) => "Unidentified".to_owned(),
    }
}

pub(crate) fn logical_key_string(key: &Key) -> String {
    match key {
        Key::Character(character) => character.to_string(),
        Key::Named(winit::keyboard::NamedKey::Space) => " ".to_owned(),
        Key::Named(named) => format!("{named:?}"),
        Key::Dead(_) => "Dead".to_owned(),
        Key::Unidentified(_) => "Unidentified".to_owned(),
    }
}

pub(crate) fn queue_ime_input(
    controller: &RefCell<BrowserController>,
    composition_tab_id: &mut Option<TabId>,
    ime: &Ime,
) {
    match ime {
        Ime::Enabled => {}
        Ime::Preedit(data, _) => {
            let tab_id = *composition_tab_id.get_or_insert_with(|| {
                let tab_id = controller.borrow().active_tab_id();
                controller.borrow_mut().queue_command_for(
                    tab_id,
                    BrowserCommand::ImeInput(ImeInput::Composition {
                        state: CompositionState::Start,
                        data: String::new(),
                    }),
                );
                tab_id
            });
            controller.borrow_mut().queue_command_for(
                tab_id,
                BrowserCommand::ImeInput(ImeInput::Composition {
                    state: CompositionState::Update,
                    data: data.clone(),
                }),
            );
        }
        Ime::Commit(data) => {
            let (tab_id, needs_start) = match composition_tab_id.take() {
                Some(tab_id) => (tab_id, false),
                None => (controller.borrow().active_tab_id(), true),
            };
            if needs_start {
                controller.borrow_mut().queue_command_for(
                    tab_id,
                    BrowserCommand::ImeInput(ImeInput::Composition {
                        state: CompositionState::Start,
                        data: String::new(),
                    }),
                );
            }
            controller.borrow_mut().queue_command_for(
                tab_id,
                BrowserCommand::ImeInput(ImeInput::Composition {
                    state: CompositionState::End,
                    data: data.clone(),
                }),
            );
        }
        Ime::Disabled => {
            dismiss_composition(controller, composition_tab_id);
        }
    }
}

pub(crate) fn dismiss_composition(
    controller: &RefCell<BrowserController>,
    composition_tab_id: &mut Option<TabId>,
) {
    if let Some(tab_id) = composition_tab_id.take() {
        controller
            .borrow_mut()
            .queue_command_for(tab_id, BrowserCommand::ImeInput(ImeInput::Dismissed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_keys_use_web_keyboard_event_values() {
        assert_eq!(logical_key_string(&Key::Character("ç".into())), "ç");
        assert_eq!(
            logical_key_string(&Key::Named(winit::keyboard::NamedKey::Enter)),
            "Enter"
        );
        assert_eq!(
            logical_key_string(&Key::Named(winit::keyboard::NamedKey::Space)),
            " "
        );
        assert_eq!(logical_key_string(&Key::Dead(Some('´'))), "Dead");
    }

    #[test]
    fn physical_keys_use_web_keyboard_event_code_values() {
        use winit::keyboard::KeyCode;

        assert_eq!(physical_key_string(KeyCode::KeyA.into()), "KeyA");
        assert_eq!(physical_key_string(KeyCode::SuperLeft.into()), "MetaLeft");
        assert_eq!(physical_key_string(KeyCode::SuperRight.into()), "MetaRight");
        assert_eq!(physical_key_string(KeyCode::Meta.into()), "Super");
    }

    #[test]
    fn ime_preedit_and_commit_form_one_composition_sequence() {
        let controller = RefCell::new(BrowserController::default());
        let tab_id = controller.borrow().active_tab_id();
        let mut composition_tab_id = None;

        queue_ime_input(
            &controller,
            &mut composition_tab_id,
            &Ime::Preedit("aç".to_owned(), Some((2, 2))),
        );
        queue_ime_input(
            &controller,
            &mut composition_tab_id,
            &Ime::Commit("ação".to_owned()),
        );

        let commands =
            std::iter::from_fn(|| controller.borrow_mut().next_command()).collect::<Vec<_>>();
        assert_eq!(commands.len(), 3);
        assert!(commands.iter().all(|command| command.tab_id() == tab_id));
        assert_eq!(
            commands[0].command(),
            &BrowserCommand::ImeInput(ImeInput::Composition {
                state: CompositionState::Start,
                data: String::new(),
            })
        );
        assert_eq!(
            commands[1].command(),
            &BrowserCommand::ImeInput(ImeInput::Composition {
                state: CompositionState::Update,
                data: "aç".to_owned(),
            })
        );
        assert_eq!(
            commands[2].command(),
            &BrowserCommand::ImeInput(ImeInput::Composition {
                state: CompositionState::End,
                data: "ação".to_owned(),
            })
        );
        assert_eq!(composition_tab_id, None);
    }

    #[test]
    fn ime_dismissal_stays_with_the_tab_that_started_composition() {
        let controller = RefCell::new(BrowserController::default());
        let composing_tab_id = controller.borrow().active_tab_id();
        let mut composition_tab_id = None;
        queue_ime_input(
            &controller,
            &mut composition_tab_id,
            &Ime::Preedit("her".to_owned(), None),
        );
        controller.borrow_mut().open_tab();

        queue_ime_input(&controller, &mut composition_tab_id, &Ime::Disabled);

        let commands =
            std::iter::from_fn(|| controller.borrow_mut().next_command()).collect::<Vec<_>>();
        assert!(
            commands
                .iter()
                .all(|command| command.tab_id() == composing_tab_id)
        );
        assert_eq!(
            commands.last().map(|command| command.command()),
            Some(&BrowserCommand::ImeInput(ImeInput::Dismissed))
        );
        assert_eq!(composition_tab_id, None);
    }
}
