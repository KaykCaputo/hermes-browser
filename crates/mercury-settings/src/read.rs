//! Reading a file of the profile: opening it safely, learning its format version, and decoding
//! it within bounds.
//!
//! The version is read first, from a short peek at the start of the file, so that a file from
//! a newer version is recognised whatever its size or content. A file of the current version
//! (or an older one whose migration steps change nothing) is decoded straight into the typed
//! structures, streaming from the file, with every list bounded while it is read (see
//! [`crate::bounded`]). Only a real migration builds a generic tree, and that tree is bounded by
//! node count.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read as _, Seek as _};
use std::path::Path;

use serde::de::{self, DeserializeOwned, Deserializer as _, IgnoredAny, MapAccess, Visitor};
use serde_json::{Map, Value};

use crate::bounded::{self, Unread};
use crate::error::SettingsError;
use crate::settings::truncated;

pub(crate) const VERSION_KEY: &str = "format_version";
/// How much of a file the fast version peek looks at. A file this crate writes has its version
/// within the first 30 bytes.
const PEEK_BYTES: u64 = 256;
/// The longest reason quoted from a parser in a log or a message.
const MAX_REASON_BYTES: usize = 200;

/// One step of a migration ladder: what turns version `n` into version `n + 1`.
#[derive(Clone, Copy)]
pub(crate) enum Migration {
    /// The layout did not change; the document is decoded directly.
    Unchanged,
    /// The layout changed; the document goes through a bounded tree. No real step needs this
    /// yet (version 1 is the first); the tests exercise it with an invented ladder.
    #[cfg_attr(not(test), allow(dead_code))]
    Transform(fn(&mut Map<String, Value>)),
}

/// A file format: its name in messages, its size limit and its migration ladder.
pub(crate) struct Format {
    pub(crate) what: &'static str,
    pub(crate) max_bytes: u64,
    /// `migrations[n]` migrates version `n` to `n + 1`; its length is the current version.
    pub(crate) migrations: &'static [Migration],
}

impl Format {
    pub(crate) fn current_version(&self) -> u32 {
        u32::try_from(self.migrations.len()).unwrap_or(u32::MAX)
    }
}

/// What the start of a file says about it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Peek {
    Missing,
    Damaged(String),
    /// The format version; 0 when the document has none. A number too large for `u32` is
    /// `u32::MAX`.
    Version(u32),
}

/// What is on disk at a path, decoded as far as it goes.
#[derive(Debug)]
pub(crate) enum Inspection<T> {
    Missing,
    Damaged(String),
    Newer(u32),
    Current(T, Unread),
}

pub(crate) fn short_reason(reason: impl Into<String>) -> String {
    let reason = reason.into();
    if reason.len() <= MAX_REASON_BYTES {
        reason
    } else {
        format!("{}…", truncated(&reason, MAX_REASON_BYTES))
    }
}

fn not_regular(path: &Path, metadata: &std::fs::Metadata) -> SettingsError {
    let kind = if metadata.is_dir() {
        io::ErrorKind::IsADirectory
    } else {
        io::ErrorKind::InvalidInput
    };
    SettingsError::io("read", path, io::Error::new(kind, "not a regular file"))
}

/// Opens `path` for reading without ever blocking (`O_NONBLOCK`, so a FIFO does not wait for
/// a writer), then checks the type of the file actually opened: a directory, FIFO, socket or
/// device is refused. Returns the file and its length; `None` when there is nothing at the
/// path.
pub(crate) fn open_regular(path: &Path) -> Result<Option<(File, u64)>, SettingsError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(SettingsError::io("open", path, error)),
    };
    let metadata = file
        .metadata()
        .map_err(|error| SettingsError::io("inspect", path, error))?;
    if !metadata.is_file() {
        return Err(not_regular(path, &metadata));
    }
    Ok(Some((file, metadata.len())))
}

/// What the first bytes say.
#[derive(Debug, PartialEq, Eq)]
enum Head {
    Version(u32),
    BadVersion,
    NotObject,
    /// The first member is not `format_version`, or the peek ended too soon to tell.
    Undecided,
}

