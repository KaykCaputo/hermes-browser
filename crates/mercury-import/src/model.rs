//! The neutral values every reader returns, the errors they report, and the rule for which
//! addresses may be imported at all.

use std::path::PathBuf;

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
    /// Another program holds the file so that it cannot be read now (Windows, while the browser
    /// runs).
    #[error("{} is locked by another program", .0.display())]
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

/// Whether an address may be imported: only `http`, `https`, `ftp`, `file` and `about`.
/// Script, data and browser-internal addresses (`javascript:`, `data:`, `chrome:`, `place:`, …)
/// and anything that does not parse are refused.
pub(crate) fn is_importable_url(url: &str) -> bool {
    Url::parse(url)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https" | "ftp" | "file" | "about"))
}

/// Removes bookmarks whose address may not be imported, at every depth, and logs how many.
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
            ImportedNode::Bookmark { ref url, .. } if !is_importable_url(url) => {
                *dropped += 1;
                None
            }
            node => Some(node),
        })
        .collect()
}

/// Folders nested deeper than this are opened into their parent. Real browsers nest a handful of
/// levels; a hostile file cannot exhaust the stack of the code that walks the tree.
pub(crate) const MAX_TREE_DEPTH: usize = 48;
