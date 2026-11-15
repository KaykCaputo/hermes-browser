//! Temporary keychains for tests. Every Keychain call in a test goes to a keychain file created
//! here, inside its own temporary directory, and removed when the guard drops — on success and on
//! panic alike. User interaction is switched off for the whole test process before the first
//! Keychain call, so nothing can ever show a dialog: an operation that would have asked the user
//! fails instead.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use mercury_passwords::KeychainBackend;
use security_framework::os::macos::keychain::{KeychainUserInteractionLock, SecKeychain};
use tempfile::TempDir;

/// The invented password of every test keychain.
pub const TEST_KEYCHAIN_PASSWORD: &str = "fake-test-keychain-password";

/// Turns Keychain user interaction off for the rest of this process. The lock lives in a static,
/// so it is never dropped and interaction is never turned back on while tests run.
pub fn no_user_interaction() {
    static LOCK: OnceLock<KeychainUserInteractionLock> = OnceLock::new();
    LOCK.get_or_init(|| {
        SecKeychain::disable_user_interaction().expect("user interaction can be disabled")
    });
    assert!(
        !SecKeychain::user_interaction_allowed().expect("the setting can be read"),
        "Keychain user interaction must be off before any Keychain call"
    );
}

/// A temporary directory holding one test keychain; both are deleted on drop.
pub struct TempKeychain {
    dir: TempDir,
}

impl TempKeychain {
    /// A new, empty directory; the keychain file inside it does not exist yet.
    pub fn new() -> Self {
        no_user_interaction();
        let dir = tempfile::Builder::new()
            .prefix("mercury-passwords-test-")
            .tempdir()
            .expect("a temporary directory can be created");
        Self { dir }
    }

    /// Where the keychain file lives.
    pub fn path(&self) -> PathBuf {
        self.dir.path().join("test.keychain-db")
    }

    /// The directory the keychain file lives in.
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// A backend on this keychain, creating the file on first use.
    pub fn backend(&self) -> KeychainBackend {
        KeychainBackend::file(&self.path(), TEST_KEYCHAIN_PASSWORD)
            .expect("the test keychain can be created or opened")
    }

    /// A second, independent handle on the same keychain file, for test-only operations.
    pub fn handle(&self) -> SecKeychain {
        no_user_interaction();
        assert!(self.path().exists(), "the keychain must exist already");
        let mut keychain = SecKeychain::open(self.path()).expect("the keychain opens");
        keychain
            .unlock(Some(TEST_KEYCHAIN_PASSWORD))
            .expect("the keychain unlocks");
        keychain
    }
}

/// A guard and a backend on a fresh temporary keychain.
pub fn temp_keychain() -> (TempKeychain, KeychainBackend) {
    let guard = TempKeychain::new();
    let backend = guard.backend();
    (guard, backend)
}
