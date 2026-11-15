use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;

use callbacks::connect_ui_callbacks;
use engine::connect_servo_runtime;
use keyboard::connect_native_keyboard_input;
use mercury_ui::MainWindow;
use slint::{BackendSelector, ComponentHandle};
use talaria_core::BrowserController;
use talaria_servo::run_content_process_if_requested;
use ui_sync::sync_ui;

mod callbacks;
mod engine;
mod keyboard;
mod rendering;
mod shortcuts;
mod ui_sync;

fn main() -> Result<(), Box<dyn Error>> {
    if run_content_process_if_requested()? {
        return Ok(());
    }

    use_automatic_gpu_selection();
    select_hardware_accelerated_backend()?;

    let window = MainWindow::new()?;
    let controller = Rc::new(RefCell::new(BrowserController::default()));
    let web_view_focused = Rc::new(Cell::new(false));

    connect_servo_runtime(
        &window,
        Rc::clone(&controller),
        Rc::clone(&web_view_focused),
    )?;
    sync_ui(&window, &controller.borrow());
    connect_ui_callbacks(
        &window,
        Rc::clone(&controller),
        Rc::clone(&web_view_focused),
    );
    connect_native_keyboard_input(&window, controller, web_view_focused);

    window.run()?;
    Ok(())
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
