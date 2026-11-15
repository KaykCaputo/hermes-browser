use std::fmt;

use url::{Host, Url};

use crate::PasswordError;

/// `scheme://host[:port]`, ASCII, lower-case host, default port omitted. Logins belong to
/// exactly one origin.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Origin {
    text: String,
    host_start: usize,
    host_end: usize,
}

impl Origin {
    /// Only `https`, and `http` for loopback hosts (`localhost`, `127.0.0.0/8`, `[::1]`).
    ///
    /// The `url` crate has already lower-cased the host, converted an internationalised host to
    /// its ASCII (`xn--`) form and dropped a default port. This also removes one trailing dot
    /// from a domain (`example.com.` is `example.com`) and refuses empty labels. User name,
    /// password, path, query and fragment never become part of the origin.
    pub fn from_url(url: &Url) -> Result<Self, PasswordError> {
        let scheme = url.scheme();
        if scheme != "https" && scheme != "http" {
            return Err(PasswordError::InvalidOrigin(
                "only https and loopback http pages can hold logins",
            ));
        }
        let host = match url.host() {
            Some(Host::Domain(domain)) => normalise_domain(domain)?,
            Some(Host::Ipv4(address)) => address.to_string(),
            Some(Host::Ipv6(address)) => format!("[{address}]"),
            None => return Err(PasswordError::InvalidOrigin("the URL has no host")),
        };
        if scheme == "http" && !is_loopback(url, &host) {
            return Err(PasswordError::InvalidOrigin(
                "plain http is accepted only for loopback hosts",
            ));
        }

        let mut text = format!("{scheme}://");
        let host_start = text.len();
        text.push_str(&host);
        let host_end = text.len();
        if let Some(port) = url.port() {
            text.push_str(&format!(":{port}"));
        }
        Ok(Self {
            text,
            host_start,
            host_end,
        })
    }

    /// Parses an origin, or any URL, and returns its origin under the rules of
    /// [`Origin::from_url`].
    pub fn parse(text: &str) -> Result<Self, PasswordError> {
        let url = Url::parse(text).map_err(|_| PasswordError::InvalidOrigin("not a URL"))?;
        Self::from_url(&url)
    }

    /// The origin as text, for example `https://example.com:8443`.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The host: a lower-case ASCII domain, an IPv4 address, or an IPv6 address in brackets.
    pub fn host(&self) -> &str {
        &self.text[self.host_start..self.host_end]
    }
}

fn normalise_domain(domain: &str) -> Result<String, PasswordError> {
    let domain = domain.strip_suffix('.').unwrap_or(domain);
    if domain.is_empty() || domain.split('.').any(str::is_empty) {
        return Err(PasswordError::InvalidOrigin("the host has an empty label"));
    }
    if !domain.is_ascii() {
        return Err(PasswordError::InvalidOrigin("the host is not ASCII"));
    }
    Ok(domain.to_ascii_lowercase())
}

fn is_loopback(url: &Url, host: &str) -> bool {
    match url.host() {
        Some(Host::Domain(_)) => host == "localhost",
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

impl fmt::Debug for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Origin").field(&self.text).finish()
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}
