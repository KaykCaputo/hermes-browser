use crate::browser::BrowserCommand;

pub trait WebView {
    type Error;

    fn navigate(&mut self, url: &str) -> Result<(), Self::Error>;
    fn go_back(&mut self) -> Result<(), Self::Error>;
    fn go_forward(&mut self) -> Result<(), Self::Error>;
    fn reload(&mut self) -> Result<(), Self::Error>;
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
    }
}
