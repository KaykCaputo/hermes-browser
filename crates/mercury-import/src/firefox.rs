// Adapted from Zephium, https://github.com/zephium-browser/Zephium, commit
// 9026a1fffad74d6e201ef97fe8c02c1b01403cd2, crates/zephium-import/src/firefox.rs
// (lines 118-227, 269-392), licensed under MPL-2.0: reading `profiles.ini` and profile-group
// databases, building the bookmark tree from `moz_bookmarks`, and the history query. Rewritten
// for Mercury's values (toolbar folder, dates, separators, typed errors, URL policy) and with an
// iterative walk for damaged trees; the list of profile trees is new.

//! Firefox lists its profiles in `profiles.ini`; each profile keeps bookmarks and history
//! together in `places.sqlite`, with times in microseconds since the Unix epoch. LibreWolf,
//! Waterfox, Floorp and Zen are built on Firefox and keep the same files in a tree of their own.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use crate::model::{
    BrowserProfile, ImportError, ImportedNode, ImportedVisit, MAX_TREE_DEPTH, VisitCollector,
    keep_importable, push_unique,
};
use crate::snapshot;

/// Each browser's profile tree (the directory holding `profiles.ini`) relative to the
/// directories the caller passes: the macOS application-support directory, Windows
/// `%APPDATA%`, and on Linux the config directory (Firefox 147 and later) and the home
/// directory (older installs).
const PROFILE_TREES: &[(&str, &str)] = &[
    ("Firefox", "Firefox"),
    ("Firefox", "Mozilla/Firefox"),
    ("Firefox", "mozilla/firefox"),
    ("Firefox", ".mozilla/firefox"),
    ("LibreWolf", "LibreWolf"),
    ("LibreWolf", "librewolf/librewolf"),
    ("LibreWolf", ".librewolf"),
    ("Waterfox", "Waterfox"),
    ("Waterfox", ".waterfox"),
    ("Floorp", "Floorp"),
    ("Floorp", ".floorp"),
    ("Zen", "zen"),
    ("Zen", ".zen"),
];

/// Profile-group databases read from one tree; a real one has a handful.
const MAX_GROUPS: usize = 8;

/// Profiles found under the given base directories (the caller passes the platform's
/// application-support or config directory, so tests can pass a fixture directory).
///
/// Profiles are those `profiles.ini` lists, and those in Firefox's profile-group databases
/// (`Profile Groups/*.sqlite`), that hold a `places.sqlite`. In each tree the profile an install
/// starts by default comes first; each directory is listed once.
pub fn find_firefox_profiles(base_dirs: &[PathBuf]) -> Vec<BrowserProfile> {
    let mut found = Vec::new();
    for (browser, relative) in PROFILE_TREES {
        for base in base_dirs {
            let tree = base.join(relative);
            if !tree.join("profiles.ini").is_file() {
                continue;
            }
            let mut listed = ini_profiles(&tree);
            listed.extend(grouped(&tree));
            // A stable sort keeps the file's order within each kind.
            listed.sort_by_key(|listed| !listed.default);
            for listed in listed {
                if listed.dir.join("places.sqlite").is_file() {
                    push_unique(
                        &mut found,
                        BrowserProfile {
                            browser: (*browser).to_owned(),
                            name: listed.name,
                            path: listed.dir,
                        },
                    );
                }
            }
        }
    }
    found
}

struct Listed {
    name: String,
    dir: PathBuf,
    default: bool,
}

fn ini_profiles(tree: &Path) -> Vec<Listed> {
    let Ok(text) = std::fs::read_to_string(tree.join("profiles.ini")) else {
        return Vec::new();
    };
    let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            sections.push((name.to_owned(), HashMap::new()));
        } else if let (Some((key, value)), Some((_, values))) =
            (line.split_once('='), sections.last_mut())
        {
            values.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    let installed_default: HashSet<&str> = sections
        .iter()
        .filter(|(name, _)| name.starts_with("Install"))
        .filter_map(|(_, values)| values.get("Default").map(String::as_str))
        .collect();
    sections
        .iter()
        .filter(|(name, _)| name.starts_with("Profile"))
        .filter_map(|(_, values)| {
            let path = values.get("Path")?;
            let relative = values.get("IsRelative").is_none_or(|value| value == "1");
            Some(Listed {
                name: values.get("Name").cloned().unwrap_or_else(|| path.clone()),
                dir: if relative {
                    tree.join(path)
                } else {
                    PathBuf::from(path)
                },
                default: installed_default.contains(path.as_str())
                    || values.get("Default").is_some_and(|value| value == "1"),
            })
        })
        .collect()
}

