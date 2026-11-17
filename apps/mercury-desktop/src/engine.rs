use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use mercury_ui::MainWindow;
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, GraphicsAPI, Image, RenderingState, Timer};
use talaria_core::{
    BrowserCommand, BrowserController, BrowserEvent, DEFAULT_ZOOM_PERCENT, LoadState, TabEvent,
    TabId, WebView as TalariaWebView, dispatch_command,
};
use talaria_servo::{
    EventLoopWaker, NEW_TAB_URL, RuntimeOptions, ScreenMetrics, ServoRuntime, ServoWebView,
    ServoWebViewError, ViewportSize,
};

use crate::keyboard::system_color_scheme;
use crate::rendering::{BrowserRenderingContext, RenderingBridgeError, ReportOncePerKind};
use crate::screen_metrics::screen_metrics;
use crate::shutdown::{
    ENGINE_SHUTDOWN_TIMEOUT, EngineParts, Watchdog, drop_catching_panic, release_in_order,
    settle_period, shut_down_runtime,
};
use crate::tab_switch_frame::TabSwitchFrame;
use crate::ui_sync::sync_ui;

pub(crate) const MAX_COMMANDS_PER_FRAME: usize = 128;

/// How long the window may go without its first render before one is forced offscreen.
const GRAPHICS_BOOTSTRAP_DELAY: Duration = Duration::from_millis(300);
const GRAPHICS_BOOTSTRAP_RETRY_INTERVAL: Duration = Duration::from_secs(1);

thread_local! {
    /// Lets Servo's waker, whose callback must be `Send`, reach the engine on the main thread.
    static ENGINE: RefCell<Weak<Engine>> = const { RefCell::new(Weak::new()) };
}

/// Drives Servo independently of drawn frames. Servo is pumped whenever it wakes the event
/// loop and before every render; only painting into the window waits for a render, so pages
/// keep loading while the window is covered and the GPU surface skips frames.
struct Engine {
    window: slint::Weak<MainWindow>,
    /// `None` once `shut_down_engine` has run; every later pump or render then does nothing.
    state: RefCell<Option<EngineState>>,
    bootstrap_failure_logged: Cell<bool>,
}

/// Dropped any way other than `shut_down` (an early return during start-up, unwinding), its
/// `Drop` still releases the web views, the runtime, the rendering context and the device in
/// that order; see `take_parts`.
struct EngineState {
    /// `None` only once `take_parts` has run.
    runtime: Option<ServoRuntime>,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
    rendering_context: Option<Rc<BrowserRenderingContext>>,
    wgpu_device: Option<slint::wgpu_29::wgpu::Device>,
    web_views: HashMap<TabId, ServoWebView>,
    /// One channel per web view, so dropping a replaced web view also drops its late events.
    event_receivers: HashMap<TabId, Receiver<TabEvent>>,
    visible_tab_id: Option<TabId>,
    current_viewport_size: Option<ViewportSize>,
    current_scale_factor: Option<f32>,
    /// Whether shutdown has content processes (and WebGL contexts) to wait for.
    web_view_created: bool,
    /// What pages were last told about the screen and window.
    screen_metrics: Option<ScreenMetrics>,
    /// tabs: bounds how long the previous tab's frame stays up after a switch.
    tab_switch_frame: TabSwitchFrame,
    frame_import_errors: ReportOncePerKind<RenderingBridgeError>,
}

/// The parts of `EngineState` that are released in order.
type OwnedEngineParts = EngineParts<
    (
        HashMap<TabId, ServoWebView>,
        HashMap<TabId, Receiver<TabEvent>>,
    ),
    ServoRuntime,
    Option<Rc<BrowserRenderingContext>>,
    Option<slint::wgpu_29::wgpu::Device>,
>;

enum GraphicsStatus {
    Ready,
    Missing,
    /// The engine state is borrowed; nothing can be concluded right now.
    Busy,
    /// The engine has been shut down; there is nothing left to set up.
    ShutDown,
}

