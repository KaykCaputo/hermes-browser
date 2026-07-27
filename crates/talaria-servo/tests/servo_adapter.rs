use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use servo::EventLoopWaker as ServoEventLoopWaker;
use talaria_core::WebView;
use talaria_servo::{EventLoopWaker, ServoWebView, ServoWebViewError};
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