fn is_json_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// Looks for `{ "format_version": <digits>` at the start. `complete` says the bytes are the
/// whole file.
fn scan_head(bytes: &[u8], complete: bool) -> Head {
    let mut at = 0;
    let skip_space = |at: &mut usize| {
        while bytes.get(*at).copied().is_some_and(is_json_space) {
            *at += 1;
        }
    };
    skip_space(&mut at);
    match bytes.get(at) {
        Some(b'{') => at += 1,
        Some(_) => return Head::NotObject,
        None if complete => return Head::NotObject,
        None => return Head::Undecided,
    }
    skip_space(&mut at);
    let key = br#""format_version""#;
    if !bytes[at..].starts_with(key) {
        return Head::Undecided;
    }
    at += key.len();
    skip_space(&mut at);
    if bytes.get(at) != Some(&b':') {
        return Head::Undecided;
    }
    at += 1;
    skip_space(&mut at);
    let digits_start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
    }
    if at == bytes.len() && !complete {
        return Head::Undecided;
    }
    if at == digits_start {
        return Head::BadVersion;
    }
    let version = bytes[digits_start..at]
        .iter()
        .fold(0_u32, |version, digit| {
            version
                .saturating_mul(10)
                .saturating_add(u32::from(digit - b'0'))
        });
    skip_space(&mut at);
    match bytes.get(at) {
        Some(b',' | b'}') => Head::Version(version),
        None if !complete => Head::Undecided,
        _ => Head::BadVersion,
    }
}

/// Reads the version of the file at `path`: from the first member when it is there (the case
/// for every file this crate writes), otherwise by scanning the whole document without
/// building anything, within the size limit.
pub(crate) fn peek(path: &Path, format: &Format) -> Result<Peek, SettingsError> {
    let Some((mut file, length)) = open_regular(path)? else {
        return Ok(Peek::Missing);
    };
    let mut head = Vec::new();
    (&mut file)
        .take(PEEK_BYTES)
        .read_to_end(&mut head)
        .map_err(|error| SettingsError::io("read", path, error))?;
    let complete = u64::try_from(head.len()).unwrap_or(u64::MAX) >= length;
    match scan_head(&head, complete) {
        Head::Version(version) => return Ok(Peek::Version(version)),
        Head::BadVersion => {
            return Ok(Peek::Damaged(format!(
                "{VERSION_KEY} is not a version number"
            )));
        }
        Head::NotObject => return Ok(Peek::Damaged("not a JSON object".to_owned())),
        Head::Undecided => {}
    }

    if length > format.max_bytes {
        return Ok(Peek::Damaged(format!(
            "larger than {} bytes",
            format.max_bytes
        )));
    }
    file.rewind()
        .map_err(|error| SettingsError::io("read", path, error))?;
    let reader = BufReader::new(file.take(format.max_bytes.saturating_add(1)));
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let scanned = deserializer
        .deserialize_map(VersionScan)
        .and_then(|version| deserializer.end().map(|()| version));
    Ok(match scanned {
        Ok(version) => Peek::Version(version.unwrap_or(0)),
        Err(error) => Peek::Damaged(short_reason(error.to_string())),
    })
}

/// Walks the top-level members, keeping only `format_version`.
struct VersionScan;

impl<'de> Visitor<'de> for VersionScan {
    type Value = Option<u32>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut members: A) -> Result<Option<u32>, A::Error> {
        let mut version = None;
        while let Some(key) = members.next_key::<KeyIsVersion>()? {
            if key.0 && version.is_none() {
                version = Some(members.next_value::<VersionNumber>()?.0);
            } else {
                members.next_value::<IgnoredAny>()?;
            }
        }
        Ok(version)
    }
}

/// Whether a key is `format_version`, without keeping the key.
struct KeyIsVersion(bool);

impl<'de> serde::Deserialize<'de> for KeyIsVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Key;
        impl Visitor<'_> for Key {
            type Value = KeyIsVersion;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a key")
            }
            fn visit_str<E: de::Error>(self, key: &str) -> Result<KeyIsVersion, E> {
                Ok(KeyIsVersion(key == VERSION_KEY))
            }
        }
        deserializer.deserialize_str(Key)
    }
}

