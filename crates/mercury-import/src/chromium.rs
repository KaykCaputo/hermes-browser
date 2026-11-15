// Adapted from Zephium, https://github.com/zephium-browser/Zephium, commit
// 9026a1fffad74d6e201ef97fe8c02c1b01403cd2, crates/zephium-import/src/chromium.rs
// (lines 16-19, 214-343), licensed under MPL-2.0: reading `Bookmarks` and `AccountBookmarks`
// together with a size bound, merging them root by root, and the `History` query. Rewritten for
// Mercury's values (toolbar folder, dates, typed errors, URL policy); profile discovery is new.

//! Chrome and the browsers built on Chromium share one profile layout: a user-data directory
//! holding `Default` and `Profile N` directories, each with a `Bookmarks` JSON file and a
//! `History` SQLite database. A profile signed in to an account may keep that account's
//! bookmarks apart, in `AccountBookmarks`, in the same shape. Times in both are microseconds
//! since 1601-01-01.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::{
    BrowserProfile, ImportError, ImportedNode, ImportedVisit, MAX_TREE_DEPTH, VisitCollector,
    keep_importable, push_unique,
};
use crate::snapshot;

/// Milliseconds between 1601-01-01 (Chromium's epoch) and 1970-01-01.
const EPOCH_OFFSET_MS: i64 = 11_644_473_600_000;
/// The profile's own bookmarks, and those of the account it is signed in to.
const BOOKMARK_FILES: [&str; 2] = ["Bookmarks", "AccountBookmarks"];
/// Larger than any real bookmarks file.
const MAX_BOOKMARKS_BYTES: u64 = 32 * 1024 * 1024;

/// Each browser's user-data directory relative to the directories the caller passes: the
/// macOS application-support directory, the Linux config directory, and Windows
/// `%LOCALAPPDATA%`. Arc keeps Chromium's history but its bookmarks live in its own sidebar
/// file, which is not read here.
const USER_DATA_DIRS: &[(&str, &str)] = &[
    ("Chrome", "Google/Chrome"),
    ("Chrome", "google-chrome"),
    ("Chrome", "Google/Chrome/User Data"),
    ("Chromium", "Chromium"),
    ("Chromium", "chromium"),
    ("Chromium", "Chromium/User Data"),
    ("Brave", "BraveSoftware/Brave-Browser"),
    ("Brave", "BraveSoftware/Brave-Browser/User Data"),
    ("Edge", "Microsoft Edge"),
    ("Edge", "microsoft-edge"),
    ("Edge", "Microsoft/Edge/User Data"),
    ("Vivaldi", "Vivaldi"),
    ("Vivaldi", "vivaldi"),
    ("Vivaldi", "Vivaldi/User Data"),
    ("Arc", "Arc/User Data"),
];

/// Profiles found under the given base directories (the caller passes the platform's
/// application-support or config directory, so tests can pass a fixture directory).
///
/// A profile is a `Default` or `Profile N` directory holding a bookmarks file or a history
/// database; its name is the one the browser shows, from `Local State`. Profiles come grouped by
/// browser, `Default` first and then by number, and each directory is listed once.
pub fn find_chromium_profiles(base_dirs: &[PathBuf]) -> Vec<BrowserProfile> {
    let mut found = Vec::new();
    for (browser, relative) in USER_DATA_DIRS {
        for base in base_dirs {
            let user_data = base.join(relative);
            for profile in profiles_in(browser, &user_data) {
                push_unique(&mut found, profile);
            }
        }
    }
    found
}

