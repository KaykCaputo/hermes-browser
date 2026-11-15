//! Logins in a macOS keychain through the Security framework's `SecItem` API.
//!
//! Every call names its keychain: items are added with `kSecUseKeychain` and searched, updated
//! and deleted with `kSecMatchSearchList` set to that one keychain. There is no code path that
//! leaves the keychain unspecified; [`KeychainBackend::login`] resolves the user's default
//! keychain (`SecKeychainCopyDefault`) and then passes it explicitly like any other.
//!
//! Item layout (one generic-password item per login):
//! - service (`svce`): [`KEYCHAIN_SERVICE`] for every login;
//! - account (`acct`): the versioned encoding of the login's key described in `account.rs`;
//! - label (`labl`): the site and user name, for example `example.com (alice)`, so the item is
//!   recognisable in Keychain Access;
//! - description (`desc`, shown as "Kind"): `Mercury Browser web login` or
//!   `Mercury Browser HTTP login`;
//! - data: the password as UTF-8;
//! - creation and modification dates (`cdat`, `mdat`): kept by the Keychain itself.
//!
//! Listing reads attributes only, never secret data, so it cannot make macOS ask the user.

use std::path::Path;

use core_foundation::data::CFData;
use security_framework::base::Error as SecError;
use security_framework::item::{
    ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, ItemUpdateOptions, ItemUpdateValue,
    Limit, Location, SearchResult, update_item,
};
use security_framework::os::macos::keychain::{CreateOptions, SecKeychain};
use zeroize::Zeroizing;

use crate::{
    LoginKey, LoginKind, LoginSummary, Password, PasswordError, SaveOutcome, SecretBackend, account,
};

/// The Keychain service name of every login this application saves: the application's bundle
/// identifier followed by `.logins`.
pub const KEYCHAIN_SERVICE: &str = "io.github.thepigdestroyer.mercury-browser.logins";

// `OSStatus` codes from `Security/SecBase.h`.
const ERR_SEC_USER_CANCELED: i32 = -128;
const ERR_SEC_AUTH_FAILED: i32 = -25293;
const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;

/// Logins in a macOS keychain, one generic-password item per login.
pub struct KeychainBackend {
    target: Target,
}

enum Target {
    /// The user's default keychain, looked up on every operation.
    Default,
    /// One keychain file.
    File(SecKeychain),
}

impl KeychainBackend {
    /// The user's default keychain. Used by the application only.
    pub fn login() -> Self {
        Self {
            target: Target::Default,
        }
    }

    /// A keychain file at `path`, created with `password` if absent. For tests and tools.
    ///
    /// An existing file is opened and unlocked with `password`; a wrong password gives
    /// [`PasswordError::AccessDenied`]. A new file is created unlocked. Neither adds the file to
    /// the user's keychain search list.
    pub fn file(path: &Path, password: &str) -> Result<Self, PasswordError> {
        if path.as_os_str().as_encoded_bytes().contains(&0) {
            return Err(PasswordError::InvalidInput(
                "the keychain path contains a NUL byte",
            ));
        }
        let keychain = if path.exists() {
            let mut keychain = SecKeychain::open(path).map_err(map_error)?;
            keychain
                .unlock(Some(password))
                .map_err(|error| match error.code() {
                    ERR_SEC_AUTH_FAILED => PasswordError::AccessDenied,
                    _ => map_error(error),
                })?;
            keychain
        } else {
            CreateOptions::new()
                .password(password)
                .create(path)
                .map_err(map_error)?
        };
        Ok(Self {
            target: Target::File(keychain),
        })
    }

    fn keychain(&self) -> Result<SecKeychain, PasswordError> {
        match &self.target {
            Target::Default => SecKeychain::default().map_err(map_error),
            Target::File(keychain) => Ok(keychain.clone()),
        }
    }
}

