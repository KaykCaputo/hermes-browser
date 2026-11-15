//! The neutral values every reader returns, the errors they report, and the rule for which
//! addresses may be imported at all.

use std::path::{Path, PathBuf};

use rusqlite::Row;
use url::Url;

/// One entry of an imported bookmark tree, in the source's own order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportedNode {
    Folder {
        title: String,
        /// The folder the source shows as its bookmarks bar (toolbar).
        toolbar: bool,
        /// Milliseconds since the Unix epoch, when the source recorded it.
        added_at_ms: Option<i64>,
        children: Vec<ImportedNode>,
    },
    Bookmark {
        title: String,
        url: String,
        /// Milliseconds since the Unix epoch, when the source recorded it.
        added_at_ms: Option<i64>,
    },
    Separator,
}

/// One visit read from another browser's history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedVisit {
    pub url: String,
    /// The page title the source kept; `None` when it kept none or an empty one.
    pub title: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub visited_at_ms: i64,
}

/// The most visits one history import returns, whatever `limit` the caller passes.
///
/// Chrome keeps 90 days of history; a heavy user visits around a thousand pages a day, so this
/// covers a full Chrome history and the most recent months of a Firefox one. At a few hundred
/// bytes a visit the result stays in the tens of megabytes.
pub const MAX_IMPORTED_VISITS: usize = 100_000;

/// A browser profile found on disk that may be imported from.
///
/// Arc profiles (`browser == "Arc"`) are Chromium profiles whose history imports as usual, but
/// Arc keeps its bookmarks in its own sidebar file (`StorableSidebar.json`), which this crate
/// does not read: for an Arc profile `read_chromium_bookmarks` returns `NotFound` or nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserProfile {
    /// The browser's product name, for example `Chrome` or `Firefox`.
    pub browser: String,
    /// The profile's name as the browser shows it.
    pub name: String,
    /// The profile directory, to pass to the matching `read_*` function.
    pub path: PathBuf,
}

/// Why an import failed. An import either returns everything it read or one of these.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The file or profile is not there.
    #[error("{} was not found", .0.display())]
    NotFound(PathBuf),
    /// The file cannot be read consistently now: another program holds it so that it cannot be
    /// copied (Windows, while the browser runs), or the browser kept writing to it while it was
    /// being copied.
    #[error(
        "{} is in use: the browser is writing to it; try again, or close the browser first",
        .0.display()
    )]
    Locked(PathBuf),
    /// The file is larger than any real one of its kind.
    #[error("{} is larger than {limit} bytes", path.display())]
    TooLarge { path: PathBuf, limit: u64 },
    /// The file could not be read.
    #[error("{} could not be read: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The database could not be opened or queried, for example because it is damaged.
    #[error("{} could not be read as a database: {source}", path.display())]
    Database {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    /// The content is not in the shape this browser writes: truncated JSON, a database with an
    /// unexpected schema, or a file that is not a bookmark file.
    #[error("unexpected format: {0}")]
    Malformed(String),
}

impl ImportError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> ImportError {
        let path = path.to_owned();
        match source.kind() {
            std::io::ErrorKind::NotFound => ImportError::NotFound(path),
            // ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION on Windows.
            _ if cfg!(windows) && matches!(source.raw_os_error(), Some(32 | 33)) => {
                ImportError::Locked(path)
            }
            _ => ImportError::Io { path, source },
        }
    }

    pub(crate) fn database(path: &Path, source: rusqlite::Error) -> ImportError {
        let path = path.to_owned();
        match source.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                ImportError::Locked(path)
            }
            _ => ImportError::Database { path, source },
        }
    }
}

/// The address in normalised form if it may be imported: only `http`, `https`, `ftp`, `file`
/// and `about`. Script, data and browser-internal addresses (`javascript:`, `data:`, `chrome:`,
/// `place:`, …) and anything that does not parse are refused. What is returned is the parsed
/// URL's serialisation, so surrounding spaces, letter case in the scheme and host, and
/// characters that need escaping never leave this crate.
pub(crate) fn importable_url(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https" | "ftp" | "file" | "about"))
        .map(String::from)
}

/// Removes bookmarks whose address may not be imported, at every depth, normalises the
/// addresses of the rest, and logs how many were removed.
pub(crate) fn keep_importable(nodes: Vec<ImportedNode>, source: &str) -> Vec<ImportedNode> {
    let mut dropped = 0;
    let kept = filter_nodes(nodes, &mut dropped);
    if dropped > 0 {
        log::debug!(
            "{source}: left out {dropped} bookmarks with addresses that cannot be imported"
        );
    }
    kept
}

