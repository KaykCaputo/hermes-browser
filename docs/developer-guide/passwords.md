---
title: Saved Logins
description: How the mercury-passwords crate stores website logins in the macOS Keychain, generates passwords and reads and writes login CSV files.
---

# Saved logins

`crates/mercury-passwords` is the storage and logic behind Mercury's password manager. It keeps
website logins in the operating system's credential store, generates passwords, and reads and
writes the login CSV files other browsers export. It has no UI and does not detect, offer to
save or fill forms; those parts build on it.

## The model

| Type | Meaning |
| --- | --- |
| `Origin` | `scheme://host[:port]`: `https`, or `http` for loopback hosts only. Lower-case ASCII host (internationalised names in their `xn--` form), no trailing dot, default port omitted. |
| `LoginKey` | origin + kind (`Form` or `HttpAuth { realm }`) + user name. Two keys are the same login exactly when they are equal. |
| `Password` | A secret in memory: wiped on drop, `Debug` prints `Password(…)`, no `Display`. |
| `SecretBackend` | Where secrets live: `put`, `get`, `delete`, `list` (never returns secrets). |
| `PasswordStore<B>` | Validation and exact-origin lookup on top of a backend. |

A login is offered on **exactly one origin**. `https://example.com`, `https://www.example.com`,
`http://example.com` and `https://example.com:8443` are four different sites (and the third is not
an origin that can hold logins at all). Nothing matches subdomains.

`PasswordStore::save` refuses empty passwords, control characters (NUL, tab, line breaks and the
rest) in user names and realms, and values longer than `MAX_USERNAME_BYTES` (1024),
`MAX_REALM_BYTES` (1024) and `MAX_PASSWORD_BYTES` (4096). An empty user name is allowed.

## Storage on macOS

`KeychainBackend` keeps one **generic-password** Keychain item per login, written with the
`SecItem` API through the `security-framework` crate. Every call names its keychain explicitly
(`kSecUseKeychain` when adding, `kSecMatchSearchList` when searching, updating and deleting).
`KeychainBackend::login()` resolves the user's default keychain and passes it like any other;
`KeychainBackend::file(path, password)` targets one keychain file and is what tests use.

| Attribute | Value |
| --- | --- |
| service (`svce`) | `io.github.thepigdestroyer.mercury-browser.logins` (`KEYCHAIN_SERVICE`) |
| account (`acct`) | a versioned encoding of origin, kind, realm and user name — see below |
| label (`labl`) | the site and user name, for example `example.com (alice)`; HTTP logins add ` – realm` |
| description (`desc`) | `Mercury Browser web login` or `Mercury Browser HTTP login` |
| data | the password, UTF-8 |
| `cdat`, `mdat` | creation and modification times, kept by the Keychain (one-second resolution) |

The account string lets `list` rebuild every login from attributes alone, without reading a
secret:

```text
account  = "v1" "|" kind "|" origin "|" realm "|" username
kind     = "form" / "http"
origin   = Origin::as_str()     ; canonical, never contains "|" or "%"
realm    = escaped              ; always empty for "form"
username = escaped
escaped  = any text, with each UTF-8 byte of "|", "%" and of control characters written as
           "%" and two upper-case hex digits
```