pub(crate) fn connect_servo_runtime(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
    runtime_options: RuntimeOptions,
) -> Result<(), slint::SetRenderingNotifierError> {
    let event_loop_waker = EventLoopWaker::new(|| {
        let _queued = slint::invoke_from_event_loop(pump_after_wake);
    });

    // tabs opened by pages
    let allocator_window = window.as_weak();
    let new_tab_allocator = crate::page_tabs::new_tab_allocator(&controller, move || {
        allocator_window
            .upgrade()
            .is_some_and(|window| window.get_address_has_focus())
    });
    let engine = Rc::new(Engine {
        window: window.as_weak(),
        state: RefCell::new(Some(EngineState {
            runtime: Some(ServoRuntime::new(event_loop_waker, runtime_options)),
            controller,
            web_view_focused,
            rendering_context: None,
            wgpu_device: None,
            web_views: HashMap::new(),
            event_receivers: HashMap::new(),
            visible_tab_id: None,
            current_viewport_size: None,
            current_scale_factor: None,
            web_view_created: false,
            screen_metrics: None,
            tab_switch_frame: TabSwitchFrame::default(),
            frame_import_errors: ReportOncePerKind::default(),
        })),
        bootstrap_failure_logged: Cell::new(false),
    });
    // tabs opened by pages
    if let Some(runtime) = engine
        .state
        .borrow()
        .as_ref()
        .and_then(|state| state.runtime.as_ref())
    {
        runtime.set_new_tab_allocator(new_tab_allocator);
        // activation (tabs lane): a new document does not inherit the last one's activation
        runtime.set_document_started_listener(crate::page_tabs::document_started_listener());
    }
    ENGINE.with(|current| *current.borrow_mut() = Rc::downgrade(&engine));
    schedule_graphics_bootstrap(Rc::downgrade(&engine), GRAPHICS_BOOTSTRAP_DELAY);

    let rendering_engine = Rc::clone(&engine);
    window
        .window()
        .set_rendering_notifier(move |state, graphics_api| {
            let _slint = crate::callback_scope::InSlintCallback::enter();
            rendering_engine.on_rendering_state(state, graphics_api);
        })
}

// settings-ui: lets the settings reach the engine's switches (content blocking, clearing at exit).
/// Runs `work` with the engine's runtime; false if the engine is gone or busy.
pub(crate) fn with_runtime(work: impl FnOnce(&ServoRuntime)) -> bool {
    let Some(engine) = ENGINE.with(|current| current.borrow().upgrade()) else {
        return false;
    };
    let Ok(state) = engine.state.try_borrow() else {
        return false;
    };
    let Some(runtime) = state.as_ref().and_then(|state| state.runtime.as_ref()) else {
        return false;
    };
    work(runtime);
    true
}

/// Shuts the engine down in order. Call once, after the event loop has ended and before the
/// window is dropped: otherwise the engine is dropped with the rendering notifier, from inside
/// the renderer's own destruction, after the device Servo renders through has gone.
///
/// If Servo has not finished shutting down within `ENGINE_SHUTDOWN_TIMEOUT`, the process exits
/// with `exit_code_if_stuck` rather than hang.
pub(crate) fn shut_down_engine(exit_code_if_stuck: u8) {
    // Queued re-pumps and Servo's waker reach the engine only through this; from now on they
    // find nothing.
    let Some(engine) = ENGINE.with(|current| current.replace(Weak::new()).upgrade()) else {
        return;
    };
    // The bootstrap timers and the rendering notifier see `None` from here on and do nothing.
    let Some(state) = engine.state.borrow_mut().take() else {
        return;
    };
    let window = engine.window.upgrade();
    if let Some(window) = &window {
        // Quitting looks instant while the engine winds down. The state is taken first because
        // hiding can tear rendering down (Wayland destroys the window), and that teardown must
        // not touch the engine now.
        if let Err(error) = window.hide() {
            log::warn!("could not hide the window before shutting down: {error}");
        }
    }
    state.shut_down(window.as_ref(), exit_code_if_stuck);
}

/// Whether the engine's state is borrowed right now (a pump or a render is running).
#[cfg(target_os = "macos")]
pub(crate) fn engine_busy() -> bool {
    ENGINE.with(|current| {
        current
            .borrow()
            .upgrade()
            .is_some_and(|engine| engine.state.try_borrow_mut().is_err())
    })
}

fn pump_after_wake() {
    let _slint = crate::callback_scope::InSlintCallback::enter();
    let Some(engine) = ENGINE.with(|current| current.borrow().upgrade()) else {
        return;
    };

    engine.pump();
    if let Some(window) = engine.window.upgrade() {
        // Slint skips this frame while the window is covered; the pump above already ran.
        window.window().request_redraw();
    }
}

/// The wgpu device reaches the notifier only through `RenderingSetup`, which needs one render.
/// A window that is covered from the start never renders to its surface, so force a render
/// into an offscreen snapshot target instead; `take_snapshot` bypasses the occlusion check.
fn schedule_graphics_bootstrap(engine: Weak<Engine>, delay: Duration) {
    Timer::single_shot(delay, move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let Some(strong_engine) = engine.upgrade() else {
            return;
        };
        match strong_engine.graphics_status() {
            GraphicsStatus::Ready => {
                strong_engine.bootstrap_failure_logged.set(false);
                return;
            }
            GraphicsStatus::ShutDown => return,
            GraphicsStatus::Busy => {
                schedule_graphics_bootstrap(engine, GRAPHICS_BOOTSTRAP_DELAY);
                return;
            }
            GraphicsStatus::Missing => {}
        }
        let Some(window) = strong_engine.window.upgrade() else {
            return;
        };

        log::info!("the window has not rendered yet; forcing an offscreen render");
        // Never called while the engine state is borrowed: this re-enters the notifier.
        let snapshot_result = window.window().take_snapshot();
        match strong_engine.graphics_status() {
            GraphicsStatus::Ready => {
                strong_engine.bootstrap_failure_logged.set(false);
                return;
            }
            GraphicsStatus::ShutDown => return,
            GraphicsStatus::Busy => {}
            GraphicsStatus::Missing => {
                if !strong_engine.bootstrap_failure_logged.replace(true) {
                    match snapshot_result {
                        Ok(_) => log::warn!(
                            "an offscreen render did not set up graphics; retrying every {} ms",
                            GRAPHICS_BOOTSTRAP_RETRY_INTERVAL.as_millis()
                        ),
                        Err(error) => log::warn!(
                            "an offscreen render failed ({error}); retrying every {} ms",
                            GRAPHICS_BOOTSTRAP_RETRY_INTERVAL.as_millis()
                        ),
                    }
                }
            }
        }
        schedule_graphics_bootstrap(engine, GRAPHICS_BOOTSTRAP_RETRY_INTERVAL);
    });
}

