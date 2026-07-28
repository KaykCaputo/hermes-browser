use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::error::Error;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};

use hermes_ui::{BrowserTab, MainWindow, WebViewPointerButton};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{
    BackendSelector, ComponentHandle, GraphicsAPI, Image, ModelRc, RenderingState, Rgba8Pixel,
    SharedPixelBuffer, VecModel,
};
use talaria_core::{
    BrowserCommand, BrowserController, BrowserEvent, CompositionState, ImeInput, KeyLocation,
    KeyState, KeyboardInput, KeyboardModifiers, LoadState, PageIcon, PointerButton,
    PointerButtonAction, TabEvent, TabId, WebView as TalariaWebView, dispatch_command,
};
use talaria_servo::{
    EventLoopWaker, NEW_TAB_URL, ServoRuntime, ServoWebView, ServoWebViewError,
    SharedOpenGlContext, ViewportSize,
};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::keyboard::{Key, KeyLocation as WinitKeyLocation, ModifiersState, PhysicalKey};

const MAX_COMMANDS_PER_FRAME: usize = 128;

fn main() -> Result<(), Box<dyn Error>> {
    select_hardware_accelerated_backend()?;

    let window = MainWindow::new()?;
    let controller = Rc::new(RefCell::new(BrowserController::default()));
    let web_view_focused = Rc::new(Cell::new(false));

    connect_servo_runtime(
        &window,
        Rc::clone(&controller),
        Rc::clone(&web_view_focused),
    )?;
    sync_ui(&window, &controller.borrow());
    connect_ui_callbacks(
        &window,
        Rc::clone(&controller),
        Rc::clone(&web_view_focused),
    );
    connect_native_keyboard_input(&window, controller, web_view_focused);

    window.run()?;
    Ok(())
}

fn select_hardware_accelerated_backend() -> Result<(), slint::platform::PlatformError> {
    BackendSelector::new()
        .backend_name("winit".to_owned())
        .renderer_name("femtovg".to_owned())
        .require_opengl()
        .select()
}

