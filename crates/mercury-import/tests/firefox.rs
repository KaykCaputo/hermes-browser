//! Firefox-family profiles, built in temporary directories: `places.sqlite` is created here with
//! the tables of Firefox's `nsPlacesTables.h`, and `profiles.ini` is written by hand.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mercury_import::{
    BrowserProfile, ImportError, ImportedNode, ImportedVisit, find_firefox_profiles,
    read_firefox_bookmarks, read_firefox_history,
};
use rusqlite::{Connection, params};

const SCHEMA: &str = "
    CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR,
        rev_host LONGVARCHAR, visit_count INTEGER DEFAULT 0, hidden INTEGER DEFAULT 0 NOT NULL,
        typed INTEGER DEFAULT 0 NOT NULL, frecency INTEGER DEFAULT -1 NOT NULL,
        last_visit_date INTEGER, guid TEXT, foreign_count INTEGER DEFAULT 0 NOT NULL,
        url_hash INTEGER DEFAULT 0 NOT NULL);
    CREATE TABLE moz_historyvisits (id INTEGER PRIMARY KEY, from_visit INTEGER,
        place_id INTEGER, visit_date INTEGER, visit_type INTEGER, session INTEGER,
        source INTEGER DEFAULT 0 NOT NULL);
    CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER DEFAULT NULL,
        parent INTEGER, position INTEGER, title LONGVARCHAR, keyword_id INTEGER,
        folder_type TEXT, dateAdded INTEGER, lastModified INTEGER, guid TEXT,
        syncStatus INTEGER NOT NULL DEFAULT 0, syncChangeCounter INTEGER NOT NULL DEFAULT 1);
";

/// The fixed roots every `places.sqlite` has, added 2023-07-22 (1 690 000 000 000 000 µs).
const ROOTS: &str = "
    INSERT INTO moz_bookmarks (id, type, parent, position, title, dateAdded, guid) VALUES
        (1, 2, 0, 0, '', 1690000000000000, 'root________'),
        (2, 2, 1, 0, 'menu', 1690000000000000, 'menu________'),
        (3, 2, 1, 1, 'toolbar', 1690000000000000, 'toolbar_____'),
        (4, 2, 1, 2, 'tags', 1690000000000000, 'tags________'),
        (5, 2, 1, 3, 'unfiled', 1690000000000000, 'unfiled_____'),
        (6, 2, 1, 4, 'mobile', 1690000000000000, 'mobile______');
";

const ROOT_ADDED: Option<i64> = Some(1_690_000_000_000);

fn places(dir: &Path) -> Connection {
    std::fs::create_dir_all(dir).unwrap();
    let conn = Connection::open(dir.join("places.sqlite")).unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    conn
}

/// A profile with bookmarks in the toolbar, the menu, tags and mobile roots, and some history.
fn sample_places(dir: &Path) -> Connection {
    let conn = places(dir);
    conn.execute_batch(ROOTS).unwrap();
    conn.execute_batch(
        "INSERT INTO moz_places (id, url, title) VALUES
             (1, 'https://news.example/', 'News'),
             (2, 'https://docs.example/', NULL),
             (3, 'place:sort=8&maxResults=10', 'Most Visited'),
             (4, 'https://menu.example/', 'Menu'),
             (5, 'https://phone.example/', ''),
             (6, 'javascript:alert(1)', 'Script');
         -- Rows in another order than their positions.
         INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, dateAdded, guid) VALUES
             (12, 2, NULL, 3, 2, 'Reading', 1700000100000000, 'reading_____'),
             (11, 3, NULL, 3, 1, NULL, 1700000050000000, 'separator___'),
             (10, 1, 1, 3, 0, 'News', 1700000000123456, 'news________'),
             (13, 1, 2, 12, 0, NULL, 1700000200000000, 'docs________'),
             (14, 1, 3, 3, 3, 'Most Visited', 1700000300000000, 'mostvisited_'),
             (15, 1, 6, 12, 1, 'Script', 1700000300000000, 'script______'),
             (20, 1, 4, 2, 0, 'Menu item', 0, 'menuitem____'),
             (30, 2, NULL, 4, 0, 'rust', 1700000400000000, 'tagrust_____'),
             (31, 1, 1, 30, 0, NULL, 1700000400000000, 'tagged______'),
             (40, 1, 5, 6, 0, 'Phone', NULL, 'phone_______');
         INSERT INTO moz_historyvisits (place_id, visit_date, visit_type) VALUES
             (1, 1700000000000999, 1),
             (2, 1700000300000000, 2),
             (3, 1700000400000000, 1),
             (6, 1700000450000000, 1),
             (1, 1700000500000000, 1),
             (5, 1700000100000000, 1);",
    )
    .unwrap();
    conn
}