impl Engine {
    fn graphics_status(&self) -> GraphicsStatus {
        match self.state.try_borrow().as_deref() {
            Ok(Some(state)) if state.rendering_context.is_some() => GraphicsStatus::Ready,
            Ok(Some(_)) => GraphicsStatus::Missing,
            Ok(None) => GraphicsStatus::ShutDown,
            Err(_) => GraphicsStatus::Busy,
        }
    }

    /// Advances Servo and the browser state. Skips, rather than panics, if re-entered.
    fn pump(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let Ok(mut state) = self.state.try_borrow_mut() else {
            log::debug!("skipping a re-entrant engine pump");
            return;
        };
        let Some(state) = state.as_mut() else {
            return;
        };
        state.pump(&window);
    }

    fn on_rendering_state(&self, rendering_state: RenderingState, graphics_api: &GraphicsAPI<'_>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let Ok(mut state_slot) = self.state.try_borrow_mut() else {
            log::warn!("skipping {rendering_state:?}: the engine is busy");
            return;
        };
        let Some(state) = state_slot.as_mut() else {
            return;
        };

        match rendering_state {
            RenderingState::RenderingSetup => state.set_up_rendering(&window, graphics_api),
            RenderingState::BeforeRendering => {
                if state.rendering_context.is_none() {
                    // An earlier setup failed; every render offers the device again.
                    state.set_up_rendering(&window, graphics_api);
                }
                state.pump(&window);
                state.present(&window);
            }
            RenderingState::RenderingTeardown => {
                state.tear_down_rendering();
                drop(state_slot);
                // A later setup needs bootstrapping again if the window is covered by then.
                let engine = ENGINE.with(|current| current.borrow().clone());
                schedule_graphics_bootstrap(engine, GRAPHICS_BOOTSTRAP_DELAY);
            }
            _ => {}
        }
    }
}

impl EngineState {
    fn set_up_rendering(&mut self, window: &MainWindow, graphics_api: &GraphicsAPI<'_>) {
        if self.rendering_context.is_some() {
            return;
        }

        // A texture left from before a `RenderingTeardown` belongs to the device Slint gave up.
        window.set_web_view_texture(Image::default());
        window.set_web_view_visible(false);

        let GraphicsAPI::WGPU29 { device, queue, .. } = graphics_api else {
            window.set_content_message(
                "The selected Slint renderer does not provide WGPU 29.".into(),
            );
            return;
        };
        let window_size = window.window().size();
        let Some(viewport_size) = ViewportSize::new(window_size.width, window_size.height) else {
            window.set_content_message("The OpenGL viewport has invalid dimensions.".into());
            return;
        };

        let context = match BrowserRenderingContext::new(viewport_size, device, queue) {
            Ok(context) => context,
            Err(error) => {
                window.set_content_message(
                    format!("Failed to create Servo's rendering context: {error}").into(),
                );
                return;
            }
        };

        self.rendering_context = Some(Rc::new(context));
        self.wgpu_device = Some(device.clone());
        // The window exists now, so the system appearance can be read. Later changes arrive as
        // `ThemeChanged` window events.
        let winit_theme = window
            .window()
            .with_winit_window(|winit_window| winit_window.theme())
            .flatten();
        if let Some(scheme) = system_color_scheme(winit_theme, window.get_system_color_scheme()) {
            self.controller.borrow_mut().set_color_scheme(scheme);
        }
        window.set_content_message(
            "Offscreen rendering initialized. Waiting for Servo WebView.".into(),
        );
    }

