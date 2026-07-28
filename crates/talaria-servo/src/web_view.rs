use dpi::PhysicalSize;
use euclid::{Point2D, Scale};
use servo::{
    Code, CompositionEvent as ServoCompositionEvent, CompositionState as ServoCompositionState,
    ImeEvent as ServoImeEvent, InputEvent, Key, KeyState as ServoKeyState,
    KeyboardEvent as ServoKeyboardEvent, Location, Modifiers,
    MouseButtonAction as ServoMouseButtonAction, MouseButtonEvent, MouseMoveEvent, NamedKey,
    WebViewPoint, WheelDelta, WheelEvent, WheelMode,
};
use talaria_core::{
    CompositionState, ImeInput, KeyLocation, KeyState, KeyboardInput, PointerButton,
    PointerButtonAction, TabId, WebView as TalariaWebView,
};
use url::Url;

use crate::frame_ready::FrameReadySignal;
use crate::{ServoWebViewError, ViewportSize};

pub struct ServoWebView {
    inner: servo::WebView,
    frame_ready: FrameReadySignal,
    tab_id: TabId,
}

impl ServoWebView {
    pub(crate) fn new(inner: servo::WebView, frame_ready: FrameReadySignal, tab_id: TabId) -> Self {
        Self {
            inner,
            frame_ready,
            tab_id,
        }
    }

    pub fn resize(&self, size: ViewportSize) {
        self.inner
            .resize(PhysicalSize::new(size.width(), size.height()));
    }

    pub fn set_scale_factor(&self, scale_factor: f32) {
        self.inner.set_hidpi_scale_factor(Scale::new(scale_factor));
    }

    pub fn paint_if_available(&self) -> bool {
        if !self.frame_ready.has_frame() {
            return false;
        }

        self.inner.paint();
        true
    }

    pub fn show(&self) {
        self.inner.show();
    }

    pub fn hide(&self) {
        self.inner.hide();
    }

    pub fn set_throttled(&self, throttled: bool) {
        self.inner.set_throttled(throttled);
    }
}

impl Drop for ServoWebView {
    fn drop(&mut self) {
        log::info!("closing Servo WebView for tab {}", self.tab_id.value());
    }
}

impl TalariaWebView for ServoWebView {
    type Error = ServoWebViewError;

    fn navigate(&mut self, url: &str) -> Result<(), Self::Error> {
        let url = Url::parse(url)?;
        self.inner.load(url);
        Ok(())
    }

    fn go_back(&mut self) -> Result<(), Self::Error> {
        let _traversal_id = self.inner.go_back(1);
        Ok(())
    }

    fn go_forward(&mut self) -> Result<(), Self::Error> {
        let _traversal_id = self.inner.go_forward(1);
        Ok(())
    }

    fn reload(&mut self) -> Result<(), Self::Error> {
        self.inner.reload();
        Ok(())
    }

    fn pointer_moved(&mut self, x: f32, y: f32) -> Result<(), Self::Error> {
        let _event_id = self
            .inner
            .notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(web_view_point(
                x, y,
            ))));

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
                    web_view_point(x, y),
                )));

        Ok(())
    }

    fn scroll(&mut self, delta_x: f32, delta_y: f32, x: f32, y: f32) -> Result<(), Self::Error> {
        let delta = WheelDelta {
            x: f64::from(delta_x),
            y: f64::from(delta_y),
            z: 0.0,
            mode: WheelMode::DeltaPixel,
        };
        let _event_id = self
            .inner
            .notify_input_event(InputEvent::Wheel(WheelEvent::new(
                delta,
                web_view_point(x, y),
            )));

        Ok(())
    }

    fn set_focus(&mut self, focused: bool) -> Result<(), Self::Error> {
        if focused {
            self.inner.focus();
        } else {
            self.inner.blur();
        }

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

fn web_view_point(x: f32, y: f32) -> WebViewPoint {
    WebViewPoint::Page(Point2D::new(x, y))
}

fn servo_mouse_button_action(action: PointerButtonAction) -> ServoMouseButtonAction {
    match action {
        PointerButtonAction::Down => ServoMouseButtonAction::Down,
        PointerButtonAction::Up => ServoMouseButtonAction::Up,
    }
}

fn servo_mouse_button(button: PointerButton) -> servo::MouseButton {
    match button {
        PointerButton::Left => servo::MouseButton::Left,
        PointerButton::Middle => servo::MouseButton::Middle,
        PointerButton::Right => servo::MouseButton::Right,
        PointerButton::Other => servo::MouseButton::Other(5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_coordinates_become_page_coordinates() {
        assert_eq!(
            web_view_point(12.5, 24.0),
            WebViewPoint::Page(Point2D::new(12.5, 24.0))
        );
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
            servo::MouseButton::Left
        );
        assert_eq!(
            servo_mouse_button(PointerButton::Middle),
            servo::MouseButton::Middle
        );
        assert_eq!(
            servo_mouse_button(PointerButton::Right),
            servo::MouseButton::Right
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
