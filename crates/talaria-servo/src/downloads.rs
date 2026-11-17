//! What a download fetched outside the engine needs from it: the cookies it holds for a URL.
//!
//! Servo's cookie store applies domain, path, `Secure` and `HttpOnly` (`CookieSource::HTTP`
//! includes `HttpOnly` cookies, as an HTTP request would). It does not enforce `SameSite` in
//! 0.8.0 (research report R7, gap H8 and item A6: no `SameSite` logic in `servo-net`), so
//! `Strict` cookies come back too. Mercury asks only for downloads the user started or
//! accepted, top-level requests for which browsers send `Lax` cookies as well; a `Strict`
//! cookie may reach a cross-site download link here when a browser would hold it back.

use std::panic::AssertUnwindSafe;

use url::Url;

/// Asks Servo's network thread for the cookies it would send with an HTTP request to `url`;
/// `answer` runs later, on the thread that spins the engine, with name and value pairs. A
/// panic while asking (the network thread has gone, during shutdown) answers at once with
/// no cookies.
pub(crate) fn cookies_for_url(
    servo: &servo::Servo,
    url: &Url,
    answer: impl FnOnce(Vec<(String, String)>) + 'static,
) {
    let answer = std::rc::Rc::new(std::cell::RefCell::new(Some(answer)));
    let deliver = std::rc::Rc::clone(&answer);
    let asked = std::panic::catch_unwind(AssertUnwindSafe(|| {
        servo.site_data_manager().cookies_for_url_async(
            url.clone(),
            servo::CookieSource::HTTP,
            move |cookies| {
                if let Some(answer) = deliver.borrow_mut().take() {
                    answer(
                        cookies
                            .iter()
                            .map(|cookie| (cookie.name().to_owned(), cookie.value().to_owned()))
                            .collect(),
                    );
                }
            },
        );
    }));
    if asked.is_err() {
        log::error!("the engine did not answer a request for cookies");
        if let Some(answer) = answer.borrow_mut().take() {
            answer(Vec::new());
        }
    }
}