    /// Everything that does not need a frame: Servo's event loop, browser events, web-view
    /// lifetimes, tab visibility, sizing and command dispatch.
    fn pump(&mut self, window: &MainWindow) {
        // Before spinning, so that a page's first query after the window moved is current.
        self.update_screen_metrics(window);
        let Some(runtime) = &self.runtime else {
            return;
        };
        runtime.spin_event_loop();

        let ready_before = {
            let controller = self.controller.borrow();
            is_engine_ready(&controller, controller.active_tab_id())
        };
        // tabs opened by pages: before the events, so the new tabs' first events are applied
        let adoption = crate::page_tabs::adopt_page_web_views(
            runtime,
            &self.controller,
            &mut self.web_views,
            &mut self.event_receivers,
        );
        self.web_view_created |= adoption.web_views_added;
        let mut state_changed = adoption.ui_changed;
        for receiver in self.event_receivers.values() {
            state_changed |= apply_pending_browser_events(&self.controller, receiver);
        }
        // tabs: title reads owed while another was in flight
        for web_view in self.web_views.values() {
            web_view.refresh_page_feedback();
        }
        let active_became_ready = !ready_before && {
            let controller = self.controller.borrow();
            is_engine_ready(&controller, controller.active_tab_id())
        };
        if state_changed {
            sync_ui(window, &self.controller.borrow());
        }

        let Some(rendering_context) = self.rendering_context.clone() else {
            return;
        };
        let scale_factor = window.window().scale_factor();
        let Some(viewport_size) = ViewportSize::from_logical(
            window.get_web_view_width(),
            window.get_web_view_height(),
            scale_factor,
        ) else {
            window.set_content_message("The WebView viewport has invalid dimensions.".into());
            return;
        };

        let (active_tab_id, tab_ids) = {
            let controller = self.controller.borrow();
            let active_tab_id = controller.active_tab_id();
            let tab_ids = controller.tabs().map(|tab| tab.id()).collect::<Vec<_>>();
            (active_tab_id, tab_ids)
        };
        // activation (tabs lane): closed tabs forget their activation
        for closed in self
            .web_views
            .keys()
            .filter(|tab_id| !tab_ids.contains(tab_id))
        {
            crate::activation::forget_tab(*closed);
            crate::page_tabs::forget_tab(*closed);
        }
        self.web_views.retain(|tab_id, _| tab_ids.contains(tab_id));
        self.event_receivers
            .retain(|tab_id, _| tab_ids.contains(tab_id));
        for tab_id in tab_ids {
            let initial_url = self.controller.borrow_mut().take_initial_url(tab_id);
            let url = match plan_web_view(self.web_views.contains_key(&tab_id), initial_url) {
                WebViewPlan::Keep => continue,
                WebViewPlan::Create { url } => url,
                WebViewPlan::Replace { url } => {
                    log::info!(
                        "tab {}: replacing a page that never committed",
                        tab_id.value()
                    );
                    self.web_views.remove(&tab_id);
                    self.event_receivers.remove(&tab_id);
                    if self.visible_tab_id == Some(tab_id) {
                        // Show, focus and size the new web view like a newly selected tab.
                        self.visible_tab_id = None;
                    }
                    url
                }
            };

            let (event_sender, event_receiver) = mpsc::channel::<TabEvent>();
            let create_result = runtime.create_web_view(
                rendering_context.clone(),
                &url,
                scale_factor,
                tab_id,
                event_sender,
            );

            match create_result {
                Ok(mut created_web_view) => {
                    created_web_view.hide();
                    let zoom_percent = tab_zoom_percent(&self.controller.borrow(), tab_id);
                    if zoom_percent != DEFAULT_ZOOM_PERCENT {
                        let _zoom_result = created_web_view.set_page_zoom(zoom_percent);
                    }
                    let color_scheme = self.controller.borrow().color_scheme();
                    let _scheme_result = created_web_view.set_color_scheme(color_scheme);
                    self.web_views.insert(tab_id, created_web_view);
                    self.web_view_created = true;
                    self.event_receivers.insert(tab_id, event_receiver);
                }
                Err(error) => {
                    report_web_view_error(window, &self.controller, tab_id, &error);
                    return;
                }
            }
        }

        let active_tab_changed = self.visible_tab_id != Some(active_tab_id);
        if active_tab_changed && let Some(previous_tab_id) = self.visible_tab_id {
            queue_leaving_tab(&mut self.controller.borrow_mut(), previous_tab_id);
        }
        if active_tab_changed {
            if let Some(previous_tab_id) = self.visible_tab_id
                && let Some(previous_web_view) = self.web_views.get_mut(&previous_tab_id)
            {
                let _focus_result = previous_web_view.set_focus(false);
                previous_web_view.hide();
            }

            if let Some(active_web_view) = self.web_views.get_mut(&active_tab_id) {
                // Servo 0.8 couples visibility and throttling: show() resumes the active
                // pipeline and hide() throttles background tabs.
                active_web_view.show();
                // tabs: only a frame announced after show() shows this tab; painting without
                // importing ends the repaint Servo still owes for the earlier scene, which
                // would otherwise announce it to every web view again
                active_web_view.paint();
                active_web_view.discard_ready_frame();
                let _focus_result = active_web_view.set_focus(self.web_view_focused.get());
            }
            // tabs: keep the previous frame briefly instead of flashing the background
            self.tab_switch_frame.begin(window);
            self.visible_tab_id = Some(active_tab_id);
        }

        if self.current_scale_factor != Some(scale_factor) {
            for web_view in self.web_views.values() {
                web_view.set_scale_factor(scale_factor);
            }
            self.current_scale_factor = Some(scale_factor);
        }

        if let Some(pointer_move) = resting_pointer_move(
            active_tab_changed,
            active_became_ready,
            window.get_web_view_hovered(),
            window.get_web_view_pointer_x(),
            window.get_web_view_pointer_y(),
        ) {
            self.controller.borrow_mut().queue_command(pointer_move);
        }

        if (active_tab_changed || self.current_viewport_size != Some(viewport_size))
            && let Some(active_web_view) = self.web_views.get(&active_tab_id)
        {
            // Each Servo WebView owns its resize notification. Resizing the shared
            // context directly would skip Servo's CSS viewport update.
            active_web_view.resize(viewport_size);
            self.current_viewport_size = Some(viewport_size);
        }

        match dispatch_pending_commands(&self.controller, &mut self.web_views) {
            Ok(has_pending_commands) => {
                if has_pending_commands {
                    // A redraw may be skipped while the window is covered; pump again instead.
                    let _queued = slint::invoke_from_event_loop(pump_after_wake);
                }
            }
            Err((tab_id, error)) => {
                report_web_view_error(window, &self.controller, tab_id, &error);
            }
        }
    }

