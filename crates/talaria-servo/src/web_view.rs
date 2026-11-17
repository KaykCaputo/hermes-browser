use std::cell::Cell;
use std::rc::Rc;

use dpi::PhysicalSize;
use euclid::{Point2D, Scale};
use servo::{
    ClipboardAction, Code, CompositionEvent as ServoCompositionEvent,
    CompositionState as ServoCompositionState, DevicePoint, ImeEvent as ServoImeEvent, InputEvent,
    Key, KeyState as ServoKeyState, KeyboardEvent as ServoKeyboardEvent, Location, Modifiers,
    MouseButtonAction as ServoMouseButtonAction, MouseButtonEvent, MouseLeftViewportEvent,
    MouseMoveEvent, NamedKey, Theme, WebViewPoint, WheelDelta, WheelEvent, WheelMode,
};
use talaria_core::{
    ClipboardCommand, ColorScheme, CompositionState, ControlAnswer, ControlId, ImeInput,
    KeyLocation, KeyState, KeyboardInput, PointerButton, PointerButtonAction, TabId,
    WebView as TalariaWebView,
};
use talaria_core::{PageRequestAnswer, PageRequestId};
use url::Url;

use crate::document_starts::DocumentStarts;
use crate::embedder_controls::EmbedderControls;
use crate::frame_ready::FrameReadySignal;
use crate::page_feedback::{DOCUMENT_TITLE_SCRIPT, PageFeedback};
use crate::page_requests::{ClosesOnDrop, EngineRequest, Navigation, ServoPageRequest};
use crate::session_data::{AfterEngineCleanup, HandleThenCleanup};
use crate::{ServoWebViewError, ViewportSize};

pub struct ServoWebView {
    /// Servo's `WebView` holds handles to Servo (its own and its user-content manager's); the
    /// share of the after-engine clean-up beside it is released only after them.
    inner: HandleThenCleanup<servo::WebView, AfterEngineCleanup>,
    frame_ready: FrameReadySignal,
    tab_id: TabId,
    /// Device pixels per logical pixel; pointer positions arrive in logical pixels.
    scale_factor: Cell<f32>,
    // controls
    controls: EmbedderControls,
    // dialogs: closes the delegate's store when the web view goes, which gives every
    // unanswered request the engine's default.
    page_requests: ClosesOnDrop<ServoPageRequest>,
    document_start: Rc<DocumentStarts>,
    // tabs
    page_feedback: Rc<PageFeedback>,
}

/// What a tab's web view shares with its delegate.
pub(crate) struct TabParts {
    pub(crate) frame_ready: FrameReadySignal,
    pub(crate) controls: EmbedderControls,
    pub(crate) page_requests: ClosesOnDrop<ServoPageRequest>,
    pub(crate) document_start: Rc<DocumentStarts>,
    pub(crate) page_feedback: Rc<PageFeedback>,
}

impl ServoWebView {
    pub(crate) fn new(
        inner: HandleThenCleanup<servo::WebView, AfterEngineCleanup>,
        tab_id: TabId,
        scale_factor: f32,
        parts: TabParts,
    ) -> Self {
        let TabParts {
            frame_ready,
            controls,
            page_requests,
            document_start,
            page_feedback,
        } = parts;
        Self {
            inner,
            frame_ready,
            tab_id,
            scale_factor: Cell::new(scale_factor),
            controls,
            page_requests,
            document_start,
            page_feedback,
        }
    }

    // dialogs: the navigation's document may start without a `Started` Servo reports.
    // tabs: the tab's user activation is forgotten as the navigation is sent.
    fn arm_document_start(&self, navigation: Navigation) {
        self.document_start
            .navigation_sent(navigation, self.inner.load_status());
    }

    /// Starts a title read that was owed while another was in flight. Cheap when none is owed;
    /// call once per engine pump.
    pub fn refresh_page_feedback(&self) {
        self.page_feedback.refresh_title(|callback| {
            self.inner
                .evaluate_javascript(DOCUMENT_TITLE_SCRIPT, callback);
        });
    }

    pub fn resize(&self, size: ViewportSize) {
        self.inner
            .resize(PhysicalSize::new(size.width(), size.height()));
    }

    pub fn set_scale_factor(&self, scale_factor: f32) {
        self.scale_factor.set(scale_factor);
        self.inner.set_hidpi_scale_factor(Scale::new(scale_factor));
    }

    fn web_view_point(&self, x: f32, y: f32) -> WebViewPoint {
        web_view_point(x, y, self.scale_factor.get())
    }

