use std::rc::Rc;
use std::sync::Arc;

use euclid::Scale;
use rustls::crypto::CryptoProvider;
use servo::{RenderingContext, ServoBuilder, WebViewBuilder};
use url::Url;

use crate::{ServoWebView, ServoWebViewError, SharedOpenGlContext};

type WakeCallback = dyn Fn() + Send + Sync + 'static;

#[derive(Clone)]
pub struct EventLoopWaker {
    callback: Arc<WakeCallback>,
}

impl EventLoopWaker {
    pub fn new(callback: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }
}

impl servo::EventLoopWaker for EventLoopWaker {
    fn clone_box(&self) -> Box<dyn servo::EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        (self.callback)();
    }
}

pub struct ServoRuntime {
    inner: servo::Servo,
    redraw_waker: EventLoopWaker,
}

impl ServoRuntime {
    pub fn new(event_loop_waker: EventLoopWaker) -> Self {
        ensure_crypto_provider();

        let redraw_waker = event_loop_waker.clone();
        let inner = ServoBuilder::default()
            .event_loop_waker(Box::new(event_loop_waker))
            .build();

        Self {
            inner,
            redraw_waker,
        }
    }

    pub fn create_web_view(
        &self,
        rendering_context: Rc<SharedOpenGlContext>,
        initial_url: &str,
        scale_factor: f32,
    ) -> Result<ServoWebView, ServoWebViewError> {
        let initial_url = Url::parse(initial_url)?;
        let rendering_context: Rc<dyn RenderingContext> = rendering_context;
        let delegate = Rc::new(RedrawDelegate {
            event_loop_waker: self.redraw_waker.clone(),
        });

        let inner = WebViewBuilder::new(&self.inner, rendering_context)
            .url(initial_url)
            .hidpi_scale_factor(Scale::new(scale_factor))
            .delegate(delegate)
            .build();

        Ok(ServoWebView::new(inner))
    }

    pub fn spin_event_loop(&self) {
        self.inner.spin_event_loop();
    }
}

struct RedrawDelegate {
    event_loop_waker: EventLoopWaker,
}

impl servo::WebViewDelegate for RedrawDelegate {
    fn notify_new_frame_ready(&self, _web_view: servo::WebView) {
        servo::EventLoopWaker::wake(&self.event_loop_waker);
    }
}

fn ensure_crypto_provider() {
    if CryptoProvider::get_default().is_none() {
        let _installation_result = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}
