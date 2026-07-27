use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use servo::{EventLoopWaker as ServoEventLoopWaker, RenderingContext};
use talaria_core::WebView;
use talaria_servo::{
    EventLoopWaker, ServoWebView, ServoWebViewError, SharedOpenGlContext, ViewportSize,
};
use url::Url;

fn invalid_url_error() -> ServoWebViewError {
    let parse_error = Url::parse("not a valid absolute URL")
        .expect_err("a relative URL without a base should be rejected");

    ServoWebViewError::from(parse_error)
}

#[test]
fn invalid_url_error_has_a_readable_message() {
    let error = invalid_url_error();

    assert!(error.to_string().starts_with("invalid URL: "));
}

#[test]
fn invalid_url_error_preserves_its_source() {
    let error = invalid_url_error();

    assert!(error.source().is_some());
}

#[test]
fn servo_web_view_implements_the_core_web_view_contract() {
    fn assert_web_view<T>()
    where
        T: WebView<Error = ServoWebViewError>,
    {
    }

    assert_web_view::<ServoWebView>();
}

#[test]
fn viewport_size_rejects_zero_dimensions() {
    assert_eq!(ViewportSize::new(0, 720), None);
    assert_eq!(ViewportSize::new(1280, 0), None);
    assert_eq!(ViewportSize::new(0, 0), None);
}

#[test]
fn viewport_size_preserves_non_zero_dimensions() {
    let size = ViewportSize::new(1280, 720).expect("non-zero dimensions should be accepted");

    assert_eq!(size.width(), 1280);
    assert_eq!(size.height(), 720);
}

#[test]
fn viewport_size_converts_logical_dimensions_to_physical_pixels() {
    let size = ViewportSize::from_logical(800.0, 600.0, 1.5)
        .expect("valid logical dimensions should be converted");

    assert_eq!(size.width(), 1200);
    assert_eq!(size.height(), 900);
}

#[test]
fn viewport_size_rounds_fractional_physical_pixels() {
    let size = ViewportSize::from_logical(100.25, 100.5, 1.5)
        .expect("positive fractional dimensions should be converted");

    assert_eq!(size.width(), 150);
    assert_eq!(size.height(), 151);
}

#[test]
fn viewport_size_rejects_invalid_logical_dimensions_and_scale_factors() {
    assert_eq!(ViewportSize::from_logical(0.0, 600.0, 1.0), None);
    assert_eq!(ViewportSize::from_logical(800.0, -1.0, 1.0), None);
    assert_eq!(ViewportSize::from_logical(800.0, 600.0, 0.0), None);
    assert_eq!(ViewportSize::from_logical(f32::NAN, 600.0, 1.0), None);
    assert_eq!(ViewportSize::from_logical(800.0, f32::INFINITY, 1.0), None);
    assert_eq!(ViewportSize::from_logical(800.0, 600.0, f32::NAN), None);
}

#[test]
fn shared_opengl_context_implements_the_servo_rendering_contract() {
    fn assert_rendering_context<T: RenderingContext>() {}

    assert_rendering_context::<SharedOpenGlContext>();
}

#[test]
fn event_loop_waker_invokes_its_callback() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let callback_call_count = Arc::clone(&call_count);
    let waker = EventLoopWaker::new(move || {
        callback_call_count.fetch_add(1, Ordering::Relaxed);
    });

    ServoEventLoopWaker::wake(&waker);

    assert_eq!(call_count.load(Ordering::Relaxed), 1);
}

#[test]
fn cloned_event_loop_waker_shares_the_same_callback() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let callback_call_count = Arc::clone(&call_count);
    let waker = EventLoopWaker::new(move || {
        callback_call_count.fetch_add(1, Ordering::Relaxed);
    });
    let cloned_waker = ServoEventLoopWaker::clone_box(&waker);

    ServoEventLoopWaker::wake(&waker);
    cloned_waker.wake();

    assert_eq!(call_count.load(Ordering::Relaxed), 2);
}
