use std::cell::{Cell, RefCell};
use std::error::Error;
use std::process::ExitCode;
use std::rc::Rc;

use callbacks::connect_ui_callbacks;
use cli::{CliOutcome, LaunchOptions, parse_arguments, reject_non_utf8_paths};
use engine::{connect_servo_runtime, shut_down_engine};
use keyboard::connect_native_keyboard_input;
use mercury_ui::MainWindow;
use profile::{ActiveProfile, EXIT_PROFILE_IN_USE, ProfileInUse};
use screenshot::{ScreenshotResult, start_screenshot};
use slint::{BackendSelector, ComponentHandle, LogicalSize};
use talaria_core::{BrowserController, TabActivation};
use talaria_servo::{RuntimeOptions, record_browser_process_id, run_content_process_if_requested};
use ui_sync::sync_ui;

mod about;
mod callbacks;
mod cli;
mod crash_handler;
mod engine;
mod keyboard;
mod keyutils;
mod menus;
mod panic_hook;
#[cfg(target_os = "macos")]
mod platform_macos;
mod profile;
mod rendering;
mod screen_metrics;
mod screenshot;
mod shortcuts;
mod shutdown;
mod ui_sync;

fn main() -> Result<ExitCode, Box<dyn Error>> {
    // Before the content-process branch, so that content processes report their crashes too.
    crash_handler::install();
    std::panic::set_hook(Box::new(panic_hook::panic_hook));

    if run_content_process_if_requested()? {
        return Ok(ExitCode::SUCCESS);
    }
    // With the early environment changes: before any thread exists, and only in the browser.
    record_browser_process_id();

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
    select_hardware_accelerated_backend(options.is_automated())?;

    let window = MainWindow::new()?;
    if let Some((width, height)) = options.window_size {
        window
            .window()
            .set_size(LogicalSize::new(width as f32, height as f32));
    }
    let controller = Rc::new(RefCell::new(BrowserController::default()));
    open_start_up_tabs(&mut controller.borrow_mut(), &options.urls);
    let web_view_focused = Rc::new(Cell::new(false));

    connect_servo_runtime(
        &window,
        Rc::clone(&controller),
        Rc::clone(&web_view_focused),
        runtime_options(&options, &profile),
    )?;
    sync_ui(&window, &controller.borrow());
    connect_ui_callbacks(
        &window,
        Rc::clone(&controller),
        Rc::clone(&web_view_focused),
    );
    // menus lane: the macOS menu bar and the browser menu.
    menus::connect(
        &window,
        Rc::clone(&controller),
        Rc::clone(&web_view_focused),
    );
    connect_native_keyboard_input(&window, Rc::clone(&controller), web_view_focused);

    let screenshot_result = Rc::new(Cell::new(ScreenshotResult::Pending));
    let screenshot_timer = options.screenshot.map(|screenshot| {
        start_screenshot(
            &window,
            controller,
            screenshot,
            Rc::clone(&screenshot_result),
        )
    });

    let run_result = window.run();
    // menus lane: drop the About window and the menus' handles before the engine shuts down.
    menus::release();

    let exit_code = match (&run_result, screenshot_timer.is_some()) {
        (Err(_), _) => 1,
        (Ok(()), false) => 0,
        (Ok(()), true) => {
            let result = screenshot_result.get();
            if result == ScreenshotResult::Pending {
                eprintln!("mercury-browser: the browser closed before the screenshot was taken");
            }
            result.exit_code()
        }
    };
    // Nothing may capture, pump or render while the engine shuts down, and the engine must be
    // gone before the window (and the renderer whose device Servo uses) is destroyed.
    drop(screenshot_timer);
    shut_down_engine(exit_code);
    drop(window);

    run_result?;
    Ok(ExitCode::from(exit_code))
}

/// Engine settings for this run: the launch options and the profile's Servo directory.
fn runtime_options(options: &LaunchOptions, profile: &ActiveProfile) -> RuntimeOptions {
    RuntimeOptions {
        gpu_canvas: options.gpu_canvas,
        config_dir: profile.servo_config_dir(),
        experimental_web_features: options.experimental_web_features,
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