fn connect_servo_runtime(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
) -> Result<(), slint::SetRenderingNotifierError> {
    let weak_window_for_waker = window.as_weak();
    let event_loop_waker = EventLoopWaker::new(move || {
        let _wake_result = weak_window_for_waker.upgrade_in_event_loop(|window| {
            window.window().request_redraw();
        });
    });

    let runtime = ServoRuntime::new(event_loop_waker);
    let weak_window_for_rendering = window.as_weak();
    let mut rendering_context: Option<Rc<SharedOpenGlContext>> = None;
    let mut web_views = HashMap::<TabId, ServoWebView>::new();
    let mut visible_tab_id: Option<TabId> = None;
    let mut current_viewport_size: Option<ViewportSize> = None;
    let mut current_scale_factor: Option<f32> = None;
    let (browser_event_sender, browser_event_receiver) = mpsc::channel::<TabEvent>();

    window
        .window()
        .set_rendering_notifier(move |state, graphics_api| match state {
            RenderingState::RenderingSetup => {
                if rendering_context.is_some() {
                    return;
                }

                let Some(window) = weak_window_for_rendering.upgrade() else {
                    return;
                };
                window.set_web_view_visible(false);

                let GraphicsAPI::NativeOpenGL { get_proc_address } = graphics_api else {
                    window.set_content_message(
                        "The selected Slint renderer does not provide OpenGL.".into(),
                    );
                    return;
                };
                let window_size = window.window().size();
                let Some(viewport_size) = ViewportSize::new(window_size.width, window_size.height)
                else {
                    window
                        .set_content_message("The OpenGL viewport has invalid dimensions.".into());
                    return;
                };

                // SAFETY: Slint guarantees that its OpenGL context is current while this
                // notifier runs. The wrapper is dropped during RenderingTeardown.
                let context =
                    match unsafe { SharedOpenGlContext::new(viewport_size, get_proc_address) } {
                        Ok(context) => context,
                        Err(error) => {
                            window.set_content_message(
                                format!("Failed to create Surfman connection: {error:?}").into(),
                            );
                            return;
                        }
                    };

                rendering_context = Some(Rc::new(context));
                window.set_content_message(
                    "OpenGL context initialized. Waiting for Servo WebView.".into(),
                );
            }
            RenderingState::BeforeRendering => {
                runtime.spin_event_loop();

                let Some(window) = weak_window_for_rendering.upgrade() else {
                    return;
                };

                if apply_pending_browser_events(&controller, &browser_event_receiver) {
                    sync_ui(&window, &controller.borrow());
                }

                let Some(rendering_context) = rendering_context.as_ref() else {
                    return;
                };
                let scale_factor = window.window().scale_factor();
                let Some(viewport_size) = ViewportSize::from_logical(
                    window.get_web_view_width(),
                    window.get_web_view_height(),
                    scale_factor,
                ) else {
                    window
                        .set_content_message("The WebView viewport has invalid dimensions.".into());
                    return;
                };

                let (active_tab_id, tab_ids) = {
                    let controller = controller.borrow();
                    let active_tab_id = controller.active_tab_id();
                    let tab_ids = controller.tabs().map(|tab| tab.id()).collect::<Vec<_>>();
                    (active_tab_id, tab_ids)
                };
                web_views.retain(|tab_id, _| tab_ids.contains(tab_id));
                let mut active_web_view_was_created = false;

                for tab_id in tab_ids {
                    if web_views.contains_key(&tab_id) {
                        continue;
                    }

                    match runtime.create_web_view(
                        Rc::clone(rendering_context),
                        NEW_TAB_URL,
                        scale_factor,
                        tab_id,
                        browser_event_sender.clone(),
                    ) {
                        Ok(created_web_view) => {
                            created_web_view.hide();
                            active_web_view_was_created |= tab_id == active_tab_id;
                            web_views.insert(tab_id, created_web_view);
                        }
                        Err(error) => {
                            report_web_view_error(&window, &controller, tab_id, &error);
                            return;
                        }
                    }
                }

                let active_tab_changed = visible_tab_id != Some(active_tab_id);
                if active_tab_changed {
                    if let Some(previous_tab_id) = visible_tab_id
                        && let Some(previous_web_view) = web_views.get_mut(&previous_tab_id)
                    {
                        let _focus_result = previous_web_view.set_focus(false);
                        previous_web_view.hide();
                        previous_web_view.set_throttled(true);
                    }

                    if let Some(active_web_view) = web_views.get_mut(&active_tab_id) {
                        // New WebViews start unthrottled. Waiting until a later event-loop turn
                        // avoids sending SetWebViewThrottled before Servo has installed the new
                        // top-level browsing context.
                        if !active_web_view_was_created {
                            active_web_view.set_throttled(false);
                        }
                        active_web_view.show();
                        let _focus_result = active_web_view.set_focus(web_view_focused.get());
                    }
                    visible_tab_id = Some(active_tab_id);
                }

                if current_scale_factor != Some(scale_factor) {
                    for web_view in web_views.values() {
                        web_view.set_scale_factor(scale_factor);
                    }
                    current_scale_factor = Some(scale_factor);
                }

                if (active_tab_changed || current_viewport_size != Some(viewport_size))
                    && let Some(active_web_view) = web_views.get(&active_tab_id)
                {
                    // Each Servo WebView owns its resize notification. Resizing the shared
                    // context directly would skip Servo's CSS viewport update.
                    active_web_view.resize(viewport_size);
                    current_viewport_size = Some(viewport_size);
                }

                match dispatch_pending_commands(&controller, &mut web_views) {
                    Ok(has_pending_commands) => {
                        if has_pending_commands {
                            window.window().request_redraw();
                        }
                    }
                    Err((tab_id, error)) => {
                        report_web_view_error(&window, &controller, tab_id, &error);
                        return;
                    }
                }

                if let Some(active_web_view) = web_views.get(&active_tab_id) {
                    let _painted = active_web_view.paint_if_available();
                }
            }
            RenderingState::RenderingTeardown => {
                web_views.clear();
                drop(rendering_context.take());
                visible_tab_id = None;
                current_viewport_size = None;
                current_scale_factor = None;
            }
            _ => {}
        })
}