/// Profiles made with Firefox's profile groups are kept in a database per group,
/// `Profile Groups/<store id>.sqlite`; `profiles.ini` lists only the most recent profile of
/// each group, with a path relative to the tree.
fn grouped(tree: &Path) -> Vec<Listed> {
    let mut stores: Vec<PathBuf> = std::fs::read_dir(tree.join("Profile Groups"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sqlite")
        })
        .collect();
    stores.sort();
    stores.truncate(MAX_GROUPS);
    let mut listed = Vec::new();
    for store in stores {
        let Ok(snapshot) = snapshot::open(&store) else {
            continue;
        };
        let mut rows = Vec::new();
        let read = snapshot.rows("SELECT path, name FROM Profiles ORDER BY id", |row| {
            rows.push((row.get::<_, String>(0)?, row.get::<_, String>(1)?));
            Ok(true)
        });
        if read.is_err() {
            continue;
        }
        // The database is another program's file: a path may not leave the tree.
        listed.extend(
            rows.into_iter()
                .filter(|(path, _)| {
                    !path.is_empty()
                        && Path::new(path)
                            .components()
                            .all(|part| matches!(part, Component::Normal(_)))
                })
                .map(|(path, name)| Listed {
                    name,
                    dir: tree.join(path),
                    default: false,
                }),
        );
    }
    listed
}

/// The fixed roots that become folders, in the order they are returned.
const ROOTS: [(&str, &str, bool); 4] = [
    ("toolbar_____", "Bookmarks Toolbar", true),
    ("menu________", "Bookmarks Menu", false),
    ("unfiled_____", "Other Bookmarks", false),
    ("mobile______", "Mobile Bookmarks", false),
];
/// Roots whose contents are not bookmarks of their own: the tree's root and the tags.
const HIDDEN_ROOTS: [&str; 2] = ["root________", "tags________"];

const TYPE_BOOKMARK: i64 = 1;
const TYPE_FOLDER: i64 = 2;
const TYPE_SEPARATOR: i64 = 3;

/// Bookmarks of the profile at `profile`: the toolbar as a toolbar folder, then the menu,
/// "Other Bookmarks" and mobile roots, each only when it holds something. Tags are not
/// bookmarks and are left out. `places.sqlite` is copied and the copy read, so a running
/// Firefox's lock does not matter.
pub fn read_firefox_bookmarks(profile: &Path) -> Result<Vec<ImportedNode>, ImportError> {
    let path = profile.join("places.sqlite");
    let snapshot = snapshot::open(&path)?;
    snapshot.require(
        "moz_bookmarks",
        &[
            "id",
            "type",
            "fk",
            "parent",
            "position",
            "title",
            "dateAdded",
            "guid",
        ],
    )?;
    snapshot.require("moz_places", &["id", "url"])?;
    let mut tree = Tree::default();
    let mut root_ids: HashMap<String, i64> = HashMap::new();
    snapshot.rows(
        "SELECT b.id, b.parent, b.type, b.title, p.url, b.guid, b.dateAdded
         FROM moz_bookmarks b LEFT JOIN moz_places p ON p.id = b.fk
         ORDER BY b.parent, b.position, b.id",
        |row| {
            let id: i64 = row.get(0)?;
            if let Some(guid) = row.get::<_, Option<String>>(5)?
                && (ROOTS.iter().any(|(root, ..)| *root == guid)
                    || HIDDEN_ROOTS.contains(&guid.as_str()))
            {
                root_ids.insert(guid, id);
            }
            tree.rows.insert(
                id,
                Row {
                    kind: row.get::<_, Option<i64>>(2)?.unwrap_or_default(),
                    title: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    url: row.get(4)?,
                    added_at_ms: row
                        .get::<_, Option<i64>>(6)?
                        .filter(|micros| *micros > 0)
                        .map(|micros| micros.div_euclid(1000)),
                },
            );
            if let Some(parent) = row.get::<_, Option<i64>>(1)? {
                tree.children.entry(parent).or_default().push(id);
            }
            Ok(true)
        },
    )?;
    if !ROOTS.iter().any(|(guid, ..)| root_ids.contains_key(*guid)) {
        return Err(ImportError::Malformed(format!(
            "{} has none of the bookmark roots",
            path.display()
        )));
    }
    // Roots are entered only as roots, so a damaged file cannot nest one in another.
    let mut visited: HashSet<i64> = root_ids.values().copied().collect();
    let mut nodes = Vec::new();
    for (guid, title, toolbar) in ROOTS {
        let Some(&id) = root_ids.get(guid) else {
            continue;
        };
        let mut children = Vec::new();
        tree.fill(id, 1, &mut visited, &mut children);
        if !children.is_empty() {
            nodes.push(ImportedNode::Folder {
                title: title.to_owned(),
                toolbar,
                added_at_ms: tree.rows.get(&id).and_then(|row| row.added_at_ms),
                children,
            });
        }
    }
    Ok(keep_importable(nodes, &path.display().to_string()))
}

