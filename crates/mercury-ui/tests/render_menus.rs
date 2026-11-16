//! Renders the browser menu and the About window with Slint's software renderer, so they can be
//! seen without opening them by hand. Set `MERCURY_UI_RENDER_DIR` to keep the PNGs.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use mercury_ui::{AboutWindow, BrowserMenuCommand, BrowserMenuEntry, MainWindow};
use slint::language::ColorScheme;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalSize, ModelRc, PhysicalSize, SharedPixelBuffer, VecModel};

// The menu's panel in a window of its own, with a forced colour scheme. Built here, in the test,
// so the shipped UI exports nothing for it. Its types are the macro's own, hence the module.
mod preview {
    slint::slint! {
        import { Palette } from "std-widgets.slint";
        import { BrowserMenuPanel, BrowserMenuEntry } from "ui/menus/browser-menu.slint";

        export component BrowserMenuPreview inherits Window {
            in property <[BrowserMenuEntry]> top-entries;
            in property <[BrowserMenuEntry]> bottom-entries;
            in property <int> zoom-percent: 100;
            in property <ColorScheme> preview-color-scheme: ColorScheme.unknown;

            changed preview-color-scheme => {
                Palette.color-scheme = self.preview-color-scheme;
            }

            width: 280px;
            height: 320px;
            background: Palette.background;

            BrowserMenuPanel {
                x: 8px;
                y: 8px;
                width: 264px;
                top-entries: root.top-entries;
                bottom-entries: root.bottom-entries;
                zoom-percent: root.zoom-percent;
            }
        }
    }

    /// The same rows, as the preview's own types.
    pub fn rows(
        rows: &slint::ModelRc<mercury_ui::BrowserMenuEntry>,
    ) -> slint::ModelRc<BrowserMenuEntry> {
        use slint::Model;
        let entries = rows
            .iter()
            .map(|entry| BrowserMenuEntry {
                title: entry.title,
                shortcut: entry.shortcut,
                enabled: entry.enabled,
                command: command(entry.command),
                separator_before: entry.separator_before,
            })
            .collect::<Vec<_>>();
        slint::ModelRc::new(slint::VecModel::from(entries))
    }

    fn command(command: mercury_ui::BrowserMenuCommand) -> BrowserMenuCommand {
        use mercury_ui::BrowserMenuCommand as Shipped;
        match command {
            Shipped::NewTab => BrowserMenuCommand::NewTab,
            Shipped::ZoomIn => BrowserMenuCommand::ZoomIn,
            Shipped::ZoomOut => BrowserMenuCommand::ZoomOut,
            Shipped::ZoomReset => BrowserMenuCommand::ZoomReset,
            Shipped::Find => BrowserMenuCommand::Find,
            Shipped::Settings => BrowserMenuCommand::Settings,
            Shipped::About => BrowserMenuCommand::About,
            Shipped::Help => BrowserMenuCommand::Help,
            Shipped::Quit => BrowserMenuCommand::Quit,
        }
    }
}

struct SoftwarePlatform;

impl Platform for SoftwarePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

const SCHEMES: [(ColorScheme, &str); 3] = [
    (ColorScheme::Light, "light"),
    (ColorScheme::Dark, "dark"),
    (ColorScheme::Unknown, "system"),
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
fn render_browser_menu_in_window() {
    let window = MainWindow::new().expect("the main window can be created");
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
    save(&pixels, "browser-menu.png");
    assert_eq!((pixels.width(), pixels.height()), (900, 560));
    // The "⋮" button ends 14 px before the window's right edge (the toolbar's padding); the
    // panel's right edge lines up with it.
    let panel = [0x1a, 0x22, 0x2c];
    assert_eq!(pixel(&pixels, 900 - 14 - 130, 140), panel);
    assert_eq!(pixel(&pixels, 900 - 14 - 4, 140), panel);
    assert_ne!(pixel(&pixels, 900 - 14 + 4, 140), panel);

    // Down twice skips the two disabled rows to "About Mercury Browser".
    press(window.window(), Key::DownArrow);
    press(window.window(), Key::DownArrow);
    let pixels = window.window().take_snapshot().expect("the window renders");
    save(&pixels, "browser-menu-keyboard.png");
    press(window.window(), Key::Return);
    assert_eq!(*chosen.borrow(), [BrowserMenuCommand::About]);
    window.hide().expect("the main window can be hidden");
}

fn render_browser_menu(scheme: ColorScheme, name: &str) -> [u8; 3] {
    let preview = preview::BrowserMenuPreview::new().expect("the preview can be created");
    preview.window().set_size(LogicalSize::new(280.0, 320.0));
    let (top, bottom) = rows();
    preview.set_top_entries(preview::rows(&top));
    preview.set_bottom_entries(preview::rows(&bottom));
    preview.set_zoom_percent(110);
    preview.set_preview_color_scheme(scheme);
    preview.show().expect("the preview can be shown");
    let pixels = preview
        .window()
        .take_snapshot()
        .expect("the preview renders");
    save(&pixels, &format!("browser-menu-{name}.png"));
    preview.hide().expect("the preview can be hidden");
    // Inside the panel, left of the rows' text.
    pixel(&pixels, 12, 40)
}

fn render_about(scheme: ColorScheme, name: &str) -> [u8; 3] {
    let about = AboutWindow::new().expect("the About window can be created");
    about.set_version_line("Version 0.2.0".into());
    about.set_engine_line("Servo 0.8.0 web engine".into());
    about.set_media_line("GStreamer 1.28.9 media framework".into());
    about.set_copyright_line(
        "Copyright © 2026 Kayk Caputo and Mercury Browser contributors. Licensed under MPL-2.0."
            .into(),
    );
    about.set_preview_color_scheme(scheme);
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
    pixel(&pixels, 4, 4)
}

#[test]
fn the_browser_menu_and_the_about_window_render() {
    slint::platform::set_platform(Box::new(SoftwarePlatform)).expect("no platform is set yet");

    render_browser_menu_in_window();

    let brightness = |[r, g, b]: [u8; 3]| u32::from(r) + u32::from(g) + u32::from(b);
    for (scheme, name) in SCHEMES {
        // The browser menu follows the browser chrome, which is dark in every scheme for now.
        assert_eq!(
            render_browser_menu(scheme, name),
            [0x1a, 0x22, 0x2c],
            "{name}"
        );
        let background = render_about(scheme, name);
        match scheme {
            ColorScheme::Dark => assert!(brightness(background) < 150, "{name}: {background:?}"),
            ColorScheme::Light => assert!(brightness(background) > 600, "{name}: {background:?}"),
            _ => {}
        }
    }
}
