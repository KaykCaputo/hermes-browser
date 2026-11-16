//! What a download fetched outside the engine needs from it: the cookies it holds for a URL.

use std::panic::AssertUnwindSafe;

use url::Url;

/// Servo's cookies for `url`, as it would send them with an HTTP request. A panic (the network
/// thread has gone, during shutdown) gives no cookies rather than taking the browser down.
pub(crate) fn cookies_for_url(servo: &servo::Servo, url: &Url) -> Vec<(String, String)> {
    let asked = std::panic::catch_unwind(AssertUnwindSafe(|| {
        servo
            .site_data_manager()
            .cookies_for_url(url.clone(), servo::CookieSource::HTTP)
    }));
    match asked {
        Ok(cookies) => cookies
            .iter()
            .map(|cookie| (cookie.name().to_owned(), cookie.value().to_owned()))
            .collect(),
        Err(_) => {
            log::error!("the engine did not answer a request for cookies");
            Vec::new()
        }
    }
}
