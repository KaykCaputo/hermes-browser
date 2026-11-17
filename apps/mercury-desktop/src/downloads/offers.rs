//! Pages the engine could not show (its "Unknown content type" page), offered as downloads.
//!
//! Who started the navigation is decided when its top-level request *starts*
//! (`Navigations::request_started`), so a slow server does not make a click look like the page's
//! own doing: a gesture in the tab (consumed there), or an address the user typed for exactly
//! that URL. Redirect hops belong to the navigation they continue.
//!
//! The decision for one offer, in order:
//! 1. an offer from a tab in the background is dropped silently: no bar, no download;
//! 2. only `http` and `https` can be fetched again: `blob:`, `data:`, `file:` and the rest
//!    are refused ("cannot be saved");
//! 3. a page fetched by a form's POST is refused: its body is never seen by Mercury and must
//!    not be sent twice;
//! 4. a page the user blocked in this session (by the origin of the page that started it) is
//!    refused silently;
//! 5. a navigation the user started, fetched by a known GET, is downloaded;
//! 6. otherwise (the page started it, or its method is not known) the tab shows a bar,
//!    "<page> wants to download <name>", at most `ASK_LIMIT` times per tab in `ASK_WINDOW`;
//!    offers beyond that are dropped silently. Nothing is ever fetched again silently on an
//!    unknown method.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use talaria_core::TabId;
use url::Url;

/// How many bars one tab may raise in `ASK_WINDOW`.
pub(crate) const ASK_LIMIT: usize = 3;
pub(crate) const ASK_WINDOW: Duration = Duration::from_secs(60);
/// How long an address the user typed waits for its request to start.
pub(crate) const TYPED_LIFETIME: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The tab is not the one the user is looking at.
    Background,
    /// The address's scheme cannot be fetched again.
    Unsupported(String),
    /// The page came from a form's POST.
    Posted,
    /// The user blocked downloads started by this page's origin.
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

/// One offer, as the decision sees it.
#[derive(Debug, Clone)]
pub(crate) struct OfferFacts<'a> {
    pub(crate) tab: TabId,
    pub(crate) url: &'a Url,
    /// `Some(true)`: a form's POST; `None`: not known.
    pub(crate) posted: Option<bool>,
    pub(crate) user_started: bool,
    pub(crate) foreground: bool,
    /// The origin of the page that started it (`site_of`).
    pub(crate) initiator_site: &'a str,
}

/// What is remembered across offers: blocked pages and recent bars per tab.
#[derive(Debug, Default)]
pub(crate) struct OfferPolicy {
    blocked: HashSet<String>,
    asked: HashMap<TabId, VecDeque<Instant>>,
}

/// The origin a decision is about, as text.
pub(crate) fn site_of(url: &Url) -> String {
    url.origin().ascii_serialization()
}

/// The page that started a download, for its bar and for Block: the request's referrer, else
/// the page the tab showed when the request started, else (a tab that opened on the file)
/// the file's own URL.
pub(crate) fn initiator<'a>(
    referrer: Option<&'a Url>,
    from: Option<&'a Url>,
    file: &'a Url,
) -> &'a Url {
    referrer.or(from).unwrap_or(file)
}

/// "host" or "host:port", for people.
pub(crate) fn host_label(url: &Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_owned(),
        (None, _) => site_of(url),
    }
}

impl OfferPolicy {
    pub(crate) fn decide(&mut self, offer: &OfferFacts<'_>, now: Instant) -> Decision {
        if !offer.foreground {
            return Decision::Refuse(Refusal::Background);
        }
        if !matches!(offer.url.scheme(), "http" | "https") {
            return Decision::Refuse(Refusal::Unsupported(offer.url.scheme().to_owned()));
        }
        if offer.posted == Some(true) {
            return Decision::Refuse(Refusal::Posted);
        }
        if self.blocked.contains(offer.initiator_site) {
            return Decision::Refuse(Refusal::Blocked);
        }
        if offer.user_started && offer.posted == Some(false) {
            return Decision::Download;
        }
        let asked = self.asked.entry(offer.tab).or_default();
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

    /// The user chose Block on a bar: no more offers started by this page origin.
    pub(crate) fn block(&mut self, initiator_site: &str) {
        self.blocked.insert(initiator_site.to_owned());
    }

    pub(crate) fn forget_tab(&mut self, tab: TabId) {
        self.asked.remove(&tab);
    }
}

/// The top-level navigation of a tab: the URLs of its hops, who started it, and the page it
/// started from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Navigation {
    pub(crate) urls: Vec<String>,
    pub(crate) user_started: bool,
    pub(crate) from: Option<String>,
}

