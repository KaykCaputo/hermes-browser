use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use hermes_ui::MainWindow;
use slint::{ComponentHandle, GraphicsAPI, RenderingState};
use talaria_core::{BrowserCommand, BrowserController, BrowserState, LoadState};
use talaria_servo::{
    EventLoopWaker, ServoRuntime, ServoWebView, SharedOpenGlContext, ViewportSize,
};

const INITIAL_URL: &str = "https://servo.org";

fn main() -> Result<(), Box<dyn Error>> {
    let window = MainWindow::new()?;
    let controller = Rc::new(RefCell::new(BrowserController::default()));

    connect_servo_runtime(&window)?;
    {
        let controller = controller.borrow();
        sync_ui(&window, controller.state());
    }

    connect_ui_callbacks(&window, Rc::clone(&controller));
    window.run()?;
    Ok(())
}

fn connect_servo_runtime(window: &MainWindow) -> Result<(), slint::SetRenderingNotifierError> {
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
                        INITIAL_URL,
                        scale_factor,
                    ) {
                        Ok(created_web_view) => {
                            web_view = Some(created_web_view);
                            window.set_web_view_visible(true);
                        }
                        Err(error) => {
                            window.set_content_message(
                                format!("Failed to create Servo WebView: {error}").into(),
                            );
                            return;
                        }
                    }
                }

                if let Some(web_view) = web_view.as_ref() {
                    web_view.paint();
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

fn sync_ui(window: &MainWindow, state: &BrowserState) {
    window.set_page_title(state.page_title().into());
    window.set_address(state.url().into());
    window.set_can_go_back(state.can_go_back());
    window.set_can_go_forward(state.can_go_forward());
    window.set_is_loading(matches!(state.load_state(), LoadState::Loading));

    let content_message = match state.load_state() {
        LoadState::Idle => "Servo runtime initialized. Waiting for WebView.",
        LoadState::Loading => "The page is loading.",
        LoadState::Loaded => "",
        LoadState::Failed { message } => message.as_str(),
    };

    window.set_content_message(content_message.into());
}

fn connect_ui_callbacks(window: &MainWindow, controller: Rc<RefCell<BrowserController>>) {
    let navigate_controller = Rc::clone(&controller);

    window.on_navigate(move |address| {
        navigate_controller
            .borrow_mut()
            .queue_command(BrowserCommand::Navigate {
                url: address.to_string(),
            });
    });

    let back_controller = Rc::clone(&controller);

    window.on_go_back(move || {
        back_controller
            .borrow_mut()
            .queue_command(BrowserCommand::GoBack);
    });

    let forward_controller = Rc::clone(&controller);

    window.on_go_forward(move || {
        forward_controller
            .borrow_mut()
            .queue_command(BrowserCommand::GoForward);
    });

    let reload_controller = Rc::clone(&controller);

    window.on_reload(move || {
        reload_controller
            .borrow_mut()
            .queue_command(BrowserCommand::Reload);
    });
}
