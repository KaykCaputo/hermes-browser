//! Renders the settings window, and the browser chrome in each theme, to PNG files without a
//! display, through Slint's software renderer.
//!
//! `cargo run --release -p mercury-ui --example render_settings -- <output directory>`

use std::cell::RefCell;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mercury_ui::{
    BrowserTab, ChromePalette, ChromeTheme, MainWindow, SearchEngineRow, SettingsWindow,
};
use slint::language::ColorScheme;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Image, ModelRc, PhysicalSize, Rgb8Pixel, SharedString, VecModel};

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
    let size = PhysicalSize::new(width, height);
    window.set_size(size);
    // Let layouts, bindings and any animation settle.
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        let _ = window.take_snapshot();
    }
    let snapshot = window.take_snapshot()?;
    let pixels: &[Rgb8Pixel] = &snapshot
        .as_slice()
        .iter()
        .map(|pixel| Rgb8Pixel::new(pixel.r, pixel.g, pixel.b))
        .collect::<Vec<_>>();
    let bytes = pixels
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

fn strings(values: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values
            .iter()
            .map(|value| SharedString::from(*value))
            .collect::<Vec<_>>(),
    ))
}

fn sample_settings_window() -> Result<SettingsWindow, Box<dyn Error>> {
    let window = SettingsWindow::new()?;
    window.set_startup_index(0);
    window.set_homepage("https://example.org/".into());
    window.set_engine_choices(strings(&[
        "DuckDuckGo",
        "Bing",
        "Brave Search",
        "Ecosia",
        "Google",
        "Startpage",
        "Wikipedia (en)",
        "Rust docs",
    ]));
    window.set_engine_index(5);
    let engine = |id: &str, name: &str, keyword: &str, custom: bool| SearchEngineRow {
        id: id.into(),
        name: name.into(),
        keyword: keyword.into(),
        custom,
    };
    window.set_engines(ModelRc::new(VecModel::from(vec![
        engine("duckduckgo", "DuckDuckGo", "@duckduckgo", false),
        engine("bing", "Bing", "@bing", false),
        engine("brave", "Brave Search", "@brave", false),
        engine("ecosia", "Ecosia", "@ecosia", false),
        engine("google", "Google", "@google", false),
        engine("startpage", "Startpage", "@startpage", false),
        engine("wikipedia", "Wikipedia (en)", "@wikipedia", false),
        engine("custom-rust-docs", "Rust docs", "@rs", true),
    ])));
    window.set_zoom_choices(strings(&[
        "30%", "50%", "67%", "80%", "90%", "100%", "110%", "120%", "133%", "150%", "170%", "200%",
        "240%", "300%", "400%", "500%",
    ]));
    window.set_zoom_index(5);
    window.set_theme_index(0);
    window.set_show_bookmarks_bar(true);
    window.set_block_ads_and_trackers(true);
    window.set_block_popups(true);
    window.set_clear_cookies_on_exit(true);
    window.set_offer_to_save_passwords(true);
    window.set_never_save_origins(strings(&["https://bank.example", "http://localhost:8080"]));
    window.set_experimental_web_features(true);
    window.set_gpu_canvas(true);
    Ok(window)
}

fn sample_main_window(theme: ChromeTheme) -> Result<MainWindow, Box<dyn Error>> {
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
        tab("New Tab", true),
        tab("Mercury Browser", false),
    ])));
    window.set_search_engine_name("Startpage".into());
    window.set_content_message("Searches go to Startpage.".into());
    window.set_show_home_button(true);
    window.set_settings_notice(
        "Your settings could not be read, so Mercury Browser is using its defaults. The old \
         file was kept as settings.json.damaged-1."
            .into(),
    );
    Ok(window)
}

fn main() -> Result<(), Box<dyn Error>> {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: render_settings <output directory>")?,
    );
    std::fs::create_dir_all(&output)?;
    slint::platform::set_platform(Box::new(HeadlessPlatform {
        windows: RefCell::default(),
    }))
    .map_err(|error| format!("{error:?}"))?;

    let sections = [
        "general",
        "appearance",
        "privacy",
        "downloads",
        "passwords",
        "advanced",
    ];
    for (scheme, scheme_name) in [(ColorScheme::Light, "light"), (ColorScheme::Dark, "dark")] {
        let window = sample_settings_window()?;
        window.set_forced_color_scheme(scheme);
        window.show()?;
        for (index, section) in sections.iter().enumerate() {
            window.set_section(i32::try_from(index)?);
            render(
                window.window(),
                1560,
                1240,
                &output.join(format!("settings-{scheme_name}-{index}-{section}.png")),
            )?;
        }
        window.set_section(0);
        window.set_editor_open(true);
        window.set_editor_name("Broken engine".into());
        window.set_editor_url("https://search.example/find".into());
        window.set_editor_problem("The template has no {searchTerms}.".into());
        window.set_homepage_problem(
            "Enter an http, https, file or about address, without a user name or password.".into(),
        );
        render(
            window.window(),
            1560,
            1800,
            &output.join(format!("settings-{scheme_name}-general-editor.png")),
        )?;
        window.hide()?;
    }

    // "Follow the system" on a dark system: the window is told the system's scheme.
    let window = sample_settings_window()?;
    window.set_forced_color_scheme(ColorScheme::Unknown);
    window.set_system_color_scheme(ColorScheme::Dark);
    window.show()?;
    window.set_section(2);
    render(
        window.window(),
        1560,
        1240,
        &output.join("settings-system-dark-privacy.png"),
    )?;
    window.set_section(0);
    window.set_editor_open(true);
    window.set_editor_name("Plain engine".into());
    window.set_editor_url("http://search.example/find".into());
    window.set_editor_warning(
        "This address starts with http: what you search for could be read on the way. Use \
         https if the engine offers it."
            .into(),
    );
    window.set_editor_problem("The template has no {searchTerms}.".into());
    window.set_homepage_problem(
        "Enter an http, https, file or about address, without a user name or password.".into(),
    );
    render(
        window.window(),
        1560,
        1900,
        &output.join("settings-system-dark-general-editor.png"),
    )?;
    window.hide()?;

    for (theme, name) in [(ChromeTheme::Light, "light"), (ChromeTheme::Dark, "dark")] {
        let window = sample_main_window(theme)?;
        window.show()?;
        render(
            window.window(),
            2000,
            1200,
            &output.join(format!("chrome-{name}.png")),
        )?;
        window.hide()?;
    }
    Ok(())
}