fn bookmark(title: &str, url: &str, added_at_ms: Option<i64>) -> ImportedNode {
    ImportedNode::Bookmark {
        title: title.into(),
        url: url.into(),
        added_at_ms,
    }
}

fn folder(
    title: &str,
    toolbar: bool,
    added_at_ms: Option<i64>,
    children: Vec<ImportedNode>,
) -> ImportedNode {
    ImportedNode::Folder {
        title: title.into(),
        toolbar,
        added_at_ms,
        children,
    }
}

fn visit(url: &str, title: Option<&str>, visited_at_ms: i64) -> ImportedVisit {
    ImportedVisit {
        url: url.into(),
        title: title.map(Into::into),
        visited_at_ms,
    }
}

fn sample_tree() -> Vec<ImportedNode> {
    vec![
        folder(
            "Bookmarks Toolbar",
            true,
            ROOT_ADDED,
            vec![
                bookmark("News", "https://news.example/", Some(1_700_000_000_123)),
                ImportedNode::Separator,
                folder(
                    "Reading",
                    false,
                    Some(1_700_000_100_000),
                    vec![bookmark(
                        "",
                        "https://docs.example/",
                        Some(1_700_000_200_000),
                    )],
                ),
            ],
        ),
        folder(
            "Bookmarks Menu",
            false,
            ROOT_ADDED,
            vec![bookmark("Menu item", "https://menu.example/", None)],
        ),
        folder(
            "Mobile Bookmarks",
            false,
            ROOT_ADDED,
            vec![bookmark("Phone", "https://phone.example/", None)],
        ),
    ]
}

#[test]
fn the_toolbar_root_becomes_a_toolbar_folder_and_tags_are_not_bookmarks() {
    let profile = tempfile::tempdir().unwrap();
    drop(sample_places(profile.path()));

    let nodes = read_firefox_bookmarks(profile.path()).expect("the fixture should be read");

    assert_eq!(
        nodes,
        sample_tree(),
        "the empty unfiled root is left out, place: and javascript: are dropped"
    );
}

#[test]
fn history_is_newest_first_in_unix_milliseconds_and_capped_by_the_limit() {
    let profile = tempfile::tempdir().unwrap();
    drop(sample_places(profile.path()));

    let all = read_firefox_history(profile.path(), 100).unwrap();

    assert_eq!(
        all,
        vec![
            visit("https://news.example/", Some("News"), 1_700_000_500_000),
            visit("https://docs.example/", None, 1_700_000_300_000),
            visit("https://phone.example/", None, 1_700_000_100_000),
            visit("https://news.example/", Some("News"), 1_700_000_000_000),
        ]
    );
    assert_eq!(
        read_firefox_history(profile.path(), 1).unwrap(),
        all[..1].to_vec()
    );
    assert_eq!(read_firefox_history(profile.path(), 0).unwrap(), vec![]);
}

#[test]
fn changes_still_in_the_write_ahead_log_are_read_and_the_files_left_untouched() {
    let profile = tempfile::tempdir().unwrap();
    let conn = places(profile.path());
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;")
        .unwrap();
    conn.execute_batch(ROOTS).unwrap();
    conn.execute_batch(
        "INSERT INTO moz_places (id, url, title) VALUES (1, 'https://wal.example/', 'WAL');
         INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, dateAdded, guid)
             VALUES (10, 1, 1, 3, 0, 'In the log', NULL, 'wal_________');
         INSERT INTO moz_historyvisits (place_id, visit_date) VALUES (1, 1700000000000000);",
    )
    .unwrap();
    let database = profile.path().join("places.sqlite");
    let wal = profile.path().join("places.sqlite-wal");
    assert!(
        std::fs::metadata(&wal).unwrap().len() > 0,
        "the rows are in the log"
    );
    let before = (
        std::fs::read(&database).unwrap(),
        std::fs::read(&wal).unwrap(),
    );

    let nodes = read_firefox_bookmarks(profile.path()).unwrap();
    let visits = read_firefox_history(profile.path(), 10).unwrap();

    assert_eq!(
        nodes,
        vec![folder(
            "Bookmarks Toolbar",
            true,
            ROOT_ADDED,
            vec![bookmark("In the log", "https://wal.example/", None)]
        )]
    );
    assert_eq!(
        visits,
        vec![visit(
            "https://wal.example/",
            Some("WAL"),
            1_700_000_000_000
        )]
    );
    assert_eq!(
        (
            std::fs::read(&database).unwrap(),
            std::fs::read(&wal).unwrap()
        ),
        before,
        "the browser's files are not changed"
    );
    drop(conn);
}

