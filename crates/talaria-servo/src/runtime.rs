use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Instant;

use euclid::Scale;
use log::info;
use rustls::crypto::CryptoProvider;
use servo::{
    Cursor, LoadStatus, Opts, PixelFormat, PrefValue, Preferences, RenderingContext, ServoBuilder,
    WebViewBuilder,
};
use talaria_core::{BrowserEvent, LoadState, PageIcon, PointerCursor, TabEvent, TabId};
use talaria_core::{PageRequest, PageRequestKind};
use url::Url;

use crate::adblocker::ContentBlocker;
use crate::embedder_controls::{ControlIds, EmbedderControls};
use crate::exit_clearing::{EngineSwitches, ExitClearing, clear_at_exit};
use crate::frame_ready::FrameReadySignal;
use crate::load_metrics::{LoadMilestone, PageLoadMetrics};
use crate::page_defaults::page_defaults_user_content;
use crate::page_requests::{
    ClosesOnDrop, DocumentStartTracker, PendingEngineRequests, ServoPageRequest, page_permission,
    simple_dialog_kind,
};
use crate::screen::ScreenMetrics;
use crate::session_data::{
    AfterEngineCleanup, HandleThenCleanup, clean_up_before_engine_start, end_session_cookies,
};
use crate::{ServoWebView, ServoWebViewError};

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

/// Engine settings chosen at launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeOptions {
    /// Draw 2D canvases on the GPU (Servo's vello backend) instead of the CPU.
    pub gpu_canvas: bool,
    /// Where Servo keeps cookies, HSTS, HTTP authentication and site storage. `None` keeps
    /// nothing across runs.
    pub config_dir: Option<PathBuf>,
    /// Turn on servoshell's experimental web-platform features, except the async clipboard and
    /// WebGPU (see `EXCLUDED_EXPERIMENTAL_PREFS`).
    pub experimental_web_features: bool,
    // settings
    /// Block ads and trackers from the start; changed later through `ServoRuntime::switches`.
    pub block_ads_and_trackers: bool,
    /// What to clear at quit; also cleared before the engine starts, in case the last quit
    /// never got to it. Changed later through `ServoRuntime::switches`.
    pub clear_on_exit: ExitClearing,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            gpu_canvas: true,
            config_dir: None,
            experimental_web_features: true,
            block_ads_and_trackers: true,
            clear_on_exit: ExitClearing::default(),
        }
    }
}

pub struct ServoRuntime {
    inner: servo::Servo,
    redraw_waker: EventLoopWaker,
    content_blocker: Rc<ContentBlocker>,
    screen_metrics: Rc<Cell<Option<ScreenMetrics>>>,
    // controls
    control_ids: ControlIds,
    /// The user-origin style sheet every web view shares. Servo's `UserContentManager` holds a
    /// handle to Servo of its own, so this field must stay before `after_engine_cleanup`.
    user_content: Rc<servo::UserContentManager>,
    /// Declared after `inner` and `user_content`, the two fields that hold a handle to Servo,
    /// so it is dropped after the last of them. Every web view holds a clone too, released
    /// after its own handle; see `AfterEngineCleanup`.
    after_engine_cleanup: Option<Rc<AfterEngineCleanup>>,
    /// Content blocking and what to clear at quit, shared with the browser.
    switches: EngineSwitches,
}

