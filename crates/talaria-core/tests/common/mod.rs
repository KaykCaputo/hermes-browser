//! A search builder for tests: the core has none of its own.

use talaria_core::SearchUrlBuilder;
use url::Url;

/// Puts the text, form-encoded, after `https://search.test/?q=`.
pub fn test_search() -> SearchUrlBuilder {
    SearchUrlBuilder::new(|text| {
        let mut url = Url::parse("https://search.test/").map_err(|error| error.to_string())?;
        url.query_pairs_mut().append_pair("q", text);
        Ok(url)
    })
}
