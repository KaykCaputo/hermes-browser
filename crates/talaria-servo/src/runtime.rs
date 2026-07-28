use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Instant;

use euclid::Scale;
use log::info;
use rustls::crypto::CryptoProvider;
use servo::{LoadStatus, PixelFormat, RenderingContext, ServoBuilder, WebViewBuilder};
use talaria_core::{BrowserEvent, LoadState, PageIcon};
use url::Url;

use crate::frame_ready::FrameReadySignal;
use crate::load_metrics::{LoadMilestone, PageLoadMetrics};
use crate::{ServoWebView, ServoWebViewError, SharedOpenGlContext};

type WakeCallback = dyn Fn() + Send + Sync + 'static;

pub const NEW_TAB_URL: &str = "about:blank";

#[derive(Clone)]
pub struct EventLoopWaker {
    callback: Arc<WakeCallback>,
    wake_pending: Arc<AtomicBool>,
}

impl EventLoopWaker {
    pub fn new(callback: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
            wake_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn acknowledge(&self) {
        self.wake_pending.store(false, Ordering::Release);
    }
}

impl servo::EventLoopWaker for EventLoopWaker {
    fn clone_box(&self) -> Box<dyn servo::EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        if self
            .wake_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            (self.callback)();
        }
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
        inner.setup_logging();

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
        let frame_ready = FrameReadySignal::default();
        let delegate = Rc::new(BrowserDelegate {
            event_loop_waker: self.redraw_waker.clone(),
            browser_event_sender,
            frame_ready: frame_ready.clone(),
            load_metrics: RefCell::new(PageLoadMetrics::default()),
        });

        let inner = WebViewBuilder::new(&self.inner, rendering_context)
            .url(initial_url)
            .hidpi_scale_factor(Scale::new(scale_factor))
            .delegate(delegate)
            .build();

        Ok(ServoWebView::new(inner, frame_ready))
    }

    pub fn spin_event_loop(&self) {
        self.redraw_waker.acknowledge();
        self.inner.spin_event_loop();
    }
}

struct BrowserDelegate {
    event_loop_waker: EventLoopWaker,
    browser_event_sender: Sender<BrowserEvent>,
    frame_ready: FrameReadySignal,
    load_metrics: RefCell<PageLoadMetrics>,
}

impl BrowserDelegate {
    fn send_browser_event(&self, event: BrowserEvent) {
        let _send_result = self.browser_event_sender.send(event);
    }

    fn record_load_milestone(&self, milestone: LoadMilestone) {
        let elapsed = self
            .load_metrics
            .borrow_mut()
            .record(milestone, Instant::now());

        if let Some(elapsed) = elapsed {
            info!(
                "page load milestone: {} at {:.1} ms",
                milestone.label(),
                elapsed.as_secs_f64() * 1_000.0
            );
        }
    }
}

impl servo::WebViewDelegate for BrowserDelegate {
    fn notify_url_changed(&self, _web_view: servo::WebView, url: Url) {
        self.send_browser_event(BrowserEvent::AddressChanged {
            url: display_url(&url),
        });
    }

    fn notify_page_title_changed(&self, _web_view: servo::WebView, title: Option<String>) {
        self.send_browser_event(BrowserEvent::PageTitleChanged { title });
    }

    fn notify_favicon_changed(&self, web_view: servo::WebView) {
        let icon = web_view.favicon().and_then(|favicon| {
            page_icon_from_pixels(
                favicon.width,
                favicon.height,
                favicon.format,
                favicon.data(),
            )
        });

        self.send_browser_event(BrowserEvent::PageIconChanged { icon });
    }

    fn notify_load_status_changed(&self, _web_view: servo::WebView, status: LoadStatus) {
        match status {
            LoadStatus::Started => {
                self.load_metrics.borrow_mut().start(Instant::now());
                info!("page load started");
            }
            LoadStatus::HeadParsed => self.record_load_milestone(LoadMilestone::HeadParsed),
            LoadStatus::Complete => self.record_load_milestone(LoadMilestone::Complete),
        }

        self.send_browser_event(BrowserEvent::LoadStateChanged {
            state: load_state_from_servo(status),
        });
    }

