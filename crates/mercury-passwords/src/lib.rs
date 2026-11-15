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
//!
//! [`generate_password`] makes new passwords from the operating system's random source.
//! [`parse_logins_csv`] and [`write_logins_csv`] read and write the login CSV files other
//! browsers export; an exported file holds every password in clear text (see
//! [`write_logins_csv`] for what the caller must do about that).

#![warn(missing_docs)]

#[cfg(target_os = "macos")]
mod account;
mod backend;
mod csv;
mod error;
mod generator;
#[cfg(target_os = "macos")]
mod keychain;
mod login;
mod memory;
mod origin;
mod secret;

pub use backend::{
    MAX_PASSWORD_BYTES, MAX_REALM_BYTES, MAX_USERNAME_BYTES, PasswordStore, SecretBackend,
};
pub use csv::{CsvImport, CsvLogin, CsvSkipped, parse_logins_csv, write_logins_csv};
pub use error::PasswordError;
pub use generator::{
    AMBIGUOUS_CHARACTERS, DIGITS, GENERATOR_MAX_LENGTH, GENERATOR_MIN_LENGTH, GeneratorOptions,
    LOWERCASE, OsRandom, RandomSource, SYMBOLS, UPPERCASE, entropy_bits, generate_password,
};
#[cfg(target_os = "macos")]
pub use keychain::{KEYCHAIN_SERVICE, KeychainBackend};
pub use login::{LoginKey, LoginKind, LoginSummary, SaveOutcome};
pub use memory::MemoryBackend;
pub use origin::Origin;
pub use secret::Password;
