//! What the downloads work needs from the engine's events: top-level requests as they start,
//! and pages the engine could not show, offered as downloads. Both are queued per tab, in order.

use talaria_core::{BrowserController, BrowserEvent, DownloadOffer, DownloadSignal};

fn offer(url: &str, posted: Option<bool>) -> BrowserEvent {
    BrowserEvent::DownloadOffered {
        url: url.to_owned(),
        mime_type: "application/zip".to_owned(),
        posted,
        referrer: Some("https://a.test/page".to_owned()),
    }
}

fn request(url: &str, redirect: bool) -> BrowserEvent {
    BrowserEvent::DocumentRequestStarted {
        url: url.to_owned(),
        redirect,
        from: Some("https://a.test/page".to_owned()),
    }
}

#[test]
fn requests_and_offers_are_queued_per_tab_in_order_and_taken_once() {
    let mut controller = BrowserController::default();
    let first = controller.active_tab_id();
    let second = controller.open_tab();
    let before = controller.active_state().clone();

    assert!(controller.handle_event(first, request("https://a.test/go", false)));
    assert!(controller.handle_event(first, request("https://cdn.test/a.zip", true)));
    assert!(controller.handle_event(first, offer("https://cdn.test/a.zip", Some(false))));
    assert!(controller.handle_event(second, offer("https://b.test/b.zip", None)));

    assert_eq!(
        controller.take_download_signals(),
        vec![
            (
                first,
                DownloadSignal::RequestStarted {
                    url: "https://a.test/go".into(),
                    redirect: false,
                    from: Some("https://a.test/page".into()),
                }
            ),
            (
                first,
                DownloadSignal::RequestStarted {
                    url: "https://cdn.test/a.zip".into(),
                    redirect: true,
                    from: Some("https://a.test/page".into()),
                }
            ),
            (
                first,
                DownloadSignal::Offered(DownloadOffer {
                    url: "https://cdn.test/a.zip".into(),
                    mime_type: "application/zip".into(),
                    posted: Some(false),
                    referrer: Some("https://a.test/page".into()),
                })
            ),
            (
                second,
                DownloadSignal::Offered(DownloadOffer {
                    url: "https://b.test/b.zip".into(),
                    mime_type: "application/zip".into(),
                    posted: None,
                    referrer: Some("https://a.test/page".into()),
                })
            ),
        ]
    );
    assert!(controller.take_download_signals().is_empty());
    // The tab's shown state is not touched by either.
    assert_eq!(controller.active_state(), &before);
}

#[test]
fn signals_for_a_closed_tab_are_dropped() {
    let mut controller = BrowserController::default();
    let second = controller.open_tab();
    controller.close_tab(second);
    assert!(!controller.handle_event(second, offer("https://b.test/b.zip", Some(false))));
    assert!(!controller.handle_event(second, request("https://b.test/", false)));
    assert!(controller.take_download_signals().is_empty());
}
