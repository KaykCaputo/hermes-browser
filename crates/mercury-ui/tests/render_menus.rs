//! Renders the browser menu and the About window with Slint's software renderer, so they can be
//! seen without opening them by hand. Set `MERCURY_UI_RENDER_DIR` to keep the PNGs.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use mercury_ui::{
    AboutWindow, BrowserMenuCommand, BrowserMenuEntry, ChromePalette, ChromeTheme, MainWindow,
};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalSize, ModelRc, PhysicalSize, SharedPixelBuffer, VecModel};

struct SoftwarePlatform;

impl Platform for SoftwarePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

const THEMES: [(ChromeTheme, &str); 3] = [
    (ChromeTheme::Light, "light"),
    (ChromeTheme::Dark, "dark"),
    // Headless, the system's scheme is unknown, and the chrome then stays dark.
    (ChromeTheme::System, "system"),
];

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

/// The rows `apps/mercury-desktop/src/menus` builds on macOS.
fn rows() -> (ModelRc<BrowserMenuEntry>, ModelRc<BrowserMenuEntry>) {
    (
        ModelRc::new(VecModel::from(vec![entry(
            "New Tab",
            "⌘T",
            true,
            BrowserMenuCommand::NewTab,
            false,
        )])),
        ModelRc::new(VecModel::from(vec![
            entry("Find…", "", false, BrowserMenuCommand::Find, false),
            entry("Settings…", "⌘,", true, BrowserMenuCommand::Settings, true),
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
        ])),
    )
}

fn press(window: &slint::Window, key: impl Into<slint::SharedString> + Clone) {
    window.dispatch_event(WindowEvent::KeyPressed {
        text: key.clone().into(),
    });
    window.dispatch_event(WindowEvent::KeyReleased { text: key.into() });
}

/// The menu open in the browser window, anchored under the "⋮" button; then chosen with the
/// keyboard.
fn render_browser_menu_in_window(theme: ChromeTheme, name: &str) {
    let window = MainWindow::new().expect("the main window can be created");
    window.global::<ChromePalette>().set_theme(theme);
    window.window().set_size(LogicalSize::new(900.0, 560.0));
    let (top, bottom) = rows();
    window.set_browser_menu_top_entries(top);
    window.set_browser_menu_bottom_entries(bottom);
    window.set_zoom_percent(110);
    let chosen = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&chosen);
    window.on_browser_menu_activated(move |command| recorded.borrow_mut().push(command));
    window.show().expect("the main window can be shown");
    window.invoke_show_browser_menu();

    let pixels = window.window().take_snapshot().expect("the window renders");
    save(&pixels, &format!("browser-menu-{name}.png"));
    assert_eq!((pixels.width(), pixels.height()), (900, 560));
    // The "⋮" button ends 14 px before the window's right edge (the toolbar's padding); the
    // panel's right edge lines up with it.
    let panel = rgb(window.global::<ChromePalette>().get_popup_background());
    assert_eq!(pixel(&pixels, 900 - 14 - 130, 140), panel);
    assert_eq!(pixel(&pixels, 900 - 14 - 4, 140), panel);
    assert_ne!(pixel(&pixels, 900 - 14 + 4, 140), panel);

    // New Tab, then past the disabled "Find…" to Settings, then "About Mercury Browser".
    press(window.window(), Key::DownArrow);
    press(window.window(), Key::DownArrow);
    press(window.window(), Key::DownArrow);
    let pixels = window.window().take_snapshot().expect("the window renders");
    save(&pixels, &format!("browser-menu-keyboard-{name}.png"));
    press(window.window(), Key::Return);
    assert_eq!(*chosen.borrow(), [BrowserMenuCommand::About]);
    window.hide().expect("the main window can be hidden");
}

fn render_about(theme: ChromeTheme, name: &str) -> ([u8; 3], [u8; 3]) {
    let about = AboutWindow::new().expect("the About window can be created");
    about.global::<ChromePalette>().set_theme(theme);
    about.set_version_line("Version 0.2.0".into());
    about.set_engine_line("Servo 0.8.0 web engine".into());
    about.set_media_line("GStreamer 1.28.9 media framework".into());
    about.set_copyright_line(
        "Copyright © 2026 Kayk Caputo and Mercury Browser contributors. Licensed under MPL-2.0."
            .into(),
    );
    // A software window has no size until it is given one; this is the size the window declares.
    about.window().set_size(LogicalSize::new(380.0, 370.0));
    about.show().expect("the About window can be shown");
    let pixels = about
        .window()
        .take_snapshot()
        .expect("the About window renders");
    save(&pixels, &format!("about-{name}.png"));
    assert_eq!(about.window().size(), PhysicalSize::new(380, 370));
    about.hide().expect("the About window can be hidden");
    (
        pixel(&pixels, 4, 4),
        rgb(about.global::<ChromePalette>().get_window_background()),
    )
}

fn rgb(color: slint::Color) -> [u8; 3] {
    [color.red(), color.green(), color.blue()]
}

#[test]
fn the_browser_menu_and_the_about_window_render() {
    slint::platform::set_platform(Box::new(SoftwarePlatform)).expect("no platform is set yet");

    let brightness = |[r, g, b]: [u8; 3]| u32::from(r) + u32::from(g) + u32::from(b);
    for (theme, name) in THEMES {
        render_browser_menu_in_window(theme, name);
        let (background, expected) = render_about(theme, name);
        assert_eq!(background, expected, "{name}");
        match theme {
            ChromeTheme::Light => assert!(brightness(background) > 600, "{name}: {background:?}"),
            _ => assert!(brightness(background) < 150, "{name}: {background:?}"),
        }
    }
}
