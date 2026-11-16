//! Renders the page-request components (dialog sheet and permission prompt) headlessly with
//! Slint's software renderer and checks how they answer keys and clicks. Set
//! `MERCURY_UI_RENDER_DIR` to also save each rendering as a PNG.

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, SharedString};

slint::slint! {
    import { PageRequestSheet, PageRequestStyle, PageRequestView, PermissionPrompt } from "../ui/dialogs/page-requests.slint";
    import { ChromePalette, ChromeTheme } from "../ui/theme/chrome-palette.slint";

    export { ChromePalette, ChromeTheme, PageRequestStyle, PageRequestView }

    // A stand-in for the browser window: toolbar, then a white page with the request above it.
    export component Harness inherits Window {
        in property <PageRequestView> request;
        callback answered(int, bool, string, string, bool);

        width: 900px;
        height: 520px;
        background: ChromePalette.window-background;

        Rectangle {
            y: 0px;
            height: 60px;
            background: ChromePalette.toolbar-background;

            Rectangle {
                x: 146px;
                y: 11px;
                width: parent.width - 160px;
                height: 38px;
                border-radius: 9px;
                border-width: 1px;
                border-color: ChromePalette.field-border;
                background: ChromePalette.field-background;

                Text {
                    x: 14px;
                    text: "http://127.0.0.1:8123/test.html";
                    color: ChromePalette.text;
                    font-size: 14px;
                    vertical-alignment: center;
                }
            }
        }

        content := Rectangle {
            y: 60px;
            height: parent.height - 60px;
            background: #ffffff;

            Text {
                x: 24px;
                y: 24px;
                text: "Page content beneath the request";
                color: #000000;
                font-size: 26px;
            }

            if root.request.style != PageRequestStyle.none && root.request.style != PageRequestStyle.permission: PageRequestSheet {
                request: root.request;
                answered(id, accepted, text, password, checked) => {
                    root.answered(id, accepted, text, password, checked);
                }
            }

            if root.request.style == PageRequestStyle.permission: PermissionPrompt {
                request: root.request;
                anchor-x: 146px;
                answered(id, accepted) => {
                    root.answered(id, accepted, "", "", false);
                }
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
const HEIGHT: u32 = 1040;

type Answer = (i32, bool, String, String, bool);

struct Fixture {
    /// Put before each saved file name: the theme being rendered.
    prefix: RefCell<String>,
    window: Rc<MinimalSoftwareWindow>,
    harness: Harness,
    answers: Rc<RefCell<Vec<Answer>>>,
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
        let answers = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&answers);
        harness.on_answered(move |id, accepted, text, password, checked| {
            recorded.borrow_mut().push((
                id,
                accepted,
                text.to_string(),
                password.to_string(),
                checked,
            ));
        });
        Self {
            prefix: RefCell::default(),
            window,
            harness,
            answers,
        }
    }

    fn show(&self, request: PageRequestView) {
        self.harness.set_request(request);
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
        let pixels = self.render();
        if let Some(directory) = std::env::var_os("MERCURY_UI_RENDER_DIR") {
            let bytes = pixels
                .iter()
                .flat_map(|pixel| [pixel.red, pixel.green, pixel.blue, 255])
                .collect::<Vec<_>>();
            let path = std::path::Path::new(&directory)
                .join(format!("{}-{name}.png", self.prefix.borrow()));
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

    fn key(&self, key: impl Into<SharedString>) {
        let text = key.into();
        self.window
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.window
            .dispatch_event(WindowEvent::KeyReleased { text });
        self.settle();
    }

    fn type_text(&self, text: &str) {
        for character in text.chars() {
            self.key(SharedString::from(character.to_string()));
        }
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

    fn take_answers(&self) -> Vec<Answer> {
        std::mem::take(&mut *self.answers.borrow_mut())
    }
}

fn pixel_at(pixels: &[PremultipliedRgbaColor], x: f32, y: f32) -> (u8, u8, u8) {
    let index = (y * SCALE) as usize * WIDTH as usize + (x * SCALE) as usize;
    let pixel = pixels[index];
    (pixel.red, pixel.green, pixel.blue)
}

fn view(id: i32, style: PageRequestStyle) -> PageRequestView {
    PageRequestView {
        id,
        style,
        ..PageRequestView::default()
    }
}

fn dialog(id: i32, style: PageRequestStyle, message: &str) -> PageRequestView {
    PageRequestView {
        heading: "127.0.0.1:8123 says".into(),
        message: message.into(),
        accept_label: "OK".into(),
        decline_label: if style == PageRequestStyle::Alert {
            ""
        } else {
            "Cancel"
        }
        .into(),
        ..view(id, style)
    }
}

/// Whether the page's white is dimmed by the scrim (in either theme).
fn is_dimmed(pixel: (u8, u8, u8)) -> bool {
    pixel.0 < 200 && pixel.1 < 200 && pixel.2 < 200
}

// Slint's platform is set once per thread, so every check runs in this one test, once for each
// chrome theme. System follows the window's colour scheme, which is unknown here: the chrome then
// stays dark.
#[test]
fn page_request_components_render_and_answer() {
    let fixture = Fixture::new();
    for (theme, name) in [
        (ChromeTheme::System, "system"),
        (ChromeTheme::Light, "light"),
        (ChromeTheme::Dark, "dark"),
    ] {
        fixture.harness.global::<ChromePalette>().set_theme(theme);
        *fixture.prefix.borrow_mut() = name.to_owned();
        render_and_answer(&fixture);
    }
}

fn render_and_answer(fixture: &Fixture) {
    // Nothing pending: the page is untouched.
    fixture.show(view(0, PageRequestStyle::None));
    let pixels = fixture.save("00-none");
    assert_eq!(pixel_at(&pixels, 450.0, 400.0), (255, 255, 255));

    // An alert: a sheet at the top of the content area over a dimmed page.
    fixture.show(dialog(
        1,
        PageRequestStyle::Alert,
        "Hello from the alert test page.\nThis is a second line.",
    ));
    let pixels = fixture.save("01-alert");
    let dimmed = pixel_at(&pixels, 40.0, 480.0);
    assert!(is_dimmed(dimmed), "the page beneath is dimmed: {dimmed:?}");
    let card = fixture
        .harness
        .global::<ChromePalette>()
        .get_sheet_background();
    assert_eq!(
        pixel_at(&pixels, 450.0, 64.0),
        (card.red(), card.green(), card.blue()),
        "the sheet's card"
    );
    // Escape gives an alert's only answer, OK.
    fixture.key(Key::Escape);
    assert_eq!(
        fixture.take_answers(),
        vec![(1, true, String::new(), String::new(), false)]
    );

    // A confirm with the block choice: Escape is Cancel, the checkbox is passed on.
    fixture.show(PageRequestView {
        choice_label: "Don't allow this page to show more dialogs".into(),
        ..dialog(2, PageRequestStyle::Confirm, "Do you want to continue?")
    });
    fixture.save("02-confirm-with-block-choice");
    fixture.key(Key::Escape);
    assert_eq!(
        fixture.take_answers(),
        vec![(2, false, String::new(), String::new(), false)]
    );

    // A confirm: Return is OK.
    fixture.show(dialog(
        3,
        PageRequestStyle::Confirm,
        "Do you want to continue?",
    ));
    fixture.key(Key::Return);
    assert_eq!(
        fixture.take_answers(),
        vec![(3, true, String::new(), String::new(), false)]
    );

    // A prompt's card has the keyboard, not its field: characters typed for the page (the end of
    // a password) do not reach the field, and Return is OK with the page's own default.
    fixture.show(PageRequestView {
        value: "Ada Lovelace".into(),
        ..dialog(4, PageRequestStyle::Prompt, "What is your name?")
    });
    fixture.save("03-prompt");
    fixture.type_text("ssword1");
    fixture.key(Key::Return);
    assert_eq!(
        fixture.take_answers(),
        vec![(4, true, "Ada Lovelace".to_owned(), String::new(), false)]
    );

    // Tab enters the field with the default selected, so typing replaces it.
    fixture.show(PageRequestView {
        value: "Ada Lovelace".into(),
        ..dialog(15, PageRequestStyle::Prompt, "What is your name?")
    });
    fixture.key(Key::Tab);
    fixture.save("03b-prompt-field-entered");
    fixture.type_text("Grace");
    fixture.key(Key::Return);
    assert_eq!(
        fixture.take_answers(),
        vec![(15, true, "Grace".to_owned(), String::new(), false)]
    );

    // Escape on a prompt is Cancel, from the card or from the field.
    fixture.show(PageRequestView {
        value: "x".into(),
        ..dialog(5, PageRequestStyle::Prompt, "Again?")
    });
    fixture.key(Key::Escape);
    assert_eq!(
        fixture.take_answers(),
        vec![(5, false, "x".to_owned(), String::new(), false)]
    );

    // A long message scrolls inside the sheet instead of growing past it.
    let long = "A long message from the page. ".repeat(200);
    fixture.show(dialog(6, PageRequestStyle::Alert, &long));
    let pixels = fixture.save("04-long-message");
    assert!(
        is_dimmed(pixel_at(&pixels, 40.0, 480.0)),
        "the sheet stays short"
    );
    fixture.key(Key::Return);
    fixture.take_answers();

    // Leave this page? Escape is Stay.
    fixture.show(PageRequestView {
        heading: "Leave this page?".into(),
        message: "Changes you made may not be saved.".into(),
        detail: "Asked by 127.0.0.1:8123".into(),
        accept_label: "Leave".into(),
        decline_label: "Stay".into(),
        ..view(7, PageRequestStyle::LeavePage)
    });
    fixture.save("05-leave-page");
    fixture.key(Key::Escape);
    assert_eq!(
        fixture.take_answers(),
        vec![(7, false, String::new(), String::new(), false)]
    );

    // Sign-in: the card has the keyboard, not a field, so keys meant for the page cannot land in
    // the user name. Return does nothing there; Tab enters the user-name field; Return moves to
    // the password; Return signs in.
    fixture.show(PageRequestView {
        heading: "Sign in to 127.0.0.1:8123".into(),
        message: "127.0.0.1:8123 needs a user name and password.".into(),
        warning: "This connection is not secure (not HTTPS): your user name and password will be sent unencrypted.".into(),
        accept_label: "Sign in".into(),
        decline_label: "Cancel".into(),
        ..view(8, PageRequestStyle::SignIn)
    });
    fixture.save("06-sign-in");
    fixture.type_text("typed for the page");
    fixture.key(Key::Return);
    assert_eq!(fixture.take_answers(), vec![]);
    fixture.key(Key::Tab);
    fixture.type_text("alice");
    fixture.key(Key::Return);
    fixture.type_text("secret");
    fixture.save("07-sign-in-filled");
    fixture.key(Key::Return);
    assert_eq!(
        fixture.take_answers(),
        vec![(8, true, "alice".to_owned(), "secret".to_owned(), false)]
    );

    // A click on the dimmed page is not an answer.
    fixture.show(dialog(9, PageRequestStyle::Alert, "Click elsewhere"));
    fixture.click(40.0, 480.0);
    assert_eq!(fixture.take_answers(), vec![]);
    fixture.key(Key::Return);
    fixture.take_answers();

    // A sign-in's Escape is Cancel even before a field was chosen.
    fixture.show(PageRequestView {
        heading: "Sign in to 127.0.0.1:8123".into(),
        accept_label: "Sign in".into(),
        decline_label: "Cancel".into(),
        ..view(11, PageRequestStyle::SignIn)
    });
    fixture.key(Key::Escape);
    assert_eq!(
        fixture.take_answers(),
        vec![(11, false, String::new(), String::new(), false)]
    );

    // Stacked combining marks and a 253-character host stay inside the card once the message
    // has been cleaned as the browser cleans it.
    let stacked = format!("x{}", "\u{0301}".repeat(500));
    fixture.show(dialog(
        12,
        PageRequestStyle::Alert,
        &talaria_core::sanitize_page_text(&stacked, true, talaria_core::MAX_MESSAGE_CHARS),
    ));
    fixture.save("09-combining-marks");
    fixture.key(Key::Return);
    fixture.take_answers();
    let label = [
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(61),
    ]
    .join(".");
    assert_eq!(label.len(), 253);
    fixture.show(PageRequestView {
        heading: format!("{label} says").into(),
        ..dialog(
            13,
            PageRequestStyle::Confirm,
            "A page on a very long host name.",
        )
    });
    let pixels = fixture.save("10-long-host");
    assert!(
        is_dimmed(pixel_at(&pixels, 40.0, 480.0)),
        "the sheet stays inside the page"
    );
    fixture.key(Key::Escape);
    fixture.take_answers();

    // A reload prompt with the repeated-prompt choice.
    fixture.show(PageRequestView {
        heading: "Reload this page?".into(),
        message: "Changes you made may not be saved.".into(),
        detail: "Asked by 127.0.0.1:8123".into(),
        accept_label: "Reload".into(),
        decline_label: "Stay".into(),
        choice_label: "Don't allow this page to ask again".into(),
        ..view(14, PageRequestStyle::LeavePage)
    });
    fixture.save("11-reload-page");
    fixture.key(Key::Escape);
    assert_eq!(
        fixture.take_answers(),
        vec![(14, false, String::new(), String::new(), false)]
    );

    // A permission prompt under the address bar never takes the keyboard: neither Return nor
    // Escape reaches it (the browser window handles Escape); it is answered with the mouse.
    fixture.show(PageRequestView {
        heading: "127.0.0.1:8123 wants to store data permanently on this device".into(),
        accept_label: "Allow".into(),
        decline_label: "Block".into(),
        ..view(10, PageRequestStyle::Permission)
    });
    let pixels = fixture.save("08-permission");
    assert_eq!(
        pixel_at(&pixels, 40.0, 480.0),
        (255, 255, 255),
        "the page beneath a permission prompt is not dimmed"
    );
    fixture.key(Key::Return);
    fixture.key(Key::Escape);
    assert_eq!(fixture.take_answers(), vec![]);

    // Keyboard users reach its buttons with Tab: Block first, then Allow.
    fixture.key(Key::Tab);
    fixture.save("12-permission-block-focused");
    fixture.key(Key::Return);
    assert_eq!(
        fixture.take_answers(),
        vec![(10, false, String::new(), String::new(), false)]
    );
    // A new bubble starts without focus.
    fixture.show(view(0, PageRequestStyle::None));
    fixture.show(PageRequestView {
        heading: "127.0.0.1:8123 wants to store data permanently on this device".into(),
        accept_label: "Allow".into(),
        decline_label: "Block".into(),
        ..view(16, PageRequestStyle::Permission)
    });
    fixture.key(Key::Tab);
    fixture.key(Key::Tab);
    fixture.key(SharedString::from(" "));
    assert_eq!(
        fixture.take_answers(),
        vec![(16, true, String::new(), String::new(), false)]
    );
}