    fn update_screen_metrics(&mut self, window: &MainWindow) {
        let metrics = window.window().with_winit_window(screen_metrics).flatten();
        if metrics != self.screen_metrics
            && let Some(runtime) = &self.runtime
        {
            runtime.set_screen_metrics(metrics);
            self.screen_metrics = metrics;
        }
    }

    /// Paints the visible tab's latest Servo frame into the window. Only runs inside a render.
    fn present(&mut self, window: &MainWindow) {
        let (Some(rendering_context), Some(device), Some(visible_tab_id)) = (
            self.rendering_context.as_ref(),
            self.wgpu_device.as_ref(),
            self.visible_tab_id,
        ) else {
            return;
        };
        let Some(active_web_view) = self.web_views.get(&visible_tab_id) else {
            return;
        };
        if !active_web_view.paint_if_available() {
            return;
        }

        match rendering_context.current_frame_as_image(device) {
            Ok(image) => {
                window.set_web_view_texture(image);
                // tabs: the new tab's frame is up
                self.tab_switch_frame.frame_shown();
            }
            Err(error) => {
                if self.frame_import_errors.first(&error) {
                    log::error!("failed to import Servo frame into Slint: {error}");
                    window.set_content_message(
                        format!("Failed to display Servo frame: {error}").into(),
                    );
                }
            }
        }
    }

    /// Slint is giving up its device (the window is suspended or its renderer destroyed). The
    /// runtime stays, so this never starts Servo's shutdown from inside the renderer; the web
    /// views close while the context and the device they paint through are still alive.
    ///
    /// Nothing here may call into the window. Slint runs this notifier from
    /// `WinitWindowAdapter::suspend`, which holds the adapter's window cell mutably borrowed;
    /// setting a property marks the window dirty, and its redraw request borrows that cell
    /// again and panics unless a redraw is already pending
    /// (i-slint-backend-winit-1.17.1/winitwindowadapter.rs:628-632, 1282-1288). The texture
    /// imported from the old device is released by the next `set_up_rendering`, before
    /// anything renders with the new device, or by `shut_down`.
    fn tear_down_rendering(&mut self) {
        release_in_order(
            EngineParts {
                web_views: (
                    std::mem::take(&mut self.web_views),
                    std::mem::take(&mut self.event_receivers),
                ),
                runtime: None::<ServoRuntime>,
                rendering_context: self.rendering_context.take(),
                wgpu_device: self.wgpu_device.take(),
            },
            drop,
        );
        self.visible_tab_id = None;
        self.current_viewport_size = None;
        self.current_scale_factor = None;
    }

    /// Closes the web views, then the runtime, then the rendering context, then the device.
    /// The order is spelled out in `release_in_order` rather than left to field order.
    fn shut_down(mut self, window: Option<&MainWindow>, exit_code_if_stuck: u8) {
        if let Some(window) = window {
            // The window's texture was imported from Servo's frames; let it go first. Safe
            // here: the event loop has ended, so Slint holds nothing borrowed.
            window.set_web_view_texture(Image::default());
        }
        let settle = settle_period(self.web_view_created);

        release_in_order(self.take_parts(), |runtime| {
            let watchdog = Watchdog::start(ENGINE_SHUTDOWN_TIMEOUT, move || {
                crate::shutdown::exit_because_stuck("Servo did not shut down", exit_code_if_stuck)
            });
            // Let content processes release their WebGL contexts before the constellation is
            // told to exit (see `WEBGL_SETTLE_PERIOD`); dropping the last `Servo` handle then
            // exits the constellation and the WebGL thread and waits for them.
            if !shut_down_runtime(runtime, settle, ServoRuntime::spin_event_loop, drop) {
                log::error!("the engine did not shut down cleanly (it panicked while exiting)");
            }
            watchdog.disarm();
        });
        // `self` is dropped here with its parts already taken; its `Drop` finds nothing.
    }

