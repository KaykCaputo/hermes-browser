use talaria_core::{BrowserCommand, WebView, dispatch_command};

#[derive(Debug, PartialEq, Eq)]
enum FakeCall {
    Navigate(String),
    GoBack,
    GoForward,
    Reload,
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
fn web_view_errors_are_returned_without_conversion() {
    let mut web_view = FakeWebView::default();
    web_view.reject_next_command();

    let result = dispatch_command(&mut web_view, BrowserCommand::Reload);

    assert_eq!(result, Err(FakeError::CommandRejected));
    assert_eq!(web_view.calls, vec![FakeCall::Reload]);
}