impl ServoRuntime {
    pub fn new(event_loop_waker: EventLoopWaker, runtime_options: RuntimeOptions) -> Self {
        ensure_crypto_provider();

        let redraw_waker = event_loop_waker.clone();
        let switches = EngineSwitches::new(
            runtime_options.block_ads_and_trackers,
            runtime_options.clear_on_exit,
        );
        // Servo reads its saved files when it starts. Session cookies and HTTP credentials in
        // them belong to an earlier session that was not ended (killed, or cut short by the
        // shutdown watchdog); remove them from the files before Servo can load them.
        if let Some(config_dir) = &runtime_options.config_dir {
            clean_up_before_engine_start(config_dir, runtime_options.clear_on_exit);
        }
        let options = mercury_options(runtime_options.config_dir.clone());
        let multiprocess_enabled = options.multiprocess;
        let sandbox_enabled = options.sandbox;
        let inner = ServoBuilder::default()
            .opts(options)
            .preferences(mercury_preferences(&runtime_options))
            .event_loop_waker(Box::new(event_loop_waker))
            .build();
        inner.setup_logging();
        let after_engine_cleanup = runtime_options.config_dir.clone().map(|config_dir| {
            // Session cookies saved by a run that did not end them (it was killed) end now.
            end_session_cookies(&inner);
            Rc::new(AfterEngineCleanup::new(config_dir, switches.clone()))
        });
        if multiprocess_enabled {
            info!(
                "Servo content process isolation enabled; operating-system sandbox {}",
                if sandbox_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            );
        } else {
            info!("Servo single-process mode enabled; operating-system sandbox disabled");
        }
        let content_blocker = Rc::new(ContentBlocker::new(switches.clone()));
        inner.set_delegate(Rc::new(BrowserServoDelegate {
            content_blocker: Rc::clone(&content_blocker),
        }));
        let user_content = page_defaults_user_content(&inner);

        Self {
            inner,
            redraw_waker,
            content_blocker,
            screen_metrics: Rc::default(),
            control_ids: ControlIds::default(),
            user_content,
            after_engine_cleanup,
            switches,
        }
    }

    // settings

    /// The switches the browser changes while the engine runs; they share their state with
    /// the engine, so keep the clone and set it directly.
    pub fn switches(&self) -> EngineSwitches {
        self.switches.clone()
    }

    /// What pages see as the screen and window geometry; `None` lets Servo use its defaults.
    pub fn set_screen_metrics(&self, metrics: Option<ScreenMetrics>) {
        self.screen_metrics.set(metrics);
    }

    pub fn create_web_view(
        &self,
        rendering_context: Rc<dyn RenderingContext>,
        initial_url: &str,
        scale_factor: f32,
        tab_id: TabId,
        browser_event_sender: Sender<TabEvent>,
    ) -> Result<ServoWebView, ServoWebViewError> {
        let initial_url = Url::parse(initial_url)?;
        let frame_ready = FrameReadySignal::default();
        let controls = EmbedderControls::new(self.control_ids.clone());
        let page_requests = ClosesOnDrop::new(PendingEngineRequests::default());
        let document_start = Rc::new(RefCell::new(DocumentStartTracker::default()));
        let delegate = Rc::new(BrowserDelegate {
            tab_id,
            browser_event_sender: browser_event_sender.clone(),
            frame_ready: frame_ready.clone(),
            load_metrics: RefCell::new(PageLoadMetrics::default()),
            content_blocker: Rc::clone(&self.content_blocker),
            screen_metrics: Rc::clone(&self.screen_metrics),
            controls: controls.clone(),
            page_requests: page_requests.shared(),
            document_start: Rc::clone(&document_start),
        });

        let inner = WebViewBuilder::new(&self.inner, rendering_context)
            .url(initial_url)
            .hidpi_scale_factor(Scale::new(scale_factor))
            .delegate(delegate)
            .user_content_manager(Rc::clone(&self.user_content))
            .build();

        for event in new_web_view_feedback(inner.cursor(), inner.status_text()) {
            let _send_result = browser_event_sender.send(TabEvent::new(tab_id, event));
        }
        info!("created Servo WebView for tab {}", tab_id.value());
        Ok(ServoWebView::new(
            HandleThenCleanup::new(inner, self.after_engine_cleanup.clone()),
            frame_ready,
            tab_id,
            scale_factor,
            controls,
            page_requests,
            document_start,
        ))
    }

    pub fn spin_event_loop(&self) {
        self.redraw_waker.acknowledge();
        self.inner.spin_event_loop();
    }
}

impl Drop for ServoRuntime {
    /// Clears what the user asked to have cleared at exit, then ends the session cookies,
    /// while Servo still runs, before it saves its files.
    fn drop(&mut self) {
        let clearing = self.switches.clear_on_exit();
        if clearing.clears_anything() {
            clear_at_exit(&self.inner, clearing);
        }
        if self.after_engine_cleanup.is_some() {
            end_session_cookies(&self.inner);
        }
    }
}