    pub fn paint_if_available(&self) -> bool {
        if !self.frame_ready.take_ready() {
            return false;
        }

        self.inner.paint();
        true
    }

    /// Paints the shared rendering context now, whether or not a new frame was announced.
    pub fn paint(&self) {
        self.inner.paint();
    }

    /// Forgets a frame announced before now. A hidden web view is told about every frame of the
    /// shared rendering context, including frames drawn while another tab was shown; after
    /// `show()` only a frame announced later contains this web view.
    pub fn discard_ready_frame(&self) {
        let _was_ready = self.frame_ready.take_ready();
    }

    pub fn show(&self) {
        self.inner.show();
    }

    pub fn hide(&self) {
        self.inner.hide();
    }
}

impl Drop for ServoWebView {
    fn drop(&mut self) {
        log::info!("closing Servo WebView for tab {}", self.tab_id.value());
        // controls: requests of a closed or replaced web view get the engine's default answer.
        let _hidden = self.controls.drop_all();
    }
}

impl TalariaWebView for ServoWebView {
    type Error = ServoWebViewError;

    fn navigate(&mut self, url: &str) -> Result<(), Self::Error> {
        let url = Url::parse(url)?;
        self.arm_document_start(Navigation::Load); // dialogs
        self.inner.load(url);
        Ok(())
    }

    fn go_back(&mut self) -> Result<(), Self::Error> {
        self.arm_document_start(Navigation::Traverse); // dialogs
        let _traversal_id = self.inner.go_back(1);
        Ok(())
    }

    fn go_forward(&mut self) -> Result<(), Self::Error> {
        self.arm_document_start(Navigation::Traverse); // dialogs
        let _traversal_id = self.inner.go_forward(1);
        Ok(())
    }

    fn reload(&mut self) -> Result<(), Self::Error> {
        self.arm_document_start(Navigation::Reload); // dialogs
        self.inner.reload();
        Ok(())
    }

    fn pointer_moved(&mut self, x: f32, y: f32) -> Result<(), Self::Error> {
        let _event_id = self
            .inner
            .notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(
                self.web_view_point(x, y),
            )));

        Ok(())
    }

    fn pointer_button(
        &mut self,
        action: PointerButtonAction,
        button: PointerButton,
        x: f32,
        y: f32,
    ) -> Result<(), Self::Error> {
        let _event_id =
            self.inner
                .notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                    servo_mouse_button_action(action),
                    servo_mouse_button(button),
                    self.web_view_point(x, y),
                )));

        Ok(())
    }

    fn scroll(&mut self, delta_x: f32, delta_y: f32, x: f32, y: f32) -> Result<(), Self::Error> {
        let delta = wheel_delta(delta_x, delta_y, self.scale_factor.get());
        let _event_id = self
            .inner
            .notify_input_event(InputEvent::Wheel(WheelEvent::new(
                delta,
                self.web_view_point(x, y),
            )));

        Ok(())
    }

    fn set_focus(&mut self, focused: bool) -> Result<(), Self::Error> {
        self.inner.set_focused(focused);
        Ok(())
    }

    fn keyboard_input(&mut self, event: KeyboardInput) -> Result<(), Self::Error> {
        let keyboard_event = ServoKeyboardEvent::new_without_event(
            servo_key_state(event.state),
            event
                .key
                .parse()
                .unwrap_or(Key::Named(NamedKey::Unidentified)),
            event.code.parse().unwrap_or(Code::Unidentified),
            servo_key_location(event.location),
            servo_modifiers(event.modifiers),
            event.repeat,
            event.is_composing,
        );
        let _event_id = self
            .inner
            .notify_input_event(InputEvent::Keyboard(keyboard_event));
        Ok(())
    }

    fn ime_input(&mut self, event: ImeInput) -> Result<(), Self::Error> {
        let event = match event {
            ImeInput::Composition { state, data } => {
                ServoImeEvent::Composition(ServoCompositionEvent {
                    state: servo_composition_state(state),
                    data,
                })
            }
            ImeInput::Dismissed => ServoImeEvent::Dismissed,
        };
        let _event_id = self.inner.notify_input_event(InputEvent::Ime(event));
        Ok(())
    }

    fn set_page_zoom(&mut self, percent: u16) -> Result<(), Self::Error> {
        self.inner.set_page_zoom(page_zoom_factor(percent));
        Ok(())
    }

    fn pinch_zoom(&mut self, delta: f32, x: f32, y: f32) -> Result<(), Self::Error> {
        let scale_factor = self.scale_factor.get();
        self.inner
            .adjust_pinch_zoom(delta, device_point(x, y, scale_factor));
        Ok(())
    }

    fn set_color_scheme(&mut self, scheme: ColorScheme) -> Result<(), Self::Error> {
        self.inner.notify_theme_change(servo_theme(scheme));
        Ok(())
    }

    fn exit_fullscreen(&mut self) -> Result<(), Self::Error> {
        self.inner.exit_fullscreen();
        Ok(())
    }

    fn pointer_left(&mut self) -> Result<(), Self::Error> {
        let _event_id = self.inner.notify_input_event(InputEvent::MouseLeftViewport(
            MouseLeftViewportEvent::default(),
        ));
        Ok(())
    }

    // controls
    fn answer_control(&mut self, id: ControlId, answer: ControlAnswer) -> Result<(), Self::Error> {
        self.controls.answer(id, answer);
        Ok(())
    }

    // dialogs
    fn answer_page_request(
        &mut self,
        id: PageRequestId,
        answer: PageRequestAnswer,
    ) -> Result<(), Self::Error> {
        // Taken out first, so answering never runs while the store is borrowed.
        let request = self.page_requests.take(id);
        // Staying on a page that asked before it was left: no new document follows.
        if matches!(request, Some(ServoPageRequest::Unload(_)))
            && answer == PageRequestAnswer::Decline
        {
            self.document_start.navigation_cancelled();
        }
        match request {
            Some(request) => request.respond(answer),
            None => log::debug!(
                "tab {}: page request {} is gone",
                self.tab_id.value(),
                id.value()
            ),
        }
        Ok(())
    }

    // menus
    fn clipboard(&mut self, command: ClipboardCommand) -> Result<(), Self::Error> {
        let _event_id = self
            .inner
            .notify_input_event(InputEvent::EditingAction(servo_clipboard_action(command)));
        Ok(())
    }
}