/// Typed addresses waiting for their request, and each tab's current navigation.
#[derive(Debug, Default)]
pub(crate) struct Navigations {
    typed: HashMap<TabId, (String, Instant)>,
    current: HashMap<TabId, Navigation>,
}

impl Navigations {
    /// The user typed `url` (resolved) into the address bar of `tab`.
    pub(crate) fn note_typed(&mut self, tab: TabId, url: &str, now: Instant) {
        self.typed.insert(tab, (url.to_owned(), now));
    }

    /// A top-level request of `tab` started. A new navigation is the user's if it is the
    /// address they just typed, or if `take_gesture` finds a gesture in the tab (it consumes
    /// it); a redirect hop continues the navigation before it.
    pub(crate) fn request_started(
        &mut self,
        tab: TabId,
        url: &str,
        redirect: bool,
        from: Option<&str>,
        now: Instant,
        take_gesture: impl FnOnce() -> bool,
    ) {
        if redirect && let Some(navigation) = self.current.get_mut(&tab) {
            navigation.urls.push(url.to_owned());
            return;
        }
        // The typed note waits for its own URL (another request may come first, from the page
        // being left), and is used once.
        let typed = self.typed.get(&tab).is_some_and(|(typed, at)| {
            typed == url && now.saturating_duration_since(*at) <= TYPED_LIFETIME
        });
        if typed {
            self.typed.remove(&tab);
        }
        // A typed address is the user's without a gesture; otherwise the gesture is taken now.
        let user_started = typed || take_gesture();
        self.current.insert(
            tab,
            Navigation {
                urls: vec![url.to_owned()],
                user_started,
                from: if typed { None } else { from.map(str::to_owned) },
            },
        );
    }

    /// The navigation that ended at `url` in `tab`, taken (it is decided once).
    pub(crate) fn take(&mut self, tab: TabId, url: &str) -> Option<Navigation> {
        let ends_here = self
            .current
            .get(&tab)
            .is_some_and(|navigation| navigation.urls.iter().any(|hop| hop == url));
        if ends_here {
            self.current.remove(&tab)
        } else {
            None
        }
    }

    pub(crate) fn forget_tab(&mut self, tab: TabId) {
        self.typed.remove(&tab);
        self.current.remove(&tab);
    }
}

/// A bar stays while the tab shows the page it went back to (or, without a `GoBack`, the
/// offered page); the first other page drops it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BarWatch {
    offered: String,
    settled: Option<String>,
}

impl BarWatch {
    pub(crate) fn new(offered: &str, went_back: bool) -> Self {
        Self {
            offered: offered.to_owned(),
            settled: (!went_back).then(|| offered.to_owned()),
        }
    }