fn report_web_view_error(
    window: &MainWindow,
    controller: &RefCell<BrowserController>,
    tab_id: TabId,
    error: &ServoWebViewError,
) {
    controller.borrow_mut().handle_event(
        tab_id,
        BrowserEvent::LoadStateChanged {
            state: LoadState::Failed {
                message: error.to_string(),
            },
        },
    );
    sync_ui(window, &controller.borrow());
}

fn apply_pending_browser_events(
    controller: &RefCell<BrowserController>,
    receiver: &Receiver<TabEvent>,
) -> bool {
    let mut state_changed = false;
    let mut controller = controller.borrow_mut();

    for tab_event in receiver.try_iter() {
        let tab_id = tab_event.tab_id();
        state_changed |= controller.handle_event(tab_id, tab_event.into_event());
    }

    state_changed
}

fn dispatch_pending_commands<W: TalariaWebView>(
    controller: &RefCell<BrowserController>,
    web_views: &mut HashMap<TabId, W>,
) -> Result<bool, (TabId, W::Error)> {
    for _ in 0..MAX_COMMANDS_PER_FRAME {
        let queued = { controller.borrow_mut().next_command() };
        let Some(queued) = queued else {
            return Ok(false);
        };
        let tab_id = queued.tab_id();
        let Some(web_view) = web_views.get_mut(&tab_id) else {
            continue;
        };

        dispatch_command(web_view, queued.into_command()).map_err(|error| (tab_id, error))?;
    }

    Ok(controller.borrow().has_pending_commands())
}

fn sync_ui(window: &MainWindow, controller: &BrowserController) {
    let state = controller.active_state();
    window.set_page_title(state.page_title().into());
    window.set_address(state.url().into());
    window.set_is_new_tab(state.url().is_empty());
    window.set_can_go_back(state.can_go_back());
    window.set_can_go_forward(state.can_go_forward());
    window.set_is_loading(matches!(state.load_state(), LoadState::Loading));
    window.set_web_view_visible(
        !state.url().is_empty() && !matches!(state.load_state(), LoadState::Failed { .. }),
    );

    let active_tab_id = controller.active_tab_id();
    let mut active_tab_index = 0;
    let tabs = controller
        .tabs()
        .enumerate()
        .map(|(index, tab)| {
            let state = tab.state();
            if tab.id() == active_tab_id {
                active_tab_index = index;
            }
            let (icon, has_icon) = match state.page_icon() {
                Some(icon) => (slint_image_from_page_icon(icon), true),
                None => (Image::default(), false),
            };

            BrowserTab {
                title: state.page_title().into(),
                icon,
                has_icon,
                active: tab.id() == active_tab_id,
                loading: matches!(state.load_state(), LoadState::Loading),
            }
        })
        .collect::<Vec<_>>();
    window.set_tabs(ModelRc::new(VecModel::from(tabs)));
    window.set_active_tab_index(i32::try_from(active_tab_index).unwrap_or(i32::MAX));
    window.invoke_reveal_active_tab();

    let content_message = match state.load_state() {
        LoadState::Failed { message } => message.as_str(),
        _ if state.url().is_empty() => "Search privately with DuckDuckGo.",
        LoadState::Idle => "Servo runtime initialized. Waiting for WebView.",
        LoadState::Loading => "The page is loading.",
        LoadState::Loaded => "",
    };
    window.set_content_message(content_message.into());
}

fn slint_image_from_page_icon(icon: &PageIcon) -> Image {
    let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(icon.width(), icon.height());
    pixel_buffer.make_mut_bytes().copy_from_slice(icon.rgba());
    Image::from_rgba8(pixel_buffer)
}

fn pointer_button_from_ui(button: WebViewPointerButton) -> PointerButton {
    match button {
        WebViewPointerButton::Left => PointerButton::Left,
        WebViewPointerButton::Middle => PointerButton::Middle,
        WebViewPointerButton::Right => PointerButton::Right,
        WebViewPointerButton::Other => PointerButton::Other,
    }
}

