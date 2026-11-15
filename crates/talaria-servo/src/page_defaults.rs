use std::rc::Rc;

use servo::UserContentManager;
use servo::user_contents::UserStyleSheet;
use url::Url;

const PAGE_DEFAULTS_URL: &str = "mercury-internal:///page-defaults.css";

fn page_defaults_source() -> &'static str {
    include_str!("../resources/page-defaults.css")
}

fn page_defaults_url() -> Url {
    Url::parse(PAGE_DEFAULTS_URL).expect("the page-defaults address is a valid URL")
}

/// The user content every web view shares: a user-origin style sheet that gives pages which
/// set no colour scheme the light defaults. Servo only applies it to documents loaded after it
/// was added, so it must be built before the first web view.
pub(crate) fn page_defaults_user_content(servo: &servo::Servo) -> Rc<UserContentManager> {
    let manager = UserContentManager::new(servo);
    manager.add_stylesheet(Rc::new(UserStyleSheet::new(
        page_defaults_source().to_owned(),
        page_defaults_url(),
    )));
    Rc::new(manager)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declarations(source: &str) -> Vec<String> {
        let mut without_comments = String::new();
        let mut rest = source;
        while let Some(start) = rest.find("/*") {
            without_comments.push_str(&rest[..start]);
            let end = rest[start..].find("*/").expect("comments are closed");
            rest = &rest[start + end + 2..];
        }
        without_comments.push_str(rest);
        without_comments
            .split(['{', '}', ';'])
            .map(|part| part.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|part| !part.is_empty())
            .collect()
    }

    fn root_declarations() -> Vec<String> {
        let parts = declarations(page_defaults_source());
        assert_eq!(
            parts.first().map(String::as_str),
            Some(":root"),
            "{parts:?}"
        );
        parts[1..].to_vec()
    }

    #[test]
    fn pages_that_set_no_colour_scheme_get_the_light_one() {
        assert!(root_declarations().contains(&"color-scheme: light".to_owned()));
    }

    #[test]
    fn opted_in_pages_get_a_canvas_matching_their_scheme() {
        assert!(
            root_declarations()
                .contains(&"background-color: light-dark(transparent, Canvas)".to_owned())
        );
    }

    #[test]
    fn page_styles_can_override_every_default() {
        let parts = declarations(page_defaults_source());
        assert_eq!(
            parts.len(),
            3,
            "one :root rule, two declarations: {parts:?}"
        );
        assert!(
            parts
                .iter()
                .all(|part| !part.contains("!important") && !part.contains('@'))
        );
    }

    #[test]
    fn the_style_sheet_has_an_internal_address_pages_cannot_load() {
        let url = page_defaults_url();
        assert_eq!(url.as_str(), PAGE_DEFAULTS_URL);
        assert!(!["http", "https", "file", "data", "blob"].contains(&url.scheme()));
    }
}
