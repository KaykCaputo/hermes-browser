use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use hermes_ui::MainWindow;
use slint::{ComponentHandle, RenderingState};
use talaria_core::{BrowserCommand, BrowserController, BrowserState, LoadState};
use talaria_servo::{EventLoopWaker, ServoRuntime};

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
    let weak_window = window.as_weak();

    let event_loop_waker = EventLoopWaker::new(move || {
        let _wake_result = weak_window.upgrade_in_event_loop(|window| {
            window.window().request_redraw();
        });
    });

    let runtime = Rc::new(ServoRuntime::new(event_loop_waker));

    window
        .window()
        .set_rendering_notifier(move |state, _graphics_api| {
            if matches!(state, RenderingState::BeforeRendering) {
                runtime.spin_event_loop();
            }
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
