//! Building a `mercury_downloads::DownloadRequest` from what the browser knows about the page:
//! the engine's user agent, the cookies Servo holds for the URL, and the referrer.

use std::path::PathBuf;

use mercury_downloads::{DownloadRequest, ResumeFrom};
use url::Url;

/// Why a URL cannot be saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SaveError {
    /// Only `http` and `https` addresses are fetched by `mercury-downloads`.
    UnsupportedScheme(String),
    /// The address does not parse.
    InvalidAddress,
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedScheme(scheme) => {
                write!(formatter, "“{scheme}:” addresses cannot be saved")
            }
            Self::InvalidAddress => formatter.write_str("the address is not valid"),
        }
    }
}

/// Parses the address to save; only `http` and `https` are accepted.
pub(crate) fn savable_url(address: &str) -> Result<Url, SaveError> {
    let url = Url::parse(address).map_err(|_| SaveError::InvalidAddress)?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(SaveError::UnsupportedScheme(other.to_owned())),
    }
}

/// The referrer sent with a download started from a page.
///
/// Servo does not tell the embedder a page's referrer policy, so only the page's origin is sent
/// (what `strict-origin` and the default policy send across origins); a page that is not on
/// `http` or `https` sends none. `mercury-downloads` drops it again on a step from `https` to
/// `http`.
pub(crate) fn referrer_for(page_url: Option<&Url>) -> Option<Url> {
    let page_url = page_url?;
    if !matches!(page_url.scheme(), "http" | "https") {
        return None;
    }
    Url::parse(&format!("{}/", page_url.origin().ascii_serialization())).ok()
}

/// A `Cookie` header from name and value pairs, in the order Servo returned them (most specific
/// path first); `None` if there are none.
pub(crate) fn cookie_header(cookies: &[(String, String)]) -> Option<String> {
    if cookies.is_empty() {
        return None;
    }
    Some(
        cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// Everything a download request carries besides the URL.
#[derive(Debug, Clone, Default)]
pub(crate) struct RequestContext {
    pub(crate) referrer: Option<Url>,
    pub(crate) user_agent: String,
    pub(crate) cookies: Vec<(String, String)>,
    pub(crate) destination_dir: PathBuf,
    pub(crate) suggested_name: Option<String>,
    pub(crate) resume: Option<ResumeFrom>,
}

pub(crate) fn build_request(url: Url, context: RequestContext) -> DownloadRequest {
    DownloadRequest {
        url,
        referrer: context.referrer,
        user_agent: context.user_agent,
        cookie_header: cookie_header(&context.cookies),
        destination_dir: context.destination_dir,
        suggested_name: context
            .suggested_name
            .filter(|name| !name.trim().is_empty()),
        resume: context.resume,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn only_web_addresses_can_be_saved() {
        assert_eq!(
            savable_url("https://example.com/a.zip").unwrap().as_str(),
            "https://example.com/a.zip"
        );
        assert!(savable_url("http://example.com/").is_ok());
        assert_eq!(
            savable_url("file:///etc/passwd"),
            Err(SaveError::UnsupportedScheme("file".into()))
        );
        assert_eq!(
            savable_url("about:blank"),
            Err(SaveError::UnsupportedScheme("about".into()))
        );
        assert_eq!(
            savable_url("data:text/plain,hi"),
            Err(SaveError::UnsupportedScheme("data".into()))
        );
        assert_eq!(savable_url("not a url"), Err(SaveError::InvalidAddress));
    }

    #[test]
    fn the_referrer_is_the_page_origin_only() {
        assert_eq!(
            referrer_for(Some(&url(
                "https://user:pw@example.com:8443/a/b?q=secret#frag"
            ))),
            Some(url("https://example.com:8443/"))
        );
        assert_eq!(
            referrer_for(Some(&url("http://127.0.0.1:8000/page.html"))),
            Some(url("http://127.0.0.1:8000/"))
        );
        assert_eq!(referrer_for(Some(&url("file:///Users/me/page.html"))), None);
        assert_eq!(referrer_for(Some(&url("about:blank"))), None);
        assert_eq!(referrer_for(Some(&url("data:text/html,x"))), None);
        assert_eq!(referrer_for(None), None);
    }

    #[test]
    fn cookies_become_one_header_in_servo_order() {
        assert_eq!(cookie_header(&[]), None);
        assert_eq!(
            cookie_header(&[
                ("session".into(), "abc".into()),
                ("theme".into(), "dark".into()),
            ])
            .as_deref(),
            Some("session=abc; theme=dark")
        );
    }

    #[test]
    fn the_request_carries_the_engine_user_agent_cookies_and_referrer() {
        let request = build_request(
            url("https://files.example.com/a.zip"),
            RequestContext {
                referrer: referrer_for(Some(&url("https://example.com/page?id=7"))),
                user_agent: "Mozilla/5.0 Gecko/20100101 Firefox/153.0".into(),
                cookies: vec![("k".into(), "v".into())],
                destination_dir: PathBuf::from("/scratch/downloads"),
                suggested_name: Some("  ".into()),
                resume: None,
            },
        );
        assert_eq!(request.url.as_str(), "https://files.example.com/a.zip");
        assert_eq!(
            request.referrer.as_ref().map(Url::as_str),
            Some("https://example.com/")
        );
        assert_eq!(
            request.user_agent,
            "Mozilla/5.0 Gecko/20100101 Firefox/153.0"
        );
        assert_eq!(request.cookie_header.as_deref(), Some("k=v"));
        assert_eq!(request.destination_dir, PathBuf::from("/scratch/downloads"));
        // A blank suggestion is no suggestion: the crate names the file.
        assert_eq!(request.suggested_name, None);
        assert_eq!(request.resume, None);

        let named = build_request(
            url("https://example.com/img"),
            RequestContext {
                suggested_name: Some("photo.png".into()),
                ..RequestContext::default()
            },
        );
        assert_eq!(named.suggested_name.as_deref(), Some("photo.png"));
        assert_eq!(named.cookie_header, None);
        assert_eq!(named.referrer, None);
    }
}
