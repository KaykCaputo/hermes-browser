// Adapted from Zervo (https://github.com/Goddv/Zervo), commit
// 86cb71267066f3b1f402abbd5c26faaf24ddf165, `src/downloads.rs` (`sanitize`, `filename_from_url`,
// `unique_path`, `format_bytes` and their tests), licensed MPL-2.0.
//
// Changes from Zervo: names are cut to 255 bytes on a character boundary keeping the extension;
// a name that was only an extension keeps it (`.pdf` → `download.pdf`); every Unicode
// default-ignorable character (except joiners and presentation selectors) and the line, paragraph
// and annotation separators are removed; COM0/LPT0, the superscript-digit device names and
// CONIN$/CONOUT$/CLOCK$ are covered; `unique_path` also treats the
// `.part` file of a name as taken, keeps the result short enough for that suffix, numbers
// `.tar.gz` as `name (1).tar.gz`, and never reads the clock; URL segments are decoded with the
// `percent-encoding` crate instead of a hand-written decoder.

//! Choosing a safe, unique file name for a download.
//!
//! Everything here is pure: no I/O and no clock, so the UI can show the name a download will get
//! before anything is written.

use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;

use crate::content_disposition;

/// The longest file name, in bytes, that APFS, ext4 and most other file systems accept.
const MAX_NAME_BYTES: usize = 255;

/// Appended to a download's final name while it is in progress.
pub(crate) const PART_SUFFIX: &str = ".part";

/// The longest name `unique_path` returns, so that `<name>.part` still fits.
/// Evaluated at compile time, so it cannot underflow.
const MAX_UNIQUE_BYTES: usize = MAX_NAME_BYTES - PART_SUFFIX.len();

/// The name used when nothing better is available.
const FALLBACK_NAME: &str = "download";

/// How many numbered alternatives `unique_path` tries before giving up.
const MAX_NUMBERED: u32 = 10_000;

/// The name a download should be saved under, before making it unique.
///
/// In order of preference: the `Content-Disposition` header's file name (`filename*` before
/// `filename`), the page's suggestion (for example `<a download="…">`), the last non-empty path
/// segment of the **final** URL after redirects (percent-decoded), and finally `download`. Every
/// source is sanitised, and one that sanitises to nothing is skipped. If the result has no
/// extension and `mime_type` maps to one, the extension is appended.
pub fn file_name_for(
    content_disposition: Option<&str>,
    final_url: &url::Url,
    mime_type: Option<&str>,
    suggested: Option<&str>,
) -> String {
    let name = content_disposition
        .and_then(content_disposition::filename)
        .and_then(|name| clean(&name))
        .or_else(|| suggested.and_then(clean))
        .or_else(|| last_url_segment(final_url).and_then(|segment| clean(&segment)))
        .unwrap_or_else(|| FALLBACK_NAME.to_owned());

    if Path::new(&name).extension().is_none()
        && let Some(extension) = mime_type.and_then(extension_for_mime)
    {
        return sanitize_file_name(&format!("{name}.{extension}"));
    }
    name
}

/// Turns a name chosen by a web server into a single safe path component.
///
/// The result never contains `/`, `\`, `:`, NUL, control characters or bidirectional and
/// invisible formatting characters; never starts with a dot or white space, so a download never
/// arrives hidden; never ends with a dot or white space, which Windows silently drops; is never
/// empty, `.` or `..` (those become `download`); has Windows device names (`CON`, `NUL.txt`, …)
/// prefixed with `_`; and is at most 255 bytes of UTF-8, cut on a character boundary with the
/// extension kept.
pub fn sanitize_file_name(name: &str) -> String {
    clean(name).unwrap_or_else(|| FALLBACK_NAME.to_owned())
}

/// A path in `dir` for `file_name` that is not taken, numbering it `name (1).ext`,
/// `name (2).ext`, … if needed.
///
/// A name counts as taken when `exists` returns `true` for it **or** for its `.part` file, so an
/// in-flight download keeps its name. The name is sanitised again and kept short enough that its
/// `.part` file also fits the 255-byte limit. If every numbered name up to 10,000 is taken the
/// last one is returned; the caller's exclusive create then fails rather than overwriting.
pub fn unique_path(dir: &Path, file_name: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let budget = MAX_UNIQUE_BYTES;
    let base = fit(&sanitize_file_name(file_name), budget);
    let is_free =
        |name: &str| !exists(&dir.join(name)) && !exists(&dir.join(format!("{name}{PART_SUFFIX}")));
    if is_free(&base) {
        return dir.join(base);
    }

    // An "extension" too long to keep when fitting is not kept when numbering either, so the
    // suffix is at most ` (10000)` plus a quarter of the budget and the stem always has room.
    let (stem, extension) = split_for_numbering(&base, budget);
    let mut candidate = base.clone();
    for index in 1..=MAX_NUMBERED {
        let suffix = format!(" ({index}){extension}");
        let stem =
            trim_end_dots_and_spaces(truncate_bytes(stem, budget.saturating_sub(suffix.len())));
        let stem = if stem.is_empty() { FALLBACK_NAME } else { stem };
        // Cannot be over budget given the limits above; `fit` makes that hold regardless.
        candidate = fit(&format!("{stem}{suffix}"), budget);
        if is_free(&candidate) {
            break;
        }
    }
    dir.join(candidate)
}

