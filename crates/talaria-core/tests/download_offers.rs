//! Pages the engine could not show and offered as downloads are queued for the browser.

use talaria_core::{BrowserController, BrowserEvent, DownloadOffer};

fn offer(url: &str) -> BrowserEvent {
    BrowserEvent::DownloadOffered {
        url: url.to_owned(),
        mime_type: "application/zip".to_owned(),
        posted: false,
        referrer: Some("https://a.test/page".to_owned()),
    }
}

#[test]
fn offers_are_queued_per_tab_in_order_and_taken_once() {
    let mut controller = BrowserController::default();
    let first = controller.active_tab_id();
    let second = controller.open_tab();
    let before = controller.active_state().clone();

    assert!(controller.handle_event(first, offer("https://a.test/a.zip")));
    assert!(controller.handle_event(second, offer("https://b.test/b.zip")));

    assert_eq!(
        controller.take_download_offers(),
        vec![
            (
                first,
                DownloadOffer {
                    url: "https://a.test/a.zip".into(),
                    mime_type: "application/zip".into(),
                    posted: false,
                    referrer: Some("https://a.test/page".into()),
                }
            ),
            (
                second,
                DownloadOffer {
                    url: "https://b.test/b.zip".into(),
                    mime_type: "application/zip".into(),
                    posted: false,
                    referrer: Some("https://a.test/page".into()),
                }
            ),
        ]
    );
    assert!(controller.take_download_offers().is_empty());
    // The tab's shown state is not touched by an offer.
    assert_eq!(controller.active_state(), &before);
}

#[test]
fn an_offer_for_a_closed_tab_is_dropped() {
    let mut controller = BrowserController::default();
    let first = controller.active_tab_id();
    let second = controller.open_tab();
    controller.close_tab(second);
    assert!(!controller.handle_event(second, offer("https://b.test/b.zip")));
    assert!(controller.take_download_offers().is_empty());
    let _ = first;
}
