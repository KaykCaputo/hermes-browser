//! Saved website logins for Mercury Browser.
//!
//! Logins are kept in the operating system's credential store, one item per login: on macOS the
//! Keychain, through `KeychainBackend`. A login belongs to exactly one [`Origin`] (scheme, host
//! and port); nothing here matches subdomains or "related" sites. [`PasswordStore`] adds
//! validation and lookup on top of any [`SecretBackend`]; [`MemoryBackend`] keeps logins in
//! memory for tests and for platforms that have no backend yet (everything except macOS, for
//! now).
//!
//! Passwords travel as [`Password`], which is wiped when dropped and never printed. Listing
//! logins never reads a password: on macOS, reading one is what can make the system ask the user.

#[cfg(target_os = "macos")]
mod account;
mod backend;
mod error;
#[cfg(target_os = "macos")]
mod keychain;
mod login;
mod memory;
mod origin;
mod secret;

pub use backend::{
    MAX_PASSWORD_BYTES, MAX_REALM_BYTES, MAX_USERNAME_BYTES, PasswordStore, SecretBackend,
};
pub use error::PasswordError;
#[cfg(target_os = "macos")]
pub use keychain::{KEYCHAIN_SERVICE, KeychainBackend};
pub use login::{LoginKey, LoginKind, LoginSummary, SaveOutcome};
pub use memory::MemoryBackend;
pub use origin::Origin;
pub use secret::Password;
