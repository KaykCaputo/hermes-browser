use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use mercury_ui::MainWindow;
use slint::{ComponentHandle, GraphicsAPI, Image, RenderingState, Timer};
use talaria_core::{
    BrowserController, BrowserEvent, LoadState, TabEvent, TabId, WebView as TalariaWebView,
    dispatch_command,
};
use talaria_servo::{
    EventLoopWaker, NEW_TAB_URL, ServoRuntime, ServoWebView, ServoWebViewError, ViewportSize,
};

use crate::rendering::BrowserRenderingContext;
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
    state: RefCell<EngineState>,
    bootstrap_failure_logged: Cell<bool>,
}

struct EngineState {
    runtime: ServoRuntime,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
    rendering_setup_seen: bool,
    rendering_context: Option<Rc<BrowserRenderingContext>>,
    wgpu_device: Option<slint::wgpu_29::wgpu::Device>,
    web_views: HashMap<TabId, ServoWebView>,
    visible_tab_id: Option<TabId>,
    current_viewport_size: Option<ViewportSize>,
    current_scale_factor: Option<f32>,
    browser_event_sender: Sender<TabEvent>,
    browser_event_receiver: Receiver<TabEvent>,
}

pub(crate) fn connect_servo_runtime(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
) -> Result<(), slint::SetRenderingNotifierError> {
    let event_loop_waker = EventLoopWaker::new(|| {
        let _queued = slint::invoke_from_event_loop(pump_after_wake);
    });
    let (browser_event_sender, browser_event_receiver) = mpsc::channel::<TabEvent>();

    let engine = Rc::new(Engine {
        window: window.as_weak(),
        state: RefCell::new(EngineState {
            runtime: ServoRuntime::new(event_loop_waker),
            controller,
            web_view_focused,
            rendering_setup_seen: false,
            rendering_context: None,
            wgpu_device: None,
            web_views: HashMap::new(),
            visible_tab_id: None,
            current_viewport_size: None,
            current_scale_factor: None,
            browser_event_sender,
            browser_event_receiver,
        }),
        bootstrap_failure_logged: Cell::new(false),
    });
    ENGINE.with(|current| *current.borrow_mut() = Rc::downgrade(&engine));
    schedule_graphics_bootstrap(Rc::downgrade(&engine), GRAPHICS_BOOTSTRAP_DELAY);

    let rendering_engine = Rc::clone(&engine);
    window
        .window()
        .set_rendering_notifier(move |state, graphics_api| {
            rendering_engine.on_rendering_state(state, graphics_api);
        })
}

fn pump_after_wake() {
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
        let Some(strong_engine) = engine.upgrade() else {
            return;
        };
        if strong_engine.rendering_setup_seen() {
            return;
        }
        let Some(window) = strong_engine.window.upgrade() else {
            return;
        };

        log::info!("the window has not rendered yet; forcing an offscreen render");
        // Never called while the engine state is borrowed: this re-enters the notifier.
        let snapshot_result = window.window().take_snapshot();
        if strong_engine.rendering_setup_seen() {
            return;
        }

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
        schedule_graphics_bootstrap(engine, GRAPHICS_BOOTSTRAP_RETRY_INTERVAL);
    });
}

impl Engine {
    fn rendering_setup_seen(&self) -> bool {
        self.state
            .try_borrow()
            .is_ok_and(|state| state.rendering_setup_seen)
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
        state.pump(&window);
    }

    fn on_rendering_state(&self, rendering_state: RenderingState, graphics_api: &GraphicsAPI<'_>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let Ok(mut state) = self.state.try_borrow_mut() else {
            log::warn!("skipping {rendering_state:?}: the engine is busy");
            return;
        };

        match rendering_state {
            RenderingState::RenderingSetup => state.set_up_rendering(&window, graphics_api),
            RenderingState::BeforeRendering => {
                state.pump(&window);
                state.present(&window);
            }
            RenderingState::RenderingTeardown => state.tear_down_rendering(&window),
            _ => {}
        }
    }
}

impl EngineState {
    fn set_up_rendering(&mut self, window: &MainWindow, graphics_api: &GraphicsAPI<'_>) {
        self.rendering_setup_seen = true;
        if self.rendering_context.is_some() {
            return;
        }

        window.set_web_view_visible(false);

        let GraphicsAPI::WGPU29 { device, .. } = graphics_api else {
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

        let context = match BrowserRenderingContext::new(viewport_size, device) {
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
        window.set_content_message(
            "Offscreen rendering initialized. Waiting for Servo WebView.".into(),
        );
    }

    /// Everything that does not need a frame: Servo's event loop, browser events, web-view
    /// lifetimes, tab visibility, sizing and command dispatch.
    fn pump(&mut self, window: &MainWindow) {
        self.runtime.spin_event_loop();

        if apply_pending_browser_events(&self.controller, &self.browser_event_receiver) {
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
        self.web_views.retain(|tab_id, _| tab_ids.contains(tab_id));
        for tab_id in tab_ids {
            if self.web_views.contains_key(&tab_id) {
                continue;
            }

            let create_result = self.runtime.create_web_view(
                rendering_context.clone(),
                NEW_TAB_URL,
                scale_factor,
                tab_id,
                self.browser_event_sender.clone(),
            );

            match create_result {
                Ok(created_web_view) => {
                    created_web_view.hide();
                    self.web_views.insert(tab_id, created_web_view);
                }
                Err(error) => {
                    report_web_view_error(window, &self.controller, tab_id, &error);
                    return;
                }
            }
        }

        let active_tab_changed = self.visible_tab_id != Some(active_tab_id);
        if active_tab_changed {
            if let Some(previous_tab_id) = self.visible_tab_id
                && let Some(previous_web_view) = self.web_views.get_mut(&previous_tab_id)
            {
                let _focus_result = previous_web_view.set_focus(false);
                previous_web_view.hide();
            }

            if let Some(active_web_view) = self.web_views.get_mut(&active_tab_id) {
                // Servo 0.7 couples visibility and throttling: show() resumes the active
                // pipeline and hide() throttles background tabs.
                active_web_view.show();
                let _focus_result = active_web_view.set_focus(self.web_view_focused.get());
            }
            self.visible_tab_id = Some(active_tab_id);
        }

        if self.current_scale_factor != Some(scale_factor) {
            for web_view in self.web_views.values() {
                web_view.set_scale_factor(scale_factor);
            }
            self.current_scale_factor = Some(scale_factor);
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
                    window.window().request_redraw();
                }
            }
            Err((tab_id, error)) => {
                report_web_view_error(window, &self.controller, tab_id, &error);
            }
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
            Ok(image) => window.set_web_view_texture(image),
            Err(error) => {
                log::error!("failed to import Servo frame into Slint: {error}");
                window
                    .set_content_message(format!("Failed to display Servo frame: {error}").into());
            }
        }
    }

    fn tear_down_rendering(&mut self, window: &MainWindow) {
        window.set_web_view_texture(Image::default());
        self.web_views.clear();
        drop(self.rendering_context.take());
        drop(self.wgpu_device.take());
        self.visible_tab_id = None;
        self.current_viewport_size = None;
        self.current_scale_factor = None;
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
        state_changed |= controller.handle_event(tab_id, tab_event.into_event());
    }

    state_changed
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
        BrowserCommand, ImeInput, KeyboardInput, PointerButton, PointerButtonAction,
    };

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
    }

    fn mark_engine_ready(controller: &RefCell<BrowserController>, tab_id: TabId) {
        let handled = controller
            .borrow_mut()
            .handle_event(tab_id, BrowserEvent::AddressChanged { url: String::new() });
        assert!(handled);
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
