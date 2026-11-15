//! The crate's error type.

use std::path::PathBuf;

/// Everything that can go wrong while reading, writing or using settings and sessions.
///
/// A damaged file is not an error: loading reports it as [`crate::Loaded::Damaged`] and carries
/// on with defaults.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SettingsError {
    /// Reading, writing or moving a file failed. A directory or other non-file at the path, and
    /// a file that cannot be read, end up here.
    #[error("could not {action} {}: {source}", path.display())]
    Io {
        /// What was being done, for example "read" or "write".
        action: &'static str,
        /// The file concerned.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The file on disk was written by a newer version of the browser. It is left untouched,
    /// and saving over it is refused.
    #[error(
        "{} was written by a newer version of Mercury Browser (format {found}; this version \
         understands up to {supported}) and is left untouched",
        path.display()
    )]
    NewerVersion {
        /// The file concerned.
        path: PathBuf,
        /// The format version found in the file.
        found: u32,
        /// The newest format version this crate reads and writes.
        supported: u32,
    },
    /// The value could not be turned into JSON.
    #[error("could not serialise the {what}: {source}")]
    Serialize {
        /// What was being serialised.
        what: &'static str,
        /// The underlying error.
        #[source]
        source: serde_json::Error,
    },
    /// The value would make a file larger than this crate reads back, so it is not written.
    #[error("the {what} would take {size} bytes, more than the limit of {limit}")]
    TooLarge {
        /// What was being written.
        what: &'static str,
        /// The serialised size in bytes.
        size: u64,
        /// The limit in bytes.
        limit: u64,
    },
    /// A search engine definition is not usable.
    #[error("search engine {id:?} is not usable: {reason}")]
    InvalidSearchEngine {
        /// The engine's id as given.
        id: String,
        /// Why it was refused.
        reason: String,
    },
    /// The query cannot be placed where this engine's template puts it (for example `..` as a
    /// whole path segment, which a URL parser would resolve away).
    #[error("this query cannot be placed in the URL of search engine {id:?}")]
    UnplaceableQuery {
        /// The engine's id.
        id: String,
    },
}

impl SettingsError {
    pub(crate) fn io(
        action: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}