/// The same editing Servo performs for Cmd/Ctrl+X, C and V in the page.
fn servo_clipboard_action(command: ClipboardCommand) -> ClipboardAction {
    match command {
        ClipboardCommand::Cut => ClipboardAction::Cut,
        ClipboardCommand::Copy => ClipboardAction::Copy,
        ClipboardCommand::Paste => ClipboardAction::Paste,
    }
}

fn servo_theme(scheme: ColorScheme) -> Theme {
    match scheme {
        ColorScheme::Light => Theme::Light,
        ColorScheme::Dark => Theme::Dark,
    }
}

fn page_zoom_factor(percent: u16) -> f32 {
    f32::from(percent) / 100.0
}

fn servo_key_state(state: KeyState) -> ServoKeyState {
    match state {
        KeyState::Down => ServoKeyState::Down,
        KeyState::Up => ServoKeyState::Up,
    }
}

fn servo_key_location(location: KeyLocation) -> Location {
    match location {
        KeyLocation::Standard => Location::Standard,
        KeyLocation::Left => Location::Left,
        KeyLocation::Right => Location::Right,
        KeyLocation::Numpad => Location::Numpad,
    }
}

fn servo_modifiers(modifiers: talaria_core::KeyboardModifiers) -> Modifiers {
    let mut servo_modifiers = Modifiers::empty();

    if modifiers.alt {
        servo_modifiers.insert(Modifiers::ALT);
    }
    if modifiers.control {
        servo_modifiers.insert(Modifiers::CONTROL);
    }
    if modifiers.meta {
        servo_modifiers.insert(Modifiers::META);
    }
    if modifiers.shift {
        servo_modifiers.insert(Modifiers::SHIFT);
    }

    servo_modifiers
}

fn servo_composition_state(state: CompositionState) -> ServoCompositionState {
    match state {
        CompositionState::Start => ServoCompositionState::Start,
        CompositionState::Update => ServoCompositionState::Update,
        CompositionState::End => ServoCompositionState::End,
    }
}

/// Converts a logical-pixel position to device pixels. Servo multiplies `Page` points by a
/// scale that includes page and pinch zoom, so `Device` points are the ones that stay under
/// the pointer once a page is zoomed.
fn device_point(x: f32, y: f32, scale_factor: f32) -> DevicePoint {
    Point2D::new(x * scale_factor, y * scale_factor)
}

/// Slint reports wheel deltas in logical pixels (trackpad pixel deltas are converted to
/// logical, mouse-wheel lines become 60 logical pixels each); Servo scrolls by the delta as
/// device pixels.
fn wheel_delta(delta_x: f32, delta_y: f32, scale_factor: f32) -> WheelDelta {
    WheelDelta {
        x: f64::from(delta_x * scale_factor),
        y: f64::from(delta_y * scale_factor),
        z: 0.0,
        mode: WheelMode::DeltaPixel,
    }
}

