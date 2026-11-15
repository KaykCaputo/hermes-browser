//! Chromium-family profiles, built in temporary directories: the `Bookmarks` JSON is written by
//! hand and the `History` database is created here with the columns Chromium's
//! `url_database.cc` and `visit_database.cc` define.

use std::path::{Path, PathBuf};

use mercury_import::{
    BrowserProfile, ImportError, ImportedNode, ImportedVisit, MAX_IMPORTED_VISITS,
    find_chromium_profiles, read_chromium_bookmarks, read_chromium_history,
};
use rusqlite::{Connection, params};

const BOOKMARKS: &str = include_str!("fixtures/chromium_Bookmarks.json");

/// Microseconds since 1601-01-01, Chromium's time, for a Unix time in milliseconds.
fn chrome_time(unix_ms: i64) -> i64 {
    (unix_ms + 11_644_473_600_000) * 1000
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

fn history_db(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE urls(id INTEGER PRIMARY KEY AUTOINCREMENT, url LONGVARCHAR,
             title LONGVARCHAR, visit_count INTEGER DEFAULT 0 NOT NULL,
             typed_count INTEGER DEFAULT 0 NOT NULL, last_visit_time INTEGER NOT NULL,
             hidden INTEGER DEFAULT 0 NOT NULL);
         CREATE TABLE visits(id INTEGER PRIMARY KEY AUTOINCREMENT, url INTEGER NOT NULL,
             visit_time INTEGER NOT NULL, from_visit INTEGER, transition INTEGER DEFAULT 0 NOT NULL,
             segment_id INTEGER, visit_duration INTEGER DEFAULT 0 NOT NULL);",
    )
    .unwrap();
    conn
}

fn add_url(conn: &Connection, id: i64, url: &str, title: Option<&str>) {
    conn.execute(
        "INSERT INTO urls(id, url, title, last_visit_time) VALUES (?1, ?2, ?3, 0)",
        params![id, url, title],
    )
    .unwrap();
}

fn add_visit(conn: &Connection, url_id: i64, unix_ms: i64) {
    conn.execute(
        "INSERT INTO visits(url, visit_time) VALUES (?1, ?2)",
        params![url_id, chrome_time(unix_ms)],
    )
    .unwrap();
}

#[test]
fn the_bar_becomes_a_toolbar_folder_and_dates_become_unix_milliseconds() {
    let profile = tempfile::tempdir().unwrap();
    std::fs::write(profile.path().join("Bookmarks"), BOOKMARKS).unwrap();

    let nodes = read_chromium_bookmarks(profile.path()).expect("the fixture should be read");

    assert_eq!(
        nodes,
        vec![
            folder(
                "Bookmarks bar",
                true,
                Some(1_699_999_900_000),
                vec![
                    bookmark(
                        "News & Views",
                        "https://news.example/",
                        Some(1_700_000_000_123)
                    ),
                    folder(
                        "Work",
                        false,
                        Some(1_700_000_050_000),
                        vec![bookmark(
                            "Mail",
                            "https://mail.example/inbox?a=1&b=2",
                            Some(1_700_000_100_000)
                        )]
                    ),
                ]
            ),
            folder(
                "Other bookmarks",
                false,
                Some(1_699_999_900_000),
                vec![bookmark(
                    "Über uns — 日本語",
                    "https://later.example/%C3%BCber",
                    None
                )]
            ),
        ],
        "the empty mobile root is left out, javascript: and unknown node kinds are dropped"
    );
}