/// A byte count for people: `0 B`, `1023 B`, `1.5 KB`, `3.0 GB` (binary multiples).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The sanitised name, or `None` when nothing usable is left.
fn clean(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        // A right-to-left override makes `photo<RLO>gpj.exe` display as `photoexe.jpg`; the other
        // bidirectional and zero-width characters hide or reorder text in the same way, and
        // control characters (newlines included) corrupt every listing that prints the name.
        .filter(|character| !character.is_control() && !is_invisible_format(*character))
        .map(|character| match character {
            '/' | '\\' | ':' => '_',
            other => other,
        })
        .collect();

    let without_leading = cleaned.trim_start_matches(is_dot_or_space);
    let leading = cleaned.len().saturating_sub(without_leading.len());
    let had_leading_dot = cleaned[..leading].contains('.');
    let core = trim_end_dots_and_spaces(without_leading);
    if core.is_empty() {
        return None;
    }

    // `.pdf` is all extension: keep the type rather than produce an extensionless `pdf`.
    let mut name = if had_leading_dot && !core.contains('.') {
        format!("{FALLBACK_NAME}.{core}")
    } else {
        core.to_owned()
    };
    if is_windows_device_name(&name) {
        name.insert(0, '_');
    }
    Some(fit(&name, MAX_NAME_BYTES))
}

fn is_dot_or_space(character: char) -> bool {
    character == '.' || character.is_whitespace()
}

fn trim_end_dots_and_spaces(text: &str) -> &str {
    text.trim_end_matches(is_dot_or_space)
}

/// Unicode's `Default_Ignorable_Code_Point` property, from `DerivedCoreProperties.txt` of
/// Unicode 16.0.0 (unchanged in 17.0.0): characters that render as nothing, including the
/// bidirectional controls, soft hyphen, Hangul fillers, variation selectors and tag characters.
const DEFAULT_IGNORABLE: [(char, char); 17] = [
    ('\u{00ad}', '\u{00ad}'),
    ('\u{034f}', '\u{034f}'),
    ('\u{061c}', '\u{061c}'),
    ('\u{115f}', '\u{1160}'),
    ('\u{17b4}', '\u{17b5}'),
    ('\u{180b}', '\u{180f}'),
    ('\u{200b}', '\u{200f}'),
    ('\u{202a}', '\u{202e}'),
    ('\u{2060}', '\u{206f}'),
    ('\u{3164}', '\u{3164}'),
    ('\u{fe00}', '\u{fe0f}'),
    ('\u{feff}', '\u{feff}'),
    ('\u{ffa0}', '\u{ffa0}'),
    ('\u{fff0}', '\u{fff8}'),
    ('\u{1bca0}', '\u{1bca3}'),
    ('\u{1d173}', '\u{1d17a}'),
    ('\u{e0000}', '\u{e0fff}'),
];

/// Default-ignorable characters that are kept because real text needs them: the zero-width
/// non-joiner (Persian and other scripts) and joiner (emoji sequences), and the text/emoji
/// presentation selectors. Each is only ever drawn as part of its neighbours.
const KEPT_IGNORABLE: [char; 4] = ['\u{200c}', '\u{200d}', '\u{fe0e}', '\u{fe0f}'];

/// Invisible characters to remove that are neither `char::is_control` nor default-ignorable:
/// the line and paragraph separators and the interlinear annotation controls.
const OTHER_INVISIBLE: [(char, char); 2] = [('\u{2028}', '\u{2029}'), ('\u{fff9}', '\u{fffb}')];

/// Characters that hide, reorder or break up a name without being control characters.
fn is_invisible_format(character: char) -> bool {
    if KEPT_IGNORABLE.contains(&character) {
        return false;
    }
    DEFAULT_IGNORABLE
        .iter()
        .chain(OTHER_INVISIBLE.iter())
        .any(|(first, last)| (*first..=*last).contains(&character))
}

