use dpi::PhysicalSize;
use euclid::{Point2D, Scale};
use servo::{
    InputEvent, MouseButtonAction as ServoMouseButtonAction, MouseButtonEvent, MouseMoveEvent,
    WebViewPoint, WheelDelta, WheelEvent, WheelMode,
};
use talaria_core::{PointerButton, PointerButtonAction, WebView as TalariaWebView};
use url::Url;

use crate::frame_ready::FrameReadySignal;
use crate::{ServoWebViewError, ViewportSize};

pub struct ServoWebView {
    inner: servo::WebView,
    frame_ready: FrameReadySignal,
}

impl ServoWebView {
    pub(crate) fn new(inner: servo::WebView, frame_ready: FrameReadySignal) -> Self {
        Self { inner, frame_ready }
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
}
