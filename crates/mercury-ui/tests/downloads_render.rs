//! Renders the downloads components (toolbar button, popover, full list, quit question)
//! headlessly with Slint's software renderer, in the dark and light palettes, and checks how
//! they answer clicks. Set `MERCURY_UI_RENDER_DIR` to also save each rendering as a PNG.

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, PhysicalSize, VecModel};

slint::slint! {
    import { DownloadsButton, DownloadsPopover, DownloadsPanel, QuitDownloadsPrompt, DownloadOfferBar, DownloadsPalette, DownloadRow, DownloadAction } from "../ui/downloads/downloads.slint";

    export { DownloadsPalette, DownloadRow, DownloadAction }

    // A stand-in for the browser window: tab strip, toolbar with the downloads button, a white
    // page, and the downloads UI over it as the main window places it.
    export component Harness inherits Window {
        in property <[DownloadRow]> rows;
        in property <bool> button-active;
        in property <float> button-progress;
        in property <bool> popover-open;
        in property <bool> panel-open;
        in property <bool> prompt-open;
        in property <bool> can-show-more;
        in property <bool> bar-open;
        in property <bool> attention;
        callback acted(string, DownloadAction);
        callback quit-answer(bool);
        callback bar-answer(string);
        property <length> anchor-right;
        property <length> anchor-bottom;

        width: 900px;
        height: 640px;
        background: DownloadsPalette.dark ? #0b0f14 : #e9edf2;

        Rectangle {
            y: 48px;
            height: 60px;
            background: DownloadsPalette.dark ? #11171f : #f6f8fa;

            Rectangle {
                x: 14px;
                y: 11px;
                width: parent.width - 100px;
                height: 38px;
                border-radius: 9px;
                border-width: 1px;
                border-color: DownloadsPalette.border;
                background: DownloadsPalette.surface-raised;

                Text {
                    x: 14px;
                    text: "http://127.0.0.1:8123/downloads.html";
                    color: DownloadsPalette.text;
                    font-size: 14px;
                    vertical-alignment: center;
                }
            }

            DownloadsButton {
                x: parent.width - 80px;
                y: 13px;
                active: root.button-active;
                progress: root.button-progress;
                open: root.popover-open;
                attention: root.attention;
                anchor-changed(right, bottom) => {
                    root.anchor-right = right;
                    root.anchor-bottom = bottom;
                }
            }
        }

        Rectangle {
            y: 108px;
            height: parent.height - 108px;
            background: #ffffff;

            Text {
                x: 24px;
                y: 24px;
                text: "Page content";
                color: #000000;
                font-size: 26px;
            }

            if root.bar-open: DownloadOfferBar {
                x: 0px;
                y: 0px;
                width: parent.width;
                text: "127.0.0.1:8123 wants to download installer.dmg";
                download => {
                    root.bar-answer("download");
                }
                block => {
                    root.bar-answer("block");
                }
                dismiss => {
                    root.bar-answer("dismiss");
                }
            }

            if root.panel-open: DownloadsPanel {
                width: parent.width;
                height: parent.height;
                rows: root.rows;
                show-label: "Show in Finder";
                can-show-more: root.can-show-more;
                action(guid, action) => {
                    root.acted(guid, action);
                }
            }
        }

        if root.popover-open: DownloadsPopover {
            x: root.anchor-right - self.width;
            y: root.anchor-bottom + 6px;
            rows: root.rows;
            show-label: "Show in Finder";
            action(guid, action) => {
                root.acted(guid, action);
            }
        }

        if root.prompt-open: QuitDownloadsPrompt {
            width: root.width;
            height: root.height;
            title: "Cancel 2 downloads and quit?";
            body: "If the servers allow it, you can resume them from the downloads list the next time you open Mercury Browser.";
            confirm-label: "Cancel downloads and quit";
            keep-label: "Keep downloading";
            confirm => {
                root.quit-answer(true);
            }
            keep => {
                root.quit-answer(false);
            }
        }
    }
}

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const SCALE: f32 = 2.0;
const WIDTH: u32 = 1800;
const HEIGHT: u32 = 1280;

fn row(guid: &str, name: &str, status: &str, progress: f32) -> DownloadRow {
    DownloadRow {
        guid: guid.into(),
        name: name.into(),
        status: status.into(),
        progress,
        failed: false,
        can_open: false,
        can_show: false,
        can_cancel: false,
        can_resume: false,
        can_retry: false,
        can_remove: false,
    }
}

