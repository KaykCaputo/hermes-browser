use std::fmt;

use crate::Origin;

/// Shows a value's length but not the value, in `Debug` output.
pub(crate) struct Redacted(pub(crate) usize);

impl Redacted {
    pub(crate) fn of(text: &str) -> Self {
        Self(text.chars().count())
    }
}

impl fmt::Debug for Redacted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "<redacted, {} chars>", self.0)
    }
}

/// How a login is used on its origin.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LoginKind {
    /// A user name and password typed into a form on a page.
    Form,
    /// HTTP authentication (`WWW-Authenticate`) for one realm of the origin.
    HttpAuth {
        /// The realm the server names in its challenge, exactly as sent.
        realm: String,
    },
}

/// What identifies one saved login. Two keys are the same login exactly when they are equal.
///
/// `Debug` shows the origin and kind but only the length of the user name.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct LoginKey {
    /// The only origin the login is offered on.
    pub origin: Origin,
    /// Form or HTTP authentication.
    pub kind: LoginKind,
    /// The user name; may be empty for sites that ask only for a password.
    pub username: String,
}

impl LoginKey {
    /// A form login.
    pub fn form(origin: Origin, username: impl Into<String>) -> Self {
        Self {
            origin,
            kind: LoginKind::Form,
            username: username.into(),
        }
    }

    /// An HTTP-authentication login for `realm`.
    pub fn http_auth(
        origin: Origin,
        realm: impl Into<String>,
        username: impl Into<String>,
    ) -> Self {
        Self {
            origin,
            kind: LoginKind::HttpAuth {
                realm: realm.into(),
            },
            username: username.into(),
        }
    }
}

impl fmt::Debug for LoginKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginKey")
            .field("origin", &self.origin)
            .field("kind", &self.kind)
            .field("username", &Redacted::of(&self.username))
            .finish()
    }
}

/// A saved login without its password. `Debug` redacts the user name as [`LoginKey`] does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginSummary {
    /// Which login this is.
    pub key: LoginKey,
    /// When the login was first saved, in milliseconds since the Unix epoch, if known.
    pub created_at_ms: Option<i64>,
    /// When its password last changed, in milliseconds since the Unix epoch, if known.
    pub modified_at_ms: Option<i64>,
}

/// What [`SecretBackend::put`](crate::SecretBackend::put) did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    /// There was no such login; it now exists.
    Created,
    /// The login existed with another password; the password was replaced.
    Updated,
    /// The login existed with this very password; nothing was written.
    Unchanged,
}
