//! The Keychain account string of a login: an unambiguous, versioned encoding of its key, so a
//! login can be rebuilt from the item's attributes without reading its secret.
//!
//! ```text
//! account  = "v1" "|" kind "|" origin "|" realm "|" username
//! kind     = "form" / "http"
//! origin   = the text of Origin::as_str()   ; canonical, never contains "|" or "%"
//! realm    = escaped                        ; always empty for "form"
//! username = escaped
//! escaped  = *( plain / "%" HEXUPPER HEXUPPER )
//! plain    = any Unicode scalar value except "|", "%" and control characters
//! ```
//!
//! Escaping writes each UTF-8 byte of `|`, `%` and of every control character as `%` and two
//! upper-case hex digits. A string is accepted only if it is exactly what encoding its decoded
//! key produces, so every login has one account string and every account string at most one
//! login. Strings that do not decode — another version, someone else's item — yield `None`.

use crate::{LoginKey, LoginKind, Origin};

const VERSION: &str = "v1";
const SEPARATOR: char = '|';

/// The account string of `key`.
pub(crate) fn encode(key: &LoginKey) -> String {
    let (kind, realm) = match &key.kind {
        LoginKind::Form => ("form", ""),
        LoginKind::HttpAuth { realm } => ("http", realm.as_str()),
    };
    let mut account = String::with_capacity(
        VERSION.len()
            + kind.len()
            + key.origin.as_str().len()
            + realm.len()
            + key.username.len()
            + 4,
    );
    account.push_str(VERSION);
    account.push(SEPARATOR);
    account.push_str(kind);
    account.push(SEPARATOR);
    account.push_str(key.origin.as_str());
    account.push(SEPARATOR);
    escape_into(realm, &mut account);
    account.push(SEPARATOR);
    escape_into(&key.username, &mut account);
    account
}

/// The key whose account string is `account`, or `None` if `account` is not one.
pub(crate) fn decode(account: &str) -> Option<LoginKey> {
    let mut fields = account.split(SEPARATOR);
    let (version, kind, origin, realm, username) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    if fields.next().is_some() || version != VERSION {
        return None;
    }
    let parsed_origin = Origin::parse(origin).ok()?;
    if parsed_origin.as_str() != origin {
        return None;
    }
    let kind = match kind {
        "form" if realm.is_empty() => LoginKind::Form,
        "http" => LoginKind::HttpAuth {
            realm: unescape(realm)?,
        },
        _ => return None,
    };
    let key = LoginKey {
        origin: parsed_origin,
        kind,
        username: unescape(username)?,
    };
    (encode(&key) == account).then_some(key)
}

fn needs_escape(character: char) -> bool {
    character == SEPARATOR || character == '%' || character.is_control()
}

fn escape_into(text: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for character in text.chars() {
        if needs_escape(character) {
            let mut buffer = [0u8; 4];
            for byte in character.encode_utf8(&mut buffer).bytes() {
                out.push('%');
                out.push(char::from(HEX[usize::from(byte >> 4)]));
                out.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        } else {
            out.push(character);
        }
    }
}

fn unescape(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_value(*bytes.get(index + 1)?)?;
            let low = hex_value(*bytes.get(index + 2)?)?;
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(text: &str) -> Origin {
        Origin::parse(text).expect("valid test origin")
    }

    #[test]
    fn form_login_encodes_with_an_empty_realm() {
        let key = LoginKey::form(origin("https://example.com"), "alice");
        assert_eq!(encode(&key), "v1|form|https://example.com||alice");
        assert_eq!(decode("v1|form|https://example.com||alice"), Some(key));
    }

    #[test]
    fn separators_percent_signs_and_control_characters_are_escaped() {
        let key = LoginKey::http_auth(origin("https://example.com:8443"), "a|b%c", "tab\there|");
        let account = encode(&key);
        assert_eq!(
            account,
            "v1|http|https://example.com:8443|a%7Cb%25c|tab%09here%7C"
        );
        assert_eq!(decode(&account), Some(key));
    }

    #[test]
    fn non_ascii_is_kept_and_multibyte_controls_are_escaped_bytewise() {
        let key = LoginKey::form(origin("https://example.com"), "Jürgen\u{85}名前");
        let account = encode(&key);
        assert_eq!(account, "v1|form|https://example.com||Jürgen%C2%85名前");
        assert_eq!(decode(&account), Some(key));
    }

    #[test]
    fn empty_realm_and_empty_user_name_round_trip() {
        let key = LoginKey::http_auth(origin("https://example.com"), "", "");
        assert_eq!(encode(&key), "v1|http|https://example.com||");
        assert_eq!(decode("v1|http|https://example.com||"), Some(key));
    }

    #[test]
    fn non_canonical_and_foreign_strings_do_not_decode() {
        for account in [
            "",
            "not a login",
            "v1",
            "v1|form|https://example.com|",
            "v1|form|https://example.com||alice|extra",
            "v2|form|https://example.com||alice",
            "V1|form|https://example.com||alice",
            "v1|FORM|https://example.com||alice",
            "v1|basic|https://example.com||alice",
            "v1|form|https://example.com|realm|alice",
            "v1|form|https://EXAMPLE.com||alice",
            "v1|form|https://example.com/||alice",
            "v1|form|https://example.com:443||alice",
            "v1|form|http://example.com||alice",
            "v1|form|not-a-url||alice",
            "v1|form|https://example.com||a%7cb",
            "v1|form|https://example.com||%41",
            "v1|form|https://example.com||%",
            "v1|form|https://example.com||%7",
            "v1|form|https://example.com||%ZZ",
            "v1|form|https://example.com||%FF",
            "v1|form|https://example.com||tab\there",
        ] {
            assert_eq!(decode(account), None, "{account:?} must not decode");
        }
    }
}