/// The sample records every rendering uses.
fn sample_rows() -> Vec<DownloadRow> {
    vec![
        DownloadRow {
            can_cancel: true,
            ..row(
                "running",
                "ubuntu-26.04-desktop-arm64.iso",
                "1.2 GB of 4.9 GB — 18.4 MB/s, 4 min left",
                0.25,
            )
        },
        DownloadRow {
            can_cancel: true,
            ..row("starting", "report-2026-q3.csv", "Starting…", 2.0)
        },
        DownloadRow {
            can_open: true,
            can_show: true,
            can_remove: true,
            ..row(
                "done",
                "annual-report.pdf",
                "2.4 MB — files.example.com",
                -1.0,
            )
        },
        DownloadRow {
            can_resume: true,
            can_cancel: true,
            can_remove: true,
            ..row(
                "interrupted",
                "dataset.tar.gz",
                "Interrupted — 310.0 MB of 1.1 GB",
                0.28,
            )
        },
        DownloadRow {
            failed: true,
            can_retry: true,
            can_remove: true,
            ..row(
                "failed",
                "installer.pkg",
                "Failed — the server answered HTTP 404 Not Found",
                -1.0,
            )
        },
        DownloadRow {
            can_show: true,
            can_remove: true,
            ..row(
                "program",
                "Setup.app.zip.app",
                "18.0 MB — example.org",
                -1.0,
            )
        },
    ]
}

struct Fixture {
    window: Rc<MinimalSoftwareWindow>,
    harness: Harness,
    acted: Rc<RefCell<Vec<(String, DownloadAction)>>>,
    answers: Rc<RefCell<Vec<bool>>>,
    bar_answers: Rc<RefCell<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(window.clone())))
            .expect("the platform is set once");
        let harness = Harness::new().expect("the harness builds");
        window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: SCALE,
        });
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        harness.show().expect("the harness shows");
        let acted = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&acted);
        harness.on_acted(move |guid, action| {
            recorded.borrow_mut().push((guid.to_string(), action));
        });
        let answers = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&answers);
        harness.on_quit_answer(move |confirmed| recorded.borrow_mut().push(confirmed));
        harness.set_rows(ModelRc::new(VecModel::from(sample_rows())));
        let bar_answers = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&bar_answers);
        harness.on_bar_answer(move |answer| recorded.borrow_mut().push(answer.to_string()));
        Self {
            window,
            harness,
            acted,
            answers,
            bar_answers,
        }
    }

    fn reset(&self, dark: bool) {
        self.harness.global::<DownloadsPalette>().set_dark(dark);
        self.harness.set_popover_open(false);
        self.harness.set_panel_open(false);
        self.harness.set_prompt_open(false);
        self.harness.set_button_active(false);
        self.harness.set_can_show_more(false);
        self.harness.set_bar_open(false);
        self.harness.set_attention(false);
        self.settle();
    }

    fn settle(&self) {
        for _ in 0..3 {
            slint::platform::update_timers_and_animations();
            let _ = self.render();
        }
    }

    fn render(&self) -> Vec<PremultipliedRgbaColor> {
        self.window.request_redraw();
        let mut buffer = vec![PremultipliedRgbaColor::default(); (WIDTH * HEIGHT) as usize];
        self.window.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, WIDTH as usize);
        });
        buffer
    }

    fn save(&self, name: &str) -> Vec<PremultipliedRgbaColor> {
        self.settle();
        let pixels = self.render();
        if let Some(directory) = std::env::var_os("MERCURY_UI_RENDER_DIR") {
            let bytes = pixels
                .iter()
                .flat_map(|pixel| [pixel.red, pixel.green, pixel.blue, 255])
                .collect::<Vec<_>>();
            let path = std::path::Path::new(&directory).join(format!("{name}.png"));
            image::save_buffer(
                &path,
                &bytes,
                WIDTH,
                HEIGHT,
                image::ExtendedColorType::Rgba8,
            )
            .expect("the PNG is written");
        }
        pixels
    }

    fn click(&self, x: f32, y: f32) {
        let position = LogicalPosition::new(x, y);
        self.window
            .dispatch_event(WindowEvent::PointerMoved { position });
        self.window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        self.window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        self.settle();
    }

    fn key(&self, key: impl Into<slint::SharedString>) {
        let text = key.into();
        self.window
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.window
            .dispatch_event(WindowEvent::KeyReleased { text });
        self.settle();
    }

    fn pixel(pixels: &[PremultipliedRgbaColor], x: f32, y: f32) -> (u8, u8, u8) {
        let index = (y * SCALE) as usize * WIDTH as usize + (x * SCALE) as usize;
        let pixel = pixels[index];
        (pixel.red, pixel.green, pixel.blue)
    }
}

