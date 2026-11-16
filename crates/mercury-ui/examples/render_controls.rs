//! Renders the page-control popups headless, through Slint's software renderer, to PNG files:
//! `cargo run --release -p mercury-ui --example render_controls -- <output directory>`.
//!
//! The popups only appear after a click in a page, which cannot be synthesised on the
//! development machine; these renderings are how their look is checked.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::language::ColorScheme;
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Model, ModelRc, PhysicalSize, PlatformError, VecModel};

slint::slint! {
    import { SelectPopup, SelectPopupRow } from "../ui/controls/select-popup.slint";
    import { ColorPicker } from "../ui/controls/color-picker.slint";
    import { ContextMenuPopup, ContextMenuRow } from "../ui/controls/context-menu.slint";
    import { Palette } from "std-widgets.slint";
    export { ChromePalette, ChromeTheme } from "../ui/theme/chrome-palette.slint";

    // A `<select>` box on the page with its list open below it.
    export component SelectPreview inherits Window {
        in property <bool> dark;
        // The system's scheme, for the "System" theme.
        in property <ColorScheme> system-scheme: ColorScheme.unknown;
        changed system-scheme => {
            Palette.color-scheme = self.system-scheme;
        }
        background: root.dark ? #202124 : #f1f3f4;
        in property <[SelectPopupRow]> rows;
        in property <bool> multiple;
        in property <int> highlighted: 5;
        in-out property <length> scroll-y;
        width: 300px;
        height: 340px;

        Rectangle {
            x: 20px;
            y: 16px;
            width: 180px;
            height: 26px;
            border-width: 1px;
            border-radius: 4px;
            border-color: #8a8a8a;
            background: root.dark ? #303134 : #ffffff;
            Text {
                x: 8px;
                text: "Blueberry ▾";
                color: root.dark ? #e8eaed : #202124;
                vertical-alignment: center;
            }
        }

        SelectPopup {
            x: 20px;
            y: 42px;
            width: 220px;
            height: root.multiple ? 290px : 250px;
            rows: root.rows;
            highlighted: root.highlighted;
            scroll-y <=> root.scroll-y;
            multiple: root.multiple;
            origin: "A page on https://shop.example";
        }
    }

    export component ColorPreview inherits Window {
        in property <bool> dark;
        // The system's scheme, for the "System" theme.
        in property <ColorScheme> system-scheme: ColorScheme.unknown;
        changed system-scheme => {
            Palette.color-scheme = self.system-scheme;
        }
        background: root.dark ? #202124 : #f1f3f4;
        width: 272px;
        height: 330px;

        ColorPicker {
            x: 20px;
            y: 24px;
            width: 232px;
            height: 282px;
            origin: "A page on https://paint.example";
            swatches: [
                #000000, #555555, #aaaaaa, #ffffff, #e53935, #fb8c00, #fdd835, #43a047,
                #00897b, #00acc1, #1e88e5, #3949ab, #8e24aa, #d81b60, #6d4c41, #546e7a,
            ];
            current: #1e88e5;
            hex: "#1e88e5";
            hue: 0.5806;
            lightness: 0.5078;
        }
    }

    export component MenuPreview inherits Window {
        in property <bool> dark;
        // The system's scheme, for the "System" theme.
        in property <ColorScheme> system-scheme: ColorScheme.unknown;
        changed system-scheme => {
            Palette.color-scheme = self.system-scheme;
        }
        background: root.dark ? #202124 : #f1f3f4;
        in property <[ContextMenuRow]> rows;
        in property <length> menu-height;
        width: 280px;
        height: 430px;

        ContextMenuPopup {
            x: 30px;
            y: 20px;
            width: 220px;
            height: root.menu-height;
            rows: root.rows;
            highlighted: 0;
        }
    }
}

/// Sets the appearance setting and, for "System", the system's scheme.
trait Themed {
    fn apply(&self, theme: ChromeTheme, system_scheme: ColorScheme, dark: bool);
}

macro_rules! themed {
    ($($component:ty),*) => {$(
        impl Themed for $component {
            fn apply(&self, theme: ChromeTheme, system_scheme: ColorScheme, dark: bool) {
                self.global::<ChromePalette>().set_theme(theme);
                self.set_system_scheme(system_scheme);
                self.set_dark(dark);
            }
        }
    )*};
}

themed!(SelectPreview, ColorPreview, MenuPreview);

struct HeadlessPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }
}

const SCALE_FACTOR: f32 = 2.0;

