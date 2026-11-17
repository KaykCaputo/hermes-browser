//! Recognising Servo's "Unknown content type" page, which it shows when a navigation's response
//! is of a type it cannot display (Servo 0.8.0 has no downloads of its own).
//!
//! Servo synthesises that page in `servo-script-0.8.0/dom/servoparser/mod.rs:1139-1150`
//! (`<html><body><p>Unknown content type (…).</p></body></html>`, with an opaque origin from
//! `Document::mark_as_internal`), and tells the embedder nothing beyond the usual commit, an
//! empty title and `Complete`. The check below asks the page itself, once it is complete and
//! has no title: an opaque origin, a type other than HTML or XML, and exactly that one
//! paragraph.
//!
//! **A page can imitate it.** Any page whose response makes its own origin opaque (for example
//! with `Content-Security-Policy: sandbox`) and whose body is that one paragraph, in a type
//! other than HTML or XML, passes the check — so can a same-origin page arrange it on its own
//! URL. What it gains is nothing it could not get otherwise: Mercury offers *that page's own
//! URL* as a download, under the same rules (a gesture, or the bar), exactly as a response of
//! a type Servo cannot show would. No other URL, no request body and no other origin's data
//! are involved.

use url::Url;

/// Evaluated in the page: the response's type and the document's URL, one per line, if this
/// is the engine's unknown-content page; otherwise an empty string.
pub(crate) const RECOGNISE_SCRIPT: &str = r#"(() => {
  const body = document.body;
  const type = document.contentType || "";
  if (!body || self.origin !== "null" || type === "text/html" || type.includes("xml")) return "";
  if (body.children.length !== 1 || body.firstElementChild.tagName !== "P") return "";
  if (document.head && document.head.children.length !== 0) return "";
  if (!/^Unknown content type \(.+\)\.$/.test(body.textContent)) return "";
  return type + "\n" + location.href;
})()"#;

/// The response's type and the page's URL, from the script's answer.
pub(crate) fn parse_answer(answer: &str) -> Option<(String, Url)> {
    let (mime_type, href) = answer.split_once('\n')?;
    let mime_type = mime_type.trim();
    if mime_type.is_empty() {
        return None;
    }
    Some((mime_type.to_owned(), Url::parse(href.trim()).ok()?))
}

/// Whether the request that fetched `url` used another method than GET: a form's POST, whose
/// body the embedder never sees and must not send again. `None` when it is not known: the last
/// top-level request seen was for another URL, or none was seen.
pub(crate) fn was_posted(last_request: Option<&(String, Url)>, url: &Url) -> Option<bool> {
    let (method, requested) = last_request?;
    (requested == url).then(|| !method.eq_ignore_ascii_case("GET"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_names_the_type_and_the_page() {
        assert_eq!(
            parse_answer("application/zip\nhttp://127.0.0.1:8000/a.zip"),
            Some((
                "application/zip".to_owned(),
                Url::parse("http://127.0.0.1:8000/a.zip").unwrap()
            ))
        );
        assert_eq!(parse_answer(""), None);
        assert_eq!(parse_answer("\nhttp://a.test/"), None);
        assert_eq!(parse_answer("application/zip\nnot a url"), None);
    }

    #[test]
    fn a_post_is_recognised_only_for_the_same_page() {
        let page = Url::parse("https://a.test/report").unwrap();
        let other = Url::parse("https://a.test/other").unwrap();
        assert_eq!(
            was_posted(Some(&("POST".to_owned(), page.clone())), &page),
            Some(true)
        );
        assert_eq!(
            was_posted(Some(&("GET".to_owned(), page.clone())), &page),
            Some(false)
        );
        // Another URL, or nothing seen: unknown, never taken for a GET.
        assert_eq!(was_posted(Some(&("GET".to_owned(), other)), &page), None);
        assert_eq!(was_posted(None, &page), None);
    }

    #[test]
    fn the_script_checks_origin_type_shape_and_text() {
        for needed in [
            "self.origin !== \"null\"",
            "type === \"text/html\"",
            "includes(\"xml\")",
            "tagName !== \"P\"",
            "Unknown content type",
        ] {
            assert!(RECOGNISE_SCRIPT.contains(needed), "{needed}");
        }
    }
}