#[test]
fn a_database_locked_by_a_running_firefox_is_read_from_a_copy() {
    let profile = tempfile::tempdir().unwrap();
    let conn = sample_places(profile.path());
    // Firefox holds places.sqlite in WAL mode under an exclusive lock while it runs.
    conn.execute_batch(
        "PRAGMA locking_mode = EXCLUSIVE; PRAGMA journal_mode = WAL;
         UPDATE moz_places SET title = 'News (locked)' WHERE id = 1;",
    )
    .unwrap();
    let files = [
        profile.path().join("places.sqlite"),
        profile.path().join("places.sqlite-wal"),
    ];
    let contents = || {
        files
            .iter()
            .map(|file| std::fs::read(file).unwrap())
            .collect::<Vec<_>>()
    };
    let before = contents();
    let other_reader = Connection::open(profile.path().join("places.sqlite")).unwrap();
    other_reader.busy_timeout(Duration::ZERO).unwrap();
    assert!(
        other_reader
            .query_row("SELECT count(*) FROM moz_places", [], |row| row
                .get::<_, i64>(0))
            .is_err(),
        "the database really is locked against other readers"
    );

    let visits = read_firefox_history(profile.path(), 1).unwrap();

    assert_eq!(
        visits,
        vec![visit(
            "https://news.example/",
            Some("News (locked)"),
            1_700_000_500_000
        )]
    );
    assert_eq!(read_firefox_bookmarks(profile.path()).unwrap().len(), 3);
    assert_eq!(contents(), before, "the original files are not changed");
    // Firefox's own connection is unaffected.
    conn.execute(
        "UPDATE moz_places SET title = ?1 WHERE id = 1",
        params!["Again"],
    )
    .unwrap();
}

// Robustness: every failure is an error, never a panic and never an empty success.

#[test]
fn a_profile_without_places_reports_not_found() {
    let profile = tempfile::tempdir().unwrap();

    assert!(matches!(
        read_firefox_bookmarks(profile.path()),
        Err(ImportError::NotFound(_))
    ));
    assert!(matches!(
        read_firefox_history(profile.path(), 10),
        Err(ImportError::NotFound(_))
    ));
}

#[test]
fn a_damaged_database_is_an_error() {
    let profile = tempfile::tempdir().unwrap();
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.resize(8192, 0xA5);
    std::fs::write(profile.path().join("places.sqlite"), bytes).unwrap();

    assert!(matches!(
        read_firefox_bookmarks(profile.path()),
        Err(ImportError::Database { .. })
    ));
    assert!(matches!(
        read_firefox_history(profile.path(), 10),
        Err(ImportError::Database { .. })
    ));
}

#[test]
fn a_database_with_another_schema_is_malformed() {
    let profile = tempfile::tempdir().unwrap();
    Connection::open(profile.path().join("places.sqlite"))
        .unwrap()
        .execute_batch("CREATE TABLE moz_places (id INTEGER PRIMARY KEY, address TEXT);")
        .unwrap();

    assert!(matches!(
        read_firefox_bookmarks(profile.path()),
        Err(ImportError::Malformed(_))
    ));
    assert!(matches!(
        read_firefox_history(profile.path(), 10),
        Err(ImportError::Malformed(_))
    ));
}

#[test]
fn bookmarks_without_the_fixed_roots_are_malformed() {
    let profile = tempfile::tempdir().unwrap();
    drop(places(profile.path()));

    assert!(matches!(
        read_firefox_bookmarks(profile.path()),
        Err(ImportError::Malformed(_))
    ));
}

