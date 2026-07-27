use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum ServoWebViewError {
    InvalidUrl(url::ParseError),
}

impl Display for ServoWebViewError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl(error) => write!(f, "invalid URL: {error}"),
        }
    }
}

impl Error for ServoWebViewError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidUrl(error) => Some(error),
        }
    }
}

impl From<url::ParseError> for ServoWebViewError {
    fn from(error: url::ParseError) -> Self {
        Self::InvalidUrl(error)
    }
}
