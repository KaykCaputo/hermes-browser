use dpi::PhysicalSize;
use euclid::Scale;
use talaria_core::WebView as TalariaWebView;
use url::Url;

use crate::{ServoWebViewError, ViewportSize};

pub struct ServoWebView {
    inner: servo::WebView,
}

impl ServoWebView {
    pub fn new(inner: servo::WebView) -> Self {
        Self { inner }
    }

    pub fn resize(&self, size: ViewportSize) {
        self.inner
            .resize(PhysicalSize::new(size.width(), size.height()));
    }

    pub fn set_scale_factor(&self, scale_factor: f32) {
        self.inner.set_hidpi_scale_factor(Scale::new(scale_factor));
    }

    pub fn paint(&self) {
        self.inner.paint();
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