#[test]
fn a_loop_or_a_very_deep_tree_in_a_damaged_file_ends() {
    let profile = tempfile::tempdir().unwrap();
    let mut conn = places(profile.path());
    conn.execute_batch(ROOTS).unwrap();
    conn.execute_batch(
        "INSERT INTO moz_places (id, url, title) VALUES (1, 'https://deep.example/', 'Deep');
         -- A folder in the toolbar that is also the toolbar's parent.
         INSERT INTO moz_bookmarks (id, type, parent, position, title, guid)
             VALUES (60, 2, 3, 0, 'Loop', 'loop________');
         UPDATE moz_bookmarks SET parent = 60 WHERE id = 3;",
    )
    .unwrap();
    // 5 000 folders, each inside the one before, under the menu, and a bookmark at the bottom.
    let tx = conn.transaction().unwrap();
    for level in 0..5_000_i64 {
        let parent = if level == 0 { 2 } else { 1_000 + level - 1 };
        tx.execute(
            "INSERT INTO moz_bookmarks (id, type, parent, position, title, guid)
             VALUES (?1, 2, ?2, 0, ?3, ?4)",
            params![
                1_000 + level,
                parent,
                format!("Level {level}"),
                format!("deep{level:08}")
            ],
        )
        .unwrap();
    }
    tx.execute(
        "INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, guid)
         VALUES (9999999, 1, 1, ?1, 0, 'Deep', 'deepbookmark')",
        [1_000 + 4_999],
    )
    .unwrap();
    tx.commit().unwrap();
    drop(conn);

    let nodes = read_firefox_bookmarks(profile.path()).expect("a damaged tree is still read");

    assert_eq!(
        nodes[0],
        folder(
            "Bookmarks Toolbar",
            true,
            ROOT_ADDED,
            vec![folder("Loop", false, None, vec![])]
        )
    );
    let mut levels = 0;
    let mut node = &nodes[1];
    while let ImportedNode::Folder { children, .. } = node {
        levels += 1;
        node = children.last().expect("each level holds the next");
    }
    assert!(levels < 100, "nesting is capped, got {levels} levels");
    assert_eq!(node, &bookmark("Deep", "https://deep.example/", None));
}

// Profile discovery through profiles.ini.

fn profile(browser: &str, name: &str, path: PathBuf) -> BrowserProfile {
    BrowserProfile {
        browser: browser.into(),
        name: name.into(),
        path: path.canonicalize().unwrap(),
    }
}

fn canonical(found: Vec<BrowserProfile>) -> Vec<BrowserProfile> {
    found
        .into_iter()
        .map(|p| profile(&p.browser, &p.name, p.path))
        .collect()
}

#[test]
fn profiles_ini_lists_profiles_with_the_installed_default_first() {
    let support = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let tree = support.path().join("Firefox");
    drop(places(&tree.join("Profiles/abcd1234.default")));
    drop(places(&tree.join("Profiles/efgh5678.default-release")));
    drop(places(&elsewhere.path().join("custom-profile")));
    std::fs::create_dir_all(tree.join("Profiles/empty.never")).unwrap();
    std::fs::write(
        tree.join("profiles.ini"),
        format!(
            "[General]\nStartWithLastProfile=1\nVersion=2\n\n\
             [Profile1]\nName=default\nIsRelative=1\nPath=Profiles/abcd1234.default\n\n\
             [Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/efgh5678.default-release\n\n\
             [Profile2]\nName=Elsewhere\nIsRelative=0\nPath={}\n\n\
             [Profile3]\nName=Never used\nIsRelative=1\nPath=Profiles/empty.never\n\n\
             [Install308046B0AF4A39CB]\nDefault=Profiles/efgh5678.default-release\nLocked=1\n",
            elsewhere.path().join("custom-profile").display()
        ),
    )
    .unwrap();

    let found = find_firefox_profiles(&[support.path().to_owned()]);

    assert_eq!(
        canonical(found),
        vec![
            profile(
                "Firefox",
                "default-release",
                tree.join("Profiles/efgh5678.default-release")
            ),
            profile("Firefox", "default", tree.join("Profiles/abcd1234.default")),
            profile(
                "Firefox",
                "Elsewhere",
                elsewhere.path().join("custom-profile")
            ),
        ]
    );
}