fn main() -> Result<(), Box<dyn Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: render_controls <output directory>")?;
    std::fs::create_dir_all(&output)?;

    let window = MinimalSoftwareWindow::new(Default::default());
    slint::platform::set_platform(Box::new(HeadlessPlatform {
        window: window.clone(),
    }))
    .map_err(|error| format!("{error:?}"))?;
    window.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: SCALE_FACTOR,
    });

    for (theme, chrome_theme, system_scheme, dark) in [
        ("light", ChromeTheme::Light, ColorScheme::Unknown, false),
        ("dark", ChromeTheme::Dark, ColorScheme::Unknown, true),
        (
            "system-light",
            ChromeTheme::System,
            ColorScheme::Light,
            false,
        ),
        ("system-dark", ChromeTheme::System, ColorScheme::Dark, true),
    ] {
        let themed = |component: &dyn Themed| {
            component.apply(chrome_theme, system_scheme, dark);
        };

        let select = SelectPreview::new()?;
        themed(&select);
        select.set_rows(select_rows());
        render(
            &window,
            &select,
            300,
            340,
            &output.join(format!("select-{theme}.png")),
        )?;
        select.set_multiple(true);
        render(
            &window,
            &select,
            300,
            340,
            &output.join(format!("select-multiple-{theme}.png")),
        )?;
        drop(select);

        // 100,000 options, scrolled to the highlighted one near the middle.
        let long = SelectPreview::new()?;
        themed(&long);
        long.set_rows(ModelRc::new(VecModel::from(
            (0..100_000)
                .map(|index| SelectPopupRow {
                    label: format!("Option {index}").into(),
                    heading: false,
                    disabled: index % 7 == 3,
                    selected: index == 50_000,
                    in_group: false,
                })
                .collect::<Vec<_>>(),
        )));
        long.set_highlighted(50_002);
        long.set_scroll_y(-(50_000.0 * 24.0));
        let started_at = std::time::Instant::now();
        render(
            &window,
            &long,
            300,
            340,
            &output.join(format!("select-long-{theme}.png")),
        )?;
        println!("rendered 100,000 options in {:?}", started_at.elapsed());
        drop(long);

        let colour = ColorPreview::new()?;
        themed(&colour);
        render(
            &window,
            &colour,
            272,
            330,
            &output.join(format!("color-{theme}.png")),
        )?;
        drop(colour);

        for (name, rows) in [("link", link_menu()), ("image", image_menu())] {
            let menu = MenuPreview::new()?;
            themed(&menu);
            menu.set_menu_height(menu_height(&rows));
            menu.set_rows(rows);
            render(
                &window,
                &menu,
                280,
                430,
                &output.join(format!("menu-{name}-{theme}.png")),
            )?;
        }
    }
    Ok(())
}

fn render(
    window: &MinimalSoftwareWindow,
    component: &impl ComponentHandle,
    width: u32,
    height: u32,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    let width = (width as f32 * SCALE_FACTOR) as u32;
    let height = (height as f32 * SCALE_FACTOR) as u32;
    component.show()?;
    window.set_size(PhysicalSize::new(width, height));
    window.request_redraw();
    let mut pixels = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, width as usize);
    });
    component.hide()?;

    let mut image = image::RgbaImage::new(width, height);
    for (pixel, rendered) in image.pixels_mut().zip(&pixels) {
        // The page background is opaque, so the premultiplied colour is the colour.
        *pixel = image::Rgba([rendered.red, rendered.green, rendered.blue, 255]);
    }
    image.save(path)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn select_rows() -> ModelRc<SelectPopupRow> {
    let option = |label: &str, disabled: bool, selected: bool, in_group: bool| SelectPopupRow {
        label: label.into(),
        heading: false,
        disabled,
        selected,
        in_group,
    };
    let heading = |label: &str| SelectPopupRow {
        label: label.into(),
        heading: true,
        disabled: false,
        selected: false,
        in_group: false,
    };
    ModelRc::new(VecModel::from(vec![
        option("None", false, false, false),
        heading("Fruit"),
        option("Apple", false, false, true),
        option("Banana (out of stock)", true, false, true),
        option("Blueberry", false, true, true),
        option("Cherry", false, false, true),
        heading("Other"),
        option("Bread", false, false, true),
        option(
            "A very long option label that does not fit in the list and is elided",
            false,
            false,
            true,
        ),
        option("Zucchini", true, false, true),
    ]))
}

fn menu_rows(items: &[(&str, bool)]) -> ModelRc<ContextMenuRow> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|(label, enabled)| ContextMenuRow {
                label: (*label).into(),
                enabled: *enabled,
                separator: label.is_empty(),
            })
            .collect::<Vec<_>>(),
    ))
}

/// The menu `build_menu` makes for a link with nothing selected, on a page with history.
fn link_menu() -> ModelRc<ContextMenuRow> {
    menu_rows(&[
        ("Open link in new tab", true),
        ("Copy link address", true),
        ("", false),
        ("Cut", false),
        ("Copy", false),
        ("Paste", false),
        ("Select all", true),
        ("", false),
        ("Back", true),
        ("Forward", false),
        ("Reload", true),
    ])
}

fn image_menu() -> ModelRc<ContextMenuRow> {
    menu_rows(&[
        ("Open image in new tab", true),
        ("Copy image address", true),
        ("", false),
        ("Cut", false),
        ("Copy", false),
        ("Paste", false),
        ("Select all", true),
        ("", false),
        ("Back", false),
        ("Forward", false),
        ("Reload", true),
    ])
}

fn menu_height(rows: &ModelRc<ContextMenuRow>) -> f32 {
    8.0 + rows
        .iter()
        .map(|row| if row.separator { 9.0 } else { 26.0 })
        .sum::<f32>()
}
