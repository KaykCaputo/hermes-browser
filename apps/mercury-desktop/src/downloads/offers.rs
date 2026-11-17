//! Pages the engine could not show (its "Unknown content type" page), offered as downloads.
//!
//! The decision for one offer, in order:
//! 1. only `http` and `https` can be fetched again: `blob:`, `data:`, `file:` and the rest
//!    are refused ("cannot be saved");
//! 2. a page fetched by a form's POST is refused: its body is never seen by Mercury and must
//!    not be sent twice;
//! 3. a site the user blocked in this session is refused silently;
//! 4. with a user gesture in the tab within the activation window, the file is downloaded;
//! 5. otherwise the tab shows a bar, "<site> wants to download <name>", at most
//!    `ASK_LIMIT` times per tab in `ASK_WINDOW`; offers beyond that are dropped silently.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use talaria_core::TabId;
use url::Url;

/// How many bars one tab may raise in `ASK_WINDOW`.
pub(crate) const ASK_LIMIT: usize = 3;
pub(crate) const ASK_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The address's scheme cannot be fetched again.
    Unsupported(String),
    /// The page came from a form's POST.
    Posted,
    /// The user blocked downloads from this site.
    Blocked,
    /// The tab raised too many bars recently.
    RateLimited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    Download,
    Ask,
    Refuse(Refusal),
}

/// What is remembered across offers: blocked sites and recent bars per tab.
#[derive(Debug, Default)]
pub(crate) struct OfferPolicy {
    blocked: HashSet<String>,
    asked: HashMap<TabId, VecDeque<Instant>>,
}

/// The site a decision is about: the URL's origin.
pub(crate) fn site_of(url: &Url) -> String {
    url.origin().ascii_serialization()
}

impl OfferPolicy {
    pub(crate) fn decide(
        &mut self,
        tab: TabId,
        url: &Url,
        posted: bool,
        user_activated: bool,
        now: Instant,
    ) -> Decision {
        if !matches!(url.scheme(), "http" | "https") {
            return Decision::Refuse(Refusal::Unsupported(url.scheme().to_owned()));
        }
        if posted {
            return Decision::Refuse(Refusal::Posted);
        }
        if self.blocked.contains(&site_of(url)) {
            return Decision::Refuse(Refusal::Blocked);
        }
        if user_activated {
            return Decision::Download;
        }
        let asked = self.asked.entry(tab).or_default();
        while asked
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= ASK_WINDOW)
        {
            asked.pop_front();
        }
        if asked.len() >= ASK_LIMIT {
            return Decision::Refuse(Refusal::RateLimited);
        }
        asked.push_back(now);
        Decision::Ask
    }

    /// The user chose Block on a bar: no more offers from this site in this session.
    pub(crate) fn block(&mut self, url: &Url) {
        self.blocked.insert(site_of(url));
    }

    pub(crate) fn forget_tab(&mut self, tab: TabId) {
        self.asked.remove(&tab);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn the_decision_table() {
        let now = Instant::now();
        let tab = TabId::new(1);
        let file = url("https://files.example.com/a.zip");
        let mut policy = OfferPolicy::default();

        // Schemes that cannot be fetched again, with or without a gesture.
        for (address, scheme) in [
            ("blob:https://example.com/0f2c", "blob"),
            ("data:application/octet-stream;base64,aGk=", "data"),
            ("file:///Users/me/a.zip", "file"),
            ("ftp://example.com/a.zip", "ftp"),
        ] {
            for activated in [false, true] {
                assert_eq!(
                    policy.decide(tab, &url(address), false, activated, now),
                    Decision::Refuse(Refusal::Unsupported(scheme.into())),
                    "{address}"
                );
            }
        }
        // A form's POST, with or without a gesture.
        for activated in [false, true] {
            assert_eq!(
                policy.decide(tab, &file, true, activated, now),
                Decision::Refuse(Refusal::Posted)
            );
        }
        // A gesture downloads; without one the user is asked.
        assert_eq!(
            policy.decide(tab, &file, false, true, now),
            Decision::Download
        );
        assert_eq!(policy.decide(tab, &file, false, false, now), Decision::Ask);
        assert_eq!(
            policy.decide(tab, &url("http://127.0.0.1:8000/x.bin"), false, false, now),
            Decision::Ask
        );
    }

    #[test]
    fn a_blocked_site_is_refused_even_after_a_gesture() {
        let now = Instant::now();
        let tab = TabId::new(1);
        let mut policy = OfferPolicy::default();
        policy.block(&url("https://files.example.com/a.zip"));
        for activated in [false, true] {
            assert_eq!(
                policy.decide(
                    tab,
                    &url("https://files.example.com/other.iso"),
                    false,
                    activated,
                    now
                ),
                Decision::Refuse(Refusal::Blocked)
            );
        }
        // Only that origin: another port or host is not blocked.
        assert_eq!(
            policy.decide(
                tab,
                &url("https://files.example.com:8443/a.zip"),
                false,
                true,
                now
            ),
            Decision::Download
        );
        assert_eq!(
            policy.decide(tab, &url("https://example.com/a.zip"), false, true, now),
            Decision::Download
        );
    }

    #[test]
    fn bars_are_limited_per_tab_and_window() {
        let start = Instant::now();
        let first = TabId::new(1);
        let second = TabId::new(2);
        let file = url("https://example.com/a.zip");
        let mut policy = OfferPolicy::default();
        for _ in 0..ASK_LIMIT {
            assert_eq!(
                policy.decide(first, &file, false, false, start),
                Decision::Ask
            );
        }
        assert_eq!(
            policy.decide(first, &file, false, false, start + Duration::from_secs(1)),
            Decision::Refuse(Refusal::RateLimited)
        );
        // Another tab has its own allowance, and a gesture is never rate limited.
        assert_eq!(
            policy.decide(second, &file, false, false, start),
            Decision::Ask
        );
        assert_eq!(
            policy.decide(first, &file, false, true, start + Duration::from_secs(1)),
            Decision::Download
        );
        // Once the window has passed, the tab may ask again.
        assert_eq!(
            policy.decide(first, &file, false, false, start + ASK_WINDOW),
            Decision::Ask
        );
        policy.forget_tab(first);
        assert_eq!(
            policy.decide(first, &file, false, false, start + ASK_WINDOW),
            Decision::Ask
        );
    }

    #[test]
    fn the_site_is_the_origin() {
        assert_eq!(
            site_of(&url("https://user:pw@example.com:8443/a/b.zip?x#y")),
            "https://example.com:8443"
        );
    }
}