impl SecretBackend for KeychainBackend {
    /// `SecItemAdd`; if the item exists, `SecItemCopyMatching` for its data to compare, then
    /// `SecItemUpdate` only if the password differs.
    fn put(&self, key: &LoginKey, password: &Password) -> Result<SaveOutcome, PasswordError> {
        let keychain = self.keychain()?;
        let account = account::encode(key);
        match add(&keychain, key, &account, password) {
            Ok(()) => return Ok(SaveOutcome::Created),
            Err(error) if error.code() == ERR_SEC_DUPLICATE_ITEM => {}
            Err(error) => return Err(map_error(error)),
        }
        if let Some(existing) = read(&keychain, &account)?
            && existing.same_as(password)
        {
            return Ok(SaveOutcome::Unchanged);
        }
        let mut update = ItemUpdateOptions::new();
        update.set_value(ItemUpdateValue::Data(CFData::from_buffer(
            password.expose().as_bytes(),
        )));
        match update_item(search(&keychain).account(&account), &update) {
            Ok(()) => Ok(SaveOutcome::Updated),
            // Deleted by someone else between the two calls.
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => {
                add(&keychain, key, &account, password).map_err(map_error)?;
                Ok(SaveOutcome::Created)
            }
            Err(error) => Err(map_error(error)),
        }
    }

    /// `SecItemCopyMatching` with `kSecReturnData`.
    fn get(&self, key: &LoginKey) -> Result<Option<Password>, PasswordError> {
        read(&self.keychain()?, &account::encode(key))
    }

    /// `SecItemDelete`.
    fn delete(&self, key: &LoginKey) -> Result<bool, PasswordError> {
        let keychain = self.keychain()?;
        match search(&keychain).account(&account::encode(key)).delete() {
            Ok(()) => Ok(true),
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(false),
            Err(error) => Err(map_error(error)),
        }
    }

    /// `SecItemCopyMatching` with `kSecReturnAttributes` only. Items under the service whose
    /// account does not decode are skipped with a `debug` log line.
    fn list(&self) -> Result<Vec<LoginSummary>, PasswordError> {
        let keychain = self.keychain()?;
        let results = match search(&keychain)
            .load_attributes(true)
            .limit(Limit::All)
            .search()
        {
            Ok(results) => results,
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => return Ok(Vec::new()),
            Err(error) => return Err(map_error(error)),
        };
        let mut summaries = Vec::with_capacity(results.len());
        for result in &results {
            // Attribute names are the four-character codes the Keychain uses.
            let Some(attributes) = result.simplify_dict() else {
                log::debug!("skipping a Keychain search result that has no attributes");
                continue;
            };
            let Some(key) = attributes
                .get("acct")
                .and_then(|text| account::decode(text))
            else {
                log::debug!("skipping a Keychain item whose account is not a version-1 login");
                continue;
            };
            summaries.push(LoginSummary {
                key,
                created_at_ms: attributes.get("cdat").and_then(|text| parse_date(text)),
                modified_at_ms: attributes.get("mdat").and_then(|text| parse_date(text)),
            });
        }
        Ok(summaries)
    }
}

/// A search for this application's generic-password items in `keychain` only.
fn search(keychain: &SecKeychain) -> ItemSearchOptions {
    let mut options = ItemSearchOptions::new();
    options
        .keychains(std::slice::from_ref(keychain))
        .class(ItemClass::generic_password())
        .service(KEYCHAIN_SERVICE);
    options
}

fn add(
    keychain: &SecKeychain,
    key: &LoginKey,
    account: &str,
    password: &Password,
) -> Result<(), SecError> {
    let mut options = ItemAddOptions::new(ItemAddValue::Data {
        class: ItemClass::generic_password(),
        data: CFData::from_buffer(password.expose().as_bytes()),
    });
    options
        .set_location(Location::FileKeychain(keychain.clone()))
        .set_service(KEYCHAIN_SERVICE)
        .set_account_name(account)
        .set_label(label(key))
        .set_description(match key.kind {
            LoginKind::Form => "Mercury Browser web login",
            LoginKind::HttpAuth { .. } => "Mercury Browser HTTP login",
        });
    options.add()
}

fn read(keychain: &SecKeychain, account: &str) -> Result<Option<Password>, PasswordError> {
    let results = match search(keychain)
        .account(account)
        .load_data(true)
        .limit(1)
        .search()
    {
        Ok(results) => results,
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => return Ok(None),
        Err(error) => return Err(map_error(error)),
    };
    match results.into_iter().next() {
        Some(SearchResult::Data(bytes)) => {
            let bytes = Zeroizing::new(bytes);
            let text = std::str::from_utf8(&bytes).map_err(|_| PasswordError::CorruptItem)?;
            Ok(Some(Password::new(text.to_owned())))
        }
        Some(_) => Err(PasswordError::CorruptItem),
        None => Ok(None),
    }
}

