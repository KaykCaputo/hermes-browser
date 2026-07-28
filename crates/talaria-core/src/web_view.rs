use crate::browser::{BrowserCommand, PointerButton, PointerButtonAction};

pub trait WebView {
    type Error;

    fn navigate(&mut self, url: &str) -> Result<(), Self::Error>;
    fn go_back(&mut self) -> Result<(), Self::Error>;
    fn go_forward(&mut self) -> Result<(), Self::Error>;
    fn reload(&mut self) -> Result<(), Self::Error>;
    fn pointer_moved(&mut self, x: f32, y: f32) -> Result<(), Self::Error>;
    fn pointer_button(
        &mut self,
        action: PointerButtonAction,
        button: PointerButton,
        x: f32,
        y: f32,
    ) -> Result<(), Self::Error>;
    fn scroll(&mut self, delta_x: f32, delta_y: f32, x: f32, y: f32) -> Result<(), Self::Error>;
    fn set_focus(&mut self, focused: bool) -> Result<(), Self::Error>;
}

pub fn dispatch_command<W>(web_view: &mut W, command: BrowserCommand) -> Result<(), W::Error>
where
    W: WebView,
{
    match command {
        BrowserCommand::Navigate { url } => web_view.navigate(url.as_str()),
        BrowserCommand::GoBack => web_view.go_back(),
        BrowserCommand::GoForward => web_view.go_forward(),
        BrowserCommand::Reload => web_view.reload(),
        BrowserCommand::PointerMoved { x, y } => web_view.pointer_moved(x, y),
        BrowserCommand::PointerButton {
            action,
            button,
            x,
            y,
        } => web_view.pointer_button(action, button, x, y),
        BrowserCommand::Scrolled {
            delta_x,
            delta_y,
            x,
            y,
        } => web_view.scroll(delta_x, delta_y, x, y),
        BrowserCommand::FocusChanged { focused } => web_view.set_focus(focused),
    }
}