    /// Whether the bar stays while the tab shows `url`.
    pub(crate) fn keep(&mut self, url: &str) -> bool {
        match &self.settled {
            Some(settled) => settled == url,
            // Still on the offered page: the `GoBack` has not committed yet.
            None if url == self.offered => true,
            // The `GoBack`'s own commit: the page the bar now belongs to.
            None => {
                self.settled = Some(url.to_owned());
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    const PAGE: &str = "https://shop.example.com";

    fn facts<'a>(file: &'a Url, posted: Option<bool>, user_started: bool) -> OfferFacts<'a> {
        OfferFacts {
            tab: TabId::new(1),
            url: file,
            posted,
            user_started,
            foreground: true,
            initiator_site: PAGE,
        }
    }

    #[test]
    fn the_decision_table() {
        let now = Instant::now();
        let file = url("https://files.example.net/a.zip");
        let mut policy = OfferPolicy::default();

        // A tab in the background: dropped, whatever else holds.
        for user_started in [false, true] {
            assert_eq!(
                policy.decide(
                    &OfferFacts {
                        foreground: false,
                        ..facts(&file, Some(false), user_started)
                    },
                    now
                ),
                Decision::Refuse(Refusal::Background)
            );
        }
        // Schemes that cannot be fetched again.
        for (address, scheme) in [
            ("blob:https://example.com/0f2c", "blob"),
            ("data:application/octet-stream;base64,aGk=", "data"),
            ("file:///Users/me/a.zip", "file"),
            ("ftp://example.com/a.zip", "ftp"),
        ] {
            let other = url(address);
            for user_started in [false, true] {
                assert_eq!(
                    policy.decide(&facts(&other, Some(false), user_started), now),
                    Decision::Refuse(Refusal::Unsupported(scheme.into())),
                    "{address}"
                );
            }
        }
        // A form's POST.
        for user_started in [false, true] {
            assert_eq!(
                policy.decide(&facts(&file, Some(true), user_started), now),
                Decision::Refuse(Refusal::Posted)
            );
        }
        // Started by the user, by a known GET: downloaded.
        assert_eq!(
            policy.decide(&facts(&file, Some(false), true), now),
            Decision::Download
        );
        // An unknown method is never fetched silently, even for the user: the bar asks.
        assert_eq!(policy.decide(&facts(&file, None, true), now), Decision::Ask);
        // Started by the page: the bar asks.
        assert_eq!(
            policy.decide(&facts(&file, Some(false), false), now),
            Decision::Ask
        );
    }

    #[test]
    fn block_keys_on_the_page_that_started_it_not_the_file_host() {
        let now = Instant::now();
        let file = url("https://files.example.net/a.zip");
        let mut policy = OfferPolicy::default();
        policy.block(PAGE);
        // The blocked page, offering a file on another host: refused, even after a gesture.
        for user_started in [false, true] {
            assert_eq!(
                policy.decide(&facts(&file, Some(false), user_started), now),
                Decision::Refuse(Refusal::Blocked)
            );
        }
        // Another page offering a file on the same host is not blocked.
        assert_eq!(
            policy.decide(
                &OfferFacts {
                    initiator_site: "https://blog.example.org",
                    ..facts(&file, Some(false), false)
                },
                now
            ),
            Decision::Ask
        );
    }

    #[test]
    fn bars_are_limited_per_tab_and_window() {
        let start = Instant::now();
        let file = url("https://example.com/a.zip");
        let first = facts(&file, Some(false), false);
        let second = OfferFacts {
            tab: TabId::new(2),
            ..first.clone()
        };
        let mut policy = OfferPolicy::default();
        for _ in 0..ASK_LIMIT {
            assert_eq!(policy.decide(&first, start), Decision::Ask);
        }
        assert_eq!(
            policy.decide(&first, start + Duration::from_secs(1)),
            Decision::Refuse(Refusal::RateLimited)
        );
        assert_eq!(policy.decide(&second, start), Decision::Ask);
        // A download the user started is never rate limited.
        assert_eq!(
            policy.decide(&facts(&file, Some(false), true), start),
            Decision::Download
        );
        assert_eq!(policy.decide(&first, start + ASK_WINDOW), Decision::Ask);
    }

    #[test]
    fn the_initiator_is_the_referrer_else_the_page_shown_else_the_file() {
        let file = url("https://files.example.net/a.zip");
        let referrer = url("https://shop.example.com/");
        let from = url("https://shop.example.com/basket");
        assert_eq!(initiator(Some(&referrer), Some(&from), &file), &referrer);
        assert_eq!(initiator(None, Some(&from), &file), &from);
        assert_eq!(initiator(None, None, &file), &file);
        assert_eq!(site_of(&from), "https://shop.example.com");
        assert_eq!(
            host_label(&url("http://127.0.0.1:8123/x")),
            "127.0.0.1:8123"
        );
        assert_eq!(
            host_label(&url("https://shop.example.com/x")),
            "shop.example.com"
        );
    }

    #[test]
    fn a_gesture_counts_when_the_request_starts_not_when_the_server_answers() {
        let now = Instant::now();
        let tab = TabId::new(1);
        let mut navigations = Navigations::default();
        // The click's gesture is taken as the request starts, however slow the server.
        navigations.request_started(
            tab,
            "https://shop.example.com/download?id=7",
            false,
            Some("https://shop.example.com/basket"),
            now,
            || true,
        );
        navigations.request_started(
            tab,
            "https://files.example.net/a.zip",
            true,
            None,
            now,
            || panic!("a redirect hop takes no gesture"),
        );
        assert_eq!(
            navigations.take(tab, "https://files.example.net/a.zip"),
            Some(Navigation {
                urls: vec![
                    "https://shop.example.com/download?id=7".into(),
                    "https://files.example.net/a.zip".into(),
                ],
                user_started: true,
                from: Some("https://shop.example.com/basket".into()),
            })
        );
        // Decided once.
        assert_eq!(
            navigations.take(tab, "https://files.example.net/a.zip"),
            None
        );

        // Without a gesture, the page started it.
        navigations.request_started(tab, "https://x.test/b.zip", false, None, now, || false);
        assert!(
            !navigations
                .take(tab, "https://x.test/b.zip")
                .unwrap()
                .user_started
        );
    }

    #[test]
    fn a_typed_address_counts_only_for_its_own_url_and_briefly() {
        let now = Instant::now();
        let tab = TabId::new(1);
        let mut navigations = Navigations::default();
        navigations.note_typed(tab, "https://files.example.net/a.zip", now);
        // Another URL first (a page redirecting elsewhere by script): not the typed one, and no
        // gesture either.
        navigations.request_started(tab, "https://ads.test/", false, None, now, || false);
        assert!(
            !navigations
                .take(tab, "https://ads.test/")
                .unwrap()
                .user_started
        );
        // The typed URL itself, without any gesture.
        navigations.request_started(
            tab,
            "https://files.example.net/a.zip",
            false,
            None,
            now,
            || false,
        );
        assert!(
            navigations
                .take(tab, "https://files.example.net/a.zip")
                .unwrap()
                .user_started
        );
        // Used once.
        navigations.request_started(
            tab,
            "https://files.example.net/a.zip",
            false,
            None,
            now,
            || false,
        );
        assert!(
            !navigations
                .take(tab, "https://files.example.net/a.zip")
                .unwrap()
                .user_started
        );
        // Too old.
        navigations.note_typed(tab, "https://files.example.net/b.zip", now);
        navigations.request_started(
            tab,
            "https://files.example.net/b.zip",
            false,
            None,
            now + TYPED_LIFETIME + Duration::from_millis(1),
            || false,
        );
        assert!(
            !navigations
                .take(tab, "https://files.example.net/b.zip")
                .unwrap()
                .user_started
        );
        // Another tab's typed address is not this tab's.
        navigations.note_typed(TabId::new(2), "https://files.example.net/c.zip", now);
        navigations.request_started(
            tab,
            "https://files.example.net/c.zip",
            false,
            None,
            now,
            || false,
        );
        assert!(
            !navigations
                .take(tab, "https://files.example.net/c.zip")
                .unwrap()
                .user_started
        );
    }

    #[test]
    fn a_bar_is_dropped_on_the_first_page_after_the_one_gone_back_to() {
        // Went back: the commit of the `GoBack` itself does not drop it.
        let mut watch = BarWatch::new("https://x.test/a.zip", true);
        assert!(watch.keep("https://x.test/a.zip"));
        assert!(watch.keep("https://shop.test/basket"));
        assert!(watch.keep("https://shop.test/basket"));
        assert!(!watch.keep("https://shop.test/other"));
        // No page to go back to: the first other page drops it.
        let mut watch = BarWatch::new("https://x.test/a.zip", false);
        assert!(watch.keep("https://x.test/a.zip"));
        assert!(!watch.keep("https://news.test/"));
    }
}