/// Opening `NUL` or `nul.txt` on Windows opens the device rather than creating a file.
fn is_windows_device_name(name: &str) -> bool {
    const DEVICES: [&str; 33] = [
        "CON",
        "PRN",
        "AUX",
        "NUL",
        "COM0",
        "COM1",
        "COM2",
        "COM3",
        "COM4",
        "COM5",
        "COM6",
        "COM7",
        "COM8",
        "COM9",
        "LPT0",
        "LPT1",
        "LPT2",
        "LPT3",
        "LPT4",
        "LPT5",
        "LPT6",
        "LPT7",
        "LPT8",
        "LPT9",
        "COM\u{b9}",
        "COM\u{b2}",
        "COM\u{b3}",
        "LPT\u{b9}",
        "LPT\u{b2}",
        "LPT\u{b3}",
        "CONIN$",
        "CONOUT$",
        "CLOCK$",
    ];
    let stem = name.split('.').next().unwrap_or("").trim_end();
    DEVICES
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device))
}

/// Cuts `name` to at most `max` bytes, keeping a reasonably short extension and never splitting
/// a character or leaving a trailing dot or space.
fn fit(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_owned();
    }
    let (stem, extension) = match split_extension(name) {
        (stem, extension) if keeps_extension(extension, max) => (stem, extension),
        _ => (name, ""),
    };
    let stem = trim_end_dots_and_spaces(truncate_bytes(stem, max.saturating_sub(extension.len())));
    let stem = if stem.is_empty() { FALLBACK_NAME } else { stem };
    format!("{stem}{extension}")
}

/// The longest prefix of `text` that is at most `max` bytes and ends on a character boundary.
fn truncate_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let end = (0..=max)
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(0);
    &text[..end]
}

/// `("report", ".pdf")`; `("README", "")`. A leading dot does not start an extension.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(index) if index > 0 && index + 1 < name.len() => name.split_at(index),
        _ => (name, ""),
    }
}

/// Whether an extension is short enough to be kept whole when a name is cut to `max` bytes.
fn keeps_extension(extension: &str, max: usize) -> bool {
    extension.len() <= max / 4
}

/// Like `split_extension`, but keeps `.tar.gz` and friends together so numbering gives
/// `archive (1).tar.gz`, and, like `fit`, does not treat an over-long tail as an extension.
fn split_for_numbering(name: &str, max: usize) -> (&str, &str) {
    let (stem, extension) = split_extension(name);
    if !keeps_extension(extension, max) {
        return (name, "");
    }
    if extension.is_empty() {
        return (stem, extension);
    }
    let (inner_stem, inner_extension) = split_extension(stem);
    let (compound_stem, compound) = name.split_at(inner_stem.len());
    if inner_extension.eq_ignore_ascii_case(".tar") && keeps_extension(compound, max) {
        (compound_stem, compound)
    } else {
        (stem, extension)
    }
}

/// The last non-empty path segment of `url`, percent-decoded, so `%2F` cannot hide a separator
/// from the sanitiser.
fn last_url_segment(url: &url::Url) -> Option<String> {
    url.path_segments()?
        .rev()
        .find(|segment| !segment.is_empty())
        .map(|segment| percent_decode_str(segment).decode_utf8_lossy().into_owned())
}

/// A file extension for a MIME type, or `None` when there is no single obvious one.
fn extension_for_mime(mime_type: &str) -> Option<&'static str> {
    let essence = mime_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let preferred = match essence.as_str() {
        "application/octet-stream" => return None,
        "text/plain" => "txt",
        "text/html" => "html",
        "text/css" => "css",
        "text/csv" => "csv",
        "text/javascript" | "application/javascript" => "js",
        "application/json" => "json",
        "application/xml" | "text/xml" => "xml",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-tar" => "tar",
        "application/x-7z-compressed" => "7z",
        "application/vnd.rar" | "application/x-rar-compressed" => "rar",
        "application/x-apple-diskimage" => "dmg",
        "application/epub+zip" => "epub",
        "application/msword" => "doc",
        "application/vnd.ms-excel" => "xls",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/svg+xml" => "svg",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/wav" | "audio/x-wav" => "wav",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        _ => "",
    };
    if !preferred.is_empty() {
        return Some(preferred);
    }

    // `mime_guess` lists extensions alphabetically (`text/plain` starts with `asm`), so only
    // trust it when the answer is unambiguous.
    let extensions = mime_guess::get_mime_extensions_str(&essence)?;
    let subtype = essence.split_once('/')?.1;
    let subtype = subtype.strip_prefix("x-").unwrap_or(subtype);
    extensions
        .iter()
        .copied()
        .find(|extension| *extension == subtype)
        .or(match extensions {
            [only] => Some(*only),
            _ => None,
        })
}