#[test]
fn profiles_made_in_a_profile_group_are_listed_but_may_not_leave_the_tree() {
    let support = tempfile::tempdir().unwrap();
    let tree = support.path().join("Firefox");
    drop(places(&tree.join("Profiles/aaa.Work")));
    drop(places(&tree.join("Profiles/bbb.Home")));
    std::fs::write(
        tree.join("profiles.ini"),
        "[Profile0]\nName=Work\nIsRelative=1\nPath=Profiles/aaa.Work\nStoreID=6f2a0c1b\n",
    )
    .unwrap();
    std::fs::create_dir_all(tree.join("Profile Groups")).unwrap();
    let store = Connection::open(tree.join("Profile Groups/6f2a0c1b.sqlite")).unwrap();
    store
        .execute_batch(
            "CREATE TABLE Profiles(id INTEGER NOT NULL, path TEXT NOT NULL UNIQUE,
                 name TEXT NOT NULL, avatar TEXT NOT NULL, themeId TEXT NOT NULL,
                 themeFg TEXT NOT NULL, themeBg TEXT NOT NULL, PRIMARY KEY(id));
             INSERT INTO Profiles(path, name, avatar, themeId, themeFg, themeBg) VALUES
                 ('Profiles/aaa.Work', 'Work', 'book', 'default', 'x', 'y'),
                 ('Profiles/bbb.Home', 'Home', 'book', 'default', 'x', 'y'),
                 ('../../outside', 'Up', 'book', 'default', 'x', 'y'),
                 ('/etc', 'Absolute', 'book', 'default', 'x', 'y'),
                 ('', 'Nowhere', 'book', 'default', 'x', 'y');",
        )
        .unwrap();
    drop(store);
    drop(places(&support.path().join("outside")));

    let found = find_firefox_profiles(&[support.path().to_owned()]);

    assert_eq!(
        canonical(found),
        vec![
            profile("Firefox", "Work", tree.join("Profiles/aaa.Work")),
            profile("Firefox", "Home", tree.join("Profiles/bbb.Home")),
        ]
    );
}

#[test]
fn trees_of_firefox_and_its_forks_are_found_under_a_home_directory() {
    let home = tempfile::tempdir().unwrap();
    for (tree, profile_dir) in [
        (".mozilla/firefox", "x1.default-release"),
        (".librewolf", "y2.default-default"),
    ] {
        let tree = home.path().join(tree);
        drop(places(&tree.join(profile_dir)));
        std::fs::write(
            tree.join("profiles.ini"),
            format!("[Profile0]\nName=main\nIsRelative=1\nPath={profile_dir}\nDefault=1\n"),
        )
        .unwrap();
    }
    // A profiles.ini with no profile holding places.sqlite offers nothing.
    std::fs::create_dir_all(home.path().join(".waterfox")).unwrap();
    std::fs::write(
        home.path().join(".waterfox/profiles.ini"),
        "[Profile0]\nName=gone\nIsRelative=1\nPath=gone\n",
    )
    .unwrap();

    let found = find_firefox_profiles(&[home.path().to_owned(), home.path().to_owned()]);

    assert_eq!(
        canonical(found),
        vec![
            profile(
                "Firefox",
                "main",
                home.path().join(".mozilla/firefox/x1.default-release")
            ),
            profile(
                "LibreWolf",
                "main",
                home.path().join(".librewolf/y2.default-default")
            ),
        ]
    );
}

#[test]
fn a_damaged_profile_group_leaves_profiles_ini_alone() {
    let support = tempfile::tempdir().unwrap();
    let tree = support.path().join("Firefox");
    drop(places(&tree.join("a.default")));
    std::fs::write(
        tree.join("profiles.ini"),
        "[Profile0]\nName=main\nIsRelative=1\nPath=a.default\n",
    )
    .unwrap();
    std::fs::create_dir_all(tree.join("Profile Groups")).unwrap();
    std::fs::write(
        tree.join("Profile Groups/6f2a0c1b.sqlite"),
        b"not a database",
    )
    .unwrap();

    assert_eq!(find_firefox_profiles(&[support.path().to_owned()]).len(), 1);
    assert_eq!(find_firefox_profiles(&[]), vec![]);
}

#[test]
fn a_root_left_empty_by_the_address_rule_is_left_out() {
    let profile = tempfile::tempdir().unwrap();
    let conn = places(profile.path());
    conn.execute_batch(ROOTS).unwrap();
    conn.execute_batch(
        "INSERT INTO moz_places (id, url, title) VALUES
             (1, 'https://kept.example/', 'Kept'),
             (2, 'place:sort=8&maxResults=10', 'Most Visited'),
             (3, 'javascript:alert(1)', 'Script');
         INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, guid) VALUES
             (10, 1, 1, 3, 0, 'Kept', 'kept________'),
             (11, 1, 2, 2, 0, 'Most Visited', 'mostvisited_'),
             (12, 1, 3, 5, 0, 'Script', 'script______');",
    )
    .unwrap();
    drop(conn);

    assert_eq!(
        read_firefox_bookmarks(profile.path()).unwrap(),
        vec![folder(
            "Bookmarks Toolbar",
            true,
            ROOT_ADDED,
            vec![bookmark("Kept", "https://kept.example/", None)]
        )]
    );
}

