use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{LoginKey, LoginSummary, Password, PasswordError, SaveOutcome, SecretBackend};

/// Logins kept in this process's memory only. For tests, and for platforms without a backend.
///
/// Passwords are wiped when they are replaced, deleted, or when the backend is dropped.
#[derive(Default)]
pub struct MemoryBackend {
    entries: Mutex<HashMap<LoginKey, Entry>>,
}

struct Entry {
    password: Password,
    created_at_ms: i64,
    modified_at_ms: i64,
}

impl MemoryBackend {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<LoginKey, Entry>> {
        // A panic while the lock was held cannot leave a map entry half-written.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

impl SecretBackend for MemoryBackend {
    fn put(&self, key: &LoginKey, password: &Password) -> Result<SaveOutcome, PasswordError> {
        let mut entries = self.entries();
        match entries.get_mut(key) {
            Some(entry) if entry.password.same_as(password) => Ok(SaveOutcome::Unchanged),
            Some(entry) => {
                entry.password = password.clone();
                entry.modified_at_ms = now_ms();
                Ok(SaveOutcome::Updated)
            }
            None => {
                let now = now_ms();
                entries.insert(
                    key.clone(),
                    Entry {
                        password: password.clone(),
                        created_at_ms: now,
                        modified_at_ms: now,
                    },
                );
                Ok(SaveOutcome::Created)
            }
        }
    }

    fn get(&self, key: &LoginKey) -> Result<Option<Password>, PasswordError> {
        Ok(self.entries().get(key).map(|entry| entry.password.clone()))
    }

    fn delete(&self, key: &LoginKey) -> Result<bool, PasswordError> {
        Ok(self.entries().remove(key).is_some())
    }

    fn list(&self) -> Result<Vec<LoginSummary>, PasswordError> {
        Ok(self
            .entries()
            .iter()
            .map(|(key, entry)| LoginSummary {
                key: key.clone(),
                created_at_ms: Some(entry.created_at_ms),
                modified_at_ms: Some(entry.modified_at_ms),
            })
            .collect())
    }
}
