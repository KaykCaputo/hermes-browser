use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mercury_ui::MainWindow;
use servo::Location;
use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use talaria_core::{
    BrowserCommand, BrowserController, ColorScheme, CompositionState, ImeInput, KeyLocation,
    KeyState, KeyboardInput, KeyboardModifiers, TabId,
};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::keyboard::{
    Key, KeyLocation as WinitKeyLocation, ModifiersState, NamedKey, PhysicalKey,
};
use winit::window::Theme;

use crate::keyutils;
use crate::shortcuts::{exits_fullscreen, handle_browser_shortcut};

pub(crate) fn connect_native_keyboard_input(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
) {
    let weak_window = window.as_weak();
    let mut modifiers = ModifiersState::empty();
    let mut composition_tab_id: Option<TabId> = None;
    let mut escape_exited_fullscreen = false;

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
                    let page_fullscreen = controller.borrow().active_state().is_fullscreen();
                    if event.state == ElementState::Pressed
                        && exits_fullscreen(&event.logical_key, page_fullscreen)
                    {
                        controller
                            .borrow_mut()
                            .queue_command(BrowserCommand::ExitFullscreen);
                        escape_exited_fullscreen = true;
                        window.window().request_redraw();
                        return EventResult::PreventDefault;
                    }
                    if event.state == ElementState::Released
                        && escape_exited_fullscreen
                        && matches!(event.logical_key, Key::Named(NamedKey::Escape))
                    {
                        escape_exited_fullscreen = false;
                        return EventResult::PreventDefault;
                    }

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
                WindowEvent::PinchGesture { delta, .. }
                    if window.get_web_view_visible() && window.get_web_view_hovered() =>
                {
                    controller.borrow_mut().queue_command(pinch_zoom_command(
                        *delta,
                        window.get_web_view_pointer_x(),
                        window.get_web_view_pointer_y(),
                    ));
                    window.window().request_redraw();
                    return EventResult::PreventDefault;
                }
                WindowEvent::ThemeChanged(theme) => {
                    controller
                        .borrow_mut()
                        .set_color_scheme(color_scheme_from_winit(*theme));
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
        location: key_location(event.location),
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

pub(crate) fn color_scheme_from_winit(theme: Theme) -> ColorScheme {
    match theme {
        Theme::Light => ColorScheme::Light,
        Theme::Dark => ColorScheme::Dark,
    }
}

/// winit reports the change in magnification (positive when zooming in); Servo multiplies the
/// current pinch zoom by the factor. The position is in logical web-view pixels.
pub(crate) fn pinch_zoom_command(delta: f64, x: f32, y: f32) -> BrowserCommand {
    BrowserCommand::PinchZoom {
        delta: delta as f32 + 1.0,
        x,
        y,
    }
}

pub(crate) fn physical_key_string(key: PhysicalKey) -> String {
    keyutils::code_from_winit(key).to_string()
}

pub(crate) fn logical_key_string(key: &Key) -> String {
    keyutils::key_from_winit(key).to_string()
}

fn key_location(location: WinitKeyLocation) -> KeyLocation {
    match keyutils::location_from_winit(location) {
        Location::Left => KeyLocation::Left,
        Location::Right => KeyLocation::Right,
        Location::Numpad => KeyLocation::Numpad,
        Location::Standard => KeyLocation::Standard,
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
    use talaria_core::BrowserEvent;

    use super::*;

    fn mark_engine_ready(controller: &RefCell<BrowserController>, tab_id: TabId) {
        let handled = controller
            .borrow_mut()
            .handle_event(tab_id, BrowserEvent::AddressChanged { url: String::new() });
        assert!(handled);
    }

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
    fn arrow_keys_and_function_keys_map_to_w3c_names() {
        use winit::keyboard::KeyCode;

        assert_eq!(
            logical_key_string(&Key::Named(NamedKey::ArrowLeft)),
            "ArrowLeft"
        );
        assert_eq!(
            logical_key_string(&Key::Named(NamedKey::ArrowDown)),
            "ArrowDown"
        );
        assert_eq!(logical_key_string(&Key::Named(NamedKey::F5)), "F5");
        assert_eq!(logical_key_string(&Key::Named(NamedKey::F12)), "F12");
        assert_eq!(logical_key_string(&Key::Named(NamedKey::Super)), "Meta");
        assert_eq!(
            physical_key_string(KeyCode::ArrowRight.into()),
            "ArrowRight"
        );
        assert_eq!(physical_key_string(KeyCode::F5.into()), "F5");
        assert_eq!(
            physical_key_string(KeyCode::NumpadEnter.into()),
            "NumpadEnter"
        );
    }

    #[test]
    fn pinch_gestures_become_pinch_zoom_factors_at_the_pointer() {
        assert_eq!(
            pinch_zoom_command(0.25, 10.0, 20.0),
            BrowserCommand::PinchZoom {
                delta: 1.25,
                x: 10.0,
                y: 20.0,
            }
        );
        assert_eq!(
            pinch_zoom_command(-0.5, 0.0, 0.0),
            BrowserCommand::PinchZoom {
                delta: 0.5,
                x: 0.0,
                y: 0.0,
            }
        );
    }

    #[test]
    fn winit_themes_become_colour_schemes() {
        assert_eq!(color_scheme_from_winit(Theme::Dark), ColorScheme::Dark);
        assert_eq!(color_scheme_from_winit(Theme::Light), ColorScheme::Light);
    }

    #[test]
    fn key_locations_map_through_the_w3c_location() {
        assert_eq!(key_location(WinitKeyLocation::Numpad), KeyLocation::Numpad);
        assert_eq!(key_location(WinitKeyLocation::Right), KeyLocation::Right);
    }

    #[test]
    fn ime_preedit_and_commit_form_one_composition_sequence() {
        let controller = RefCell::new(BrowserController::default());
        let tab_id = controller.borrow().active_tab_id();
        mark_engine_ready(&controller, tab_id);
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
        mark_engine_ready(&controller, composing_tab_id);
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
