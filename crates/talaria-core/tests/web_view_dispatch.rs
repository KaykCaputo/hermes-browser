use talaria_core::{BrowserCommand, PointerButton, PointerButtonAction, WebView, dispatch_command};

#[derive(Debug, PartialEq)]
enum FakeCall {
    Navigate(String),
    GoBack,
    GoForward,
    Reload,
    PointerMoved {
        x: f32,
        y: f32,
    },
    PointerButton {
        action: PointerButtonAction,
        button: PointerButton,
        x: f32,
        y: f32,
    },
    Scrolled {
        delta_x: f32,
        delta_y: f32,
        x: f32,
        y: f32,
    },
    FocusChanged(bool),
}

#[derive(Debug, PartialEq, Eq)]
enum FakeError {
    CommandRejected,
}

#[derive(Debug, Default)]
struct FakeWebView {
    calls: Vec<FakeCall>,
    next_error: Option<FakeError>,
}

impl FakeWebView {
    fn reject_next_command(&mut self) {
        self.next_error = Some(FakeError::CommandRejected);
    }

    fn record(&mut self, call: FakeCall) -> Result<(), FakeError> {
        self.calls.push(call);

        match self.next_error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl WebView for FakeWebView {
    type Error = FakeError;

    fn navigate(&mut self, url: &str) -> Result<(), Self::Error> {
        self.record(FakeCall::Navigate(url.to_owned()))
    }

    fn go_back(&mut self) -> Result<(), Self::Error> {
        self.record(FakeCall::GoBack)
    }

    fn go_forward(&mut self) -> Result<(), Self::Error> {
        self.record(FakeCall::GoForward)
    }

    fn reload(&mut self) -> Result<(), Self::Error> {
        self.record(FakeCall::Reload)
    }

    fn pointer_moved(&mut self, x: f32, y: f32) -> Result<(), Self::Error> {
        self.record(FakeCall::PointerMoved { x, y })
    }

    fn pointer_button(
        &mut self,
        action: PointerButtonAction,
        button: PointerButton,
        x: f32,
        y: f32,
    ) -> Result<(), Self::Error> {
        self.record(FakeCall::PointerButton {
            action,
            button,
            x,
            y,
        })
    }

    fn scroll(&mut self, delta_x: f32, delta_y: f32, x: f32, y: f32) -> Result<(), Self::Error> {
        self.record(FakeCall::Scrolled {
            delta_x,
            delta_y,
            x,
            y,
        })
    }

    fn set_focus(&mut self, focused: bool) -> Result<(), Self::Error> {
        self.record(FakeCall::FocusChanged(focused))
    }
}

#[test]
fn navigate_command_forwards_the_url() {
    let mut web_view = FakeWebView::default();

    let result = dispatch_command(
        &mut web_view,
        BrowserCommand::Navigate {
            url: "https://example.com".to_owned(),
        },
    );

    assert_eq!(result, Ok(()));
    assert_eq!(
        web_view.calls,
        vec![FakeCall::Navigate("https://example.com".to_owned())]
    );
}

#[test]
fn go_back_command_calls_go_back() {
    let mut web_view = FakeWebView::default();

    let result = dispatch_command(&mut web_view, BrowserCommand::GoBack);

    assert_eq!(result, Ok(()));
    assert_eq!(web_view.calls, vec![FakeCall::GoBack]);
}

#[test]
fn go_forward_command_calls_go_forward() {
    let mut web_view = FakeWebView::default();

    let result = dispatch_command(&mut web_view, BrowserCommand::GoForward);

    assert_eq!(result, Ok(()));
    assert_eq!(web_view.calls, vec![FakeCall::GoForward]);
}

#[test]
fn reload_command_calls_reload() {
    let mut web_view = FakeWebView::default();

    let result = dispatch_command(&mut web_view, BrowserCommand::Reload);

    assert_eq!(result, Ok(()));
    assert_eq!(web_view.calls, vec![FakeCall::Reload]);
}

#[test]
fn pointer_moved_command_forwards_logical_coordinates() {
    let mut web_view = FakeWebView::default();

    let result = dispatch_command(
        &mut web_view,
        BrowserCommand::PointerMoved { x: 12.5, y: 24.0 },
    );

    assert_eq!(result, Ok(()));
    assert_eq!(
        web_view.calls,
        vec![FakeCall::PointerMoved { x: 12.5, y: 24.0 }]
    );
}

#[test]
fn pointer_button_command_forwards_action_button_and_coordinates() {
    let mut web_view = FakeWebView::default();

    let result = dispatch_command(
        &mut web_view,
        BrowserCommand::PointerButton {
            action: PointerButtonAction::Down,
            button: PointerButton::Right,
            x: 30.0,
            y: 45.0,
        },
    );

    assert_eq!(result, Ok(()));
    assert_eq!(
        web_view.calls,
        vec![FakeCall::PointerButton {
            action: PointerButtonAction::Down,
            button: PointerButton::Right,
            x: 30.0,
            y: 45.0,
        }]
    );
}

#[test]
fn scrolled_command_forwards_delta_and_pointer_coordinates() {
    let mut web_view = FakeWebView::default();

    let result = dispatch_command(
        &mut web_view,
        BrowserCommand::Scrolled {
            delta_x: -2.0,
            delta_y: 18.0,
            x: 100.0,
            y: 200.0,
        },
    );

    assert_eq!(result, Ok(()));
    assert_eq!(
        web_view.calls,
        vec![FakeCall::Scrolled {
            delta_x: -2.0,
            delta_y: 18.0,
            x: 100.0,
            y: 200.0,
        }]
    );
}

#[test]
fn focus_changed_command_forwards_focus_state() {
    let mut web_view = FakeWebView::default();

    let result = dispatch_command(
        &mut web_view,
        BrowserCommand::FocusChanged { focused: true },
    );

    assert_eq!(result, Ok(()));
    assert_eq!(web_view.calls, vec![FakeCall::FocusChanged(true)]);
}

#[test]
fn web_view_errors_are_returned_without_conversion() {
    let mut web_view = FakeWebView::default();
    web_view.reject_next_command();

    let result = dispatch_command(&mut web_view, BrowserCommand::Reload);

    assert_eq!(result, Err(FakeError::CommandRejected));
    assert_eq!(web_view.calls, vec![FakeCall::Reload]);
}