fn mercury_options(config_dir: Option<PathBuf>) -> Opts {
    Opts {
        // Servo 0.8.0 content-process isolation renders correctly in Mercury.
        // Keep the operating-system sandbox disabled independently: the
        // current Linux gaol path is not yet reliable enough for a release.
        multiprocess: true,
        sandbox: false,
        // With `hard_fail`, a page that panics would exit the whole browser; without it,
        // Servo replaces the page with its crash page and reports it through `notify_crashed`.
        hard_fail: false,
        config_dir,
        ..Opts::default()
    }
}

struct BrowserServoDelegate {
    content_blocker: Rc<ContentBlocker>,
}

impl servo::ServoDelegate for BrowserServoDelegate {
    fn load_web_resource(&self, load: servo::WebResourceLoad) {
        self.content_blocker.handle_web_resource_load(None, load);
    }
}

fn mercury_preferences(runtime_options: &RuntimeOptions) -> Preferences {
    let mut preferences = Preferences::default();
    if runtime_options.gpu_canvas {
        preferences.dom_canvas_backend = "vello".to_owned();
    }
    if runtime_options.experimental_web_features {
        for name in EXPERIMENTAL_PREFS
            .iter()
            .filter(|name| !EXCLUDED_EXPERIMENTAL_PREFS.contains(name))
        {
            preferences.set_value(name, PrefValue::Bool(true));
        }
    }
    // Always off: in Servo 0.8.0 `navigator.clipboard.readText()`/`writeText()` skip the
    // permission and user-activation checks (`dom/clipboard/clipboard.rs`, the "check clipboard
    // read permission" step is a TODO), and with no `ClipboardDelegate` installed Servo's default
    // one reads and writes the system clipboard. Any secure page or iframe could read copied
    // passwords on load. Re-enable only with a delegate that answers after a user gesture.
    preferences.dom_async_clipboard_enabled = false;
    // Always off while Mercury does not build Servo's `webgpu` cargo feature: the preference alone
    // exposes `GPUCanvasContext` and other WebGPU names over unimplemented stubs, which makes
    // feature detection report WebGPU that is not there.
    preferences.dom_webgpu_enabled = false;
    // Always off: in Servo 0.8.0 `getUserMedia` opens the camera and microphone without asking
    // the embedder for permission (`mediadevices.rs`), so WebRTC must stay disabled until
    // Mercury can gate device access itself.
    preferences.dom_webrtc_enabled = false;
    // controls: always off. With it on, a page can call `selectFiles()` on a file input and have
    // Servo accept the paths it names without any file panel (`htmlinputelement.rs`); it exists
    // for Servo's own tests.
    preferences.dom_testing_html_input_element_select_files_enabled = false;
    preferences.user_agent = firefox_compatible_user_agent(&preferences.user_agent);
    preferences
}

// From Servo's servoshell, https://github.com/servo/servo, tag `v0.8.0` (commit
// `4e136d8ef31b7c4748fbfac606ca463f3efd7537`), `ports/servoshell/prefs.rs` lines 29-53
// (`EXPERIMENTAL_PREFS`). Licensed under the Mozilla Public License 2.0.
/// Preferences that servoshell turns on with `--enable-experimental-web-platform-features`:
/// implemented in Servo but off by default. Mercury enables this list minus
/// `EXCLUDED_EXPERIMENTAL_PREFS`.
const EXPERIMENTAL_PREFS: &[&str] = &[
    "dom_async_clipboard_enabled",
    "dom_exec_command_enabled",
    "dom_fontface_enabled",
    "dom_indexeddb_enabled",
    "dom_intersection_observer_enabled",
    "dom_navigator_protocol_handlers_enabled",
    "dom_notification_enabled",
    "dom_offscreen_canvas_enabled",
    "dom_permissions_enabled",
    "dom_sanitizer_enabled",
    "dom_storage_manager_api_enabled",
    "dom_webgl2_enabled",
    "dom_webgpu_enabled",
    "layout_css_backdrop_filter_enabled",
    "layout_css_columns_enabled",
    "layout_css_container_queries_enabled",
    "layout_css_ellipse_corners_enabled",
    "layout_css_font_variations_enabled",
];

/// Entries of `EXPERIMENTAL_PREFS` that Mercury leaves off; the reasons are given where they
/// are pinned in `mercury_preferences`.
const EXCLUDED_EXPERIMENTAL_PREFS: &[&str] = &["dom_async_clipboard_enabled", "dom_webgpu_enabled"];

