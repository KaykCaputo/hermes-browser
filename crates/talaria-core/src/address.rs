use std::error::Error;
use std::fmt::{Display, Formatter};
use std::net::IpAddr;

use url::Url;

const DUCKDUCKGO_SEARCH_URL: &str = "https://duckduckgo.com/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationTarget {
    Url(Url),
    SearchQuery(String),
}

impl NavigationTarget {
    pub fn into_url(self) -> Result<Url, AddressResolutionError> {
        match self {
            Self::Url(url) => Ok(url),
            Self::SearchQuery(query) => duckduckgo_search_url(&query),
        }
    }
}

#[derive(Debug)]
pub enum AddressResolutionError {
    EmptyInput,
    InvalidUrl(url::ParseError),
    UnsupportedScheme(String),
}

impl Display for AddressResolutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyInput => formatter.write_str("address input cannot be empty"),
            Self::InvalidUrl(error) => write!(formatter, "invalid URL: {error}"),
            Self::UnsupportedScheme(scheme) => {
                write!(formatter, "unsupported URL scheme: {scheme}")
            }
        }
    }
}

impl Error for AddressResolutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidUrl(error) => Some(error),
            Self::EmptyInput | Self::UnsupportedScheme(_) => None,
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

pub fn resolve_address(input: &str) -> Result<Url, AddressResolutionError> {
    classify_address(input)?.into_url()
}

fn is_supported_scheme(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "about")
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

fn duckduckgo_search_url(query: &str) -> Result<Url, AddressResolutionError> {
    let mut search_url = Url::parse(DUCKDUCKGO_SEARCH_URL)?;

    {
        let mut query_pairs = search_url.query_pairs_mut();
        query_pairs.append_pair("q", query);
    }

    Ok(search_url)
}