#[test]
fn bookmark_and_history_addresses_come_out_normalised() {
    let profile = tempfile::tempdir().unwrap();
    let conn = places(profile.path());
    conn.execute_batch(ROOTS).unwrap();
    conn.execute_batch(
        "INSERT INTO moz_places (id, url, title) VALUES (1, ' HTTPS://Upper.Example/a b', 'Upper');
         INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, guid)
             VALUES (10, 1, 1, 3, 0, 'Upper', 'upper_______');
         INSERT INTO moz_historyvisits (place_id, visit_date) VALUES (1, 1700000000000000);",
    )
    .unwrap();
    drop(conn);

    assert_eq!(
        read_firefox_bookmarks(profile.path()).unwrap(),
        vec![folder(
            "Bookmarks Toolbar",
            true,
            ROOT_ADDED,
            vec![bookmark("Upper", "https://upper.example/a%20b", None)]
        )]
    );
    assert_eq!(
        read_firefox_history(profile.path(), 10).unwrap(),
        vec![visit(
            "https://upper.example/a%20b",
            Some("Upper"),
            1_700_000_000_000
        )]
    );
}

#[test]
fn visits_without_a_time_are_skipped() {
    let profile = tempfile::tempdir().unwrap();
    let conn = places(profile.path());
    conn.execute_batch(
        "INSERT INTO moz_places (id, url, title) VALUES (1, 'https://a.example/', 'A');
         INSERT INTO moz_historyvisits (place_id, visit_date) VALUES
             (1, 1700000000000000), (1, 0), (1, -5), (1, NULL);",
    )
    .unwrap();
    drop(conn);

    assert_eq!(
        read_firefox_history(profile.path(), 10).unwrap(),
        vec![visit("https://a.example/", Some("A"), 1_700_000_000_000)]
    );
}

#[test]
fn an_unreadable_history_row_is_skipped_not_fatal() {
    let profile = tempfile::tempdir().unwrap();
    let conn = places(profile.path());
    conn.execute_batch(
        "INSERT INTO moz_places (id, url, title) VALUES
             (1, 'https://a.example/', 'A'),
             (2, X'68747470733a2f2f00ff', 'Blob address'),
             (3, 'https://c.example/', CAST(X'ff' AS TEXT)),
             (4, 'https://d.example/', 'D');
         INSERT INTO moz_historyvisits (place_id, visit_date) VALUES
             (1, 1700000001000000), (2, 1700000002000000), (3, 1700000003000000),
             (4, 1700000004000000), (4, X'00');",
    )
    .unwrap();
    drop(conn);

    assert_eq!(
        read_firefox_history(profile.path(), 10).unwrap(),
        vec![
            visit("https://d.example/", Some("D"), 1_700_000_004_000),
            visit("https://a.example/", Some("A"), 1_700_000_001_000),
        ]
    );
}

#[test]
fn undecodable_titles_are_kept_lossily_and_unreadable_addresses_skipped() {
    let profile = tempfile::tempdir().unwrap();
    let conn = places(profile.path());
    conn.execute_batch(ROOTS).unwrap();
    conn.execute_batch(
        "INSERT INTO moz_places (id, url, title) VALUES
             (1, 'https://inside.example/', 'Inside'),
             (2, 'https://blob-title.example/', NULL),
             (3, X'68747470733a2f2f78ff', 'Unreadable address'),
             (4, 'https://kept.example/', NULL);
         INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, guid) VALUES
             (10, 2, NULL, 3, 0, CAST(X'466f6c646572ff' AS TEXT), 'folder______'),
             (11, 1, 1, 10, 0, 'Inside', 'inside______'),
             (12, 1, 2, 3, 1, X'426c6f62ff', 'blobtitle___'),
             (13, 1, 3, 3, 2, 'Unreadable address', 'unreadable__'),
             (14, 1, 4, 3, 3, 'Kept', 'kept________');",
    )
    .unwrap();
    drop(conn);

    assert_eq!(
        read_firefox_bookmarks(profile.path()).unwrap(),
        vec![folder(
            "Bookmarks Toolbar",
            true,
            ROOT_ADDED,
            vec![
                folder(
                    "Folder\u{FFFD}",
                    false,
                    None,
                    vec![bookmark("Inside", "https://inside.example/", None)]
                ),
                bookmark("Blob\u{FFFD}", "https://blob-title.example/", None),
                bookmark("Kept", "https://kept.example/", None),
            ]
        )]
    );
}
