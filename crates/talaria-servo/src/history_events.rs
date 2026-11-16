//! The events history recording needs, built from Servo's notifications.

use talaria_core::BrowserEvent;
use url::Url;

use crate::runtime::display_url;

/// `PageCommitted` for a session-history change: the current entry's URL and the position.
/// `None` if `current` is not an entry.
pub(crate) fn page_committed(entries: &[Url], current: usize) -> Option<BrowserEvent> {
    entries.get(current).map(|url| BrowserEvent::PageCommitted {
        url: display_url(url),
        history_length: entries.len(),
        history_index: current,
    })
}

/// `PageTitleAt` for a title reported while the web view showed `url`.
pub(crate) fn page_title_at(url: Option<&Url>, title: Option<String>) -> BrowserEvent {
    BrowserEvent::PageTitleAt {
        url: url.map(display_url).unwrap_or_default(),
        title,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn a_commit_names_the_current_entry_and_its_position() {
        let entries = [url("https://a.test/"), url("https://b.test/")];

        assert_eq!(
            page_committed(&entries, 1),
            Some(BrowserEvent::PageCommitted {
                url: "https://b.test/".to_owned(),
                history_length: 2,
                history_index: 1,
            })
        );
        assert_eq!(page_committed(&entries, 2), None);
    }

    #[test]
    fn the_new_tab_page_is_shown_as_an_empty_address_as_elsewhere() {
        assert_eq!(
            page_committed(&[url("about:blank")], 0),
            Some(BrowserEvent::PageCommitted {
                url: String::new(),
                history_length: 1,
                history_index: 0,
            })
        );
    }

    #[test]
    fn a_title_carries_the_web_views_url() {
        assert_eq!(
            page_title_at(Some(&url("https://a.test/x")), Some("X".to_owned())),
            BrowserEvent::PageTitleAt {
                url: "https://a.test/x".to_owned(),
                title: Some("X".to_owned()),
            }
        );
        assert_eq!(
            page_title_at(None, None),
            BrowserEvent::PageTitleAt {
                url: String::new(),
                title: None,
            }
        );
    }
}
