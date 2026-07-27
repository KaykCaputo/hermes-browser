use hermes_ui::MainWindow;
use slint::ComponentHandle;
use talaria_core::{BrowserState, LoadState};

fn main() -> Result<(), slint::PlatformError> {
    let window = MainWindow::new()?;
    let state = BrowserState::default();

    sync_ui(&window, &state);
    connect_ui_callbacks(&window);

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

fn connect_ui_callbacks(window: &MainWindow) {
    let weak_window = window.as_weak();

    window.on_navigate(move |address| {
        let Some(window) = weak_window.upgrade() else {
            return;
        };

        window.set_page_title("Navigation pending".into());
        window.set_content_message(
            format!(
                "Talaria Core will receive the command to open
              {address}"
            )
            .into(),
        );
    });

    let weak_window = window.as_weak();

    window.on_reload(move || {
        let Some(window) = weak_window.upgrade() else {
            return;
        };

        window.set_content_message(
            "The reload command will be connected to Servo
              soon."
                .into(),
        );
    });
}
