use std::error::Error;
use std::fmt::{Display, Formatter};
use std::net::IpAddr;

use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationTarget {
    Url(Url),
    SearchQuery(String),
}

impl NavigationTarget {
    /// The URL to open; a search is built by `search`, the only place search URLs come from.
    pub fn into_url(self, search: &SearchUrlBuilder) -> Result<Url, AddressResolutionError> {
        match self {
            Self::Url(url) => Ok(url),
            Self::SearchQuery(query) => search.build(&query),
        }
    }
}

#[derive(Debug)]
pub enum AddressResolutionError {
    EmptyInput,
    InvalidUrl(url::ParseError),
    UnsupportedScheme(String),
    /// The search engine could not build a URL for this text.
    Search(String),
}

impl Display for AddressResolutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyInput => formatter.write_str("address input cannot be empty"),
            Self::InvalidUrl(error) => write!(formatter, "invalid URL: {error}"),
            Self::UnsupportedScheme(scheme) => {
                write!(formatter, "unsupported URL scheme: {scheme}")
            }
            Self::Search(reason) => write!(formatter, "cannot search for this text: {reason}"),
        }
    }
}

impl Error for AddressResolutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidUrl(error) => Some(error),
            Self::EmptyInput | Self::UnsupportedScheme(_) | Self::Search(_) => None,
        }
    }
}

impl From<url::ParseError> for AddressResolutionError {
    fn from(error: url::ParseError) -> Self {
        Self::InvalidUrl(error)
    }
}

pub fn classify_address(input: &str) -> Result<NavigationTarget, AddressResolutionError> {
    let input = input.trim();

    if input.is_empty() {
        return Err(AddressResolutionError::EmptyInput);
    }

    let parsed_absolute = match Url::parse(input) {
        Ok(url) if is_supported_scheme(url.scheme()) => {
            return Ok(NavigationTarget::Url(url));
        }
        result => result,
    };

    if let Some(url) = parse_hostname(input) {
        return Ok(NavigationTarget::Url(url));
    }

    match parsed_absolute {
        Ok(url) => Err(AddressResolutionError::UnsupportedScheme(
            url.scheme().to_owned(),
        )),
        Err(error) if input.contains("://") => Err(AddressResolutionError::InvalidUrl(error)),
        Err(_) => Ok(NavigationTarget::SearchQuery(input.to_owned())),
    }
}

/// The URL for address-bar input; text that is not an address is searched for with `search`.
pub fn resolve_address(
    input: &str,
    search: &SearchUrlBuilder,
) -> Result<Url, AddressResolutionError> {
    classify_address(input)?.into_url(search)
}

/// Builds the results-page URL for address-bar text that is not an address. The application
/// supplies it (from its search-engine settings); the core never builds search URLs itself.
pub struct SearchUrlBuilder(Box<BuildSearchUrl>);

type BuildSearchUrl = dyn Fn(&str) -> Result<Url, String>;

impl SearchUrlBuilder {
    /// `build` receives the whole trimmed text, keywords included, and returns the URL or why
    /// it cannot make one.
    pub fn new(build: impl Fn(&str) -> Result<Url, String> + 'static) -> Self {
        Self(Box::new(build))
    }

    pub fn build(&self, text: &str) -> Result<Url, AddressResolutionError> {
        (self.0)(text).map_err(AddressResolutionError::Search)
    }
}

impl std::fmt::Debug for SearchUrlBuilder {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SearchUrlBuilder")
    }
}

fn is_supported_scheme(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "about" | "file")
}

fn parse_hostname(input: &str) -> Option<Url> {
    let candidate = Url::parse(&format!("https://{input}")).ok()?;

    if !candidate.username().is_empty() || candidate.password().is_some() {
        return None;
    }

    let host = candidate.host_str()?;
    let looks_like_host = host.eq_ignore_ascii_case("localhost")
        || host.contains('.')
        || host.parse::<IpAddr>().is_ok();

    looks_like_host.then_some(candidate)
}
