use std::cell::{Cell, RefCell};
use std::error::Error;
use std::process::ExitCode;
use std::rc::Rc;

use callbacks::connect_ui_callbacks;
use cli::{CliOutcome, parse_arguments};
use engine::connect_servo_runtime;
use keyboard::connect_native_keyboard_input;
use mercury_ui::MainWindow;
use screenshot::{ScreenshotResult, start_screenshot};
use slint::{BackendSelector, ComponentHandle, LogicalSize};
use talaria_core::{BrowserController, TabActivation};
use talaria_servo::{RuntimeOptions, run_content_process_if_requested};
use ui_sync::sync_ui;

mod callbacks;
mod cli;
mod engine;
mod keyboard;
#[cfg(target_os = "macos")]
mod media_isolation;
mod rendering;
mod screenshot;
mod shortcuts;
mod ui_sync;

fn main() -> Result<ExitCode, Box<dyn Error>> {
    // Media: keep the bundled GStreamer away from the user's own plugins and registry. First,
    // before any thread starts; content processes are this executable and inherit it.
    #[cfg(target_os = "macos")]
    media_isolation::isolate_gstreamer_from_user_environment();

    if run_content_process_if_requested()? {
        return Ok(ExitCode::SUCCESS);
    }

    let options = match parse_arguments(
        std::env::args_os()
            .skip(1)
            .map(|argument| argument.to_string_lossy().into_owned()),
    ) {
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
    select_hardware_accelerated_backend(options.background || options.screenshot.is_some())?;

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
        RuntimeOptions {
            gpu_canvas: options.gpu_canvas,
        },
    )?;
    sync_ui(&window, &controller.borrow());
    connect_ui_callbacks(
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

    window.run()?;

    if screenshot_timer.is_none() {
        return Ok(ExitCode::SUCCESS);
    }
    let result = screenshot_result.get();
    if result == ScreenshotResult::Pending {
        eprintln!("mercury-browser: the browser closed before the screenshot was taken");
    }
    Ok(ExitCode::from(result.exit_code()))
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
        // SAFETY: This is the first operation in main, before Slint, Servo, or
        // GStreamer can create worker threads or read the process environment.
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
    fn no_start_up_addresses_leave_a_single_blank_tab() {
        let mut controller = BrowserController::default();
        let tab_id = controller.active_tab_id();

        open_start_up_tabs(&mut controller, &[]);

        assert_eq!(controller.tabs().len(), 1);
        assert_eq!(controller.take_initial_url(tab_id), None);
    }
}
