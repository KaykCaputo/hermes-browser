use std::cell::{Cell, RefCell};
use std::error::Error;
use std::process::ExitCode;
use std::rc::Rc;

use callbacks::connect_ui_callbacks;
use cli::{CliOutcome, LaunchOptions, parse_arguments, reject_non_utf8_paths};
use engine::connect_servo_runtime;
use keyboard::connect_native_keyboard_input;
use mercury_ui::MainWindow;
use profile::{ActiveProfile, EXIT_PROFILE_IN_USE, ProfileInUse};
use screenshot::{ScreenshotResult, start_screenshot};
use slint::{BackendSelector, ComponentHandle, LogicalSize};
use talaria_core::{BrowserController, TabActivation};
use talaria_servo::{RuntimeOptions, record_browser_process_id, run_content_process_if_requested};
use ui_sync::sync_ui;

// activation (tabs lane)
mod activation;
mod callback_scope;
mod callbacks;
mod cli;
// controls
mod controls;
mod crash_handler;
mod engine;
mod keyboard;
mod keyutils;
#[cfg(target_os = "macos")]
mod macos_app_delegate;
#[cfg(target_os = "macos")]
mod macos_terminate;
#[cfg(target_os = "macos")]
mod media_buffers;
#[cfg(target_os = "macos")]
mod media_isolation;
mod page_requests; // dialogs
mod panic_hook;
#[cfg(target_os = "macos")]
mod platform_macos;
mod post_run;
mod profile;
mod quit;
mod rendering;
mod screen_metrics;
mod screenshot;
mod settings;
mod shortcuts;
mod shutdown;
mod signals;
mod ui_sync;