fn profiles_in(browser: &str, user_data: &Path) -> Vec<BrowserProfile> {
    let Ok(entries) = std::fs::read_dir(user_data) else {
        return Vec::new();
    };
    let state: Option<Value> = std::fs::read(user_data.join("Local State"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let mut profiles: Vec<(u64, BrowserProfile)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let dir_name = entry.file_name().into_string().ok()?;
            let order = profile_order(&dir_name)?;
            let path = entry.path();
            let has_data = BOOKMARK_FILES
                .iter()
                .chain(&["History"])
                .any(|file| path.join(file).is_file());
            if !has_data {
                return None;
            }
            let name = state
                .as_ref()
                .and_then(|state| state.get("profile")?.get("info_cache")?.get(&dir_name))
                .and_then(|info| info.get("name")?.as_str())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(&dir_name)
                .to_owned();
            Some((
                order,
                BrowserProfile {
                    browser: (*browser).to_owned(),
                    name,
                    path,
                },
            ))
        })
        .collect();
    profiles.sort_by_key(|(order, _)| *order);
    profiles.into_iter().map(|(_, profile)| profile).collect()
}

/// `Default` sorts first, `Profile N` by N; other directories are not profiles a person uses.
fn profile_order(dir_name: &str) -> Option<u64> {
    if dir_name == "Default" {
        return Some(0);
    }
    let number = dir_name.strip_prefix("Profile ")?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    number.parse::<u64>().ok()?.checked_add(1)
}

