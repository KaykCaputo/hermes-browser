use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};

use hermes_ui::{MainWindow, WebViewPointerButton};
use slint::{ComponentHandle, GraphicsAPI, RenderingState};
use talaria_core::{
    BrowserCommand, BrowserController, BrowserEvent, BrowserState, LoadState, PointerButton,
    PointerButtonAction, WebView as TalariaWebView, dispatch_command,
};
use talaria_servo::{
    EventLoopWaker, NEW_TAB_URL, ServoRuntime, ServoWebView, SharedOpenGlContext, ViewportSize,
};

const MAX_COMMANDS_PER_FRAME: usize = 128;

fn main() -> Result<(), Box<dyn Error>> {
    let window = MainWindow::new()?;
    let controller = Rc::new(RefCell::new(BrowserController::default()));

    connect_servo_runtime(&window, Rc::clone(&controller))?;
    {
        let controller = controller.borrow();
        sync_ui(&window, controller.state());
    }

    connect_ui_callbacks(&window, Rc::clone(&controller));
    window.run()?;
    Ok(())
}

fn connect_servo_runtime(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
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
    let mut web_view: Option<ServoWebView> = None;
    let mut current_viewport_size: Option<ViewportSize> = None;
    let mut current_scale_factor: Option<f32> = None;
    let (browser_event_sender, browser_event_receiver) = mpsc::channel::<BrowserEvent>();

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
                // SAFETY:
                // Slint guarantees that its OpenGL context is current while the
                // rendering notifier is running. The wrapper is released during
                // RenderingTeardown, before Slint destroys the native context.
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

                let state_changed =
                    apply_pending_browser_events(&controller, &browser_event_receiver);

                if state_changed {
                    let controller = controller.borrow();
                    sync_ui(&window, controller.state());
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

                if current_viewport_size != Some(viewport_size) {
                    rendering_context.resize(viewport_size);

                    if let Some(web_view) = web_view.as_ref() {
                        web_view.resize(viewport_size);
                    }

                    current_viewport_size = Some(viewport_size);
                }

                if current_scale_factor != Some(scale_factor) {
                    if let Some(web_view) = web_view.as_ref() {
                        web_view.set_scale_factor(scale_factor);
                    }

                    current_scale_factor = Some(scale_factor);
                }

                if web_view.is_none() {
                    match runtime.create_web_view(
                        Rc::clone(rendering_context),
                        NEW_TAB_URL,
                        scale_factor,
                        browser_event_sender.clone(),
                    ) {
                        Ok(created_web_view) => {
                            web_view = Some(created_web_view);
                        }
                        Err(error) => {
                            window.set_content_message(
                                format!("Failed to create Servo WebView: {error}").into(),
                            );
                            return;
                        }
                    }
                }

                if let Some(web_view) = web_view.as_mut() {
                    match dispatch_pending_commands(&controller, web_view) {
                        Ok(has_pending_commands) => {
                            if has_pending_commands {
                                window.window().request_redraw();
                            }
                        }
                        Err(error) => {
                            controller
                                .borrow_mut()
                                .handle_event(BrowserEvent::LoadStateChanged {
                                    state: LoadState::Failed {
                                        message: error.to_string(),
                                    },
                                });

                            {
                                let controller = controller.borrow();
                                sync_ui(&window, controller.state());
                            }

                            return;
                        }
                    }

                    let _painted = web_view.paint_if_available();
                }
            }
            RenderingState::RenderingTeardown => {
                drop(web_view.take());
                drop(rendering_context.take());
                current_viewport_size = None;
                current_scale_factor = None;
            }
            _ => {}
        })
}

fn apply_pending_browser_events(
    controller: &RefCell<BrowserController>,
    receiver: &Receiver<BrowserEvent>,
) -> bool {
    let mut state_changed = false;
    let mut controller = controller.borrow_mut();

    for event in receiver.try_iter() {
        controller.handle_event(event);
        state_changed = true;
    }

    state_changed
}

fn dispatch_pending_commands<W: TalariaWebView>(
    controller: &RefCell<BrowserController>,
    web_view: &mut W,
) -> Result<bool, W::Error> {
    for _ in 0..MAX_COMMANDS_PER_FRAME {
        let command = { controller.borrow_mut().next_command() };

        let Some(command) = command else {
            return Ok(false);
        };

        dispatch_command(web_view, command)?;
    }

    Ok(controller.borrow().has_pending_commands())
}