    /// Takes the parts that must be released in order, leaving the state without them.
    fn take_parts(&mut self) -> OwnedEngineParts {
        EngineParts {
            web_views: (
                std::mem::take(&mut self.web_views),
                std::mem::take(&mut self.event_receivers),
            ),
            runtime: self.runtime.take(),
            rendering_context: self.rendering_context.take(),
            wgpu_device: self.wgpu_device.take(),
        }
    }
}

impl Drop for EngineState {
    /// Finds the parts still here only when the state was not shut down: an early return during
    /// start-up, or unwinding. There is no settle period and no watchdog on these paths.
    fn drop(&mut self) {
        release_in_order(self.take_parts(), |runtime| {
            if !drop_catching_panic(runtime) {
                log::error!("the engine did not shut down cleanly (it panicked while exiting)");
            }
        });
    }
}

/// What the engine does with a tab's web view during a pump.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WebViewPlan {
    Keep,
    Create {
        url: String,
    },
    /// The tab's page was replaced before it committed: discard the web view, start afresh.
    Replace {
        url: String,
    },
}

/// `initial_url` is what `BrowserController::take_initial_url` returned for the tab.
pub(crate) fn plan_web_view(has_web_view: bool, initial_url: Option<String>) -> WebViewPlan {
    match (has_web_view, initial_url) {
        (false, initial_url) => WebViewPlan::Create {
            url: initial_url.unwrap_or_else(|| NEW_TAB_URL.to_owned()),
        },
        (true, Some(url)) => WebViewPlan::Replace { url },
        (true, None) => WebViewPlan::Keep,
    }
}

pub(crate) fn report_web_view_error(
    window: &MainWindow,
    controller: &RefCell<BrowserController>,
    tab_id: TabId,
    error: &ServoWebViewError,
) {
    controller.borrow_mut().handle_event(
        tab_id,
        BrowserEvent::LoadStateChanged {
            state: LoadState::Failed {
                message: error.to_string(),
            },
        },
    );
    sync_ui(window, &controller.borrow());
}

pub(crate) fn apply_pending_browser_events(
    controller: &RefCell<BrowserController>,
    receiver: &Receiver<TabEvent>,
) -> bool {
    let mut state_changed = false;
    let mut controller = controller.borrow_mut();

    for tab_event in receiver.try_iter() {
        let tab_id = tab_event.tab_id();
        let event = tab_event.into_event();
        if event == BrowserEvent::CloseRequested {
            // tabs closed by pages: a page never quits the browser
            state_changed |= crate::page_tabs::close_page_tab(&mut controller, tab_id);
            continue;
        }
        let was_engine_ready = is_engine_ready(&controller, tab_id);
        state_changed |= controller.handle_event(tab_id, event);
        if !was_engine_ready && is_engine_ready(&controller, tab_id) {
            log::info!("tab {} is ready for commands", tab_id.value());
        }
    }

    state_changed
}

/// A page that has just been shown or has just become ready under a resting pointer gets one
/// pointer move at the pointer's position, so the engine hit-tests (cursor, status text,
/// hover) without waiting for the user to move.
pub(crate) fn resting_pointer_move(
    active_tab_changed: bool,
    became_ready: bool,
    pointer_over_web_view: bool,
    x: f32,
    y: f32,
) -> Option<BrowserCommand> {
    ((active_tab_changed || became_ready) && pointer_over_web_view)
        .then_some(BrowserCommand::PointerMoved { x, y })
}

/// The tab being switched away from: its page is told the pointer left (clearing hover state,
/// cursor and status text), and a fullscreen page leaves fullscreen.
pub(crate) fn queue_leaving_tab(controller: &mut BrowserController, tab_id: TabId) {
    controller.queue_command_for(tab_id, BrowserCommand::PointerLeft);
    // controls: a picker or menu left open in the tab gets the engine's default answer.
    controller.dismiss_control(tab_id);
    let is_fullscreen = controller
        .tabs()
        .any(|tab| tab.id() == tab_id && tab.state().is_fullscreen());
    if is_fullscreen {
        controller.queue_command_for(tab_id, BrowserCommand::ExitFullscreen);
    }
}

fn tab_zoom_percent(controller: &BrowserController, tab_id: TabId) -> u16 {
    controller
        .tabs()
        .find(|tab| tab.id() == tab_id)
        .map_or(DEFAULT_ZOOM_PERCENT, |tab| tab.state().zoom_percent())
}

fn is_engine_ready(controller: &BrowserController, tab_id: TabId) -> bool {
    controller
        .tabs()
        .any(|tab| tab.id() == tab_id && tab.is_engine_ready())
}