/// Leaves out root folders with nothing in them. Run after [`keep_importable`], so a root that
/// held only refused addresses is left out too.
pub(crate) fn without_empty_roots(roots: Vec<ImportedNode>) -> Vec<ImportedNode> {
    roots
        .into_iter()
        .filter(
            |root| !matches!(root, ImportedNode::Folder { children, .. } if children.is_empty()),
        )
        .collect()
}

fn filter_nodes(nodes: Vec<ImportedNode>, dropped: &mut usize) -> Vec<ImportedNode> {
    nodes
        .into_iter()
        .filter_map(|node| match node {
            ImportedNode::Folder {
                title,
                toolbar,
                added_at_ms,
                children,
            } => Some(ImportedNode::Folder {
                title,
                toolbar,
                added_at_ms,
                children: filter_nodes(children, dropped),
            }),
            ImportedNode::Bookmark {
                title,
                url,
                added_at_ms,
            } => match importable_url(&url) {
                Some(url) => Some(ImportedNode::Bookmark {
                    title,
                    url,
                    added_at_ms,
                }),
                None => {
                    *dropped += 1;
                    None
                }
            },
            ImportedNode::Separator => Some(ImportedNode::Separator),
        })
        .collect()
}

/// Collects history visits newest first, keeping only importable addresses, up to `cap`.
pub(crate) struct VisitCollector {
    visits: Vec<ImportedVisit>,
    cap: usize,
    dropped: usize,
    unreadable: usize,
}

impl VisitCollector {
    pub(crate) fn new(limit: usize) -> VisitCollector {
        VisitCollector {
            visits: Vec::new(),
            cap: limit.min(MAX_IMPORTED_VISITS),
            dropped: 0,
            unreadable: 0,
        }
    }

    pub(crate) fn is_full(&self) -> bool {
        self.visits.len() >= self.cap
    }

    /// Adds the visit in `row`, whose columns are the address, the title and the stored time;
    /// returns whether there is room for more. A row whose values cannot be read (a blob, or
    /// text that is not UTF-8), or whose time is missing, zero or negative, is skipped and
    /// counted.
    pub(crate) fn push_row(&mut self, row: &Row<'_>, to_unix_ms: fn(i64) -> Option<i64>) -> bool {
        if self.is_full() {
            return false;
        }
        let read = || -> rusqlite::Result<_> {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        };
        let visit = read().ok().and_then(|(url, title, time)| {
            let visited_at_ms = time.filter(|time| *time > 0).and_then(to_unix_ms)?;
            Some((url?, title, visited_at_ms))
        });
        let Some((url, title, visited_at_ms)) = visit else {
            self.unreadable += 1;
            return true;
        };
        match importable_url(&url) {
            Some(url) => self.visits.push(ImportedVisit {
                url,
                title: title.filter(|title| !title.is_empty()),
                visited_at_ms,
            }),
            None => self.dropped += 1,
        }
        !self.is_full()
    }

    pub(crate) fn finish(self, source: &str) -> Vec<ImportedVisit> {
        if self.dropped > 0 {
            log::debug!(
                "{source}: left out {} visits to addresses that cannot be imported",
                self.dropped
            );
        }
        if self.unreadable > 0 {
            log::warn!(
                "{source}: skipped {} history rows that could not be read",
                self.unreadable
            );
        }
        self.visits
    }
}

/// Adds `profile` unless one with the same directory is already in `found`. Directories are
/// compared after resolving links and, on case-insensitive file systems, letter case, so that
/// `Chromium` and `chromium` on macOS give one profile.
pub(crate) fn push_unique(found: &mut Vec<BrowserProfile>, profile: BrowserProfile) {
    if !found
        .iter()
        .any(|known| same_directory(&known.path, &profile.path))
    {
        found.push(profile);
    }
}

#[cfg(unix)]
fn same_directory(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => a == b,
    }
}

#[cfg(not(unix))]
fn same_directory(a: &Path, b: &Path) -> bool {
    let key = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
    key(a) == key(b)
}

/// Folders nested deeper than this are opened into their parent. Real browsers nest a handful of
/// levels; a hostile file cannot exhaust the stack of the code that walks the tree.
pub(crate) const MAX_TREE_DEPTH: usize = 48;
