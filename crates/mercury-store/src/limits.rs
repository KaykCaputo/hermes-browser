//! Bounds on stored text that comes from the web.

use crate::error::{Result, StoreError};

/// Page and bookmark titles are cut to this many characters (Firefox's `TITLE_LENGTH_MAX`).
pub const MAX_TITLE_CHARS: usize = 4096;

/// URLs longer than this many bytes are refused with [`StoreError::InvalidArgument`] wherever
/// they would be stored (Firefox's `URI_LENGTH_MAX`). Lookups by a longer URL find nothing.
pub const MAX_URL_BYTES: usize = 65_536;

/// Download MIME types are cut to this many characters (RFC 6838 allows 127 + 1 + 127).
pub const MAX_MIME_TYPE_CHARS: usize = 255;

/// Download error messages are cut to this many characters.
pub const MAX_ERROR_CHARS: usize = 1024;

/// An `ETag` or `Last-Modified` value longer than this many bytes is not stored: a cut value
/// would never match, so it is dropped instead and the download cannot be resumed.
pub const MAX_VALIDATOR_BYTES: usize = 1024;

/// Download target and temporary paths longer than this many bytes are refused with
/// [`StoreError::InvalidArgument`]; a cut path would name another file.
pub const MAX_PATH_BYTES: usize = 4096;

/// `text` cut to at most `max_chars` characters, at a character boundary.
pub(crate) fn cut(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// Refuses a URL longer than [`MAX_URL_BYTES`].
pub(crate) fn check_url(url: &str) -> Result<()> {
    if url.len() > MAX_URL_BYTES {
        return Err(StoreError::InvalidArgument("URL too long"));
    }
    Ok(())
}

/// Refuses a path longer than [`MAX_PATH_BYTES`].
pub(crate) fn check_path(path: &str) -> Result<()> {
    if path.len() > MAX_PATH_BYTES {
        return Err(StoreError::InvalidArgument("path too long"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::cut;

    #[test]
    fn cut_respects_character_boundaries() {
        assert_eq!(cut("héllo", 2), "hé");
        assert_eq!(cut("héllo", 5), "héllo");
        assert_eq!(cut("héllo", 50), "héllo");
        assert_eq!(cut("", 3), "");
        assert_eq!(cut("abc", 0), "");
    }
}
