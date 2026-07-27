use std::cell::RefCell;
use std::rc::Rc;

use hermes_ui::MainWindow;
use slint::ComponentHandle;
use talaria_core::{BrowserCommand, BrowserController, BrowserState, LoadState};

fn main() -> Result<(), slint::PlatformError> {
    let window = MainWindow::new()?;
    let controller = Rc::new(RefCell::new(BrowserController::default()));

    {
        let controller = controller.borrow();
        sync_ui(&window, controller.state());
    }

    connect_ui_callbacks(&window, Rc::clone(&controller));

    window.run()
}

fn sync_ui(window: &MainWindow, state: &BrowserState) {
    window.set_page_title(state.page_title().into());
    window.set_address(state.url().into());
    window.set_can_go_back(state.can_go_back());
    window.set_can_go_forward(state.can_go_forward());
    window.set_is_loading(matches!(state.load_state(), LoadState::Loading));

    let content_message = match state.load_state() {
        LoadState::Idle => "Waiting for Servo integration.",
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
