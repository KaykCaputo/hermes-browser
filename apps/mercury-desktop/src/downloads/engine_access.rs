//! What a download needs from the engine: its user agent and its cookies for the download's
//! own URL. The engine is reached on the UI thread between pumps; while a pump holds it, the
//! request waits for a later turn rather than going out without cookies.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use talaria_servo::ServoRuntime;
use url::Url;

use super::slint_callback::InSlintCallback;

/// How often, and how many times, a busy engine is asked again before giving up (2 s).
const RETRY_INTERVAL: Duration = Duration::from_millis(50);
const ATTEMPTS: u32 = 40;

pub(crate) type Cookies = Vec<(String, String)>;
type Answer = Box<dyn FnOnce(String, Cookies)>;

/// The two things a download asks of the engine.
pub(crate) trait EngineAccess {
    fn user_agent(&self) -> String;
    /// Answers later, once the engine's network thread replies.
    fn cookies_for_url(&self, url: &Url, answer: Box<dyn FnOnce(Cookies)>);
}

impl EngineAccess for ServoRuntime {
    fn user_agent(&self) -> String {
        ServoRuntime::user_agent(self).to_owned()
    }

    fn cookies_for_url(&self, url: &Url, answer: Box<dyn FnOnce(Cookies)>) {
        ServoRuntime::cookies_for_url(self, url, answer);
    }
}

/// Asks `engine` for its user agent and for the cookies of `download_url` — the URL fetched,
/// never the page it was started from.
pub(crate) fn ask_engine(engine: &dyn EngineAccess, download_url: &Url, then: Answer) {
    let user_agent = engine.user_agent();
    engine.cookies_for_url(
        download_url,
        Box::new(move |cookies| then(user_agent, cookies)),
    );
}

/// `ask_engine` with the running engine: now, or on a later turn while a pump holds it.
/// `gave_up` runs if the engine is still unavailable after `ATTEMPTS` tries.
pub(crate) fn with_engine(download_url: Url, then: Answer, gave_up: Box<dyn FnOnce()>) {
    let pending = Rc::new(RefCell::new(Some((then, gave_up))));
    attempt(download_url, pending, 0);
}

type Pending = Rc<RefCell<Option<(Answer, Box<dyn FnOnce()>)>>>;

fn attempt(download_url: Url, pending: Pending, tries: u32) {
    let reached = crate::engine::with_runtime(|runtime| {
        if let Some((then, _)) = pending.borrow_mut().take() {
            ask_engine(runtime, &download_url, then);
        }
    });
    if reached.is_some() {
        return;
    }
    if tries + 1 >= ATTEMPTS {
        if let Some((_, gave_up)) = pending.borrow_mut().take() {
            log::warn!("the engine stayed unavailable; the download was not started");
            gave_up();
        }
        return;
    }
    slint::Timer::single_shot(RETRY_INTERVAL, move || {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        attempt(download_url, pending, tries + 1);
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct FakeEngine {
        asked: RefCell<Vec<String>>,
    }

    impl EngineAccess for FakeEngine {
        fn user_agent(&self) -> String {
            "Mozilla/5.0 Gecko/20100101 Firefox/153.0".to_owned()
        }

        fn cookies_for_url(&self, url: &Url, answer: Box<dyn FnOnce(Cookies)>) {
            self.asked.borrow_mut().push(url.to_string());
            answer(vec![(
                "k".to_owned(),
                format!("for {}", url.host_str().unwrap()),
            )]);
        }
    }

    #[test]
    fn the_cookies_asked_for_are_the_download_urls_own() {
        let engine = FakeEngine::default();
        let got = Rc::new(RefCell::new(None));
        let slot = Rc::clone(&got);
        ask_engine(
            &engine,
            &Url::parse("https://files.example.net/a.zip").unwrap(),
            Box::new(move |user_agent, cookies| *slot.borrow_mut() = Some((user_agent, cookies))),
        );
        assert_eq!(
            engine.asked.borrow().as_slice(),
            ["https://files.example.net/a.zip"]
        );
        assert_eq!(
            got.borrow().clone(),
            Some((
                "Mozilla/5.0 Gecko/20100101 Firefox/153.0".to_owned(),
                vec![("k".to_owned(), "for files.example.net".to_owned())]
            ))
        );
    }
}
