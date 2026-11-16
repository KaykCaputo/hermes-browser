//! Renders the browser menu and the About window with Slint's software renderer, so they can be
//! seen without opening them by hand. Set `MERCURY_UI_RENDER_DIR` to keep the PNGs.

use std::path::PathBuf;
use std::rc::Rc;

use mercury_ui::{AboutWindow, BrowserMenuCommand, BrowserMenuEntry, MainWindow};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, LogicalSize, ModelRc, PhysicalSize, SharedPixelBuffer, VecModel};

struct SoftwarePlatform;

impl Platform for SoftwarePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

fn output_path(name: &str) -> PathBuf {
    let directory = std::env::var_os("MERCURY_UI_RENDER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    directory.join(name)
}

fn save(pixels: &SharedPixelBuffer<slint::Rgba8Pixel>, name: &str) {
    let path = output_path(name);
    image::save_buffer_with_format(
        &path,
        pixels.as_bytes(),
        pixels.width(),
        pixels.height(),
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .expect("the rendering can be saved");
}

fn pixel(pixels: &SharedPixelBuffer<slint::Rgba8Pixel>, x: u32, y: u32) -> [u8; 3] {
    let pixel = pixels.as_slice()[(y * pixels.width() + x) as usize];
    [pixel.r, pixel.g, pixel.b]
}

fn entry(
    title: &str,
    shortcut: &str,
    enabled: bool,
    command: BrowserMenuCommand,
    separator_before: bool,
) -> BrowserMenuEntry {
    BrowserMenuEntry {
        title: title.into(),
        shortcut: shortcut.into(),
        enabled,
        command,
        separator_before,
    }
}

fn render_browser_menu() {
    let window = MainWindow::new().expect("the main window can be created");
    window.window().set_size(LogicalSize::new(900.0, 560.0));
    // The rows `apps/mercury-desktop/src/menus` builds on macOS.
    window.set_browser_menu_top_entries(ModelRc::new(VecModel::from(vec![entry(
        "New Tab",
        "⌘T",
        true,
        BrowserMenuCommand::NewTab,
        false,
    )])));
    window.set_browser_menu_bottom_entries(ModelRc::new(VecModel::from(vec![
        entry("Find…", "", false, BrowserMenuCommand::Find, false),
        entry("Settings…", "", false, BrowserMenuCommand::Settings, true),
        entry(
            "About Mercury Browser",
            "",
            true,
            BrowserMenuCommand::About,
            false,
        ),
        entry(
            "Mercury Browser Help",
            "",
            true,
            BrowserMenuCommand::Help,
            false,
        ),
        entry(
            "Quit Mercury Browser",
            "⌘Q",
            true,
            BrowserMenuCommand::Quit,
            true,
        ),
    ])));
    window.set_zoom_percent(110);
    window.show().expect("the main window can be shown");
    window.invoke_show_browser_menu();

    let pixels = window.window().take_snapshot().expect("the window renders");
    save(&pixels, "browser-menu.png");
    assert_eq!(
        (pixels.width(), pixels.height()),
        (900, 560),
        "rendered at scale 1"
    );
    // The menu's panel (#1a222c) hangs below the "⋮" button, inside the window.
    assert_eq!(pixel(&pixels, 900 - 14 - 130, 140), [0x1a, 0x22, 0x2c]);
    assert_eq!(pixel(&pixels, 900 - 14 - 8, 140), [0x1a, 0x22, 0x2c]);
    window.hide().expect("the main window can be hidden");
}

fn render_about(scheme: slint::language::ColorScheme, name: &str) -> [u8; 3] {
    let about = AboutWindow::new().expect("the About window can be created");
    about.set_version_line("Version 0.2.0".into());
    about.set_engine_line("Servo 0.8.0 web engine".into());
    about.set_copyright_line(
        "Copyright © 2026 Kayk Caputo and Mercury Browser contributors. Licensed under MPL-2.0."
            .into(),
    );
    about.set_preview_color_scheme(scheme);
    // A software window has no size until it is given one; this is the size the window declares.
    about.window().set_size(LogicalSize::new(380.0, 350.0));
    about.show().expect("the About window can be shown");
    let pixels = about
        .window()
        .take_snapshot()
        .expect("the About window renders");
    save(&pixels, name);
    assert_eq!(about.window().size(), PhysicalSize::new(380, 350));
    about.hide().expect("the About window can be hidden");
    pixel(&pixels, 4, 4)
}

#[test]
fn the_browser_menu_and_the_about_window_render() {
    slint::platform::set_platform(Box::new(SoftwarePlatform)).expect("no platform is set yet");

    render_browser_menu();
    let dark = render_about(slint::language::ColorScheme::Dark, "about-dark.png");
    let light = render_about(slint::language::ColorScheme::Light, "about-light.png");

    let brightness = |[r, g, b]: [u8; 3]| u32::from(r) + u32::from(g) + u32::from(b);
    assert!(brightness(dark) < 150, "dark background: {dark:?}");
    assert!(brightness(light) > 600, "light background: {light:?}");
}