fn main() -> Result<ExitCode, Box<dyn Error>> {
    // (1) Before the content-process branch, so that content processes report their crashes
    // too. These only install handlers; no thread starts.
    crash_handler::install();
    std::panic::set_hook(Box::new(panic_hook::panic_hook));

    // (2) Media: keep the bundled GStreamer away from the user's own plugins and registry.
    // Before any thread starts; content processes are this executable and inherit it.
    #[cfg(target_os = "macos")]
    media_isolation::isolate_gstreamer_from_user_environment();

    if run_content_process_if_requested()? {
        return Ok(ExitCode::SUCCESS);
    }
    // With the early environment changes: before any thread exists, and only in the browser.
    record_browser_process_id();

    // (5) Media: GLib's user directories, where GStreamer writes its download buffers (copies of
    // the media being played), point into a private directory of this run under Mercury's cache;
    // content processes inherit them. Removed when this run ends (by `post_run::finish`, or
    // when dropped on an early return), and leftovers of killed runs at the next start. Before
    // any thread starts.
    #[cfg(target_os = "macos")]
    let media_buffers = media_buffers::keep_media_buffers_private();

    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let parsed = reject_non_utf8_paths(&arguments).unwrap_or_else(|| {
        parse_arguments(
            arguments
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned()),
        )
    });
    let options = match parsed {
        CliOutcome::Run(options) => options,
        CliOutcome::PrintAndExit { text, code } => {
            if code == 0 {
                println!("{text}");
            } else {
                eprintln!("{text}");
            }
            return Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)));
        }
    };

    use_automatic_gpu_selection();
    // Dropped last, after the engine has saved its data: a temporary profile is deleted then.
    let profile = match profile::open_profile(
        options.profile_dir.as_deref(),
        options.is_automated(),
        &profile::TemporaryProfileArea::for_this_run(),
    ) {
        Ok(profile) => profile,
        Err(ProfileInUse { root }) => {
            eprintln!(
                "mercury-browser: the profile at {} is in use by another Mercury Browser; \
                 close it or choose another --profile-dir",
                root.display()
            );
            return Ok(ExitCode::from(EXIT_PROFILE_IN_USE));
        }
    };
    // From here on a terminate, interrupt or hang-up signal quits in order.
    signals::install();
    // settings-ui: loaded under the profile's lock; GPU canvas and experimental features apply now.
    let settings =
        settings::SettingsHandle::start(profile.paths.as_ref().map(|p| p.root.as_path()));
    let options = settings::launch_options(options, &settings);
    select_hardware_accelerated_backend(options.is_automated())?;
    #[cfg(target_os = "macos")]
    macos_terminate::end_the_run_in_order_on_terminate_requests();

    let window = MainWindow::new()?;
    if let Some((width, height)) = options.window_size {
        window
            .window()
            .set_size(LogicalSize::new(width as f32, height as f32));
    }
    let controller = Rc::new(RefCell::new(BrowserController::default()));
    // settings-ui: search engine, zoom and colour scheme before any address is resolved.
    settings::prepare_controller(&settings, &mut controller.borrow_mut(), &options.urls);
    open_start_up_tabs(&mut controller.borrow_mut(), &options.urls);
    let web_view_focused = Rc::new(Cell::new(false));

    connect_servo_runtime(
        &window,
        Rc::clone(&controller),
        Rc::clone(&web_view_focused),
        // settings-ui: blocking and what to clear are known before the engine starts.
        settings::with_privacy(runtime_options(&options, &profile), &settings),
    )?;
    // settings-ui
    settings::connect(&settings, &window, &controller);
    sync_ui(&window, &controller.borrow());
    connect_ui_callbacks(
        &window,
        Rc::clone(&controller),
        Rc::clone(&web_view_focused),
    );
    connect_native_keyboard_input(&window, Rc::clone(&controller), web_view_focused);
    // controls
    controls::connect_controls(&window, Rc::clone(&controller));
    quit::quit_when_the_window_is_closed(&window);

    let mut timers = Vec::new();
    let screenshot_result = options.screenshot.map(|screenshot| {
        let result = Rc::new(Cell::new(ScreenshotResult::Pending));
        timers.push(start_screenshot(
            &window,
            controller,
            screenshot,
            Rc::clone(&result),
        ));
        result
    });
    timers.extend(
        options
            .exit_request
            .map(|request| quit::start_exit_request(&window, request)),
    );

    // Not `window.run()`, which hides the window as soon as the loop ends: on Wayland hiding
    // destroys the window and tears rendering down while the engine still runs.
    // `shut_down_engine` hides the window once it has taken the engine's state.
    let shown = window.show();
    post_run::prepare(post_run::RunEnd {
        window,
        timers,
        screenshot_result,
        profile,
        #[cfg(target_os = "macos")]
        media_buffers,
        settings,
    });
    let run_result = shown.and_then(|()| slint::run_event_loop());

    // If the session ended (macOS log-out), the terminate handler has already ended the run
    // and AppKit exits; otherwise this ends it.
    let exit_code = post_run::finish(run_result.is_err()).unwrap_or(0);
    if let Some(signal) = signals::received() {
        // Ended by a quit signal: now that the run has ended, end the way that signal would
        // have, so the parent sees it.
        signals::end_by(signal);
    }

    run_result?;
    Ok(ExitCode::from(exit_code))
}

/// Engine settings for this run: the launch options and the profile's Servo directory.
fn runtime_options(options: &LaunchOptions, profile: &ActiveProfile) -> RuntimeOptions {
    RuntimeOptions {
        gpu_canvas: options.gpu_canvas,
        config_dir: profile.servo_config_dir(),
        experimental_web_features: options.experimental_web_features,
        // settings-ui: filled in by `settings::with_privacy`.
        ..RuntimeOptions::default()
    }
}

/// The first resolvable address becomes the initial URL of the first tab; each further one
/// opens a background tab. Addresses that do not resolve are reported and skipped.
fn open_start_up_tabs(controller: &mut BrowserController, addresses: &[String]) {
    let first_tab_id = controller.active_tab_id();
    let mut first_tab_has_address = false;

    for address in addresses {
        let result = if first_tab_has_address {
            controller
                .open_tab_with(Some(address), TabActivation::Background)
                .map(|_| true)
        } else {
            controller.set_initial_address(first_tab_id, address)
        };

        match result {
            Ok(true) => first_tab_has_address = true,
            Ok(false) => {
                eprintln!("mercury-browser: skipping '{address}': the tab is already open")
            }
            Err(error) => eprintln!("mercury-browser: skipping '{address}': {error}"),
        }
    }
}

