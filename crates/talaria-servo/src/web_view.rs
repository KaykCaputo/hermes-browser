use talaria_core::WebView as TalariaWebView;
use url::Url;

use crate::ServoWebViewError;

pub struct ServoWebView {
    inner: servo::WebView,
}

impl ServoWebView {
    pub fn new(inner: servo::WebView) -> Self {
        Self { inner }
    }
}

impl TalariaWebView for ServoWebView {
    type Error = ServoWebViewError;

    fn navigate(&mut self, url: &str) -> Result<(), Self::Error> {
        let url = Url::parse(url)?;
        self.inner.load(url);
        Ok(())
    }

    fn go_back(&mut self) -> Result<(), Self::Error> {
        let _traversal_id = self.inner.go_back(1);
        Ok(())
    }

    fn go_forward(&mut self) -> Result<(), Self::Error> {
        let _traversal_id = self.inner.go_forward(1);
        Ok(())
    }

    fn reload(&mut self) -> Result<(), Self::Error> {
        self.inner.reload();
        Ok(())
    }
}