Only canonical strings decode: a string is accepted if encoding its decoded key gives the same
string back. Items under the service whose account does not decode (another version, someone
else's item) are skipped with a `debug` log line, never reported as errors and never deleted —
`remove_all` only deletes what `list` returned.

A Keychain item as `security dump-keychain` shows it (from a temporary keychain; the secret is
not displayed):

```text
class: "genp"
attributes:
    0x00000007 <blob>="example.com (alice)"
    "acct"<blob>="v1|form|https://example.com||alice"
    "cdat"<timedate>=0x32303236313131353231323635375A00  "20261115212657Z\000"
    "desc"<blob>="Mercury Browser web login"
    "mdat"<timedate>=0x32303236313131353231323635375A00  "20261115212657Z\000"
    "svce"<blob>="io.github.thepigdestroyer.mercury-browser.logins"
```

### What can make macOS ask the user

| Operation | Calls | Can prompt in the application? |
| --- | --- | --- |
| `list` | `SecItemCopyMatching`, attributes only | No: file-keychain attributes are not protected by the item's access list (they are readable even while the keychain is locked). |
| `get` | `SecItemCopyMatching` with `kSecReturnData` | Yes: to unlock a locked keychain, and when the item's access list does not trust this build. |
| `put`, new login | `SecItemAdd` | Only to unlock a locked keychain. |
| `put`, existing login | `SecItemAdd` (duplicate), `SecItemCopyMatching` with data to compare, `SecItemUpdate` if different | Yes, as `get`. |
| `delete` | `SecItemDelete` | Not observed (deleting worked even on a locked test keychain); not verified against a rebuilt application. |

A new item's access list trusts only the program that created it. For an **ad-hoc-signed** build
that trust is pinned to the binary's code-directory hash (`requirement: cdhash H"…"` in
`security dump-keychain -a`), so **every rebuild is a different program to the Keychain**:
reading a password saved by an earlier build shows the system's "wants to use your confidential
information stored in … in your keychain" dialog, once per item, and "Always Allow" lasts only
until the next build. With user interaction disabled the same read fails with
`errSecAuthFailed`, which the crate reports as `PasswordError::Locked`. Releases signed with a
stable Developer ID identity keep a stable designated requirement and do not re-prompt. See
Apple's [TN3137: On Mac keychain APIs and implementations](https://developer.apple.com/documentation/technotes/tn3137-on-mac-keychains)
for the file-based keychain and its access-list model.

### Errors

`PasswordError` never carries a password, user name, realm, account string or CSV field. Keychain
statuses map as follows: `errSecInteractionNotAllowed` → `Locked`; `errSecAuthFailed` →
`Locked` when user interaction is off (locked keychain, or an access list that would have
prompted), otherwise `AccessDenied`; `errSecUserCanceled` → `AccessDenied`; anything else →
`Os { code, message }` with the system's own text for the code.

## Password generator

`generate_password(&GeneratorOptions, &mut impl RandomSource)` draws one character from each
enabled class, fills the rest from their union, then shuffles (Fisher–Yates). Each choice uses
one random byte and rejects bytes at or above `256 - 256 % n`, so there is no modulo bias.
`OsRandom` reads the operating system's generator through `getrandom`.

- Classes: `a–z`, `A–Z`, `0–9`, and the symbols `!#$%&*+-.:=?@^_~`.
- `avoid_ambiguous` removes `0 O o 1 I l |`.
- Length 8 to 128; the default is 20 characters, all classes, ambiguous characters avoided
  (about 114.9 bits by `entropy_bits`).

## CSV import and export

`parse_logins_csv` reads the exports of Chrome, Edge and Brave (`name,url,username,password,note`,
`note` optional), Firefox (`url,username,password,httpRealm,formActionOrigin,guid,timeCreated,timeLastUsed,timePasswordChanged`)
and Safari (`Title,URL,Username,Password,Notes,OTPAuth`, `Notes` optional), told apart by their
header. Rows with an unusable URL, an Android-app entry, an empty password, broken quoting or the
wrong number of fields are skipped and listed by line number with a reason that never contains a
field value. `write_logins_csv` writes Chrome's format.

**An exported file holds every password in clear text.** Write one only when the user asks for
it, to the place they chose, as a new file only they can read (mode `0600`), and suggest
deleting it after use. Never write it to a temporary directory or a log.

## Testing without touching real keychains

Keychain tests must never read or change the developer's own keychains, nor show a dialog:

- every test creates its own keychain file in a fresh temporary directory, with an invented
  password, and a guard deletes the directory on every path, including panics;
- before the first Keychain call, the test process disables Keychain user interaction for its
  whole life (`SecKeychain::disable_user_interaction`, held in a static), so an operation that
  would prompt fails instead;
- creating a keychain with `SecKeychainCreate` does not add it to the search list; check
  `security list-keychains -d user` before and after a test run all the same;
- the backend contract (`tests/backend_contract.rs`) runs the same suite against `MemoryBackend`
  and a temporary `KeychainBackend`.

Use the `security` tool read-only and only with an explicit path to a temporary keychain (for
example `security dump-keychain <path>`), never with `-d`, which prints secrets.

## Other platforms

On platforms other than macOS the crate builds with `MemoryBackend` only; a Secret Service
backend for Linux is planned. Every macOS-specific line is behind `cfg(target_os = "macos")`.