struct Row {
    kind: i64,
    title: String,
    url: Option<String>,
    added_at_ms: Option<i64>,
}

#[derive(Default)]
struct Tree {
    rows: HashMap<i64, Row>,
    /// Children of each folder, in position order.
    children: HashMap<i64, Vec<i64>>,
}

impl Tree {
    fn kids(&self, id: i64) -> &[i64] {
        self.children
            .get(&id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The node for a bookmark or separator row; folders are handled by the callers.
    fn leaf(&self, row: &Row) -> Option<ImportedNode> {
        match row.kind {
            TYPE_BOOKMARK => Some(ImportedNode::Bookmark {
                title: row.title.clone(),
                url: row.url.clone()?,
                added_at_ms: row.added_at_ms,
            }),
            TYPE_SEPARATOR => Some(ImportedNode::Separator),
            _ => None,
        }
    }

    /// Appends the contents of folder `id`, which sits `depth` levels down, to `out`. Every row
    /// is entered once, so a loop in a damaged file ends; folders past `MAX_TREE_DEPTH` are
    /// opened into their parent.
    fn fill(&self, id: i64, depth: usize, visited: &mut HashSet<i64>, out: &mut Vec<ImportedNode>) {
        for &child in self.kids(id) {
            let Some(row) = self.rows.get(&child) else {
                continue;
            };
            if !visited.insert(child) {
                continue;
            }
            if row.kind != TYPE_FOLDER {
                out.extend(self.leaf(row));
            } else if depth < MAX_TREE_DEPTH {
                let mut children = Vec::new();
                self.fill(child, depth + 1, visited, &mut children);
                out.push(ImportedNode::Folder {
                    title: row.title.clone(),
                    toolbar: false,
                    added_at_ms: row.added_at_ms,
                    children,
                });
            } else {
                self.flatten(child, visited, out);
            }
        }
    }

    /// Appends everything inside folder `id`, at any depth, without folders. Iterative, so a
    /// chain of thousands of folders cannot exhaust the stack.
    fn flatten(&self, id: i64, visited: &mut HashSet<i64>, out: &mut Vec<ImportedNode>) {
        let mut stack = vec![self.kids(id).iter()];
        while let Some(top) = stack.last_mut() {
            let Some(&child) = top.next() else {
                stack.pop();
                continue;
            };
            let Some(row) = self.rows.get(&child) else {
                continue;
            };
            if !visited.insert(child) {
                continue;
            }
            if row.kind == TYPE_FOLDER {
                stack.push(self.kids(child).iter());
            } else {
                out.extend(self.leaf(row));
            }
        }
    }
}

/// Visits of the profile at `profile`, newest first, at most `limit` and
/// [`MAX_IMPORTED_VISITS`](crate::MAX_IMPORTED_VISITS). `places.sqlite` is copied and the copy
/// read, so a running Firefox's lock does not matter.
pub fn read_firefox_history(
    profile: &Path,
    limit: usize,
) -> Result<Vec<ImportedVisit>, ImportError> {
    let path = profile.join("places.sqlite");
    let snapshot = snapshot::open(&path)?;
    snapshot.require("moz_places", &["id", "url", "title"])?;
    snapshot.require("moz_historyvisits", &["id", "place_id", "visit_date"])?;
    let mut collector = VisitCollector::new(limit);
    if collector.is_full() {
        return Ok(Vec::new());
    }
    snapshot.rows(
        "SELECT p.url, p.title, v.visit_date
         FROM moz_historyvisits v JOIN moz_places p ON p.id = v.place_id
         ORDER BY v.visit_date DESC, v.id DESC",
        |row| {
            let (Some(url), Some(micros)) = (
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<i64>>(2)?,
            ) else {
                return Ok(true);
            };
            Ok(collector.push(url, row.get(1)?, micros.div_euclid(1000)))
        },
    )?;
    Ok(collector.finish(&path.display().to_string()))
}
