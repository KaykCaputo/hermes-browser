use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};

use hermes_ui::{
    AddressSuggestion, BookmarkTile, BrowserTab, ChromeTheme, MainWindow, WebViewPointerButton,
};
use rendering::BrowserRenderingContext;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{
    BackendSelector, ComponentHandle, GraphicsAPI, Image, Model, ModelRc, RenderingState,
    Rgba8Pixel, SharedPixelBuffer, VecModel,
};
use talaria_core::{
    BrowserCommand, BrowserController, BrowserEvent, CompositionState, ImeInput, KeyLocation,
    KeyState, KeyboardInput, KeyboardModifiers, LoadState, PageIcon, PointerButton,
    PointerButtonAction, TabEvent, TabId, WebView as TalariaWebView, dispatch_command,
};
use talaria_servo::{
    EventLoopWaker, NEW_TAB_URL, ServoRuntime, ServoWebView, ServoWebViewError, ViewportSize,
    run_content_process_if_requested,
};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::keyboard::{Key, KeyLocation as WinitKeyLocation, ModifiersState, PhysicalKey};

const MAX_COMMANDS_PER_FRAME: usize = 128;

mod profile;
mod rendering;

fn main() -> Result<(), Box<dyn Error>> {
    if run_content_process_if_requested()? {
        return Ok(());
    }

    use_automatic_gpu_selection();
    select_hardware_accelerated_backend()?;

    let window = MainWindow::new()?;
    let temporary = std::env::args()
        .skip(1)
        .any(|arg| arg == "--temporary-profile");
    let profile = profile::profile_directory()
        .filter(|_| !temporary)
        .and_then(|directory| {
            profile::Profile::open(&directory)
                .map_err(|error| {
                    eprintln!("Session persistence unavailable: {error}");
                })
                .ok()
        });
    let mut initial = profile
        .as_ref()
        .map(|profile| profile.data.restore())
        .unwrap_or_default();
    for address in std::env::args()
        .skip(1)
        .filter(|arg| arg != "--temporary-profile")
    {
        let target = talaria_core::resolve_address(&address)?;
        if initial.tabs().len() > 1 || !initial.state().url().is_empty() {
            initial.open_tab();
        }
        initial.queue_navigation(target.as_str())?;
    }
    let controller = Rc::new(RefCell::new(initial));
    let web_view_focused = Rc::new(Cell::new(false));

    let shield_exceptions = Rc::new(RefCell::new(
        profile
            .as_ref()
            .map(|profile| profile.data.shield_exceptions.iter().cloned().collect())
            .unwrap_or_default(),
    ));
    connect_servo_runtime(
        &window,
        Rc::clone(&controller),
        Rc::clone(&web_view_focused),
        profile
            .as_ref()
            .map(|profile| profile.directory().to_owned()),
        Rc::clone(&shield_exceptions),
    )?;
    sync_ui(&window, &controller.borrow());
    connect_ui_callbacks(
        &window,
        Rc::clone(&controller),
        Rc::clone(&web_view_focused),
    );
    let profile = Rc::new(RefCell::new(profile));
    connect_bookmarks(&window, &controller, &profile);
    connect_appearance(&window, &profile);
    connect_tab_layout(&window, &profile);
    connect_shields(&window, &controller, &profile, &shield_exceptions);
    connect_native_keyboard_input(&window, Rc::clone(&controller), web_view_focused);

    window.run()?;
    drop(window);
    if let Some(profile) = profile.borrow_mut().as_mut() {
        profile.data.remember_tabs(&controller.borrow());
        profile.save()?;
    }
    Ok(())
}

fn connect_bookmarks(
    window: &MainWindow,
    controller: &Rc<RefCell<BrowserController>>,
    profile: &Rc<RefCell<Option<profile::Profile>>>,
) {
    if let Some(profile) = profile.borrow().as_ref() {
        window.set_profile_available(true);
        set_bookmarks(window, &profile.data.bookmarks);
    }
    let weak = window.as_weak();
    let controller = Rc::clone(controller);
    let profile = Rc::clone(profile);
    window.on_toggle_bookmark(move || {
        let Some(window) = weak.upgrade() else { return };
        let mut borrowed = profile.borrow_mut();
        let Some(profile) = borrowed.as_mut() else {
            return;
        };
        let controller = controller.borrow();
        let address = controller.state().url();
        if address.is_empty() {
            return;
        }
        let bookmarks = &mut profile.data.bookmarks;
        if let Some(index) = bookmarks.iter().position(|saved| saved == address) {
            bookmarks.remove(index);
        } else {
            bookmarks.push(address.to_owned());
        }
        set_bookmarks(&window, bookmarks);
        profile.data.remember_tabs(&controller);
        if let Err(error) = profile.save() {
            eprintln!("Could not save bookmarks: {error}");
        }
    });
}

fn connect_tab_layout(window: &MainWindow, profile: &Rc<RefCell<Option<profile::Profile>>>) {
    if let Some(profile) = profile.borrow().as_ref() {
        window.set_vertical_tabs(profile.data.vertical_tabs);
        window.set_collapsed_tabs(profile.data.collapsed_tabs);
    }
    let weak = window.as_weak();
    let profile = Rc::clone(profile);
    window.on_tab_layout_changed(move |vertical, collapsed| {
        let Some(window) = weak.upgrade() else { return };
        window.set_vertical_tabs(vertical);
        window.set_collapsed_tabs(collapsed);
        window.invoke_reveal_active_tab();
        if let Some(profile) = profile.borrow_mut().as_mut() {
            profile.data.vertical_tabs = vertical;
            profile.data.collapsed_tabs = collapsed;
            if let Err(error) = profile.save() {
                eprintln!("Could not save tab layout: {error}");
            }
        }
    });
}