fn sync_ui(window: &MainWindow, state: &BrowserState) {
    window.set_page_title(state.page_title().into());
    window.set_address(state.url().into());
    window.set_can_go_back(state.can_go_back());
    window.set_can_go_forward(state.can_go_forward());
    window.set_is_loading(matches!(state.load_state(), LoadState::Loading));
    window.set_web_view_visible(
        !state.url().is_empty() && !matches!(state.load_state(), LoadState::Failed { .. }),
    );

    let content_message = if state.url().is_empty() {
        "Type an address or search with DuckDuckGo."
    } else {
        match state.load_state() {
            LoadState::Idle => "Servo runtime initialized. Waiting for WebView.",
            LoadState::Loading => "The page is loading.",
            LoadState::Loaded => "",
            LoadState::Failed { message } => message.as_str(),
        }
    };

    window.set_content_message(content_message.into());
}

fn pointer_button_from_ui(button: WebViewPointerButton) -> PointerButton {
    match button {
        WebViewPointerButton::Left => PointerButton::Left,
        WebViewPointerButton::Middle => PointerButton::Middle,
        WebViewPointerButton::Right => PointerButton::Right,
        WebViewPointerButton::Other => PointerButton::Other,
    }
}

fn connect_ui_callbacks(window: &MainWindow, controller: Rc<RefCell<BrowserController>>) {
    let navigate_controller = Rc::clone(&controller);
    let navigate_window = window.as_weak();

    window.on_navigate(move |address| {
        let address = address.to_string();
        let navigation_result = { navigate_controller.borrow_mut().queue_navigation(&address) };

        let Some(window) = navigate_window.upgrade() else {
            return;
        };

        if let Err(error) = navigation_result {
            navigate_controller
                .borrow_mut()
                .handle_event(BrowserEvent::LoadStateChanged {
                    state: LoadState::Failed {
                        message: error.to_string(),
                    },
                });
        }

        {
            let controller = navigate_controller.borrow();
            sync_ui(&window, controller.state());
        }

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

    let focus_controller = Rc::clone(&controller);
    let focus_window = window.as_weak();

    window.on_web_view_focus_changed(move |focused| {
        focus_controller
            .borrow_mut()
            .queue_command(BrowserCommand::FocusChanged { focused });

        if let Some(window) = focus_window.upgrade() {
            window.window().request_redraw();
        }
    });
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
    }

    #[test]
    fn pending_browser_events_are_applied_in_order() {
        let controller = RefCell::new(BrowserController::default());
        let (sender, receiver) = mpsc::channel();

        sender
            .send(BrowserEvent::AddressChanged {
                url: "https://example.com".to_owned(),
            })
            .expect("the event receiver should be connected");
        sender
            .send(BrowserEvent::PageTitleChanged {
                title: Some("Example Domain".to_owned()),
            })
            .expect("the event receiver should be connected");

        assert!(apply_pending_browser_events(&controller, &receiver));
        assert_eq!(controller.borrow().state().url(), "https://example.com");
        assert_eq!(controller.borrow().state().page_title(), "Example Domain");
    }

    #[test]
    fn empty_browser_event_channel_does_not_report_a_state_change() {
        let controller = RefCell::new(BrowserController::default());
        let (_sender, receiver) = mpsc::channel();

        assert!(!apply_pending_browser_events(&controller, &receiver));
    }

    #[test]
    fn ui_pointer_buttons_map_to_core_pointer_buttons() {
        assert_eq!(
            pointer_button_from_ui(WebViewPointerButton::Left),
            PointerButton::Left
        );
        assert_eq!(
            pointer_button_from_ui(WebViewPointerButton::Middle),
            PointerButton::Middle
        );
        assert_eq!(
            pointer_button_from_ui(WebViewPointerButton::Right),
            PointerButton::Right
        );
        assert_eq!(
            pointer_button_from_ui(WebViewPointerButton::Other),
            PointerButton::Other
        );
    }

    #[test]
    fn command_dispatch_is_limited_per_frame() {
        let controller = RefCell::new(BrowserController::default());
        let mut web_view = CountingWebView::default();

        for _ in 0..=MAX_COMMANDS_PER_FRAME {
            controller
                .borrow_mut()
                .queue_command(BrowserCommand::Reload);
        }

        let has_pending_commands = dispatch_pending_commands(&controller, &mut web_view)
            .expect("the fake WebView should not fail");

        assert!(has_pending_commands);
        assert_eq!(web_view.reload_count, MAX_COMMANDS_PER_FRAME);

        let has_pending_commands = dispatch_pending_commands(&controller, &mut web_view)
            .expect("the fake WebView should not fail");

        assert!(!has_pending_commands);
        assert_eq!(web_view.reload_count, MAX_COMMANDS_PER_FRAME + 1);
    }
}