/// A version number: a non-negative integer, saturated to `u32::MAX`.
struct VersionNumber(u32);

impl<'de> serde::Deserialize<'de> for VersionNumber {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Number;
        impl Visitor<'_> for Number {
            type Value = VersionNumber;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a version number")
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<VersionNumber, E> {
                Ok(VersionNumber(u32::try_from(value).unwrap_or(u32::MAX)))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<VersionNumber, E> {
                // Integers beyond u64 arrive as floats.
                if value >= u64::MAX as f64 && value.fract() == 0.0 {
                    Ok(VersionNumber(u32::MAX))
                } else {
                    Err(E::custom(format!("{VERSION_KEY} is not a version number")))
                }
            }
        }
        deserializer.deserialize_any(Number)
    }
}

/// Reads, checks the version of, migrates and decodes the file at `path`.
pub(crate) fn inspect<T: DeserializeOwned>(
    path: &Path,
    format: &Format,
) -> Result<Inspection<T>, SettingsError> {
    let version = match peek(path, format)? {
        Peek::Missing => return Ok(Inspection::Missing),
        Peek::Damaged(reason) => return Ok(Inspection::Damaged(reason)),
        Peek::Version(version) => version,
    };
    let Some(steps) = format.migrations.get(version as usize..) else {
        return Ok(Inspection::Newer(version));
    };
    let Some((file, length)) = open_regular(path)? else {
        return Ok(Inspection::Missing);
    };
    if length > format.max_bytes {
        return Ok(Inspection::Damaged(format!(
            "larger than {} bytes",
            format.max_bytes
        )));
    }
    let reader = BufReader::new(file.take(format.max_bytes.saturating_add(1)));

    let (decoded, unread) = bounded::scoped(|| -> Result<T, String> {
        if steps
            .iter()
            .all(|step| matches!(step, Migration::Unchanged))
        {
            return serde_json::from_reader(reader).map_err(|error| error.to_string());
        }
        let Value::Object(mut document) = bounded::parse_tree(reader)? else {
            return Err("not a JSON object".to_owned());
        };
        document.remove(VERSION_KEY);
        for step in steps {
            if let Migration::Transform(migrate) = step {
                migrate(&mut document);
            }
        }
        serde_json::from_value(Value::Object(document)).map_err(|error| error.to_string())
    });
    Ok(match decoded {
        Ok(value) => Inspection::Current(value, unread),
        Err(reason) => Inspection::Damaged(short_reason(reason)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_head_scan_reads_a_leading_version() {
        assert_eq!(
            scan_head(b"{\n  \"format_version\": 1,\n", false),
            Head::Version(1)
        );
        assert_eq!(
            scan_head(br#"{"format_version":7}"#, true),
            Head::Version(7)
        );
        assert_eq!(
            scan_head(
                br#"{"format_version": 99999999999999999999999, "x": 1}"#,
                false
            ),
            Head::Version(u32::MAX)
        );
        assert_eq!(
            scan_head(br#"{"format_version": "1"}"#, true),
            Head::BadVersion
        );
        assert_eq!(
            scan_head(br#"{"format_version": -1}"#, true),
            Head::BadVersion
        );
        assert_eq!(
            scan_head(br#"{"format_version": 1.5}"#, true),
            Head::BadVersion
        );
        assert_eq!(
            scan_head(br#"{"format_version": 12"#, false),
            Head::Undecided
        );
        assert_eq!(
            scan_head(br#"{"homepage": "x", "format_version": 1}"#, true),
            Head::Undecided
        );
        assert_eq!(scan_head(b"{}", true), Head::Undecided);
        assert_eq!(scan_head(b"[1]", true), Head::NotObject);
        assert_eq!(scan_head(b"   ", true), Head::NotObject);
        assert_eq!(scan_head(b"   ", false), Head::Undecided);
    }

    #[cfg(unix)]
    #[test]
    fn opening_a_fifo_returns_at_once_with_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );

        let opened = open_regular(&path);

        assert!(
            matches!(opened, Err(SettingsError::Io { .. })),
            "{opened:?}"
        );
    }
}