fn connect_ui_callbacks(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
) {
    let navigate_controller = Rc::clone(&controller);
    let navigate_window = window.as_weak();
    window.on_navigate(move |address| {
        let result = navigate_controller
            .borrow_mut()
            .queue_navigation(address.as_str());
        let Some(window) = navigate_window.upgrade() else {
            return;
        };

        if let Err(error) = result {
            let tab_id = navigate_controller.borrow().active_tab_id();
            navigate_controller.borrow_mut().handle_event(
                tab_id,
                BrowserEvent::LoadStateChanged {
                    state: LoadState::Failed {
                        message: error.to_string(),
                    },
                },
            );
        }
        sync_ui(&window, &navigate_controller.borrow());
        window.window().request_redraw();
    });

    let back_controller = Rc::clone(&controller);
    let back_window = window.as_weak();
    window.on_go_back(move || {
        back_controller
            .borrow_mut()
            .queue_command(BrowserCommand::GoBack);
        if let Some(window) = back_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let forward_controller = Rc::clone(&controller);
    let forward_window = window.as_weak();
    window.on_go_forward(move || {
        forward_controller
            .borrow_mut()
            .queue_command(BrowserCommand::GoForward);
        if let Some(window) = forward_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let reload_controller = Rc::clone(&controller);
    let reload_window = window.as_weak();
    window.on_reload(move || {
        reload_controller
            .borrow_mut()
            .queue_command(BrowserCommand::Reload);
        if let Some(window) = reload_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let new_tab_controller = Rc::clone(&controller);
    let new_tab_window = window.as_weak();
    window.on_new_tab(move || {
        new_tab_controller.borrow_mut().open_tab();
        if let Some(window) = new_tab_window.upgrade() {
            sync_ui(&window, &new_tab_controller.borrow());
            window.window().request_redraw();
        }
    });

    let select_tab_controller = Rc::clone(&controller);
    let select_tab_window = window.as_weak();
    window.on_select_tab(move |index| {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if select_tab_controller.borrow_mut().select_tab_at(index)
            && let Some(window) = select_tab_window.upgrade()
        {
            sync_ui(&window, &select_tab_controller.borrow());
            window.window().request_redraw();
        }
    });

    let close_tab_controller = Rc::clone(&controller);
    let close_tab_window = window.as_weak();
    window.on_close_tab(move |index| {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if close_tab_controller.borrow_mut().close_tab_at(index)
            && let Some(window) = close_tab_window.upgrade()
        {
            sync_ui(&window, &close_tab_controller.borrow());
            window.window().request_redraw();
        }
    });

    let pointer_moved_controller = Rc::clone(&controller);
    let pointer_moved_window = window.as_weak();
    window.on_web_view_pointer_moved(move |x, y| {
        pointer_moved_controller
            .borrow_mut()
            .queue_command(BrowserCommand::PointerMoved { x, y });
        if let Some(window) = pointer_moved_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let pointer_button_controller = Rc::clone(&controller);
    let pointer_button_window = window.as_weak();
    window.on_web_view_pointer_button(move |button, pressed, x, y| {
        let action = if pressed {
            PointerButtonAction::Down
        } else {
            PointerButtonAction::Up
        };
        pointer_button_controller
            .borrow_mut()
            .queue_command(BrowserCommand::PointerButton {
                action,
                button: pointer_button_from_ui(button),
                x,
                y,
            });
        if let Some(window) = pointer_button_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let scroll_controller = Rc::clone(&controller);
    let scroll_window = window.as_weak();
    window.on_web_view_scrolled(move |delta_x, delta_y, x, y| {
        scroll_controller
            .borrow_mut()
            .queue_command(BrowserCommand::Scrolled {
                delta_x,
                delta_y,
                x,
                y,
            });
        if let Some(window) = scroll_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let focus_controller = controller;
    let focus_window = window.as_weak();
    window.on_web_view_focus_changed(move |focused| {
        web_view_focused.set(focused);
        focus_controller
            .borrow_mut()
            .queue_command(BrowserCommand::FocusChanged { focused });
        if let Some(window) = focus_window.upgrade() {
            let _native_window = window
                .window()
                .with_winit_window(|native_window| native_window.set_ime_allowed(focused));
            window.window().request_redraw();
        }
    });
}

fn connect_native_keyboard_input(
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

fn handle_browser_shortcut(
    window: &MainWindow,
    controller: &RefCell<BrowserController>,
    web_view_focused: &Cell<bool>,
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
) -> bool {
    let primary = modifiers.control_key() || modifiers.super_key();
    let character = match &event.logical_key {
        Key::Character(character) => Some(character.to_ascii_lowercase()),
        _ => None,
    };
    let is_pressed = event.state == ElementState::Pressed;

    let shortcut = if primary && character.as_deref() == Some("l") {
        if is_pressed {
            if web_view_focused.replace(false) {
                controller
                    .borrow_mut()
                    .queue_command(BrowserCommand::FocusChanged { focused: false });
            }
            window.invoke_focus_address_bar();
        }
        true
    } else if primary && character.as_deref() == Some("t") {
        if is_pressed {
            if web_view_focused.replace(false) {
                controller
                    .borrow_mut()
                    .queue_command(BrowserCommand::FocusChanged { focused: false });
            }
            controller.borrow_mut().open_tab();
            sync_ui(window, &controller.borrow());
            window.invoke_focus_address_bar();
        }
        true
    } else if primary && character.as_deref() == Some("w") {
        if is_pressed {
            let active_tab_id = controller.borrow().active_tab_id();
            controller.borrow_mut().close_tab(active_tab_id);
            sync_ui(window, &controller.borrow());
        }
        true
    } else if primary
        && matches!(
            event.logical_key,
            Key::Named(winit::keyboard::NamedKey::Tab)
        )
    {
        if is_pressed {
            controller
                .borrow_mut()
                .select_next_tab(modifiers.shift_key());
            sync_ui(window, &controller.borrow());
        }
        true
    } else if (primary && character.as_deref() == Some("r"))
        || matches!(event.logical_key, Key::Named(winit::keyboard::NamedKey::F5))
    {
        if is_pressed {
            controller
                .borrow_mut()
                .queue_command(BrowserCommand::Reload);
        }
        true
    } else if modifiers.alt_key()
        && matches!(
            event.logical_key,
            Key::Named(winit::keyboard::NamedKey::ArrowLeft)
        )
    {
        if is_pressed {
            controller
                .borrow_mut()
                .queue_command(BrowserCommand::GoBack);
        }
        true
    } else if modifiers.alt_key()
        && matches!(
            event.logical_key,
            Key::Named(winit::keyboard::NamedKey::ArrowRight)
        )
    {
        if is_pressed {
            controller
                .borrow_mut()
                .queue_command(BrowserCommand::GoForward);
        }
        true
    } else {
        false
    };

    if shortcut && is_pressed {
        window.window().request_redraw();
    }
    shortcut
}

fn keyboard_input_from_winit(
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

fn physical_key_string(key: PhysicalKey) -> String {
    match key {
        PhysicalKey::Code(winit::keyboard::KeyCode::SuperLeft) => "MetaLeft".to_owned(),
        PhysicalKey::Code(winit::keyboard::KeyCode::SuperRight) => "MetaRight".to_owned(),
        PhysicalKey::Code(winit::keyboard::KeyCode::Meta) => "Super".to_owned(),
        PhysicalKey::Code(code) => format!("{code:?}"),
        PhysicalKey::Unidentified(_) => "Unidentified".to_owned(),
    }
}

fn logical_key_string(key: &Key) -> String {
    match key {
        Key::Character(character) => character.to_string(),
        Key::Named(winit::keyboard::NamedKey::Space) => " ".to_owned(),
        Key::Named(named) => format!("{named:?}"),
        Key::Dead(_) => "Dead".to_owned(),
        Key::Unidentified(_) => "Unidentified".to_owned(),
    }
}

fn queue_ime_input(
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

fn dismiss_composition(
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
    use std::convert::Infallible;

    use super::*;

    #[derive(Default)]
    struct CountingWebView {
        reload_count: usize,
    }

    impl TalariaWebView for CountingWebView {
        type Error = Infallible;

        fn navigate(&mut self, _url: &str) -> Result<(), Self::Error> {
            Ok(())
        }
        fn go_back(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn go_forward(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn reload(&mut self) -> Result<(), Self::Error> {
            self.reload_count += 1;
            Ok(())
        }
        fn pointer_moved(&mut self, _x: f32, _y: f32) -> Result<(), Self::Error> {
            Ok(())
        }
        fn pointer_button(
            &mut self,
            _action: PointerButtonAction,
            _button: PointerButton,
            _x: f32,
            _y: f32,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        fn scroll(
            &mut self,
            _delta_x: f32,
            _delta_y: f32,
            _x: f32,
            _y: f32,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        fn set_focus(&mut self, _focused: bool) -> Result<(), Self::Error> {
            Ok(())
        }
        fn keyboard_input(&mut self, _event: KeyboardInput) -> Result<(), Self::Error> {
            Ok(())
        }
        fn ime_input(&mut self, _event: ImeInput) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn pending_browser_events_are_routed_to_their_tabs() {
        let controller = RefCell::new(BrowserController::default());
        let first_tab_id = controller.borrow().active_tab_id();
        let second_tab_id = controller.borrow_mut().open_tab();
        let (sender, receiver) = mpsc::channel();

        sender
            .send(TabEvent::new(
                first_tab_id,
                BrowserEvent::PageTitleChanged {
                    title: Some("First".to_owned()),
                },
            ))
            .expect("the event receiver should be connected");
        sender
            .send(TabEvent::new(
                second_tab_id,
                BrowserEvent::PageTitleChanged {
                    title: Some("Second".to_owned()),
                },
            ))
            .expect("the event receiver should be connected");

        assert!(apply_pending_browser_events(&controller, &receiver));
        let controller = controller.borrow();
        let titles = controller
            .tabs()
            .map(|tab| tab.state().page_title())
            .collect::<Vec<_>>();
        assert_eq!(titles, vec!["First", "Second"]);
    }

    #[test]
    fn commands_are_dispatched_to_their_own_web_views() {
        let controller = RefCell::new(BrowserController::default());
        let first_tab_id = controller.borrow().active_tab_id();
        controller
            .borrow_mut()
            .queue_command(BrowserCommand::Reload);
        let second_tab_id = controller.borrow_mut().open_tab();
        controller
            .borrow_mut()
            .queue_command(BrowserCommand::Reload);
        let mut web_views = HashMap::from([
            (first_tab_id, CountingWebView::default()),
            (second_tab_id, CountingWebView::default()),
        ]);

        let pending = dispatch_pending_commands(&controller, &mut web_views)
            .expect("fake WebViews should not fail");

        assert!(!pending);
        assert_eq!(web_views[&first_tab_id].reload_count, 1);
        assert_eq!(web_views[&second_tab_id].reload_count, 1);
    }

    #[test]
    fn command_dispatch_is_limited_per_frame() {
        let controller = RefCell::new(BrowserController::default());
        let tab_id = controller.borrow().active_tab_id();
        let mut web_views = HashMap::from([(tab_id, CountingWebView::default())]);
        for _ in 0..=MAX_COMMANDS_PER_FRAME {
            controller
                .borrow_mut()
                .queue_command(BrowserCommand::Reload);
        }

        assert!(
            dispatch_pending_commands(&controller, &mut web_views)
                .expect("the fake WebView should not fail")
        );
        assert_eq!(web_views[&tab_id].reload_count, MAX_COMMANDS_PER_FRAME);
        assert!(
            !dispatch_pending_commands(&controller, &mut web_views)
                .expect("the fake WebView should not fail")
        );
        assert_eq!(web_views[&tab_id].reload_count, MAX_COMMANDS_PER_FRAME + 1);
    }

    #[test]
    fn page_icon_pixels_are_preserved_when_converted_for_slint() {
        let icon =
            PageIcon::from_rgba(1, 1, vec![10, 20, 30, 40]).expect("the test icon should be valid");

        let pixels = slint_image_from_page_icon(&icon)
            .to_rgba8()
            .expect("an embedded RGBA image should remain CPU-readable");

        assert_eq!(pixels.width(), 1);
        assert_eq!(pixels.height(), 1);
        assert_eq!(pixels.as_bytes(), &[10, 20, 30, 40]);
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
