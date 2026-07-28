use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use euclid::Scale;
use rustls::crypto::CryptoProvider;
use servo::{LoadStatus, RenderingContext, ServoBuilder, WebViewBuilder};
use talaria_core::{BrowserEvent, LoadState};
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
        browser_event_sender: Sender<BrowserEvent>,
    ) -> Result<ServoWebView, ServoWebViewError> {
        let initial_url = Url::parse(initial_url)?;
        let rendering_context: Rc<dyn RenderingContext> = rendering_context;
        let delegate = Rc::new(BrowserDelegate {
            event_loop_waker: self.redraw_waker.clone(),
            browser_event_sender,
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

struct BrowserDelegate {
    event_loop_waker: EventLoopWaker,
    browser_event_sender: Sender<BrowserEvent>,
}

impl BrowserDelegate {
    fn send_browser_event(&self, event: BrowserEvent) {
        let _send_result = self.browser_event_sender.send(event);
    }
}

impl servo::WebViewDelegate for BrowserDelegate {
    fn notify_url_changed(&self, _web_view: servo::WebView, url: Url) {
        self.send_browser_event(BrowserEvent::AddressChanged {
            url: url.to_string(),
        });
    }

    fn notify_page_title_changed(&self, _web_view: servo::WebView, title: Option<String>) {
        self.send_browser_event(BrowserEvent::PageTitleChanged { title });
    }

    fn notify_load_status_changed(&self, _web_view: servo::WebView, status: LoadStatus) {
        self.send_browser_event(BrowserEvent::LoadStateChanged {
            state: load_state_from_servo(status),
        });
    }

    fn notify_history_changed(&self, _web_view: servo::WebView, entries: Vec<Url>, current: usize) {
        self.send_browser_event(history_changed_event(entries.len(), current));
    }

    fn notify_new_frame_ready(&self, _web_view: servo::WebView) {
        servo::EventLoopWaker::wake(&self.event_loop_waker);
    }

    fn notify_crashed(
        &self,
        _web_view: servo::WebView,
        reason: String,
        _backtrace: Option<String>,
    ) {
        self.send_browser_event(BrowserEvent::LoadStateChanged {
            state: LoadState::Failed { message: reason },
        });
    }
}

fn load_state_from_servo(status: LoadStatus) -> LoadState {
    match status {
        LoadStatus::Started | LoadStatus::HeadParsed => LoadState::Loading,
        LoadStatus::Complete => LoadState::Loaded,
    }
}

fn history_changed_event(entry_count: usize, current: usize) -> BrowserEvent {
    let can_go_back = current > 0;
    let can_go_forward = current
        .checked_add(1)
        .is_some_and(|next| next < entry_count);

    BrowserEvent::HistoryChanged {
        can_go_back,
        can_go_forward,
    }
}

fn ensure_crypto_provider() {
    if CryptoProvider::get_default().is_none() {
        let _installation_result = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn started_and_head_parsed_are_loading_states() {
        assert_eq!(
            load_state_from_servo(LoadStatus::Started),
            LoadState::Loading
        );
        assert_eq!(
            load_state_from_servo(LoadStatus::HeadParsed),
            LoadState::Loading
        );
    }

    #[test]
    fn complete_is_a_loaded_state() {
        assert_eq!(
            load_state_from_servo(LoadStatus::Complete),
            LoadState::Loaded
        );
    }

    #[test]
    fn history_event_disables_both_directions_for_the_only_entry() {
        assert_eq!(
            history_changed_event(1, 0),
            BrowserEvent::HistoryChanged {
                can_go_back: false,
                can_go_forward: false,
            }
        );
    }

    #[test]
    fn history_event_enables_both_directions_for_a_middle_entry() {
        assert_eq!(
            history_changed_event(3, 1),
            BrowserEvent::HistoryChanged {
                can_go_back: true,
                can_go_forward: true,
            }
        );
    }

    #[test]
    fn history_event_disables_forward_at_the_last_entry() {
        assert_eq!(
            history_changed_event(3, 2),
            BrowserEvent::HistoryChanged {
                can_go_back: true,
                can_go_forward: false,
            }
        );
    }

    #[test]
    fn history_event_handles_an_overflowing_current_index() {
        assert_eq!(
            history_changed_event(3, usize::MAX),
            BrowserEvent::HistoryChanged {
                can_go_back: true,
                can_go_forward: false,
            }
        );
    }
}