fn web_view_point(x: f32, y: f32, scale_factor: f32) -> WebViewPoint {
    WebViewPoint::Device(device_point(x, y, scale_factor))
}

fn servo_mouse_button_action(action: PointerButtonAction) -> ServoMouseButtonAction {
    match action {
        PointerButtonAction::Down => ServoMouseButtonAction::Down,
        PointerButtonAction::Up => ServoMouseButtonAction::Up,
    }
}

fn servo_mouse_button(button: PointerButton) -> servo::MouseButton {
    match button {
        PointerButton::Left => servo::MouseButton::Primary,
        PointerButton::Middle => servo::MouseButton::Auxiliary,
        PointerButton::Right => servo::MouseButton::Secondary,
        PointerButton::Other => servo::MouseButton::Other(5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_commands_become_servo_editing_actions() {
        assert_eq!(
            servo_clipboard_action(ClipboardCommand::Cut),
            ClipboardAction::Cut
        );
        assert_eq!(
            servo_clipboard_action(ClipboardCommand::Copy),
            ClipboardAction::Copy
        );
        assert_eq!(
            servo_clipboard_action(ClipboardCommand::Paste),
            ClipboardAction::Paste
        );
    }

    #[test]
    fn logical_coordinates_become_device_coordinates() {
        assert_eq!(
            web_view_point(12.5, 24.0, 2.0),
            WebViewPoint::Device(Point2D::new(25.0, 48.0))
        );
    }

    #[test]
    fn logical_wheel_deltas_become_device_pixel_deltas() {
        assert_eq!(
            wheel_delta(-2.0, 18.0, 2.0),
            WheelDelta {
                x: -4.0,
                y: 36.0,
                z: 0.0,
                mode: WheelMode::DeltaPixel,
            }
        );
    }

    #[test]
    fn zoom_percentages_become_servo_zoom_factors() {
        assert_eq!(page_zoom_factor(100), 1.0);
        assert_eq!(page_zoom_factor(30), 0.3);
        assert_eq!(page_zoom_factor(500), 5.0);
    }

    #[test]
    fn colour_schemes_map_to_servo_themes() {
        assert_eq!(servo_theme(ColorScheme::Light), Theme::Light);
        assert_eq!(servo_theme(ColorScheme::Dark), Theme::Dark);
    }

    #[test]
    fn pointer_button_actions_map_to_servo_actions() {
        assert_eq!(
            servo_mouse_button_action(PointerButtonAction::Down),
            ServoMouseButtonAction::Down
        );
        assert_eq!(
            servo_mouse_button_action(PointerButtonAction::Up),
            ServoMouseButtonAction::Up
        );
    }

    #[test]
    fn pointer_buttons_map_to_servo_buttons() {
        assert_eq!(
            servo_mouse_button(PointerButton::Left),
            servo::MouseButton::Primary
        );
        assert_eq!(
            servo_mouse_button(PointerButton::Middle),
            servo::MouseButton::Auxiliary
        );
        assert_eq!(
            servo_mouse_button(PointerButton::Right),
            servo::MouseButton::Secondary
        );
        assert_eq!(
            servo_mouse_button(PointerButton::Other),
            servo::MouseButton::Other(5)
        );
    }

    #[test]
    fn keyboard_state_location_and_modifiers_map_to_servo() {
        assert_eq!(servo_key_state(KeyState::Down), ServoKeyState::Down);
        assert_eq!(servo_key_state(KeyState::Up), ServoKeyState::Up);
        assert_eq!(servo_key_location(KeyLocation::Numpad), Location::Numpad);

        let modifiers = servo_modifiers(talaria_core::KeyboardModifiers {
            alt: true,
            control: true,
            meta: false,
            shift: true,
        });

        assert!(modifiers.contains(Modifiers::ALT));
        assert!(modifiers.contains(Modifiers::CONTROL));
        assert!(modifiers.contains(Modifiers::SHIFT));
        assert!(!modifiers.contains(Modifiers::META));
    }

    #[test]
    fn composition_states_map_to_servo() {
        assert_eq!(
            servo_composition_state(CompositionState::Start),
            ServoCompositionState::Start
        );
        assert_eq!(
            servo_composition_state(CompositionState::Update),
            ServoCompositionState::Update
        );
        assert_eq!(
            servo_composition_state(CompositionState::End),
            ServoCompositionState::End
        );
    }
}