fn connect_appearance(window: &MainWindow, profile: &Rc<RefCell<Option<profile::Profile>>>) {
    let weak = window.as_weak();
    window.window().with_winit_window(|native| {
        window
            .global::<ChromeTheme>()
            .set_system_light(native.theme() == Some(winit::window::Theme::Light));
    });
    if let Some(profile) = profile.borrow().as_ref() {
        window
            .global::<ChromeTheme>()
            .set_mode(profile.data.theme.clamp(0, 2));
        window.set_new_tab_gradient(profile.data.new_tab_gradient);
    }
    let profile = Rc::clone(profile);
    window.on_appearance_changed(move |theme, gradient| {
        let Some(window) = weak.upgrade() else { return };
        window.global::<ChromeTheme>().set_mode(theme);
        window.set_new_tab_gradient(gradient);
        if let Some(profile) = profile.borrow_mut().as_mut() {
            profile.data.theme = theme;
            profile.data.new_tab_gradient = gradient;
            if let Err(error) = profile.save() {
                eprintln!("Could not save appearance: {error}");
            }
        }
    });
}

fn connect_shields(
    window: &MainWindow,
    controller: &Rc<RefCell<BrowserController>>,
    profile: &Rc<RefCell<Option<profile::Profile>>>,
    exceptions: &Rc<RefCell<HashSet<String>>>,
) {
    let weak = window.as_weak();
    let exceptions_for_panel = Rc::clone(exceptions);
    window.on_open_site_panel(move || {
        let Some(window) = weak.upgrade() else { return };
        let address = window.get_committed_address();
        let url = talaria_core::resolve_address(address.as_str())
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"));
        let host = url
            .as_ref()
            .and_then(|url| url.host_str())
            .unwrap_or_default();
        window.set_site_host(host.into());
        window.set_site_https(url.as_ref().is_some_and(|url| url.scheme() == "https"));
        window.set_shield_paused(exceptions_for_panel.borrow().contains(host));
    });

    let weak = window.as_weak();
    let exceptions = Rc::clone(exceptions);
    let controller = Rc::clone(controller);
    let profile = Rc::clone(profile);
    window.on_toggle_shield(move || {
        let Some(window) = weak.upgrade() else { return };
        let host = window.get_site_host().to_string();
        let controller_ref = controller.borrow();
        let url = talaria_core::resolve_address(controller_ref.state().url()).ok();
        if host.is_empty() || url.as_ref().and_then(|url| url.host_str()) != Some(host.as_str()) {
            return;
        }
        let mut exceptions = exceptions.borrow_mut();
        if !exceptions.remove(&host) {
            exceptions.insert(host);
        }
        if let Some(profile) = profile.borrow_mut().as_mut() {
            profile.data.shield_exceptions = exceptions.iter().cloned().collect();
            profile.data.remember_tabs(&controller_ref);
            if let Err(error) = profile.save() {
                eprintln!("Could not save site preferences: {error}");
            }
        }
        drop(controller_ref);
        window.invoke_reload();
    });
}

fn sync_bookmark_state(window: &MainWindow) {
    window.set_is_bookmarked(
        window
            .get_bookmarks()
            .iter()
            .any(|bookmark| bookmark.address == window.get_committed_address()),
    );
}

fn set_bookmarks(window: &MainWindow, bookmarks: &[String]) {
    window.set_bookmarks(ModelRc::new(VecModel::from(
        bookmarks
            .iter()
            .map(|address| {
                let parsed = talaria_core::resolve_address(address).ok();
                let label = parsed
                    .as_ref()
                    .and_then(|url| url.host_str())
                    .unwrap_or(address);
                let label = label.strip_prefix("www.").unwrap_or(label);
                BookmarkTile {
                    address: address.as_str().into(),
                    label: label.into(),
                    initial: label
                        .chars()
                        .next()
                        .map(|letter| letter.to_uppercase().to_string())
                        .unwrap_or_default()
                        .into(),
                }
            })
            .collect::<Vec<_>>(),
    )));
    sync_bookmark_state(window);
}

fn use_automatic_gpu_selection() {
    #[cfg(target_os = "linux")]
    {
        // DRI_PRIME is a manual Mesa device override. Inheriting a stale value
        // such as `1` on a single-GPU machine causes every GL context to try a
        // nonexistent second device before falling back to the default GPU.
        //
        // SAFETY: This is the first operation in main, before Slint, Servo, or
        // GStreamer can create worker threads or read the process environment.
        unsafe {
            std::env::remove_var("DRI_PRIME");
        }
    }
}

fn select_hardware_accelerated_backend() -> Result<(), slint::platform::PlatformError> {
    #[cfg(target_os = "linux")]
    let settings = {
        let mut settings = slint::wgpu_29::WGPUSettings::default();
        // Linux texture sharing uses Vulkan external memory. Requiring Vulkan here keeps
        // Slint and Servo on one physical GPU and makes the transfer deterministic.
        settings.backends = slint::wgpu_29::wgpu::Backends::VULKAN;
        settings
    };
    #[cfg(not(target_os = "linux"))]
    let settings = slint::wgpu_29::WGPUSettings::default();

    BackendSelector::new()
        .backend_name("winit".to_owned())
        .require_wgpu_29(slint::wgpu_29::WGPUConfiguration::Automatic(settings))
        .select()
}

