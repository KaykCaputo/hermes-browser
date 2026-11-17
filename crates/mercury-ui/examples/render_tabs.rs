//! Renders the browser chrome with the "Pop-up blocked" indicator in each appearance (Light,
//! Dark, System) to PNG files without a display, through Slint's software renderer.
//!
//! `cargo run --release -p mercury-ui --example render_tabs -- <output directory>`

use std::cell::RefCell;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mercury_ui::{BrowserTab, ChromePalette, ChromeTheme, MainWindow};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Image, ModelRc, PhysicalSize, VecModel};

const SCALE: f32 = 2.0;

struct HeadlessPlatform {
    windows: RefCell<Vec<Rc<MinimalSoftwareWindow>>>,
}

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.windows.borrow_mut().push(Rc::clone(&window));
        Ok(window)
    }
}

fn render(
    window: &slint::Window,
    width: u32,
    height: u32,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    window.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: SCALE,
    });
    window.set_size(PhysicalSize::new(width, height));
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        let _ = window.take_snapshot();
    }
    let snapshot = window.take_snapshot()?;
    let bytes = snapshot
        .as_slice()
        .iter()
        .flat_map(|pixel| [pixel.r, pixel.g, pixel.b])
        .collect::<Vec<_>>();
    image::save_buffer(
        path,
        &bytes,
        snapshot.width(),
        snapshot.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    println!("wrote {}", path.display());
    Ok(())
}

fn window_with_blocked_pop_up(theme: ChromeTheme) -> Result<MainWindow, Box<dyn Error>> {
    let window = MainWindow::new()?;
    window.global::<ChromePalette>().set_theme(theme);
    let tab = |title: &str, active: bool| BrowserTab {
        title: title.into(),
        icon: Image::default(),
        has_icon: false,
        active,
        loading: false,
    };
    window.set_tabs(ModelRc::new(VecModel::from(vec![
        tab("Opener", true),
        tab("Example Domain", false),
    ])));
    window.set_address("https://news.example/article".into());
    window.set_is_new_tab(false);
    window.set_pop_up_blocked_origin("https://news.example".into());
    Ok(window)
}

fn main() -> Result<(), Box<dyn Error>> {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: render_tabs <output directory>")?,
    );
    std::fs::create_dir_all(&output)?;
    slint::platform::set_platform(Box::new(HeadlessPlatform {
        windows: RefCell::new(Vec::new()),
    }))
    .map_err(|error| format!("{error:?}"))?;

    for (theme, name) in [
        (ChromeTheme::Light, "light"),
        (ChromeTheme::Dark, "dark"),
        (ChromeTheme::System, "system"),
    ] {
        let window = window_with_blocked_pop_up(theme)?;
        window.show()?;
        render(
            window.window(),
            2000,
            400,
            &output.join(format!("pop-up-blocked-{name}.png")),
        )?;
        window.hide()?;
    }
    Ok(())
}