pub(crate) fn dispatch_pending_commands<W: TalariaWebView>(
    controller: &RefCell<BrowserController>,
    web_views: &mut HashMap<TabId, W>,
) -> Result<bool, (TabId, W::Error)> {
    for _ in 0..MAX_COMMANDS_PER_FRAME {
        let queued = { controller.borrow_mut().next_command() };
        let Some(queued) = queued else {
            return Ok(false);
        };
        let tab_id = queued.tab_id();
        let Some(web_view) = web_views.get_mut(&tab_id) else {
            continue;
        };

        dispatch_command(web_view, queued.into_command()).map_err(|error| (tab_id, error))?;
    }

    Ok(controller.borrow().has_pending_commands())
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use talaria_core::{
        ColorScheme, ImeInput, KeyboardInput, PointerButton, PointerButtonAction, TabCommand,
    };
    use talaria_core::{PageRequestAnswer, PageRequestId};

    use super::*;

    #[derive(Default)]
    struct CountingWebView {
        reload_count: usize,
    }

    impl TalariaWebView for CountingWebView {
        type Error = Infallible;

        fn navigate(&mut self, _url: &str) -> Result<(), Self::Error> {
            Ok(())
        }
        fn go_back(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn go_forward(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn reload(&mut self) -> Result<(), Self::Error> {
            self.reload_count += 1;
            Ok(())
        }
        fn pointer_moved(&mut self, _x: f32, _y: f32) -> Result<(), Self::Error> {
            Ok(())
        }
        fn pointer_button(
            &mut self,
            _action: PointerButtonAction,
            _button: PointerButton,
            _x: f32,
            _y: f32,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        fn scroll(
            &mut self,
            _delta_x: f32,
            _delta_y: f32,
            _x: f32,
            _y: f32,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        fn set_focus(&mut self, _focused: bool) -> Result<(), Self::Error> {
            Ok(())
        }
        fn keyboard_input(&mut self, _event: KeyboardInput) -> Result<(), Self::Error> {
            Ok(())
        }
        fn ime_input(&mut self, _event: ImeInput) -> Result<(), Self::Error> {
            Ok(())
        }
        fn set_page_zoom(&mut self, _percent: u16) -> Result<(), Self::Error> {
            Ok(())
        }
        fn pinch_zoom(&mut self, _delta: f32, _x: f32, _y: f32) -> Result<(), Self::Error> {
            Ok(())
        }
        fn set_color_scheme(&mut self, _scheme: ColorScheme) -> Result<(), Self::Error> {
            Ok(())
        }
        fn exit_fullscreen(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn pointer_left(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn answer_control(
            &mut self,
            _id: talaria_core::ControlId,
            _answer: talaria_core::ControlAnswer,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        // dialogs
        fn answer_page_request(
            &mut self,
            _id: PageRequestId,
            _answer: PageRequestAnswer,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        // menus
        fn clipboard(
            &mut self,
            _command: talaria_core::ClipboardCommand,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn mark_engine_ready(controller: &RefCell<BrowserController>, tab_id: TabId) {
        let handled = controller
            .borrow_mut()
            .handle_event(tab_id, BrowserEvent::AddressChanged { url: String::new() });
        assert!(handled);
    }

    #[test]
    fn switching_away_from_a_fullscreen_tab_leaves_fullscreen() {
        let mut controller = BrowserController::default();
        let tab_id = controller.active_tab_id();
        controller.handle_event(tab_id, BrowserEvent::AddressChanged { url: String::new() });
        controller.handle_event(tab_id, BrowserEvent::FullscreenChanged { fullscreen: true });
        controller.open_tab();

        queue_leaving_tab(&mut controller, tab_id);

        let queued = std::iter::from_fn(|| controller.next_command()).collect::<Vec<_>>();
        assert_eq!(
            queued,
            vec![
                TabCommand::new(tab_id, BrowserCommand::PointerLeft),
                TabCommand::new(tab_id, BrowserCommand::ExitFullscreen),
            ]
        );
    }

    #[test]
    fn switching_away_from_a_normal_tab_only_reports_the_pointer_leaving() {
        let mut controller = BrowserController::default();
        let tab_id = controller.active_tab_id();
        controller.handle_event(tab_id, BrowserEvent::AddressChanged { url: String::new() });

        queue_leaving_tab(&mut controller, tab_id);

        assert_eq!(
            controller.next_command(),
            Some(TabCommand::new(tab_id, BrowserCommand::PointerLeft))
        );
        assert_eq!(controller.next_command(), None);
    }

    #[test]
    fn a_resting_pointer_is_replayed_only_for_a_newly_shown_or_ready_page_under_it() {
        assert_eq!(
            resting_pointer_move(true, false, true, 12.0, 34.0),
            Some(BrowserCommand::PointerMoved { x: 12.0, y: 34.0 })
        );
        assert_eq!(
            resting_pointer_move(false, true, true, 1.0, 2.0),
            Some(BrowserCommand::PointerMoved { x: 1.0, y: 2.0 })
        );
        assert_eq!(resting_pointer_move(true, true, false, 1.0, 2.0), None);
        assert_eq!(resting_pointer_move(false, false, true, 1.0, 2.0), None);
    }

    #[test]
    fn a_tab_without_a_web_view_gets_one_at_its_initial_url_or_blank() {
        assert_eq!(
            plan_web_view(false, Some("https://a.test/".to_owned())),
            WebViewPlan::Create {
                url: "https://a.test/".to_owned()
            }
        );
        assert_eq!(
            plan_web_view(false, None),
            WebViewPlan::Create {
                url: NEW_TAB_URL.to_owned()
            }
        );
    }

    #[test]
    fn a_web_view_is_kept_unless_a_new_initial_url_is_pending() {
        assert_eq!(plan_web_view(true, None), WebViewPlan::Keep);
        assert_eq!(
            plan_web_view(true, Some("https://other.test/".to_owned())),
            WebViewPlan::Replace {
                url: "https://other.test/".to_owned()
            }
        );
    }

    #[test]
    fn pending_browser_events_are_routed_to_their_tabs() {
        let controller = RefCell::new(BrowserController::default());
        let first_tab_id = controller.borrow().active_tab_id();
        let second_tab_id = controller.borrow_mut().open_tab();
        let (sender, receiver) = mpsc::channel();

        sender
            .send(TabEvent::new(
                first_tab_id,
                BrowserEvent::PageTitleChanged {
                    title: Some("First".to_owned()),
                },
            ))
            .expect("the event receiver should be connected");
        sender
            .send(TabEvent::new(
                second_tab_id,
                BrowserEvent::PageTitleChanged {
                    title: Some("Second".to_owned()),
                },
            ))
            .expect("the event receiver should be connected");

        assert!(apply_pending_browser_events(&controller, &receiver));
        let controller = controller.borrow();
        let titles = controller
            .tabs()
            .map(|tab| tab.state().page_title())
            .collect::<Vec<_>>();
        assert_eq!(titles, vec!["First", "Second"]);
    }

    #[test]
    fn a_close_request_from_a_page_closes_its_tab() {
        let controller = RefCell::new(BrowserController::default());
        let opener = controller.borrow().active_tab_id();
        mark_engine_ready(&controller, opener);
        let adopted = controller
            .borrow_mut()
            .adopt_tab(opener, talaria_core::TabActivation::Foreground)
            .expect("the opener exists");
        let (sender, receiver) = mpsc::channel();
        sender
            .send(TabEvent::new(adopted, BrowserEvent::CloseRequested))
            .expect("the event receiver should be connected");

        assert!(apply_pending_browser_events(&controller, &receiver));

        let controller = controller.borrow();
        assert_eq!(controller.tabs().len(), 1);
        assert_eq!(controller.active_tab_id(), opener);
    }

    #[test]
    fn commands_are_dispatched_to_their_own_web_views() {
        let controller = RefCell::new(BrowserController::default());
        let first_tab_id = controller.borrow().active_tab_id();
        mark_engine_ready(&controller, first_tab_id);
        controller
            .borrow_mut()
            .queue_command(BrowserCommand::Reload);
        let second_tab_id = controller.borrow_mut().open_tab();
        mark_engine_ready(&controller, second_tab_id);
        controller
            .borrow_mut()
            .queue_command(BrowserCommand::Reload);
        let mut web_views = HashMap::from([
            (first_tab_id, CountingWebView::default()),
            (second_tab_id, CountingWebView::default()),
        ]);

        let pending = dispatch_pending_commands(&controller, &mut web_views)
            .expect("fake WebViews should not fail");

        assert!(!pending);
        assert_eq!(web_views[&first_tab_id].reload_count, 1);
        assert_eq!(web_views[&second_tab_id].reload_count, 1);
    }

    #[test]
    fn command_dispatch_is_limited_per_frame() {
        let controller = RefCell::new(BrowserController::default());
        let tab_id = controller.borrow().active_tab_id();
        mark_engine_ready(&controller, tab_id);
        let mut web_views = HashMap::from([(tab_id, CountingWebView::default())]);
        for _ in 0..=MAX_COMMANDS_PER_FRAME {
            controller
                .borrow_mut()
                .queue_command(BrowserCommand::Reload);
        }

        assert!(
            dispatch_pending_commands(&controller, &mut web_views)
                .expect("the fake WebView should not fail")
        );
        assert_eq!(web_views[&tab_id].reload_count, MAX_COMMANDS_PER_FRAME);
        assert!(
            !dispatch_pending_commands(&controller, &mut web_views)
                .expect("the fake WebView should not fail")
        );
        assert_eq!(web_views[&tab_id].reload_count, MAX_COMMANDS_PER_FRAME + 1);
    }
}