// One test: Slint's platform can be set only once per process.
#[test]
fn the_downloads_components_render_and_answer_clicks() {
    let fixture = Fixture::new();

    for (theme, dark) in [("dark", true), ("light", false)] {
        fixture.reset(dark);
        fixture.harness.set_button_active(true);
        fixture.harness.set_button_progress(0.25);
        fixture.save(&format!("downloads-01-button-active-{theme}"));

        fixture.harness.set_popover_open(true);
        let pixels = fixture.save(&format!("downloads-02-popover-{theme}"));
        // The popover's surface covers the page under it.
        let surface = Fixture::pixel(&pixels, 700.0, 200.0);
        let page = Fixture::pixel(&pixels, 100.0, 300.0);
        assert_eq!(page, (255, 255, 255));
        assert_ne!(surface, page, "{theme}");

        fixture.harness.set_popover_open(false);
        fixture.harness.set_panel_open(true);
        fixture.harness.set_can_show_more(true);
        fixture.save(&format!("downloads-03-panel-{theme}"));

        fixture.harness.set_panel_open(false);
        fixture.harness.set_prompt_open(true);
        fixture.save(&format!("downloads-04-quit-prompt-{theme}"));

        fixture.reset(dark);
        fixture.harness.set_bar_open(true);
        fixture.harness.set_attention(true);
        fixture.save(&format!("downloads-05-offer-bar-{theme}"));
    }

    // Clicks: the first row's Cancel in the popover, then the quit prompt's two answers, and a
    // click outside the prompt's card is not an answer.
    fixture.reset(true);
    fixture.harness.set_popover_open(true);
    fixture.settle();
    // The popover hangs from the button's lower right corner (856, 95); the first row's only
    // button ("Cancel") is at its right edge, read off the rendering.
    fixture.click(811.0, 176.0);
    assert_eq!(
        fixture.acted.borrow().as_slice(),
        &[("running".to_owned(), DownloadAction::Cancel)]
    );

    fixture.reset(true);
    fixture.harness.set_prompt_open(true);
    fixture.settle();
    fixture.click(20.0, 20.0);
    assert!(fixture.answers.borrow().is_empty());
    // The card is centred, 40 px above the middle: its buttons are on one row near its foot.
    fixture.click(KEEP.0, KEEP.1);
    fixture.click(CONFIRM.0, CONFIRM.1);
    assert_eq!(fixture.answers.borrow().as_slice(), &[false, true]);

    // The keyboard: the default answer has the focus, Return presses it, Tab moves to the
    // other button, Escape keeps downloading from anywhere in the question.
    let keyboard = |keys: &[Key]| {
        fixture.reset(true);
        fixture.answers.borrow_mut().clear();
        fixture.harness.set_prompt_open(true);
        fixture.settle();
        for key in keys {
            fixture.key(*key);
        }
        let answers = fixture.answers.borrow().clone();
        fixture.harness.set_prompt_open(false);
        answers
    };
    fixture.reset(true);
    fixture.harness.set_prompt_open(true);
    fixture.save("downloads-06-quit-prompt-focus");
    assert_eq!(keyboard(&[Key::Return]), [false]);
    assert_eq!(keyboard(&[Key::Tab, Key::Return]), [true]);
    assert_eq!(keyboard(&[Key::Tab, Key::Escape]), [false]);

    // The bar's three answers.
    fixture.reset(true);
    fixture.harness.set_bar_open(true);
    fixture.settle();
    for (x, answer) in [
        (BAR_DOWNLOAD, "download"),
        (BAR_BLOCK, "block"),
        (BAR_DISMISS, "dismiss"),
    ] {
        fixture.click(x, 108.0 + 22.0);
        assert_eq!(
            fixture.bar_answers.borrow().last().map(String::as_str),
            Some(answer)
        );
    }
}

/// Centres of the bar's buttons in the 900-wide harness, read off its rendering.
const BAR_DOWNLOAD: f32 = 723.0;
const BAR_BLOCK: f32 = 793.0;
const BAR_DISMISS: f32 = 857.0;

/// Centres of the quit prompt's buttons in the 900×640 harness, read off its rendering.
const KEEP: (f32, f32) = (576.0, 315.0);
const CONFIRM: (f32, f32) = (421.0, 315.0);
