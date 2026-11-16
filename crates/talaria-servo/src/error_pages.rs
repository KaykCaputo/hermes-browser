//! Recognising the pages Servo shows in place of a page it could not load.
//!
//! Servo commits a failed navigation at the requested URL and shows its own error document
//! there (`servo-script` `ServoParser`, `NetErrorHTML` and `BadCertHTML`); nothing else tells the
//! embedder that the load failed. Those documents have fixed titles, read here from the
//! resources Servo itself uses, so they stay right if Servo's pages change.

use servo::resources::{self, Resource};

/// The titles of Servo's error pages: a network error and a certificate error.
pub fn error_page_titles() -> Vec<String> {
    [Resource::NetErrorHTML, Resource::BadCertHTML]
        .into_iter()
        .filter_map(|resource| html_title(&resources::read_string(resource)))
        .collect()
}

/// The text of the first `<title>` element, with white space collapsed as `document.title`
/// does, if it is not empty.
fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let title = html[start..end]
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servos_error_pages_are_recognised_by_title() {
        assert_eq!(
            error_page_titles(),
            vec![
                "Error loading page".to_owned(),
                "Certificate error".to_owned()
            ]
        );
    }

    #[test]
    fn a_title_is_read_with_its_white_space_collapsed() {
        assert_eq!(
            html_title("<HTML><Title lang=en>\n  Two\twords </TITLE>").as_deref(),
            Some("Two words")
        );
        assert_eq!(html_title("<title> </title>"), None);
        assert_eq!(html_title("<p>no title</p>"), None);
        assert_eq!(html_title("<title>unterminated"), None);
    }
}
