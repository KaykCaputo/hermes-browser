use std::fmt;

use zeroize::Zeroizing;

/// A password in memory: wiped when dropped, never printed.
///
/// `Debug` prints `Password(…)`. There is no `Display`, no serialisation and no equality, so a
/// password cannot end up in a log line or an assertion message by accident. Read the value
/// with [`Password::expose`] only where it is handed to the store or to a page.
#[derive(Clone)]
pub struct Password(Zeroizing<String>);

impl Password {
    /// Takes ownership of `text`; its buffer is wiped when the password is dropped.
    pub fn new(text: String) -> Self {
        Self(Zeroizing::new(text))
    }

    /// The password in clear text.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether two passwords hold the same bytes. The time taken depends on the lengths only,
    /// not on where the first difference is.
    pub(crate) fn same_as(&self, other: &Password) -> bool {
        let left = self.0.as_bytes();
        let right = other.0.as_bytes();
        if left.len() != right.len() {
            return false;
        }
        left.iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Password(…)")
    }
}
