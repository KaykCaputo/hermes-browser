use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};

use mercury_ui::MainWindow;
use slint::{ComponentHandle, GraphicsAPI, Image, RenderingState};
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

pub(crate) fn connect_servo_runtime(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
) -> Result<(), slint::SetRenderingNotifierError> {
    let weak_window_for_waker = window.as_weak();
    let event_loop_waker = EventLoopWaker::new(move || {
        let _wake_result = weak_window_for_waker.upgrade_in_event_loop(|window| {
            window.window().request_redraw();
        });
    });

    let runtime = ServoRuntime::new(event_loop_waker);
    let weak_window_for_rendering = window.as_weak();
    let mut rendering_context: Option<Rc<BrowserRenderingContext>> = None;
    let mut wgpu_device: Option<slint::wgpu_29::wgpu::Device> = None;
    let mut web_views = HashMap::<TabId, ServoWebView>::new();
    let mut visible_tab_id: Option<TabId> = None;
    let mut current_viewport_size: Option<ViewportSize> = None;
    let mut current_scale_factor: Option<f32> = None;
    let (browser_event_sender, browser_event_receiver) = mpsc::channel::<TabEvent>();

    window
        .window()
        .set_rendering_notifier(move |state, graphics_api| match state {
            RenderingState::RenderingSetup => {
                if rendering_context.is_some() {
                    return;
                }

                let Some(window) = weak_window_for_rendering.upgrade() else {
                    return;
                };
                window.set_web_view_visible(false);

                let GraphicsAPI::WGPU29 { device, .. } = graphics_api else {
                    window.set_content_message(
                        "The selected Slint renderer does not provide WGPU 29.".into(),
                    );
                    return;
                };
                let window_size = window.window().size();
                let Some(viewport_size) = ViewportSize::new(window_size.width, window_size.height)
                else {
                    window
                        .set_content_message("The OpenGL viewport has invalid dimensions.".into());
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

                rendering_context = Some(Rc::new(context));
                wgpu_device = Some(device.clone());
                window.set_content_message(
                    "Offscreen rendering initialized. Waiting for Servo WebView.".into(),
                );
            }
            RenderingState::BeforeRendering => {
                runtime.spin_event_loop();

                let Some(window) = weak_window_for_rendering.upgrade() else {
                    return;
                };

                if apply_pending_browser_events(&controller, &browser_event_receiver) {
                    sync_ui(&window, &controller.borrow());
                }

                let Some(rendering_context) = rendering_context.as_ref() else {
                    return;
                };
                let scale_factor = window.window().scale_factor();
                let Some(viewport_size) = ViewportSize::from_logical(
                    window.get_web_view_width(),
                    window.get_web_view_height(),
                    scale_factor,
                ) else {
                    window
                        .set_content_message("The WebView viewport has invalid dimensions.".into());
                    return;
                };

                let (active_tab_id, tab_ids) = {
                    let controller = controller.borrow();
                    let active_tab_id = controller.active_tab_id();
                    let tab_ids = controller.tabs().map(|tab| tab.id()).collect::<Vec<_>>();
                    (active_tab_id, tab_ids)
                };
                web_views.retain(|tab_id, _| tab_ids.contains(tab_id));
                for tab_id in tab_ids {
                    if web_views.contains_key(&tab_id) {
                        continue;
                    }

                    let create_result = runtime.create_web_view(
                        rendering_context.clone(),
                        NEW_TAB_URL,
                        scale_factor,
                        tab_id,
                        browser_event_sender.clone(),
                    );

                    match create_result {
                        Ok(created_web_view) => {
                            created_web_view.hide();
                            web_views.insert(tab_id, created_web_view);
                        }
                        Err(error) => {
                            report_web_view_error(&window, &controller, tab_id, &error);
                            return;
                        }
                    }
                }

                let active_tab_changed = visible_tab_id != Some(active_tab_id);
                if active_tab_changed {
                    if let Some(previous_tab_id) = visible_tab_id
                        && let Some(previous_web_view) = web_views.get_mut(&previous_tab_id)
                    {
                        let _focus_result = previous_web_view.set_focus(false);
                        previous_web_view.hide();
                    }

                    if let Some(active_web_view) = web_views.get_mut(&active_tab_id) {
                        // Servo 0.7 couples visibility and throttling: show() resumes the active
                        // pipeline and hide() throttles background tabs.
                        active_web_view.show();
                        let _focus_result = active_web_view.set_focus(web_view_focused.get());
                    }
                    visible_tab_id = Some(active_tab_id);
                }

                if current_scale_factor != Some(scale_factor) {
                    for web_view in web_views.values() {
                        web_view.set_scale_factor(scale_factor);
                    }
                    current_scale_factor = Some(scale_factor);
                }

                if (active_tab_changed || current_viewport_size != Some(viewport_size))
                    && let Some(active_web_view) = web_views.get(&active_tab_id)
                {
                    // Each Servo WebView owns its resize notification. Resizing the shared
                    // context directly would skip Servo's CSS viewport update.
                    active_web_view.resize(viewport_size);
                    current_viewport_size = Some(viewport_size);
                }

                match dispatch_pending_commands(&controller, &mut web_views) {
                    Ok(has_pending_commands) => {
                        if has_pending_commands {
                            window.window().request_redraw();
                        }
                    }
                    Err((tab_id, error)) => {
                        report_web_view_error(&window, &controller, tab_id, &error);
                        return;
                    }
                }

                if let Some(active_web_view) = web_views.get(&active_tab_id) {
                    let painted = active_web_view.paint_if_available();

                    if painted {
                        let Some(device) = wgpu_device.as_ref() else {
                            return;
                        };
                        match rendering_context.current_frame_as_image(device) {
                            Ok(image) => window.set_web_view_texture(image),
                            Err(error) => {
                                log::error!("failed to import Servo frame into Slint: {error}");
                                window.set_content_message(
                                    format!("Failed to display Servo frame: {error}").into(),
                                );
                            }
                        }
                    }
                }
            }
            RenderingState::RenderingTeardown => {
                if let Some(window) = weak_window_for_rendering.upgrade() {
                    window.set_web_view_texture(Image::default());
                }
                web_views.clear();
                drop(rendering_context.take());
                drop(wgpu_device.take());
                visible_tab_id = None;
                current_viewport_size = None;
                current_scale_factor = None;
            }
            _ => {}
        })
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
        controller
            .borrow_mut()
            .queue_command(BrowserCommand::Reload);
        let second_tab_id = controller.borrow_mut().open_tab();
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
