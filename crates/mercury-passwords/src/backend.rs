use crate::{LoginKey, LoginKind, LoginSummary, Origin, Password, PasswordError, SaveOutcome};

/// The longest user name accepted, in bytes of UTF-8.
pub const MAX_USERNAME_BYTES: usize = 1024;
/// The longest HTTP-authentication realm accepted, in bytes of UTF-8.
pub const MAX_REALM_BYTES: usize = 1024;
/// The longest password accepted, in bytes of UTF-8.
pub const MAX_PASSWORD_BYTES: usize = 4096;

/// Where secrets live. Implementations never return a secret from `list`.
pub trait SecretBackend {
    /// Saves `password` for `key`, creating the login or replacing its password.
    fn put(&self, key: &LoginKey, password: &Password) -> Result<SaveOutcome, PasswordError>;
    /// The password saved for `key`, or `None` if there is no such login.
    fn get(&self, key: &LoginKey) -> Result<Option<Password>, PasswordError>;
    /// Deletes the login; `false` if there was none.
    fn delete(&self, key: &LoginKey) -> Result<bool, PasswordError>;
    /// Every login this application saved, without passwords, in no particular order.
    fn list(&self) -> Result<Vec<LoginSummary>, PasswordError>;
}

/// Policy on top of a backend: validation, exact-origin lookup.
pub struct PasswordStore<B: SecretBackend> {
    backend: B,
}

impl<B: SecretBackend> PasswordStore<B> {
    /// A store that keeps its logins in `backend`.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    /// The backend, for callers that need it directly.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Gives the backend back.
    pub fn into_backend(self) -> B {
        self.backend
    }

    /// Saves a login after checking the key and the password.
    ///
    /// Refused with [`PasswordError::InvalidInput`]: an empty password; a user name, realm or
    /// password longer than [`MAX_USERNAME_BYTES`], [`MAX_REALM_BYTES`] or
    /// [`MAX_PASSWORD_BYTES`]; a control character (including NUL, tab and line breaks) in a
    /// user name or realm. An empty user name is allowed.
    pub fn save(&self, key: &LoginKey, password: &Password) -> Result<SaveOutcome, PasswordError> {
        validate_key(key)?;
        validate_password(password)?;
        self.backend.put(key, password)
    }

    /// Form logins for exactly this origin, sorted by user name. No secrets.
    pub fn logins_for(&self, origin: &Origin) -> Result<Vec<LoginSummary>, PasswordError> {
        self.matching(|key| &key.origin == origin && key.kind == LoginKind::Form)
    }

    /// HTTP-authentication logins for exactly this origin and realm, sorted by user name.
    pub fn http_auth_logins_for(
        &self,
        origin: &Origin,
        realm: &str,
    ) -> Result<Vec<LoginSummary>, PasswordError> {
        self.matching(|key| {
            &key.origin == origin
                && matches!(&key.kind, LoginKind::HttpAuth { realm: saved } if saved == realm)
        })
    }

    /// The password of one login, or `None` if it is not saved.
    pub fn password(&self, key: &LoginKey) -> Result<Option<Password>, PasswordError> {
        validate_key(key)?;
        self.backend.get(key)
    }

    /// Removes one login; `false` if it was not saved.
    pub fn remove(&self, key: &LoginKey) -> Result<bool, PasswordError> {
        validate_key(key)?;
        self.backend.delete(key)
    }

    /// Every saved login, sorted by origin, then form before HTTP authentication, then realm,
    /// then user name. No secrets.
    pub fn all(&self) -> Result<Vec<LoginSummary>, PasswordError> {
        let mut all = self.backend.list()?;
        all.sort_by(|a, b| sort_key(&a.key).cmp(&sort_key(&b.key)));
        Ok(all)
    }

    /// Removes every login this application saved and returns how many were removed.
    ///
    /// Only logins the backend lists are touched; on macOS that is items under this
    /// application's service name whose account decodes as a login.
    pub fn remove_all(&self) -> Result<u64, PasswordError> {
        let mut removed = 0;
        for summary in self.backend.list()? {
            if self.backend.delete(&summary.key)? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    fn matching(
        &self,
        wanted: impl Fn(&LoginKey) -> bool,
    ) -> Result<Vec<LoginSummary>, PasswordError> {
        let mut found: Vec<LoginSummary> = self
            .backend
            .list()?
            .into_iter()
            .filter(|summary| wanted(&summary.key))
            .collect();
        found.sort_by(|a, b| a.key.username.cmp(&b.key.username));
        Ok(found)
    }
}

fn sort_key(key: &LoginKey) -> (&Origin, Option<&str>, &str) {
    let realm = match &key.kind {
        LoginKind::Form => None,
        LoginKind::HttpAuth { realm } => Some(realm.as_str()),
    };
    (&key.origin, realm, &key.username)
}

fn validate_key(key: &LoginKey) -> Result<(), PasswordError> {
    if key.username.len() > MAX_USERNAME_BYTES {
        return Err(PasswordError::InvalidInput("the user name is too long"));
    }
    if key.username.chars().any(char::is_control) {
        return Err(PasswordError::InvalidInput(
            "the user name contains a control character",
        ));
    }
    if let LoginKind::HttpAuth { realm } = &key.kind {
        if realm.len() > MAX_REALM_BYTES {
            return Err(PasswordError::InvalidInput("the realm is too long"));
        }
        if realm.chars().any(char::is_control) {
            return Err(PasswordError::InvalidInput(
                "the realm contains a control character",
            ));
        }
    }
    Ok(())
}

fn validate_password(password: &Password) -> Result<(), PasswordError> {
    let text = password.expose();
    if text.is_empty() {
        return Err(PasswordError::InvalidInput("the password is empty"));
    }
    if text.len() > MAX_PASSWORD_BYTES {
        return Err(PasswordError::InvalidInput("the password is too long"));
    }
    Ok(())
}
