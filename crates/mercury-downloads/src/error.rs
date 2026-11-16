//! Why a transfer failed. Turned into the `error` text of `DownloadEvent::Failed`.

use std::time::Duration;

use reqwest::StatusCode;

#[derive(Debug, thiserror::Error)]
pub(crate) enum TransferError {
    #[error("cannot download `{0}` URLs; only http and https are supported")]
    UnsupportedScheme(String),
    #[error("refused a redirect to a `{0}` URL; only http and https are supported")]
    UnsupportedRedirect(String),
    #[error("the server sent a redirect without a usable location")]
    BadRedirect,
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("invalid {0} header value")]
    InvalidHeader(&'static str),
    #[error("the server answered HTTP {0}")]
    Status(StatusCode),
    #[error("network error: {0}")]
    Network(#[source] reqwest::Error),
    #[error("no data received for {0:?}")]
    Stalled(Duration),
    #[error("the download ended after {received} of {expected} bytes")]
    Truncated { received: u64, expected: u64 },
    #[error("the server sent part of the file without being asked to")]
    UnexpectedPartial,
    #[error("the server's partial response does not continue the partial file")]
    RangeMismatch,
    #[error("the partial file disappeared before the download could continue")]
    PartialGone,
    #[error("the download is already being resumed (or is still running)")]
    AlreadyResuming,
    #[error("cannot resume: {0}")]
    InvalidResume(&'static str),
    #[error("the download was interrupted")]
    Interrupted,
    /// Not shown to anyone: a cancelled transfer ends with `DownloadEvent::Cancelled`.
    #[error("cancelled")]
    Cancelled,
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
}

impl TransferError {
    pub(crate) fn network(error: reqwest::Error) -> Self {
        // The URL may carry tokens; the event text is for people, not for logs.
        Self::Network(error.without_url())
    }

    /// Whether trying again later could succeed, so a partial file is worth keeping.
    pub(crate) fn is_transient(&self) -> bool {
        match self {
            Self::Network(_) | Self::Stalled(_) | Self::Truncated { .. } | Self::Interrupted => {
                true
            }
            Self::Status(status) => {
                status.is_server_error()
                    || *status == StatusCode::REQUEST_TIMEOUT
                    || *status == StatusCode::TOO_MANY_REQUESTS
            }
            _ => false,
        }
    }
}