fn connect_servo_runtime(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
    profile_directory: Option<std::path::PathBuf>,
    shield_exceptions: Rc<RefCell<HashSet<String>>>,
) -> Result<(), slint::SetRenderingNotifierError> {
    let weak_window_for_waker = window.as_weak();
    let event_loop_waker = EventLoopWaker::new(move || {
        let _wake_result = weak_window_for_waker.upgrade_in_event_loop(|window| {
            window.window().request_redraw();
        });
    });

    let mut runtime = None;
    let mut applied_exceptions = HashSet::new();
    let mut last_tab_revision = None;
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
                wgpu_device = Some(device.clone());
            }
            RenderingState::BeforeRendering => {
                if runtime.is_none() {
                    let tabs = controller.borrow();
                    if tabs.state().url().is_empty() && tabs.pending_navigations().next().is_none()
                    {
                        return;
                    }
                }
                let runtime = runtime.get_or_insert_with(|| {
                    ServoRuntime::with_profile(event_loop_waker.clone(), profile_directory.clone())
                });
                if *shield_exceptions.borrow() != applied_exceptions {
                    applied_exceptions.clone_from(&shield_exceptions.borrow());
                    runtime.set_shield_exceptions(applied_exceptions.clone());
                }
                runtime.spin_event_loop();

                let Some(window) = weak_window_for_rendering.upgrade() else {
                    return;
                };

                if apply_pending_browser_events(&controller, &browser_event_receiver) {
                    sync_ui(&window, &controller.borrow());
                }

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

                if rendering_context.is_none() {
                    let Some(device) = wgpu_device.as_ref() else {
                        return;
                    };
                    match BrowserRenderingContext::new(viewport_size, device) {
                        Ok(context) => rendering_context = Some(Rc::new(context)),
                        Err(error) => {
                            window.set_web_view_visible(false);
                            window.set_content_message(
                                format!("Failed to create Servo's rendering context: {error}")
                                    .into(),
                            );
                            return;
                        }
                    }
                }
                let Some(rendering_context) = rendering_context.as_ref() else {
                    return;
                };
                while let Some(request) = runtime.next_tab_request() {
                    match request {
                        talaria_servo::TabRequest::Open(opener, request) => {
                            if controller.borrow().active_tab_id() != opener {
                                continue;
                            }
                            let tab_id = controller.borrow_mut().open_tab();
                            let view = runtime.create_auxiliary_web_view(
                                request,
                                rendering_context.clone(),
                                scale_factor,
                                tab_id,
                                browser_event_sender.clone(),
                            );
                            web_views.insert(tab_id, view);
                        }
                        talaria_servo::TabRequest::Close(tab_id) => {
                            controller.borrow_mut().close_tab(tab_id);
                        }
                    }
                    sync_ui(&window, &controller.borrow());
                }
                let (active_tab_id, missing_views) = {
                    let tabs = controller.borrow();
                    let active = tabs.active_tab_id();
                    if last_tab_revision != Some(tabs.tab_revision()) {
                        if !web_views.is_empty() {
                            let live: HashSet<_> = tabs.tabs().map(|tab| tab.id()).collect();
                            web_views.retain(|id, _| live.contains(id));
                        }
                        last_tab_revision = Some(tabs.tab_revision());
                    }
                    let mut missing = HashMap::new();
                    if web_views.len() < tabs.tabs().len() {
                        for id in tabs
                            .pending_navigations()
                            .filter(|id| !web_views.contains_key(id))
                        {
                            missing.entry(id).or_insert_with(|| NEW_TAB_URL.to_owned());
                        }
                        if !web_views.contains_key(&active) && !tabs.state().url().is_empty() {
                            missing
                                .entry(active)
                                .or_insert_with(|| tabs.state().url().to_owned());
                        }
                    }
                    (active, missing)
                };
                let active_view_created = missing_views.contains_key(&active_tab_id);
                for (tab_id, fallback_url) in missing_views {
                    let initial_url = controller
                        .borrow_mut()
                        .take_initial_navigation(tab_id)
                        .unwrap_or(fallback_url);
                    let create_result = runtime.create_web_view(
                        rendering_context.clone(),
                        &initial_url,
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

                let active_tab_changed =
                    active_view_created || visible_tab_id != Some(active_tab_id);
                if active_tab_changed {
                    window.set_web_view_texture(Image::default());
                    if let Some(previous_tab_id) = visible_tab_id
                        && let Some(previous_web_view) = web_views.get_mut(&previous_tab_id)
                    {
                        let _focus_result = previous_web_view.set_focus(false);
                        previous_web_view.hide();
                    }

                    if let Some(active_web_view) = web_views.get_mut(&active_tab_id) {
                        // Servo couples visibility and throttling: show() resumes the active
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

fn report_web_view_error(
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

fn apply_pending_browser_events(
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

fn dispatch_pending_commands<W: TalariaWebView>(
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

fn sync_ui(window: &MainWindow, controller: &BrowserController) {
    let state = controller.active_state();
    if window.get_page_fullscreen() != state.fullscreen() {
        window.set_page_fullscreen(state.fullscreen());
        window
            .window()
            .set_fullscreen(state.fullscreen() || window.get_user_fullscreen());
    }
    window.set_page_title(state.page_title().into());
    if window.get_committed_address() != state.url()
        || window.get_displayed_tab() != controller.active_tab_id().value().to_string()
    {
        window.invoke_address_edited("".into());
        window.set_address(state.url().into());
        window.set_committed_address(state.url().into());
        window.set_displayed_tab(controller.active_tab_id().value().to_string().into());
        sync_bookmark_state(window);
    }
    window.set_is_new_tab(state.url().is_empty());
    window.set_blocked_requests(i32::try_from(state.blocked_requests()).unwrap_or(i32::MAX));
    window.set_can_go_back(state.can_go_back());
    window.set_can_go_forward(state.can_go_forward());
    window.set_is_loading(matches!(state.load_state(), LoadState::Loading));
    window.set_web_view_visible(
        !state.url().is_empty() && !matches!(state.load_state(), LoadState::Failed { .. }),
    );

    let active_tab_id = controller.active_tab_id();
    let mut active_tab_index = 0;
    let tabs = controller
        .tabs()
        .enumerate()
        .map(|(index, tab)| {
            let state = tab.state();
            if tab.id() == active_tab_id {
                active_tab_index = index;
            }
            let (icon, has_icon) = match state.page_icon() {
                Some(icon) => (slint_image_from_page_icon(icon), true),
                None => (Image::default(), false),
            };

            BrowserTab {
                title: state.page_title().into(),
                pinned: tab.pinned(),
                icon,
                has_icon,
                active: tab.id() == active_tab_id,
                loading: matches!(state.load_state(), LoadState::Loading),
            }
        })
        .collect::<Vec<_>>();
    window.set_pinned_tabs(i32::try_from(controller.pinned_tabs()).unwrap_or(i32::MAX));
    window.set_tabs(ModelRc::new(VecModel::from(tabs)));
    window.set_active_tab_index(i32::try_from(active_tab_index).unwrap_or(i32::MAX));
    window.invoke_reveal_active_tab();

    let content_message = match state.load_state() {
        LoadState::Failed { message } => message.as_str(),
        _ if state.url().is_empty() => "",
        LoadState::Idle => "",
        LoadState::Loading => "The page is loading.",
        LoadState::Loaded => "",
    };
    window.set_content_message(content_message.into());
}

fn slint_image_from_page_icon(icon: &PageIcon) -> Image {
    let mut pixel_buffer = SharedPixelBuffer::<Rgba8Pixel>::new(icon.width(), icon.height());
    pixel_buffer.make_mut_bytes().copy_from_slice(icon.rgba());
    Image::from_rgba8(pixel_buffer)
}

fn pointer_button_from_ui(button: WebViewPointerButton) -> PointerButton {
    match button {
        WebViewPointerButton::Left => PointerButton::Left,
        WebViewPointerButton::Middle => PointerButton::Middle,
        WebViewPointerButton::Right => PointerButton::Right,
        WebViewPointerButton::Other => PointerButton::Other,
    }
}

fn connect_ui_callbacks(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
) {
    let navigate_controller = Rc::clone(&controller);
    let navigate_window = window.as_weak();
    window.on_navigate(move |address| {
        let result = navigate_controller
            .borrow_mut()
            .queue_navigation(address.as_str());
        let Some(window) = navigate_window.upgrade() else {
            return;
        };

        let navigated = result.is_ok();
        if let Err(error) = result {
            let tab_id = navigate_controller.borrow().active_tab_id();
            navigate_controller.borrow_mut().handle_event(
                tab_id,
                BrowserEvent::LoadStateChanged {
                    state: LoadState::Failed {
                        message: error.to_string(),
                    },
                },
            );
        }
        sync_ui(&window, &navigate_controller.borrow());
        if navigated {
            window.invoke_focus_web_content();
        }
        window.window().request_redraw();
    });

    let suggestion_controller = Rc::clone(&controller);
    let suggestion_window = window.as_weak();
    window.on_address_edited(move |input| {
        let Some(window) = suggestion_window.upgrade() else {
            return;
        };
        let query = input.trim().to_lowercase();
        let mut results = Vec::new();
        if !query.is_empty() {
            let browser = suggestion_controller.borrow();
            let mut seen = HashSet::new();
            for (index, tab) in browser.tabs().enumerate() {
                let state = tab.state();
                if tab.id() != browser.active_tab_id()
                    && !state.url().is_empty()
                    && (state.url().to_lowercase().contains(&query)
                        || state.page_title().to_lowercase().contains(&query))
                    && seen.insert(state.url().to_owned())
                {
                    results.push(AddressSuggestion {
                        label: format!("Switch to tab · {}", state.page_title()).into(),
                        address: state.url().into(),
                        tab_index: i32::try_from(index).unwrap_or(i32::MAX),
                    });
                }
                if results.len() == 6 {
                    break;
                }
            }
            for bookmark in window.get_bookmarks().iter() {
                if results.len() == 6 {
                    break;
                }
                if bookmark.address.to_lowercase().contains(&query)
                    && seen.insert(bookmark.address.to_string())
                {
                    results.push(AddressSuggestion {
                        label: bookmark.label,
                        address: bookmark.address,
                        tab_index: -1,
                    });
                }
            }
        }
        window.set_selected_suggestion(-1);
        window.set_suggestions(ModelRc::new(VecModel::from(results)));
    });

    let menu_controller = Rc::clone(&controller);
    let menu_window = window.as_weak();
    window.on_browser_action(move |action| {
        use talaria_core::ZoomAction;

        let Some(window) = menu_window.upgrade() else {
            return;
        };
        match action.as_str() {
            "reopen" => {
                if menu_controller.borrow_mut().reopen_closed_tab() {
                    sync_ui(&window, &menu_controller.borrow());
                }
            }
            "fullscreen" => toggle_fullscreen(&window),
            action => {
                let zoom = match action {
                    "zoom-in" => ZoomAction::In,
                    "zoom-out" => ZoomAction::Out,
                    "zoom-reset" => ZoomAction::Reset,
                    _ => return,
                };
                menu_controller
                    .borrow_mut()
                    .queue_command(BrowserCommand::Zoom(zoom));
            }
        }
        window.window().request_redraw();
    });

    let back_controller = Rc::clone(&controller);
    let back_window = window.as_weak();
    window.on_go_back(move || {
        back_controller
            .borrow_mut()
            .queue_command(BrowserCommand::GoBack);
        if let Some(window) = back_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let forward_controller = Rc::clone(&controller);
    let forward_window = window.as_weak();
    window.on_go_forward(move || {
        forward_controller
            .borrow_mut()
            .queue_command(BrowserCommand::GoForward);
        if let Some(window) = forward_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let reload_controller = Rc::clone(&controller);
    let reload_window = window.as_weak();
    window.on_reload(move || {
        reload_controller
            .borrow_mut()
            .queue_command(BrowserCommand::Reload);
        if let Some(window) = reload_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let new_tab_controller = Rc::clone(&controller);
    let new_tab_window = window.as_weak();
    window.on_new_tab(move || {
        new_tab_controller.borrow_mut().open_tab();
        if let Some(window) = new_tab_window.upgrade() {
            sync_ui(&window, &new_tab_controller.borrow());
            window.window().request_redraw();
        }
    });

    let select_tab_controller = Rc::clone(&controller);
    let select_tab_window = window.as_weak();
    window.on_select_tab(move |index| {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if select_tab_controller.borrow_mut().select_tab_at(index)
            && let Some(window) = select_tab_window.upgrade()
        {
            sync_ui(&window, &select_tab_controller.borrow());
            window.window().request_redraw();
        }
    });

    let pin_controller = Rc::clone(&controller);
    let pin_window = window.as_weak();
    window.on_toggle_tab_pin(move |index| {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if pin_controller.borrow_mut().toggle_tab_pin(index)
            && let Some(window) = pin_window.upgrade()
        {
            sync_ui(&window, &pin_controller.borrow());
            window.window().request_redraw();
        }
    });

    let move_tab_controller = Rc::clone(&controller);
    let move_tab_window = window.as_weak();
    window.on_move_tab(move |from, to| {
        let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
            return;
        };
        if move_tab_controller.borrow_mut().move_tab(from, to)
            && let Some(window) = move_tab_window.upgrade()
        {
            sync_ui(&window, &move_tab_controller.borrow());
            window.window().request_redraw();
        }
    });

    let close_tab_controller = Rc::clone(&controller);
    let close_tab_window = window.as_weak();
    window.on_close_tab(move |index| {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if close_tab_controller.borrow_mut().close_tab_at(index)
            && let Some(window) = close_tab_window.upgrade()
        {
            sync_ui(&window, &close_tab_controller.borrow());
            window.window().request_redraw();
        }
    });

    let pointer_moved_controller = Rc::clone(&controller);
    let pointer_moved_window = window.as_weak();
    window.on_web_view_pointer_moved(move |x, y| {
        pointer_moved_controller
            .borrow_mut()
            .queue_command(BrowserCommand::PointerMoved { x, y });
        if let Some(window) = pointer_moved_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let pointer_button_controller = Rc::clone(&controller);
    let pointer_button_window = window.as_weak();
    window.on_web_view_pointer_button(move |button, pressed, x, y| {
        let action = if pressed {
            PointerButtonAction::Down
        } else {
            PointerButtonAction::Up
        };
        pointer_button_controller
            .borrow_mut()
            .queue_command(BrowserCommand::PointerButton {
                action,
                button: pointer_button_from_ui(button),
                x,
                y,
            });
        if let Some(window) = pointer_button_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let scroll_controller = Rc::clone(&controller);
    let scroll_window = window.as_weak();
    window.on_web_view_scrolled(move |delta_x, delta_y, x, y| {
        scroll_controller
            .borrow_mut()
            .queue_command(BrowserCommand::Scrolled {
                delta_x,
                delta_y,
                x,
                y,
            });
        if let Some(window) = scroll_window.upgrade() {
            window.window().request_redraw();
        }
    });

    let focus_controller = controller;
    let focus_window = window.as_weak();
    window.on_web_view_focus_changed(move |focused| {
        web_view_focused.set(focused);
        focus_controller
            .borrow_mut()
            .queue_command(BrowserCommand::FocusChanged { focused });
        if let Some(window) = focus_window.upgrade() {
            let _native_window = window
                .window()
                .with_winit_window(|native_window| native_window.set_ime_allowed(focused));
            window.window().request_redraw();
        }
    });
}

fn connect_native_keyboard_input(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
) {
    let weak_window = window.as_weak();
    let mut modifiers = ModifiersState::empty();
    let mut composition_tab_id: Option<TabId> = None;

    window
        .window()
        .on_winit_window_event(move |_slint_window, event| {
            let Some(window) = weak_window.upgrade() else {
                return EventResult::Propagate;
            };

            match event {
                WindowEvent::ThemeChanged(theme) => {
                    window
                        .global::<ChromeTheme>()
                        .set_system_light(*theme == winit::window::Theme::Light);
                }
                WindowEvent::ModifiersChanged(changed) => {
                    modifiers = changed.state();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    let suggestion_count =
                        i32::try_from(window.get_suggestions().row_count()).unwrap_or(i32::MAX);
                    if window.get_address_focused() && suggestion_count > 0 && modifiers.is_empty()
                    {
                        use winit::keyboard::NamedKey;
                        let selected = window.get_selected_suggestion();
                        let handled = match event.logical_key {
                            Key::Named(NamedKey::ArrowDown) => {
                                if event.state == ElementState::Pressed {
                                    window
                                        .set_selected_suggestion((selected + 1) % suggestion_count);
                                }
                                true
                            }
                            Key::Named(NamedKey::ArrowUp) => {
                                if event.state == ElementState::Pressed {
                                    window.set_selected_suggestion(if selected <= 0 {
                                        suggestion_count - 1
                                    } else {
                                        selected - 1
                                    });
                                }
                                true
                            }
                            Key::Named(NamedKey::Escape) => {
                                if event.state == ElementState::Pressed {
                                    window.invoke_address_edited("".into());
                                }
                                true
                            }
                            _ => false,
                        };
                        if handled {
                            return EventResult::PreventDefault;
                        }
                    }
                    if handle_browser_shortcut(
                        &window,
                        &controller,
                        &web_view_focused,
                        event,
                        modifiers,
                    ) {
                        return EventResult::PreventDefault;
                    }

                    if web_view_focused.get() {
                        let active_tab_id = controller.borrow().active_tab_id();
                        let keyboard_input = keyboard_input_from_winit(
                            event,
                            modifiers,
                            composition_tab_id == Some(active_tab_id),
                        );
                        controller
                            .borrow_mut()
                            .queue_command(BrowserCommand::KeyboardInput(keyboard_input));
                        window.window().request_redraw();
                    }
                }
                WindowEvent::Ime(ime) if web_view_focused.get() || composition_tab_id.is_some() => {
                    queue_ime_input(&controller, &mut composition_tab_id, ime);
                    window.window().request_redraw();
                }
                WindowEvent::Focused(false) => {
                    modifiers = ModifiersState::empty();
                    dismiss_composition(&controller, &mut composition_tab_id);
                }
                _ => {}
            }

            EventResult::Propagate
        });
}

#[derive(Debug, PartialEq)]
enum Shortcut {
    Command(BrowserCommand),
    AddressBar,
    NewTab,
    CloseTab,
    ReopenTab,
    CycleTabs { backwards: bool },
    SelectTab(usize),
    Bookmark,
    Fullscreen,
    Quit,
}

fn browser_shortcut(
    key: &Key,
    modifiers: ModifiersState,
    page_fullscreen: bool,
) -> Option<Shortcut> {
    use talaria_core::ZoomAction;
    use winit::keyboard::NamedKey;

    let primary = (modifiers.control_key() || modifiers.super_key()) && !modifiers.alt_key();
    Some(match key {
        Key::Character(key) if primary => match key.to_ascii_lowercase().as_str() {
            "l" => Shortcut::AddressBar,
            "t" if modifiers.shift_key() => Shortcut::ReopenTab,
            "t" => Shortcut::NewTab,
            "w" => Shortcut::CloseTab,
            "q" => Shortcut::Quit,
            "d" => Shortcut::Bookmark,
            "r" => Shortcut::Command(BrowserCommand::Reload),
            "+" | "=" => Shortcut::Command(BrowserCommand::Zoom(ZoomAction::In)),
            "-" => Shortcut::Command(BrowserCommand::Zoom(ZoomAction::Out)),
            "0" => Shortcut::Command(BrowserCommand::Zoom(ZoomAction::Reset)),
            number if !modifiers.shift_key() => {
                let digit = number
                    .parse::<usize>()
                    .ok()
                    .filter(|digit| (1..=9).contains(digit))?;
                Shortcut::SelectTab(digit)
            }
            _ => return None,
        },
        Key::Named(NamedKey::Tab) if primary => Shortcut::CycleTabs {
            backwards: modifiers.shift_key(),
        },
        Key::Named(NamedKey::F5) => Shortcut::Command(BrowserCommand::Reload),
        Key::Named(NamedKey::F11 | NamedKey::Escape) if page_fullscreen => {
            Shortcut::Command(BrowserCommand::ExitFullscreen)
        }
        Key::Named(NamedKey::F11) => Shortcut::Fullscreen,
        Key::Named(NamedKey::ArrowLeft) if modifiers == ModifiersState::ALT => {
            Shortcut::Command(BrowserCommand::GoBack)
        }
        Key::Named(NamedKey::ArrowRight) if modifiers == ModifiersState::ALT => {
            Shortcut::Command(BrowserCommand::GoForward)
        }
        _ => return None,
    })
}

fn handle_browser_shortcut(
    window: &MainWindow,
    controller: &RefCell<BrowserController>,
    web_view_focused: &Cell<bool>,
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
) -> bool {
    let Some(shortcut) =
        browser_shortcut(&event.logical_key, modifiers, window.get_page_fullscreen())
    else {
        return false;
    };
    if event.state != ElementState::Pressed || event.repeat {
        return true;
    }

    match shortcut {
        Shortcut::Command(command) => controller.borrow_mut().queue_command(command),
        Shortcut::AddressBar | Shortcut::NewTab => {
            if web_view_focused.replace(false) {
                controller
                    .borrow_mut()
                    .queue_command(BrowserCommand::FocusChanged { focused: false });
            }
            if window.get_page_fullscreen() {
                controller
                    .borrow_mut()
                    .queue_command(BrowserCommand::ExitFullscreen);
            }
            if shortcut == Shortcut::NewTab {
                controller.borrow_mut().open_tab();
                sync_ui(window, &controller.borrow());
            }
            window.invoke_focus_address_bar();
        }
        Shortcut::ReopenTab => {
            if controller.borrow_mut().reopen_closed_tab() {
                sync_ui(window, &controller.borrow());
            }
        }
        Shortcut::CloseTab => {
            let active = controller.borrow().active_tab_id();
            controller.borrow_mut().close_tab(active);
            sync_ui(window, &controller.borrow());
        }
        Shortcut::CycleTabs { backwards } => {
            controller.borrow_mut().select_next_tab(backwards);
            sync_ui(window, &controller.borrow());
        }
        Shortcut::SelectTab(digit) => {
            let index = if digit == 9 {
                controller.borrow().tabs().len() - 1
            } else {
                digit - 1
            };
            controller.borrow_mut().select_tab_at(index);
            sync_ui(window, &controller.borrow());
        }
        Shortcut::Bookmark => window.invoke_toggle_bookmark(),
        Shortcut::Fullscreen => toggle_fullscreen(window),
        Shortcut::Quit => {
            let _ = slint::quit_event_loop();
        }
    }
    window.window().request_redraw();
    true
}

fn toggle_fullscreen(window: &MainWindow) {
    let fullscreen = !window.window().is_fullscreen();
    window.set_user_fullscreen(fullscreen);
    window.window().set_fullscreen(fullscreen);
}

fn keyboard_input_from_winit(
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
    is_composing: bool,
) -> KeyboardInput {
    KeyboardInput {
        state: match event.state {
            ElementState::Pressed => KeyState::Down,
            ElementState::Released => KeyState::Up,
        },
        key: logical_key_string(&event.logical_key),
        code: physical_key_string(event.physical_key),
        location: match event.location {
            WinitKeyLocation::Standard => KeyLocation::Standard,
            WinitKeyLocation::Left => KeyLocation::Left,
            WinitKeyLocation::Right => KeyLocation::Right,
            WinitKeyLocation::Numpad => KeyLocation::Numpad,
        },
        modifiers: KeyboardModifiers {
            alt: modifiers.alt_key(),
            control: modifiers.control_key(),
            meta: modifiers.super_key(),
            shift: modifiers.shift_key(),
        },
        repeat: event.repeat,
        is_composing,
    }
}

fn physical_key_string(key: PhysicalKey) -> String {
    match key {
        PhysicalKey::Code(winit::keyboard::KeyCode::SuperLeft) => "MetaLeft".to_owned(),
        PhysicalKey::Code(winit::keyboard::KeyCode::SuperRight) => "MetaRight".to_owned(),
        PhysicalKey::Code(winit::keyboard::KeyCode::Meta) => "Super".to_owned(),
        PhysicalKey::Code(code) => format!("{code:?}"),
        PhysicalKey::Unidentified(_) => "Unidentified".to_owned(),
    }
}

fn logical_key_string(key: &Key) -> String {
    match key {
        Key::Character(character) => character.to_string(),
        Key::Named(winit::keyboard::NamedKey::Space) => " ".to_owned(),
        Key::Named(named) => format!("{named:?}"),
        Key::Dead(_) => "Dead".to_owned(),
        Key::Unidentified(_) => "Unidentified".to_owned(),
    }
}

fn queue_ime_input(
    controller: &RefCell<BrowserController>,
    composition_tab_id: &mut Option<TabId>,
    ime: &Ime,
) {
    match ime {
        Ime::Enabled => {}
        Ime::Preedit(data, _) => {
            let tab_id = *composition_tab_id.get_or_insert_with(|| {
                let tab_id = controller.borrow().active_tab_id();
                controller.borrow_mut().queue_command_for(
                    tab_id,
                    BrowserCommand::ImeInput(ImeInput::Composition {
                        state: CompositionState::Start,
                        data: String::new(),
                    }),
                );
                tab_id
            });
            controller.borrow_mut().queue_command_for(
                tab_id,
                BrowserCommand::ImeInput(ImeInput::Composition {
                    state: CompositionState::Update,
                    data: data.clone(),
                }),
            );
        }
        Ime::Commit(data) => {
            let (tab_id, needs_start) = match composition_tab_id.take() {
                Some(tab_id) => (tab_id, false),
                None => (controller.borrow().active_tab_id(), true),
            };
            if needs_start {
                controller.borrow_mut().queue_command_for(
                    tab_id,
                    BrowserCommand::ImeInput(ImeInput::Composition {
                        state: CompositionState::Start,
                        data: String::new(),
                    }),
                );
            }
            controller.borrow_mut().queue_command_for(
                tab_id,
                BrowserCommand::ImeInput(ImeInput::Composition {
                    state: CompositionState::End,
                    data: data.clone(),
                }),
            );
        }
        Ime::Disabled => {
            dismiss_composition(controller, composition_tab_id);
        }
    }
}

fn dismiss_composition(
    controller: &RefCell<BrowserController>,
    composition_tab_id: &mut Option<TabId>,
) {
    if let Some(tab_id) = composition_tab_id.take() {
        controller
            .borrow_mut()
            .queue_command_for(tab_id, BrowserCommand::ImeInput(ImeInput::Dismissed));
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::*;

    #[test]
    fn tab_shortcuts_distinguish_shift_and_leave_altgr_to_the_page() {
        let key = Key::Character("T".into());
        assert_eq!(
            browser_shortcut(&key, ModifiersState::CONTROL, false),
            Some(Shortcut::NewTab)
        );
        assert_eq!(
            browser_shortcut(&key, ModifiersState::CONTROL | ModifiersState::SHIFT, false),
            Some(Shortcut::ReopenTab)
        );
        assert_eq!(
            browser_shortcut(&key, ModifiersState::CONTROL | ModifiersState::ALT, false),
            None
        );
    }

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
        fn exit_fullscreen(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn zoom(&mut self, _action: talaria_core::ZoomAction) -> Result<(), Self::Error> {
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

    #[test]
    fn page_icon_pixels_are_preserved_when_converted_for_slint() {
        let icon =
            PageIcon::from_rgba(1, 1, vec![10, 20, 30, 40]).expect("the test icon should be valid");

        let pixels = slint_image_from_page_icon(&icon)
            .to_rgba8()
            .expect("an embedded RGBA image should remain CPU-readable");

        assert_eq!(pixels.width(), 1);
        assert_eq!(pixels.height(), 1);
        assert_eq!(pixels.as_bytes(), &[10, 20, 30, 40]);
    }

    #[test]
    fn logical_keys_use_web_keyboard_event_values() {
        assert_eq!(logical_key_string(&Key::Character("ç".into())), "ç");
        assert_eq!(
            logical_key_string(&Key::Named(winit::keyboard::NamedKey::Enter)),
            "Enter"
        );
        assert_eq!(
            logical_key_string(&Key::Named(winit::keyboard::NamedKey::Space)),
            " "
        );
        assert_eq!(logical_key_string(&Key::Dead(Some('´'))), "Dead");
    }

    #[test]
    fn physical_keys_use_web_keyboard_event_code_values() {
        use winit::keyboard::KeyCode;

        assert_eq!(physical_key_string(KeyCode::KeyA.into()), "KeyA");
        assert_eq!(physical_key_string(KeyCode::SuperLeft.into()), "MetaLeft");
        assert_eq!(physical_key_string(KeyCode::SuperRight.into()), "MetaRight");
        assert_eq!(physical_key_string(KeyCode::Meta.into()), "Super");
    }

    #[test]
    fn ime_preedit_and_commit_form_one_composition_sequence() {
        let controller = RefCell::new(BrowserController::default());
        let tab_id = controller.borrow().active_tab_id();
        let mut composition_tab_id = None;

        queue_ime_input(
            &controller,
            &mut composition_tab_id,
            &Ime::Preedit("aç".to_owned(), Some((2, 2))),
        );
        queue_ime_input(
            &controller,
            &mut composition_tab_id,
            &Ime::Commit("ação".to_owned()),
        );

        let commands =
            std::iter::from_fn(|| controller.borrow_mut().next_command()).collect::<Vec<_>>();
        assert_eq!(commands.len(), 3);
        assert!(commands.iter().all(|command| command.tab_id() == tab_id));
        assert_eq!(
            commands[0].command(),
            &BrowserCommand::ImeInput(ImeInput::Composition {
                state: CompositionState::Start,
                data: String::new(),
            })
        );
        assert_eq!(
            commands[1].command(),
            &BrowserCommand::ImeInput(ImeInput::Composition {
                state: CompositionState::Update,
                data: "aç".to_owned(),
            })
        );
        assert_eq!(
            commands[2].command(),
            &BrowserCommand::ImeInput(ImeInput::Composition {
                state: CompositionState::End,
                data: "ação".to_owned(),
            })
        );
        assert_eq!(composition_tab_id, None);
    }

    #[test]
    fn ime_dismissal_stays_with_the_tab_that_started_composition() {
        let controller = RefCell::new(BrowserController::default());
        let composing_tab_id = controller.borrow().active_tab_id();
        let mut composition_tab_id = None;
        queue_ime_input(
            &controller,
            &mut composition_tab_id,
            &Ime::Preedit("her".to_owned(), None),
        );
        controller.borrow_mut().open_tab();

        queue_ime_input(&controller, &mut composition_tab_id, &Ime::Disabled);

        let commands =
            std::iter::from_fn(|| controller.borrow_mut().next_command()).collect::<Vec<_>>();
        assert!(
            commands
                .iter()
                .all(|command| command.tab_id() == composing_tab_id)
        );
        assert_eq!(
            commands.last().map(|command| command.command()),
            Some(&BrowserCommand::ImeInput(ImeInput::Dismissed))
        );
        assert_eq!(composition_tab_id, None);
    }
}