    fn notify_history_changed(&self, _web_view: servo::WebView, entries: Vec<Url>, current: usize) {
        self.send_browser_event(history_changed_event(entries.len(), current));
    }

    fn notify_new_frame_ready(&self, _web_view: servo::WebView) {
        self.record_load_milestone(LoadMilestone::FirstFrame);
        self.frame_ready.mark_ready();
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

fn display_url(url: &Url) -> String {
    if url.as_str() == NEW_TAB_URL {
        String::new()
    } else {
        url.to_string()
    }
}

fn page_icon_from_pixels(
    width: u32,
    height: u32,
    format: PixelFormat,
    pixels: &[u8],
) -> Option<PageIcon> {
    let pixel_count = width.checked_mul(height)? as usize;
    let channel_count = match format {
        PixelFormat::K8 => 1,
        PixelFormat::KA8 => 2,
        PixelFormat::RGB8 => 3,
        PixelFormat::RGBA8 | PixelFormat::BGRA8 => 4,
    };

    if width == 0 || height == 0 || pixels.len() != pixel_count.checked_mul(channel_count)? {
        return None;
    }

    let mut rgba = Vec::with_capacity(pixel_count.checked_mul(4)?);

    match format {
        PixelFormat::K8 => {
            for &luminance in pixels {
                rgba.extend_from_slice(&[luminance, luminance, luminance, 255]);
            }
        }
        PixelFormat::KA8 => {
            for pixel in pixels.chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        PixelFormat::RGB8 => {
            for pixel in pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        PixelFormat::RGBA8 => rgba.extend_from_slice(pixels),
        PixelFormat::BGRA8 => {
            for pixel in pixels.chunks_exact(4) {
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
        }
    }

    PageIcon::from_rgba(width, height, rgba)
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

    #[test]
    fn new_tab_url_is_hidden_from_the_address_bar() {
        let url = Url::parse(NEW_TAB_URL).expect("the new tab URL should be valid");

        assert_eq!(display_url(&url), "");
    }

    #[test]
    fn regular_url_is_preserved_for_the_address_bar() {
        let url = Url::parse("https://example.com/path").expect("the test URL should be valid");

        assert_eq!(display_url(&url), "https://example.com/path");
    }

    #[test]
    fn rgba_favicon_pixels_are_preserved() {
        let icon = page_icon_from_pixels(1, 1, PixelFormat::RGBA8, &[10, 20, 30, 40])
            .expect("valid RGBA pixels should create an icon");

        assert_eq!(icon.rgba(), &[10, 20, 30, 40]);
    }

    #[test]
    fn bgra_favicon_pixels_are_converted_to_rgba() {
        let icon = page_icon_from_pixels(1, 1, PixelFormat::BGRA8, &[30, 20, 10, 40])
            .expect("valid BGRA pixels should create an icon");

        assert_eq!(icon.rgba(), &[10, 20, 30, 40]);
    }

    #[test]
    fn rgb_favicon_pixels_receive_an_opaque_alpha_channel() {
        let icon = page_icon_from_pixels(1, 1, PixelFormat::RGB8, &[10, 20, 30])
            .expect("valid RGB pixels should create an icon");

        assert_eq!(icon.rgba(), &[10, 20, 30, 255]);
    }

    #[test]
    fn luminance_favicon_pixels_expand_to_rgb() {
        let icon = page_icon_from_pixels(1, 1, PixelFormat::K8, &[42])
            .expect("valid luminance pixels should create an icon");

        assert_eq!(icon.rgba(), &[42, 42, 42, 255]);
    }

    #[test]
    fn luminance_alpha_favicon_pixels_expand_to_rgba() {
        let icon = page_icon_from_pixels(1, 1, PixelFormat::KA8, &[42, 128])
            .expect("valid luminance-alpha pixels should create an icon");

        assert_eq!(icon.rgba(), &[42, 42, 42, 128]);
    }

    #[test]
    fn favicon_pixels_with_an_invalid_length_are_rejected() {
        assert_eq!(
            page_icon_from_pixels(1, 1, PixelFormat::RGBA8, &[10, 20, 30]),
            None
        );
    }
}
