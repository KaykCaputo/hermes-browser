use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use hermes_ui::MainWindow;
use slint::{ComponentHandle, GraphicsAPI, RenderingState};
use talaria_core::{BrowserCommand, BrowserController, BrowserState, LoadState};
use talaria_servo::{EventLoopWaker, ServoRuntime, SharedOpenGlContext, ViewportSize};

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

    let runtime = Rc::new(ServoRuntime::new(event_loop_waker));
    let weak_window_for_rendering = window.as_weak();
    let mut rendering_context: Option<Rc<SharedOpenGlContext>> = None;

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
                let context = unsafe { SharedOpenGlContext::new(viewport_size, get_proc_address) };

                rendering_context = Some(Rc::new(context));

                window.set_content_message(
                    "OpenGL context initialized. Waiting for Servo WebView.".into(),
                );
            }
            RenderingState::BeforeRendering => {
                runtime.spin_event_loop();
            }
            RenderingState::RenderingTeardown => {
                drop(rendering_context.take());
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