/// `example.com (alice)`; `http://localhost:3000` for a loopback site without a user name;
/// HTTP-authentication logins add ` – realm`.
fn label(key: &LoginKey) -> String {
    let origin = key.origin.as_str();
    let mut label = origin.strip_prefix("https://").unwrap_or(origin).to_owned();
    if !key.username.is_empty() {
        label.push_str(" (");
        label.push_str(&key.username);
        label.push(')');
    }
    if let LoginKind::HttpAuth { realm } = &key.kind {
        label.push_str(" – ");
        label.push_str(realm);
    }
    label
}

fn map_error(error: SecError) -> PasswordError {
    match error.code() {
        ERR_SEC_INTERACTION_NOT_ALLOWED => PasswordError::Locked,
        // A locked keychain, an access list that does not trust this program, and a user who
        // clicked "Deny" all report errSecAuthFailed. With user interaction off the first two
        // are what happened: the system would have had to ask.
        ERR_SEC_AUTH_FAILED => {
            if SecKeychain::user_interaction_allowed().unwrap_or(true) {
                PasswordError::AccessDenied
            } else {
                PasswordError::Locked
            }
        }
        ERR_SEC_USER_CANCELED => PasswordError::AccessDenied,
        ERR_SEC_ITEM_NOT_FOUND => PasswordError::NotFound,
        code => PasswordError::Os {
            code,
            message: error
                .message()
                .unwrap_or_else(|| "no description".to_owned()),
        },
    }
}

/// Milliseconds since the Unix epoch of a Keychain date as `SearchResult::simplify_dict` shows
/// it: `2026-11-15 21:16:29 +0000`. Anything else gives `None`.
fn parse_date(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 25
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b' '
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b' '
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = text.get(range)?;
        if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let sign = match bytes[20] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let offset_minutes = sign * (number(21..23)? * 60 + number(23..25)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
        - offset_minutes * 60;
    seconds.checked_mul(1_000)
}

/// Days from 1970-01-01 to a date in the proleptic Gregorian calendar (the "days from civil"
/// algorithm described by Howard Hinnant, written here from its description).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_dates_parse_to_unix_milliseconds() {
        assert_eq!(parse_date("1970-01-01 00:00:00 +0000"), Some(0));
        assert_eq!(
            parse_date("2000-03-01 00:00:00 +0000"),
            Some(951_868_800_000)
        );
        assert_eq!(
            parse_date("2026-11-15 21:16:29 +0000"),
            Some(1_794_777_389_000)
        );
        assert_eq!(
            parse_date("2026-11-15 22:16:29 +0100"),
            Some(1_794_777_389_000)
        );
        assert_eq!(
            parse_date("2024-02-29 12:00:00 +0000"),
            Some(1_709_208_000_000)
        );
    }

    #[test]
    fn other_date_texts_are_unknown() {
        for text in [
            "",
            "2026-11-15",
            "2026-11-15T21:16:29Z",
            "20261115211629Z",
            "2026-13-15 21:16:29 +0000",
            "2026-11-15 24:16:29 +0000",
            "2026-11-15 21:16:29 0000",
            "2026-1a-15 21:16:29 +0000",
            "<CFDate 0x600000 [0x1f]>{time = 785}",
        ] {
            assert_eq!(parse_date(text), None, "{text:?}");
        }
    }

    #[test]
    fn labels_name_the_site_and_user() {
        let origin = |text| crate::Origin::parse(text).expect("valid");
        assert_eq!(
            label(&LoginKey::form(origin("https://example.com"), "alice")),
            "example.com (alice)"
        );
        assert_eq!(
            label(&LoginKey::form(origin("http://localhost:3000"), "")),
            "http://localhost:3000"
        );
        assert_eq!(
            label(&LoginKey::http_auth(
                origin("https://example.com:8443"),
                "Staff",
                "bob"
            )),
            "example.com:8443 (bob) – Staff"
        );
    }
}
