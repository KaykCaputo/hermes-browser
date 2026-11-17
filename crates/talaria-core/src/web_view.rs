use crate::browser::{BrowserCommand, PointerButton, PointerButtonAction};
use crate::{ClipboardCommand, ColorScheme, ControlAnswer, ControlId, ImeInput, KeyboardInput};
use crate::{PageRequestAnswer, PageRequestId};

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
    fn keyboard_input(&mut self, event: KeyboardInput) -> Result<(), Self::Error>;
    fn ime_input(&mut self, event: ImeInput) -> Result<(), Self::Error>;
    fn set_page_zoom(&mut self, percent: u16) -> Result<(), Self::Error>;
    fn pinch_zoom(&mut self, delta: f32, x: f32, y: f32) -> Result<(), Self::Error>;
    fn set_color_scheme(&mut self, scheme: ColorScheme) -> Result<(), Self::Error>;
    fn exit_fullscreen(&mut self) -> Result<(), Self::Error>;
    fn pointer_left(&mut self) -> Result<(), Self::Error>;
    // controls
    fn answer_control(&mut self, id: ControlId, answer: ControlAnswer) -> Result<(), Self::Error>;
    // dialogs
    fn answer_page_request(
        &mut self,
        id: PageRequestId,
        answer: PageRequestAnswer,
    ) -> Result<(), Self::Error>;
    // menus
    fn clipboard(&mut self, command: ClipboardCommand) -> Result<(), Self::Error>;
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
        BrowserCommand::KeyboardInput(event) => web_view.keyboard_input(event),
        BrowserCommand::ImeInput(event) => web_view.ime_input(event),
        BrowserCommand::SetPageZoom { percent } => web_view.set_page_zoom(percent),
        BrowserCommand::PinchZoom { delta, x, y } => web_view.pinch_zoom(delta, x, y),
        BrowserCommand::SetColorScheme(scheme) => web_view.set_color_scheme(scheme),
        BrowserCommand::ExitFullscreen => web_view.exit_fullscreen(),
        BrowserCommand::PointerLeft => web_view.pointer_left(),
        BrowserCommand::AnswerControl { id, answer } => web_view.answer_control(id, answer),
        BrowserCommand::AnswerPageRequest { id, answer } => {
            web_view.answer_page_request(id, answer)
        }
        BrowserCommand::Clipboard(command) => web_view.clipboard(command),
    }
}
