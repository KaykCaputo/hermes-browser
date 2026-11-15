/// Everything that can go wrong in this crate.
///
/// No variant ever carries a password, a user name, a realm, a Keychain account string or a
/// field read from a file: reasons are fixed texts, and operating-system errors carry only their
/// status code and the system's own message for that code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PasswordError {
    /// The text or URL does not name an origin that can hold logins.
    #[error("invalid origin: {0}")]
    InvalidOrigin(&'static str),
    /// A user name, realm, password, option or file is not acceptable.
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    /// The store is locked, or the operation needs the user's approval and user interaction is
    /// not allowed in this process.
    #[error(
        "the password store is locked, or the operation needs the user and user interaction is not allowed"
    )]
    Locked,
    /// The user, or the system on the user's behalf, refused access to the item.
    #[error("access to the saved login was denied")]
    AccessDenied,
    /// An item that had to exist does not.
    #[error("the saved login was not found")]
    NotFound,
    /// This platform has no credential store backend.
    #[error("this platform has no password store")]
    Unsupported,
    /// A stored item exists but its contents could not be decoded.
    #[error("a saved login could not be decoded")]
    CorruptItem,
    /// The operating system's random number generator failed.
    #[error("the operating system's random number generator failed")]
    RandomUnavailable,
    /// [`PasswordStore::remove_all`](crate::PasswordStore::remove_all) removed some logins but
    /// could not remove at least one; it went on with the rest.
    #[error(
        "removed {removed} saved logins, but not all of them; the first error was: {first_error}"
    )]
    RemoveAllIncomplete {
        /// How many logins were removed.
        removed: u64,
        /// The first error met.
        first_error: Box<PasswordError>,
    },
    /// Any other operating-system error.
    #[error("operating system error {code}: {message}")]
    Os {
        /// The status code (an `OSStatus` on macOS).
        code: i32,
        /// The system's description of the status code.
        message: String,
    },
}