/// Servo's user agent already names the Firefox release it matches, but carries a `Servo/x.y`
/// token where Firefox has `Gecko/20100101`; some sites refuse to serve unrecognised browsers.
/// Replaces that token, keeping the rest of Servo's string (platform and Firefox version).
fn firefox_compatible_user_agent(servo_user_agent: &str) -> String {
    servo_user_agent
        .split(' ')
        .map(|token| {
            if token.starts_with("Servo/") {
                "Gecko/20100101"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

struct BrowserDelegate {
    tab_id: TabId,
    browser_event_sender: Sender<TabEvent>,
    frame_ready: FrameReadySignal,
    load_metrics: RefCell<PageLoadMetrics>,
    content_blocker: Rc<ContentBlocker>,
    screen_metrics: Rc<Cell<Option<ScreenMetrics>>>,
    // controls
    controls: EmbedderControls,
    /// Dialogs, leave prompts, sign-ins and permission prompts waiting for the user; shared with
    /// the tab's `ServoWebView`, which answers them.
    page_requests: Rc<RefCell<PendingEngineRequests<ServoPageRequest>>>,
    /// Shared with the tab's `ServoWebView`, which arms it when it navigates.
    document_start: Rc<RefCell<DocumentStartTracker>>,
}

impl BrowserDelegate {
    fn send_browser_event(&self, event: BrowserEvent) {
        let _send_result = self
            .browser_event_sender
            .send(TabEvent::new(self.tab_id, event));
    }

    // controls
    fn drop_controls(&self) {
        for event in self.controls.drop_all() {
            self.send_browser_event(event);
        }
    }

    fn record_load_milestone(&self, milestone: LoadMilestone) {
        let elapsed = self
            .load_metrics
            .borrow_mut()
            .record(milestone, Instant::now());

        if let Some(elapsed) = elapsed {
            info!(
                "tab {}: page load milestone: {} at {:.1} ms",
                self.tab_id.value(),
                milestone.label(),
                elapsed.as_secs_f64() * 1_000.0
            );
        }
    }
}

impl servo::WebViewDelegate for BrowserDelegate {
    fn screen_geometry(&self, _web_view: servo::WebView) -> Option<servo::ScreenGeometry> {
        self.screen_metrics
            .get()
            .map(ScreenMetrics::to_servo_geometry)
    }

    fn load_web_resource(&self, web_view: servo::WebView, load: servo::WebResourceLoad) {
        self.content_blocker
            .handle_web_resource_load(Some(&web_view), load);
    }

    fn notify_url_changed(&self, _web_view: servo::WebView, url: Url) {
        // dialogs: user info is left out; after a sign-in it holds the password.
        log::debug!(
            "tab {}: address changed to {}",
            self.tab_id.value(),
            talaria_core::strip_user_info(url.as_str()),
        );
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
        // dialogs: a new document; the previous one's requests and page state no longer apply.
        if self.document_start.borrow_mut().document_started(status) {
            self.send_browser_event(BrowserEvent::PageLoadStarted);
        }
        match status {
            LoadStatus::Started => {
                // controls: requests of the page being left are dropped.
                self.drop_controls();
                self.load_metrics.borrow_mut().start(Instant::now());
                info!("tab {}: page load started", self.tab_id.value());
            }
            LoadStatus::HeadParsed => self.record_load_milestone(LoadMilestone::HeadParsed),
            LoadStatus::Complete => self.record_load_milestone(LoadMilestone::Complete),
            LoadStatus::Stopped => info!("tab {}: page load stopped", self.tab_id.value()),
        }

        self.send_browser_event(BrowserEvent::LoadStateChanged {
            state: load_state_from_servo(status),
        });
    }

    fn notify_history_changed(&self, _web_view: servo::WebView, entries: Vec<Url>, current: usize) {
        self.send_browser_event(history_changed_event(entries.len(), current));
    }

    fn notify_cursor_changed(&self, _web_view: servo::WebView, cursor: Cursor) {
        self.send_browser_event(BrowserEvent::CursorChanged {
            cursor: pointer_cursor_from_servo(cursor),
        });
    }

    fn notify_status_text_changed(&self, _web_view: servo::WebView, status: Option<String>) {
        self.send_browser_event(BrowserEvent::StatusTextChanged { text: status });
    }

    fn notify_fullscreen_state_changed(&self, _web_view: servo::WebView, is_fullscreen: bool) {
        self.send_browser_event(BrowserEvent::FullscreenChanged {
            fullscreen: is_fullscreen,
        });
    }

    fn notify_new_frame_ready(&self, _web_view: servo::WebView) {
        self.record_load_milestone(LoadMilestone::FirstFrame);
        self.frame_ready.mark_ready();
    }

    fn notify_crashed(&self, web_view: servo::WebView, reason: String, _backtrace: Option<String>) {
        // controls
        self.drop_controls();
        for event in crash_feedback(reason, web_view.cursor(), web_view.status_text()) {
            self.send_browser_event(event);
        }
    }

    // controls, dialogs: simple dialogs are page requests; every other kind is a control.
    fn show_embedder_control(&self, web_view: servo::WebView, control: servo::EmbedderControl) {
        match control {
            servo::EmbedderControl::SimpleDialog(dialog) => {
                self.show_simple_dialog(&web_view, dialog);
            }
            control => {
                if let Some(event) = self.controls.show(control) {
                    self.send_browser_event(event);
                }
            }
        }
    }

    // controls: Servo never sends a hide for simple dialogs.
    fn hide_embedder_control(
        &self,
        _web_view: servo::WebView,
        control_id: servo::EmbedderControlId,
    ) {
        if let Some(event) = self.controls.hide(control_id) {
            self.send_browser_event(event);
        }
    }

    fn request_unload(&self, web_view: servo::WebView, request: servo::AllowOrDenyRequest) {
        self.forward_page_request(
            web_view.url(),
            PageRequestKind::LeavePage,
            ServoPageRequest::Unload(request),
        );
    }

    fn request_authentication(
        &self,
        _web_view: servo::WebView,
        request: servo::AuthenticationRequest,
    ) {
        let url = Some(request.url().clone());
        let kind = PageRequestKind::Authentication {
            for_proxy: request.for_proxy(),
        };
        self.forward_page_request(url, kind, ServoPageRequest::Authentication(request));
    }

    fn request_permission(&self, web_view: servo::WebView, request: servo::PermissionRequest) {
        // Servo does not say which frame asked; the request is shown for the tab's page.
        let kind = PageRequestKind::Permission(page_permission(request.feature()));
        self.forward_page_request(web_view.url(), kind, ServoPageRequest::Permission(request));
    }
}

// dialogs
impl BrowserDelegate {
    fn show_simple_dialog(&self, web_view: &servo::WebView, dialog: servo::SimpleDialog) {
        let kind = simple_dialog_kind(&dialog);
        self.forward_page_request(web_view.url(), kind, ServoPageRequest::Dialog(dialog));
    }

    /// Keeps the engine's request and tells the browser about it. A request that arrives while
    /// the web view is closing is dropped at once, which gives the engine's default answer.
    fn forward_page_request(
        &self,
        url: Option<Url>,
        kind: PageRequestKind,
        request: ServoPageRequest,
    ) {
        let Some(id) = self.page_requests.borrow_mut().insert(request) else {
            return;
        };
        log::debug!(
            "tab {}: page request {} ({kind:?}) for {}",
            self.tab_id.value(),
            id.value(),
            url.as_ref().map_or_else(
                || "no address".to_owned(),
                |url| talaria_core::strip_user_info(url.as_str())
            ),
        );
        self.send_browser_event(BrowserEvent::PageRequested(PageRequest {
            id,
            url: url.map(String::from).unwrap_or_default(),
            kind,
            received_at: crate::page_request_clock(),
        }));
    }
}

/// What a freshly built web view's own caches hold. Servo only notifies changes against these
/// caches (and starts with `Cursor::Pointer`), so the tab's copies must start from the same
/// values, also when the web view replaces an earlier one.
fn new_web_view_feedback(cursor: Cursor, status_text: Option<String>) -> [BrowserEvent; 3] {
    [
        BrowserEvent::CursorChanged {
            cursor: pointer_cursor_from_servo(cursor),
        },
        BrowserEvent::StatusTextChanged { text: status_text },
        BrowserEvent::FullscreenChanged { fullscreen: false },
    ]
}

/// A crashed page never answers a pointer leave, so the web view's caches keep their pre-crash
/// values and Servo will only notify changes against those. The tab copies them right after
/// the failure, so it stays in step when the page is reloaded.
fn crash_feedback(
    reason: String,
    cursor: Cursor,
    status_text: Option<String>,
) -> [BrowserEvent; 3] {
    [
        BrowserEvent::LoadStateChanged {
            state: LoadState::Failed { message: reason },
        },
        BrowserEvent::CursorChanged {
            cursor: pointer_cursor_from_servo(cursor),
        },
        BrowserEvent::StatusTextChanged { text: status_text },
    ]
}

fn pointer_cursor_from_servo(cursor: Cursor) -> PointerCursor {
    match cursor {
        Cursor::None => PointerCursor::None,
        Cursor::Default => PointerCursor::Default,
        Cursor::Pointer => PointerCursor::Pointer,
        Cursor::ContextMenu => PointerCursor::ContextMenu,
        Cursor::Help => PointerCursor::Help,
        Cursor::Progress => PointerCursor::Progress,
        Cursor::Wait => PointerCursor::Wait,
        Cursor::Cell => PointerCursor::Cell,
        Cursor::Crosshair => PointerCursor::Crosshair,
        Cursor::Text => PointerCursor::Text,
        Cursor::VerticalText => PointerCursor::VerticalText,
        Cursor::Alias => PointerCursor::Alias,
        Cursor::Copy => PointerCursor::Copy,
        Cursor::Move => PointerCursor::Move,
        Cursor::NoDrop => PointerCursor::NoDrop,
        Cursor::NotAllowed => PointerCursor::NotAllowed,
        Cursor::Grab => PointerCursor::Grab,
        Cursor::Grabbing => PointerCursor::Grabbing,
        Cursor::EResize => PointerCursor::EResize,
        Cursor::NResize => PointerCursor::NResize,
        Cursor::NeResize => PointerCursor::NeResize,
        Cursor::NwResize => PointerCursor::NwResize,
        Cursor::SResize => PointerCursor::SResize,
        Cursor::SeResize => PointerCursor::SeResize,
        Cursor::SwResize => PointerCursor::SwResize,
        Cursor::WResize => PointerCursor::WResize,
        Cursor::EwResize => PointerCursor::EwResize,
        Cursor::NsResize => PointerCursor::NsResize,
        Cursor::NeswResize => PointerCursor::NeswResize,
        Cursor::NwseResize => PointerCursor::NwseResize,
        Cursor::ColResize => PointerCursor::ColResize,
        Cursor::RowResize => PointerCursor::RowResize,
        Cursor::AllScroll => PointerCursor::AllScroll,
        Cursor::ZoomIn => PointerCursor::ZoomIn,
        Cursor::ZoomOut => PointerCursor::ZoomOut,
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
            for pixel in pixels.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        PixelFormat::RGB8 => {
            for pixel in pixels.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        PixelFormat::RGBA8 => rgba.extend_from_slice(pixels),
        PixelFormat::BGRA8 => {
            for pixel in pixels.as_chunks::<4>().0 {
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
        }
    }

    PageIcon::from_rgba(width, height, rgba)
}

fn load_state_from_servo(status: LoadStatus) -> LoadState {
    match status {
        LoadStatus::Started | LoadStatus::HeadParsed => LoadState::Loading,
        LoadStatus::Complete | LoadStatus::Stopped => LoadState::Loaded,
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
    use super::mercury_options;

    #[test]
    fn enables_content_process_isolation() {
        assert!(mercury_options(None).multiprocess);
    }

    #[test]
    fn keeps_the_unreliable_os_sandbox_disabled() {
        assert!(!mercury_options(None).sandbox);
    }

    use super::*;

    #[test]
    fn hard_fail_is_disabled() {
        assert!(!mercury_options(None).hard_fail);
    }

    #[test]
    fn config_dir_is_passed_to_servo() {
        let config_dir = PathBuf::from("/profile/servo");

        assert_eq!(
            mercury_options(Some(config_dir.clone())).config_dir,
            Some(config_dir)
        );
        assert_eq!(mercury_options(None).config_dir, None);
    }

    #[test]
    fn mercury_keeps_servos_default_script_settings() {
        for gpu_canvas in [true, false] {
            let preferences = mercury_preferences(&RuntimeOptions {
                gpu_canvas,
                ..RuntimeOptions::default()
            });

            assert_eq!(
                preferences.dom_script_asynch,
                Preferences::default().dom_script_asynch
            );
        }
    }

    #[test]
    fn gpu_canvas_is_enabled_by_default() {
        let preferences = mercury_preferences(&RuntimeOptions::default());

        assert_eq!(preferences.dom_canvas_backend, "vello");
    }

    #[test]
    fn gpu_canvas_can_be_disabled() {
        let preferences = mercury_preferences(&RuntimeOptions {
            gpu_canvas: false,
            ..RuntimeOptions::default()
        });

        assert_eq!(
            preferences.dom_canvas_backend,
            Preferences::default().dom_canvas_backend
        );
    }

    fn preferences_with_experimental_web_features(enabled: bool) -> Preferences {
        mercury_preferences(&RuntimeOptions {
            experimental_web_features: enabled,
            ..RuntimeOptions::default()
        })
    }

    #[test]
    fn experimental_features_enable_servoshells_list_except_the_excluded_ones() {
        let preferences = preferences_with_experimental_web_features(true);

        assert_eq!(EXPERIMENTAL_PREFS.len(), 18);
        assert_eq!(
            EXCLUDED_EXPERIMENTAL_PREFS,
            &["dom_async_clipboard_enabled", "dom_webgpu_enabled"]
        );
        let enabled = EXPERIMENTAL_PREFS
            .iter()
            .filter(|name| !EXCLUDED_EXPERIMENTAL_PREFS.contains(name))
            .collect::<Vec<_>>();
        assert_eq!(enabled.len(), 16);
        for name in enabled {
            assert_eq!(preferences.get_value(name), PrefValue::Bool(true), "{name}");
        }
    }

    #[test]
    fn async_clipboard_stays_disabled() {
        for enabled in [true, false] {
            assert!(
                !preferences_with_experimental_web_features(enabled).dom_async_clipboard_enabled
            );
        }
    }

    #[test]
    fn webgpu_stays_disabled_while_the_webgpu_feature_is_not_built() {
        for enabled in [true, false] {
            assert!(!preferences_with_experimental_web_features(enabled).dom_webgpu_enabled);
        }
    }

    #[test]
    fn experimental_features_are_on_by_default() {
        assert!(RuntimeOptions::default().experimental_web_features);
    }

    #[test]
    fn experimental_features_can_be_disabled() {
        let preferences = preferences_with_experimental_web_features(false);
        let defaults = Preferences::default();

        for name in EXPERIMENTAL_PREFS {
            assert_eq!(
                preferences.get_value(name),
                defaults.get_value(name),
                "{name}"
            );
        }
    }

    #[test]
    fn pages_cannot_choose_their_own_upload_files() {
        for enabled in [true, false] {
            assert!(
                !preferences_with_experimental_web_features(enabled)
                    .dom_testing_html_input_element_select_files_enabled
            );
        }
    }

    #[test]
    fn webrtc_stays_disabled() {
        for enabled in [true, false] {
            assert!(!preferences_with_experimental_web_features(enabled).dom_webrtc_enabled);
        }
    }

    #[test]
    fn user_agent_drops_the_servo_token() {
        assert_eq!(
            firefox_compatible_user_agent(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:153.0) Servo/0.8.0 Firefox/153.0"
            ),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:153.0) Gecko/20100101 Firefox/153.0"
        );
        assert_eq!(
            firefox_compatible_user_agent(
                "Mozilla/5.0 (X11; Linux aarch64; rv:153.0) Servo/0.8.0 Firefox/153.0"
            ),
            "Mozilla/5.0 (X11; Linux aarch64; rv:153.0) Gecko/20100101 Firefox/153.0"
        );
    }

    #[test]
    fn user_agent_without_a_servo_token_is_unchanged() {
        let user_agent = "Mozilla/5.0 (X11; Linux x86_64; rv:153.0) Gecko/20100101 Firefox/153.0";

        assert_eq!(firefox_compatible_user_agent(user_agent), user_agent);
    }

    #[test]
    fn the_user_agent_is_firefox_compatible_in_both_modes() {
        for enabled in [true, false] {
            let user_agent = preferences_with_experimental_web_features(enabled).user_agent;

            assert!(!user_agent.contains("Servo/"), "{user_agent}");
            assert!(
                user_agent.contains(" Gecko/20100101 Firefox/"),
                "{user_agent}"
            );
        }
    }

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
    fn stopped_is_a_loaded_state() {
        assert_eq!(
            load_state_from_servo(LoadStatus::Stopped),
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
    fn every_servo_cursor_maps_to_a_pointer_cursor() {
        let pairs = [
            (Cursor::None, PointerCursor::None),
            (Cursor::Default, PointerCursor::Default),
            (Cursor::Pointer, PointerCursor::Pointer),
            (Cursor::ContextMenu, PointerCursor::ContextMenu),
            (Cursor::Help, PointerCursor::Help),
            (Cursor::Progress, PointerCursor::Progress),
            (Cursor::Wait, PointerCursor::Wait),
            (Cursor::Cell, PointerCursor::Cell),
            (Cursor::Crosshair, PointerCursor::Crosshair),
            (Cursor::Text, PointerCursor::Text),
            (Cursor::VerticalText, PointerCursor::VerticalText),
            (Cursor::Alias, PointerCursor::Alias),
            (Cursor::Copy, PointerCursor::Copy),
            (Cursor::Move, PointerCursor::Move),
            (Cursor::NoDrop, PointerCursor::NoDrop),
            (Cursor::NotAllowed, PointerCursor::NotAllowed),
            (Cursor::Grab, PointerCursor::Grab),
            (Cursor::Grabbing, PointerCursor::Grabbing),
            (Cursor::EResize, PointerCursor::EResize),
            (Cursor::NResize, PointerCursor::NResize),
            (Cursor::NeResize, PointerCursor::NeResize),
            (Cursor::NwResize, PointerCursor::NwResize),
            (Cursor::SResize, PointerCursor::SResize),
            (Cursor::SeResize, PointerCursor::SeResize),
            (Cursor::SwResize, PointerCursor::SwResize),
            (Cursor::WResize, PointerCursor::WResize),
            (Cursor::EwResize, PointerCursor::EwResize),
            (Cursor::NsResize, PointerCursor::NsResize),
            (Cursor::NeswResize, PointerCursor::NeswResize),
            (Cursor::NwseResize, PointerCursor::NwseResize),
            (Cursor::ColResize, PointerCursor::ColResize),
            (Cursor::RowResize, PointerCursor::RowResize),
            (Cursor::AllScroll, PointerCursor::AllScroll),
            (Cursor::ZoomIn, PointerCursor::ZoomIn),
            (Cursor::ZoomOut, PointerCursor::ZoomOut),
        ];
        assert_eq!(pairs.len(), 35);
        for (servo_cursor, pointer_cursor) in pairs {
            assert_eq!(
                pointer_cursor_from_servo(servo_cursor),
                pointer_cursor,
                "{servo_cursor:?}"
            );
        }
    }

    #[test]
    fn a_crash_reports_the_failure_then_the_web_views_cached_feedback() {
        assert_eq!(
            crash_feedback(
                "boom".to_owned(),
                Cursor::Pointer,
                Some("https://a.test/".to_owned())
            ),
            [
                BrowserEvent::LoadStateChanged {
                    state: LoadState::Failed {
                        message: "boom".to_owned(),
                    },
                },
                BrowserEvent::CursorChanged {
                    cursor: PointerCursor::Pointer,
                },
                BrowserEvent::StatusTextChanged {
                    text: Some("https://a.test/".to_owned()),
                },
            ]
        );
    }

    #[test]
    fn a_new_web_view_reports_the_engines_starting_feedback() {
        assert_eq!(
            new_web_view_feedback(Cursor::Pointer, None),
            [
                BrowserEvent::CursorChanged {
                    cursor: PointerCursor::Pointer,
                },
                BrowserEvent::StatusTextChanged { text: None },
                BrowserEvent::FullscreenChanged { fullscreen: false },
            ]
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