/// Shaped like a Chrome file with one bookmark in each root.
fn bookmarks_file(bar: &str, other: &str, synced: &str) -> String {
    let root = |name: &str, child: &str| {
        let children = if child.is_empty() {
            String::new()
        } else {
            format!(
                r#"{{"name":"{child}","type":"url","url":"https://{child}.example/","date_added":"0"}}"#
            )
        };
        format!(r#"{{"children":[{children}],"date_added":"0","name":"{name}","type":"folder"}}"#)
    };
    format!(
        r#"{{"checksum":"","roots":{{"bookmark_bar":{},"other":{},"synced":{}}},"version":1}}"#,
        root("Lesezeichenleiste", bar),
        root("Weitere Lesezeichen", other),
        root("Mobile Lesezeichen", synced),
    )
}

#[test]
fn a_signed_in_profile_reads_its_account_bookmarks_root_by_root() {
    let profile = tempfile::tempdir().unwrap();
    std::fs::write(
        profile.path().join("Bookmarks"),
        bookmarks_file("local-bar", "local-other", ""),
    )
    .unwrap();
    std::fs::write(
        profile.path().join("AccountBookmarks"),
        bookmarks_file("account-bar", "", "phone"),
    )
    .unwrap();

    let nodes = read_chromium_bookmarks(profile.path()).unwrap();

    assert_eq!(
        nodes,
        vec![
            folder(
                "Lesezeichenleiste",
                true,
                None,
                vec![
                    bookmark("local-bar", "https://local-bar.example/", None),
                    bookmark("account-bar", "https://account-bar.example/", None),
                ]
            ),
            folder(
                "Weitere Lesezeichen",
                false,
                None,
                vec![bookmark(
                    "local-other",
                    "https://local-other.example/",
                    None
                )]
            ),
            folder(
                "Mobile Lesezeichen",
                false,
                None,
                vec![bookmark("phone", "https://phone.example/", None)]
            ),
        ]
    );

    std::fs::remove_file(profile.path().join("Bookmarks")).unwrap();
    assert_eq!(
        read_chromium_bookmarks(profile.path()).unwrap().len(),
        2,
        "the account file alone is enough"
    );
}

#[test]
fn history_is_newest_first_in_unix_milliseconds_and_capped_by_the_limit() {
    let profile = tempfile::tempdir().unwrap();
    let conn = history_db(&profile.path().join("History"));
    add_url(&conn, 1, "https://a.example/", Some("A"));
    add_url(&conn, 2, "https://b.example/", None);
    add_url(&conn, 3, "https://c.example/", Some(""));
    add_url(&conn, 4, "chrome://settings/", Some("Settings"));
    add_url(&conn, 5, "javascript:void(0)", Some("Script"));
    add_visit(&conn, 1, 1_700_000_000_123);
    add_visit(&conn, 2, 1_700_000_300_000);
    add_visit(&conn, 4, 1_700_000_400_000);
    add_visit(&conn, 1, 1_700_000_500_999);
    add_visit(&conn, 3, 1_700_000_100_000);
    add_visit(&conn, 5, 1_700_000_600_000);
    drop(conn);

    let all = read_chromium_history(profile.path(), 100).unwrap();
    assert_eq!(
        all,
        vec![
            visit("https://a.example/", Some("A"), 1_700_000_500_999),
            visit("https://b.example/", None, 1_700_000_300_000),
            visit("https://c.example/", None, 1_700_000_100_000),
            visit("https://a.example/", Some("A"), 1_700_000_000_123),
        ]
    );

    let newest = read_chromium_history(profile.path(), 2).unwrap();
    assert_eq!(newest, all[..2].to_vec());
    assert_eq!(read_chromium_history(profile.path(), 0).unwrap(), vec![]);
}

#[test]
fn history_never_returns_more_than_the_import_cap() {
    let profile = tempfile::tempdir().unwrap();
    let mut conn = history_db(&profile.path().join("History"));
    add_url(&conn, 1, "https://busy.example/", Some("Busy"));
    let tx = conn.transaction().unwrap();
    {
        let mut insert = tx
            .prepare("INSERT INTO visits(url, visit_time) VALUES (1, ?1)")
            .unwrap();
        for i in 0..(MAX_IMPORTED_VISITS as i64 + 10) {
            insert
                .execute([chrome_time(1_600_000_000_000 + i)])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    drop(conn);

    let visits = read_chromium_history(profile.path(), usize::MAX).unwrap();

    assert_eq!(visits.len(), MAX_IMPORTED_VISITS);
    assert_eq!(
        visits[0].visited_at_ms,
        1_600_000_000_000 + MAX_IMPORTED_VISITS as i64 + 9
    );
}

#[test]
fn history_held_by_a_running_browser_is_read_from_a_copy_and_left_untouched() {
    let profile = tempfile::tempdir().unwrap();
    let path = profile.path().join("History");
    let conn = history_db(&path);
    add_url(&conn, 1, "https://a.example/", Some("A"));
    add_visit(&conn, 1, 1_700_000_000_000);
    // Chromium keeps its History database under an exclusive lock while it runs.
    conn.execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE; COMMIT;")
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let other_reader = Connection::open(&path).unwrap();
    other_reader
        .busy_timeout(std::time::Duration::ZERO)
        .unwrap();
    assert!(
        other_reader
            .query_row("SELECT count(*) FROM urls", [], |row| row.get::<_, i64>(0))
            .is_err(),
        "the database really is locked against other readers"
    );

    let visits = read_chromium_history(profile.path(), 10).unwrap();

    assert_eq!(
        visits,
        vec![visit("https://a.example/", Some("A"), 1_700_000_000_000)]
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the original is not changed"
    );
    // The browser's own connection still works.
    add_visit(&conn, 1, 1_700_000_000_500);
}

// Robustness: every failure is an error, never a panic and never an empty success.

#[test]
fn a_profile_without_bookmarks_or_history_reports_not_found() {
    let profile = tempfile::tempdir().unwrap();

    assert!(matches!(
        read_chromium_bookmarks(profile.path()),
        Err(ImportError::NotFound(_))
    ));
    assert!(matches!(
        read_chromium_history(profile.path(), 10),
        Err(ImportError::NotFound(_))
    ));
}

#[test]
fn truncated_or_unexpected_bookmark_json_is_malformed() {
    let profile = tempfile::tempdir().unwrap();
    let path = profile.path().join("Bookmarks");
    let cut = &BOOKMARKS[..BOOKMARKS.len() / 2];
    for text in [
        cut,
        "",
        "[]",
        "{}",
        r#"{"roots": []}"#,
        r#"{"roots": {"bookmark_bar": 3}}"#,
    ] {
        std::fs::write(&path, text).unwrap();
        assert!(
            matches!(
                read_chromium_bookmarks(profile.path()),
                Err(ImportError::Malformed(_))
            ),
            "{text:?}"
        );
    }

    // A good file does not hide a damaged account file.
    std::fs::write(&path, BOOKMARKS).unwrap();
    std::fs::write(profile.path().join("AccountBookmarks"), cut).unwrap();
    assert!(matches!(
        read_chromium_bookmarks(profile.path()),
        Err(ImportError::Malformed(_))
    ));
}

#[test]
fn an_oversized_bookmarks_file_is_refused() {
    let profile = tempfile::tempdir().unwrap();
    let file = std::fs::File::create(profile.path().join("Bookmarks")).unwrap();
    file.set_len(64 * 1024 * 1024).unwrap();

    assert!(matches!(
        read_chromium_bookmarks(profile.path()),
        Err(ImportError::TooLarge { .. })
    ));
}

#[test]
fn a_damaged_history_database_is_an_error() {
    let profile = tempfile::tempdir().unwrap();
    std::fs::write(
        profile.path().join("History"),
        b"SQLite format 3\0 but not really",
    )
    .unwrap();

    assert!(matches!(
        read_chromium_history(profile.path(), 10),
        Err(ImportError::Database { .. })
    ));
}

#[test]
fn a_history_database_with_another_schema_is_malformed() {
    let profile = tempfile::tempdir().unwrap();
    let conn = Connection::open(profile.path().join("History")).unwrap();
    conn.execute_batch(
        "CREATE TABLE urls(id INTEGER PRIMARY KEY, address TEXT, title TEXT);
         CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER);",
    )
    .unwrap();
    drop(conn);

    assert!(matches!(
        read_chromium_history(profile.path(), 10),
        Err(ImportError::Malformed(_))
    ));

    std::fs::remove_file(profile.path().join("History")).unwrap();
    Connection::open(profile.path().join("History"))
        .unwrap()
        .execute_batch("CREATE TABLE unrelated(x)")
        .unwrap();
    assert!(matches!(
        read_chromium_history(profile.path(), 10),
        Err(ImportError::Malformed(_))
    ));
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_is_an_error() {
    use std::os::unix::fs::PermissionsExt;

    let profile = tempfile::tempdir().unwrap();
    let path = profile.path().join("Bookmarks");
    std::fs::write(&path, BOOKMARKS).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A privileged runner may read it anyway; the check only means something when it cannot.
    if std::fs::File::open(&path).is_err() {
        assert!(matches!(
            read_chromium_bookmarks(profile.path()),
            Err(ImportError::Io { .. })
        ));
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
}

// Profile discovery, against a directory laid out like the platform's application-support or
// config directory.

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"").unwrap();
}

fn profile(browser: &str, name: &str, path: PathBuf) -> BrowserProfile {
    BrowserProfile {
        browser: browser.into(),
        name: name.into(),
        path,
    }
}

#[test]
fn default_and_numbered_profiles_are_found_under_each_vendor() {
    let support = tempfile::tempdir().unwrap();
    let base = support.path();
    let chrome = base.join("Google/Chrome");
    touch(&chrome.join("Default/Bookmarks"));
    touch(&chrome.join("Profile 10/History"));
    touch(&chrome.join("Profile 2/AccountBookmarks"));
    // Not profiles a person browses with, or holding nothing to import.
    touch(&chrome.join("System Profile/History"));
    touch(&chrome.join("Guest Profile/History"));
    touch(&chrome.join("Profile x/History"));
    std::fs::create_dir_all(chrome.join("Profile 3")).unwrap();
    std::fs::write(
        chrome.join("Local State"),
        r#"{"profile":{"info_cache":{"Default":{"name":"Personal"},"Profile 2":{"name":"Work"},"Profile 10":{"name":"  "}}}}"#,
    )
    .unwrap();
    touch(&base.join("BraveSoftware/Brave-Browser/Default/History"));
    touch(&base.join("Arc/User Data/Default/History"));
    touch(&base.join("Microsoft Edge/Default/Bookmarks"));

    let other = tempfile::tempdir().unwrap();
    touch(&other.path().join("vivaldi/Default/History"));
    let missing = support.path().join("does-not-exist");

    let found = find_chromium_profiles(&[base.to_owned(), missing, other.path().to_owned()]);
    let canonical = |path: PathBuf| path.canonicalize().unwrap();

    assert_eq!(
        found
            .into_iter()
            .map(|p| profile(&p.browser, &p.name, canonical(p.path)))
            .collect::<Vec<_>>(),
        vec![
            profile("Chrome", "Personal", canonical(chrome.join("Default"))),
            profile("Chrome", "Work", canonical(chrome.join("Profile 2"))),
            profile("Chrome", "Profile 10", canonical(chrome.join("Profile 10"))),
            profile(
                "Brave",
                "Default",
                canonical(base.join("BraveSoftware/Brave-Browser/Default"))
            ),
            profile(
                "Edge",
                "Default",
                canonical(base.join("Microsoft Edge/Default"))
            ),
            profile(
                "Vivaldi",
                "Default",
                canonical(other.path().join("vivaldi/Default"))
            ),
            profile(
                "Arc",
                "Default",
                canonical(base.join("Arc/User Data/Default"))
            ),
        ]
    );
}

#[test]
fn a_profile_is_listed_once_even_where_names_differ_only_in_case() {
    let support = tempfile::tempdir().unwrap();
    touch(&support.path().join("Chromium/Default/History"));
    if support.path().join("chromium").exists() {
        // A case-insensitive file system, as macOS uses by default: `Chromium` and `chromium`
        // are one directory.
        assert_eq!(
            find_chromium_profiles(&[support.path().to_owned()]).len(),
            1
        );
    }
    assert_eq!(
        find_chromium_profiles(&[support.path().to_owned(), support.path().to_owned()]).len(),
        1,
        "the same base twice"
    );
}

#[test]
fn nothing_is_found_where_no_browser_keeps_data() {
    let empty = tempfile::tempdir().unwrap();

    assert_eq!(find_chromium_profiles(&[empty.path().to_owned()]), vec![]);
    assert_eq!(find_chromium_profiles(&[]), vec![]);
}

#[test]
fn a_root_left_empty_by_the_address_rule_is_left_out() {
    let profile = tempfile::tempdir().unwrap();
    std::fs::write(
        profile.path().join("Bookmarks"),
        r#"{"roots":{
            "bookmark_bar":{"type":"folder","name":"Bar","children":[
                {"type":"url","name":"Kept","url":"https://kept.example/"}]},
            "other":{"type":"folder","name":"Other","children":[
                {"type":"url","name":"Script","url":"javascript:alert(1)"},
                {"type":"url","name":"Settings","url":"chrome://settings/"}]},
            "synced":{"type":"folder","name":"Mobile","children":[
                {"type":"url","name":"Data","url":"data:text/plain,hi"}]}
        },"version":1}"#,
    )
    .unwrap();

    assert_eq!(
        read_chromium_bookmarks(profile.path()).unwrap(),
        vec![folder(
            "Bar",
            true,
            None,
            vec![bookmark("Kept", "https://kept.example/", None)]
        )]
    );
}

