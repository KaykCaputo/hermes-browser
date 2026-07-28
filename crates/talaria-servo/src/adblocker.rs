use std::cell::Cell;

use adblock::Engine;
use adblock::request::Request;
use content_security_policy::Destination;
use log::{debug, info};
use servo::{WebResourceLoad, WebResourceRequest, WebResourceResponse, WebView};
use url::Url;

const COMPILED_ENGINE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/adblock-engine.dat"));

pub(crate) struct ContentBlocker {
    engine: Engine,
    blocked_requests: Cell<u64>,
}

impl ContentBlocker {
    pub(crate) fn new() -> Self {
        let mut engine = Engine::default();
        engine
            .deserialize(COMPILED_ENGINE)
            .expect("the bundled adblock engine is compiled by this exact adblock-rust version");

        info!("ad and tracker blocking enabled with the Brave default filter snapshot");
        Self {
            engine,
            blocked_requests: Cell::new(0),
        }
    }

    pub(crate) fn handle_web_resource_load(
        &self,
        web_view: Option<&WebView>,
        load: WebResourceLoad,
    ) {
        let source_url = load
            .request()
            .referrer_url
            .clone()
            .or_else(|| web_view.and_then(WebView::url));

        if !self.should_block(load.request(), source_url.as_ref()) {
            return;
        }

        let blocked_url = load.request().url.clone();
        let blocked_count = self.blocked_requests.get() + 1;
        self.blocked_requests.set(blocked_count);
        if blocked_count == 1 {
            info!("blocked the first ad or tracker resource: {blocked_url}");
        } else {
            debug!("blocked web resource #{blocked_count}: {blocked_url}");
        }

        load.intercept(WebResourceResponse::new(blocked_url))
            .cancel();
    }

    fn should_block(&self, request: &WebResourceRequest, source_url: Option<&Url>) -> bool {
        self.should_block_url(
            request.url.as_str(),
            source_url.map_or(request.url.as_str(), Url::as_str),
            resource_type(request.destination),
            request.method.as_str(),
            request.is_for_main_frame,
        )
    }

    fn should_block_url(
        &self,
        url: &str,
        source_url: &str,
        resource_type: &str,
        method: &str,
        is_for_main_frame: bool,
    ) -> bool {
        // Brave can show a dedicated interstitial for blocked top-level navigations. Hermes does
        // not have that UI yet, so allowing the main document avoids turning a matched navigation
        // into an unexplained blank tab while still blocking its ads and trackers.
        if is_for_main_frame {
            return false;
        }

        let Ok(request) = Request::new(url, source_url, resource_type, method) else {
            debug!("could not classify web resource for ad blocking: {url}");
            return false;
        };

        self.engine.check_network_request(&request).should_block()
    }
}

fn resource_type(destination: Destination) -> &'static str {
    match destination {
        Destination::Document => "document",
        Destination::Frame | Destination::IFrame => "subdocument",
        Destination::Font => "font",
        Destination::Image => "image",
        Destination::Audio | Destination::Track | Destination::Video => "media",
        Destination::Embed | Destination::Object => "object",
        Destination::Script
        | Destination::AudioWorklet
        | Destination::PaintWorklet
        | Destination::ServiceWorker
        | Destination::SharedWorker
        | Destination::Worker => "script",
        Destination::Style => "stylesheet",
        Destination::Report => "csp_report",
        Destination::Json | Destination::None => "xmlhttprequest",
        Destination::Manifest | Destination::WebIdentity => "other",
        Destination::Xslt => "xslt",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_brave_filters_block_ads_and_trackers() {
        let blocker = ContentBlocker::new();

        assert!(blocker.should_block_url(
            "https://googleads.g.doubleclick.net/pagead/id",
            "https://example.com/article",
            "script",
            "GET",
            false,
        ));
        assert!(blocker.should_block_url(
            "https://www.google-analytics.com/analytics.js",
            "https://example.com/article",
            "script",
            "GET",
            false,
        ));
        assert!(!blocker.should_block_url(
            "https://example.com/assets/app.js",
            "https://example.com/article",
            "script",
            "GET",
            false,
        ));
    }

    #[test]
    fn top_level_navigation_is_left_to_the_browser() {
        let blocker = ContentBlocker::new();

        assert!(!blocker.should_block_url(
            "https://googleads.g.doubleclick.net/pagead/id",
            "https://example.com/",
            "document",
            "GET",
            true,
        ));
    }

    #[test]
    fn servo_destinations_map_to_adblock_resource_types() {
        assert_eq!(resource_type(Destination::Image), "image");
        assert_eq!(resource_type(Destination::Script), "script");
        assert_eq!(resource_type(Destination::IFrame), "subdocument");
        assert_eq!(resource_type(Destination::Video), "media");
        assert_eq!(resource_type(Destination::None), "xmlhttprequest");
    }
}