fn use_automatic_gpu_selection() {
    #[cfg(target_os = "linux")]
    {
        // DRI_PRIME is a manual Mesa device override. Inheriting a stale value
        // such as `1` on a single-GPU machine causes every GL context to try a
        // nonexistent second device before falling back to the default GPU.
        //
        // SAFETY: This runs in main before Slint, Servo, or GStreamer can create worker
        // threads or read the process environment. The work done before it (installing the
        // crash handler and panic hook, the content-process check, parsing the command line)
        // starts no threads and does not read DRI_PRIME.
        unsafe {
            std::env::remove_var("DRI_PRIME");
        }
    }
}

fn select_hardware_accelerated_backend(
    in_background: bool,
) -> Result<(), slint::platform::PlatformError> {
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

    let selector = BackendSelector::new()
        .backend_name("winit".to_owned())
        .require_wgpu_29(slint::wgpu_29::WGPUConfiguration::Automatic(settings));

    if in_background {
        background_window(selector).select()
    } else {
        selector.select()
    }
}

/// Keeps a screenshot run from taking keyboard focus or activating the application, so that
/// it does not disturb whoever is using the machine.
fn background_window(selector: BackendSelector) -> BackendSelector {
    let selector =
        selector.with_winit_window_attributes_hook(|attributes| attributes.with_active(false));

    #[cfg(target_os = "macos")]
    let selector = {
        use slint::winit_030::winit::event_loop::EventLoop;
        use slint::winit_030::winit::platform::macos::{
            ActivationPolicy, EventLoopBuilderExtMacOS,
        };

        let mut event_loop_builder = EventLoop::with_user_event();
        event_loop_builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_activate_ignoring_other_apps(false);
        selector.with_winit_event_loop_builder(event_loop_builder)
    };

    selector
}

#[cfg(test)]
mod tests {
    use talaria_core::LoadState;

    use super::*;

    #[test]
    fn start_up_addresses_fill_the_first_tab_then_open_background_tabs() {
        let mut controller = BrowserController::default();
        let first_tab_id = controller.active_tab_id();

        open_start_up_tabs(
            &mut controller,
            &[
                "ftp://skipped".to_owned(),
                "a.test".to_owned(),
                "b.test".to_owned(),
            ],
        );

        assert_eq!(controller.active_tab_id(), first_tab_id);
        let tabs = controller
            .tabs()
            .map(|tab| {
                (
                    tab.state().url().to_owned(),
                    tab.state().load_state().clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            tabs,
            vec![
                ("https://a.test/".to_owned(), LoadState::Loading),
                ("https://b.test/".to_owned(), LoadState::Loading),
            ]
        );
        let initial_urls = controller
            .tabs()
            .map(|tab| tab.id())
            .collect::<Vec<_>>()
            .into_iter()
            .map(|tab_id| controller.take_initial_url(tab_id))
            .collect::<Vec<_>>();
        assert_eq!(
            initial_urls,
            vec![
                Some("https://a.test/".to_owned()),
                Some("https://b.test/".to_owned()),
            ]
        );
    }

    #[test]
    fn the_engine_keeps_its_data_in_the_opened_profile() {
        let root = std::env::temp_dir().join(format!(
            "mercury-runtime-options-test-{}",
            std::process::id()
        ));
        let area = profile::TemporaryProfileArea {
            directory: root.join("area"),
            ..profile::TemporaryProfileArea::for_this_run()
        };
        let profile =
            profile::open_profile(Some(&root), false, &area).expect("the profile is free");
        let options = LaunchOptions {
            gpu_canvas: false,
            experimental_web_features: false,
            ..LaunchOptions::default()
        };

        let runtime_options = runtime_options(&options, &profile);
        drop(profile);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(
            runtime_options,
            RuntimeOptions {
                gpu_canvas: false,
                config_dir: Some(root.join("servo")),
                experimental_web_features: false,
                ..RuntimeOptions::default()
            }
        );
    }

    #[test]
    fn no_start_up_addresses_leave_a_single_blank_tab() {
        let mut controller = BrowserController::default();
        let tab_id = controller.active_tab_id();

        open_start_up_tabs(&mut controller, &[]);

        assert_eq!(controller.tabs().len(), 1);
        assert_eq!(controller.take_initial_url(tab_id), None);
    }
}