#[test]
fn bookmark_and_history_addresses_come_out_normalised() {
    let profile = tempfile::tempdir().unwrap();
    std::fs::write(
        profile.path().join("Bookmarks"),
        r#"{"roots":{"bookmark_bar":{"type":"folder","name":"Bar","children":[
            {"type":"url","name":"Odd","url":"  HTTP://Odd.EXAMPLE/a b  "}]}},"version":1}"#,
    )
    .unwrap();
    let conn = history_db(&profile.path().join("History"));
    add_url(&conn, 1, " HTTPS://Upper.Example/Path?q=a b", Some("Upper"));
    add_visit(&conn, 1, 1_700_000_000_000);
    drop(conn);

    assert_eq!(
        read_chromium_bookmarks(profile.path()).unwrap(),
        vec![folder(
            "Bar",
            true,
            None,
            vec![bookmark("Odd", "http://odd.example/a%20b", None)]
        )]
    );
    assert_eq!(
        read_chromium_history(profile.path(), 10).unwrap(),
        vec![visit(
            "https://upper.example/Path?q=a%20b",
            Some("Upper"),
            1_700_000_000_000
        )]
    );
}

#[test]
fn visits_without_a_time_are_skipped() {
    let profile = tempfile::tempdir().unwrap();
    let conn = history_db(&profile.path().join("History"));
    add_url(&conn, 1, "https://a.example/", Some("A"));
    add_visit(&conn, 1, 1_700_000_000_000);
    // Zero, negative, and positive but before 1970 (a second after Chromium's 1601 epoch).
    for raw in [0_i64, -5, 1_000_000] {
        conn.execute("INSERT INTO visits(url, visit_time) VALUES (1, ?1)", [raw])
            .unwrap();
    }
    drop(conn);

    assert_eq!(
        read_chromium_history(profile.path(), 10).unwrap(),
        vec![visit("https://a.example/", Some("A"), 1_700_000_000_000)]
    );
}

#[test]
fn an_unreadable_history_row_is_skipped_not_fatal() {
    let profile = tempfile::tempdir().unwrap();
    let conn = history_db(&profile.path().join("History"));
    add_url(&conn, 1, "https://a.example/", Some("A"));
    conn.execute_batch(
        "INSERT INTO urls(id, url, title, last_visit_time) VALUES
             (2, X'68747470733a2f2f00ff', 'Blob address', 0),
             (3, 'https://c.example/', CAST(X'ff' AS TEXT), 0),
             (4, 'https://d.example/', 'D', 0);",
    )
    .unwrap();
    for (url, ms) in [(1, 1_000_i64), (2, 2_000), (3, 3_000), (4, 4_000)] {
        add_visit(&conn, url, 1_700_000_000_000 + ms);
    }
    conn.execute(
        "INSERT INTO visits(url, visit_time) VALUES (4, 'not a time')",
        [],
    )
    .unwrap();
    drop(conn);

    assert_eq!(
        read_chromium_history(profile.path(), 10).unwrap(),
        vec![
            visit("https://d.example/", Some("D"), 1_700_000_004_000),
            visit("https://a.example/", Some("A"), 1_700_000_001_000),
        ]
    );
}