/// Bookmarks of the profile at `profile`: the bookmarks bar as a toolbar folder, then the
/// "other" and "mobile" roots, each only when it holds something. The profile's file and its
/// account's are read together, root by root; either alone is enough, and one that is there but
/// cannot be read fails the whole import.
pub fn read_chromium_bookmarks(profile: &Path) -> Result<Vec<ImportedNode>, ImportError> {
    let mut files = Vec::new();
    for name in BOOKMARK_FILES {
        let path = profile.join(name);
        match read_bounded(&path) {
            Ok(bytes) => files.push((path, bytes)),
            Err(ImportError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    if files.is_empty() {
        return Err(ImportError::NotFound(profile.join(BOOKMARK_FILES[0])));
    }
    let mut roots = [
        Root::new("Bookmarks bar", true),
        Root::new("Other bookmarks", false),
        Root::new("Mobile bookmarks", false),
    ];
    for (path, bytes) in &files {
        let malformed =
            |detail: &str| ImportError::Malformed(format!("{}: {detail}", path.display()));
        let file: Value =
            serde_json::from_slice(bytes).map_err(|error| malformed(&error.to_string()))?;
        let file_roots = file
            .get("roots")
            .and_then(Value::as_object)
            .ok_or_else(|| malformed("no \"roots\" object"))?;
        for (root, key) in roots.iter_mut().zip(["bookmark_bar", "other", "synced"]) {
            if let Some(value) = file_roots.get(key) {
                if !value.is_object() {
                    return Err(malformed(&format!("root \"{key}\" is not an object")));
                }
                root.add(value);
            }
        }
    }
    let nodes = roots
        .into_iter()
        .filter(|root| !root.children.is_empty())
        .map(Root::into_node)
        .collect();
    Ok(keep_importable(nodes, &profile.display().to_string()))
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, ImportError> {
    let too_large = || ImportError::TooLarge {
        path: path.to_owned(),
        limit: MAX_BOOKMARKS_BYTES,
    };
    let metadata = std::fs::metadata(path).map_err(|error| ImportError::io(path, error))?;
    if metadata.len() > MAX_BOOKMARKS_BYTES {
        return Err(too_large());
    }
    let file = std::fs::File::open(path).map_err(|error| ImportError::io(path, error))?;
    let mut bytes = Vec::new();
    // Bounded again while reading: a pipe or a growing file reports a smaller size than it gives.
    file.take(MAX_BOOKMARKS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ImportError::io(path, error))?;
    if bytes.len() as u64 > MAX_BOOKMARKS_BYTES {
        return Err(too_large());
    }
    Ok(bytes)
}

/// What one root of the bookmarks tree holds across the files read.
struct Root {
    title: Option<String>,
    fallback: &'static str,
    toolbar: bool,
    added_at_ms: Option<i64>,
    children: Vec<ImportedNode>,
}

impl Root {
    fn new(fallback: &'static str, toolbar: bool) -> Root {
        Root {
            title: None,
            fallback,
            toolbar,
            added_at_ms: None,
            children: Vec::new(),
        }
    }

    fn add(&mut self, root: &Value) {
        if self.title.is_none() {
            self.title = root
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .map(str::to_owned);
        }
        self.added_at_ms = self.added_at_ms.or_else(|| date(root));
        children(root, 1, &mut self.children);
    }

    fn into_node(self) -> ImportedNode {
        ImportedNode::Folder {
            title: self.title.unwrap_or_else(|| self.fallback.to_owned()),
            toolbar: self.toolbar,
            added_at_ms: self.added_at_ms,
            children: self.children,
        }
    }
}

/// Appends the children of `folder` to `out`. Folders deeper than `MAX_TREE_DEPTH` are opened
/// into their parent; `serde_json` already refuses files nested deeper than 128 levels.
fn children(folder: &Value, depth: usize, out: &mut Vec<ImportedNode>) {
    let Some(nodes) = folder.get("children").and_then(Value::as_array) else {
        return;
    };
    for value in nodes {
        let title = value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        match value.get("type").and_then(Value::as_str) {
            Some("url") => {
                if let Some(url) = value.get("url").and_then(Value::as_str) {
                    out.push(ImportedNode::Bookmark {
                        title,
                        url: url.to_owned(),
                        added_at_ms: date(value),
                    });
                }
            }
            Some("folder") if depth < MAX_TREE_DEPTH => {
                let mut inside = Vec::new();
                children(value, depth + 1, &mut inside);
                out.push(ImportedNode::Folder {
                    title,
                    toolbar: false,
                    added_at_ms: date(value),
                    children: inside,
                });
            }
            Some("folder") => children(value, depth, out),
            // Kinds a newer Chromium may add carry nothing this import knows how to keep.
            _ => {}
        }
    }
}

/// `date_added`, a decimal string of microseconds since 1601, as Unix milliseconds; `"0"` means
/// none was recorded.
fn date(node: &Value) -> Option<i64> {
    let value = node.get("date_added")?;
    let micros = match value {
        Value::String(text) => text.trim().parse::<i64>().ok()?,
        Value::Number(number) => number.as_i64()?,
        _ => return None,
    };
    if micros <= 0 {
        return None;
    }
    chrome_time_to_unix_ms(micros)
}

fn chrome_time_to_unix_ms(micros: i64) -> Option<i64> {
    micros.div_euclid(1000).checked_sub(EPOCH_OFFSET_MS)
}

/// Visits of the profile at `profile`, newest first, at most `limit` and
/// [`MAX_IMPORTED_VISITS`](crate::MAX_IMPORTED_VISITS). The `History` database is copied and the
/// copy read, so a running browser's lock does not matter.
pub fn read_chromium_history(
    profile: &Path,
    limit: usize,
) -> Result<Vec<ImportedVisit>, ImportError> {
    let path = profile.join("History");
    let snapshot = snapshot::open(&path)?;
    snapshot.require("urls", &["id", "url", "title"])?;
    snapshot.require("visits", &["id", "url", "visit_time"])?;
    let mut collector = VisitCollector::new(limit);
    if collector.is_full() {
        return Ok(Vec::new());
    }
    snapshot.rows(
        "SELECT u.url, u.title, v.visit_time FROM visits v JOIN urls u ON u.id = v.url
         ORDER BY v.visit_time DESC, v.id DESC",
        |row| {
            let Some(url) = row.get::<_, Option<String>>(0)? else {
                return Ok(true);
            };
            let Some(visited_at_ms) = chrome_time_to_unix_ms(row.get(2)?) else {
                return Ok(true);
            };
            Ok(collector.push(url, row.get(1)?, visited_at_ms))
        },
    )?;
    Ok(collector.finish(&path.display().to_string()))
}
