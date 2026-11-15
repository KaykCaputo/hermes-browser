/// Everything that can go wrong in the profile database.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite reported an error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The visit, bookmark or download record does not exist.
    #[error("no such record")]
    NotFound,
    /// The database was written by a newer Mercury Browser and is left untouched.
    #[error(
        "the profile database has schema version {found}, but this build supports at most {supported}"
    )]
    UnsupportedSchemaVersion {
        /// The `user_version` stored in the file.
        found: u32,
        /// The newest schema version this build understands.
        supported: u32,
    },
    /// A folder cannot be moved into itself or one of its descendants.
    #[error("a folder cannot be moved into itself or one of its descendants")]
    InvalidMove,
    /// The bookmark roots cannot be moved or removed.
    #[error("the bookmark roots cannot be moved or removed")]
    CannotModifyRoot,
    /// The parent given for a bookmark is not a folder.
    #[error("the parent is not a bookmark folder")]
    NotAFolder,
    /// A bookmark or download with this GUID already exists.
    #[error("the GUID {0:?} is already in use")]
    DuplicateGuid(String),
    /// The record is not in a state that allows this operation (for example a late progress
    /// report for a finished download).
    #[error("invalid state: {0}")]
    InvalidState(&'static str),
    /// An import was refused because of one entry; nothing was imported.
    #[error("import entry {index} refused: {reason}")]
    ImportEntryRefused {
        /// The entry's index: in the slice of visits, or for bookmarks in a depth-first
        /// (pre-order) walk of all the trees of the call, in order, starting at 0.
        index: usize,
        /// Why it was refused.
        reason: ImportRefusal,
    },
    /// An argument is not valid for this operation.
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),
}

/// Why an import entry was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportRefusal {
    /// Its URL is longer than [`MAX_URL_BYTES`](crate::MAX_URL_BYTES).
    #[error("the URL is longer than the limit")]
    UrlTooLong,
    /// It is nested deeper than [`BOOKMARK_IMPORT_MAX_DEPTH`](crate::BOOKMARK_IMPORT_MAX_DEPTH).
    #[error("the bookmark is nested too deeply")]
    NestedTooDeeply,
    /// Its GUID is already in use, or used twice in the import.
    #[error("the GUID {0:?} is already in use")]
    DuplicateGuid(String),
}

/// The result type of every fallible store operation.
pub type Result<T> = std::result::Result<T, StoreError>;
