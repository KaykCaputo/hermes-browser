//! Servo's HTTP disk cache: one SQLite database (`servo-net` 0.8.0 `disk_cache.rs`), opened in
//! write-ahead-log mode, so it is the database file and its `-wal` and `-shm` companions.

use std::path::{Path, PathBuf};

/// The database's file name inside the cache directory.
const DATABASE_NAME: &str = "cache.sqlite3";
/// SQLite's companion files of a database, by suffix: the write-ahead log, its shared-memory
/// index, and the rollback journal (if the database was ever in that mode).
const COMPANION_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];

/// The value of `network_http_disk_cache`: the database inside `cache_dir`.
pub(crate) fn http_disk_cache_database(cache_dir: &Path) -> PathBuf {
    cache_dir.join(DATABASE_NAME)
}

/// Removes the HTTP disk cache in `cache_dir`: the database and its companion files. This is
/// what "clear cached files" must do on disk. `servo.network_manager().clear_cache()` empties
/// the database while Servo runs, but SQLite keeps the deleted pages' bytes in the file and its
/// write-ahead log until they are reused, so the files themselves go once no Servo has them
/// open: before the engine starts, or after it has shut down. Symbolic links are removed, never
/// followed. Failures are logged.
pub fn remove_http_disk_cache(cache_dir: &Path) {
    let database = http_disk_cache_database(cache_dir);
    let mut paths = vec![database.clone()];
    paths.extend(COMPANION_SUFFIXES.iter().map(|suffix| {
        let mut name = database.clone().into_os_string();
        name.push(suffix);
        PathBuf::from(name)
    }));
    let mut removed = 0;
    for path in paths {
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => log::warn!("could not remove {}: {error}", path.display()),
        }
    }
    if removed > 0 {
        log::info!("removed the HTTP disk cache ({removed} files)");
    }
}

/// What `repair_http_disk_cache` found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CacheCheck {
    /// No cache yet, or one Servo can read.
    Usable,
    /// A cache Servo could not read, removed: the reason.
    Removed(String),
}

/// Checks the cache before Servo opens it, and removes one it could not use. Call it only
/// while no Servo is running on the directory.
///
/// Servo 0.8.0 reads every entry's key and size when it starts (`DiskCache::new`) and
/// unwraps both: a key that is not an absolute URL, a size that is not a non-negative integer,
/// or a row it cannot read panics the network thread, and every page load then fails, at
/// every start, until the file is gone. A file that is not a database, or is damaged, only
/// turns the disk cache off, for good. Either way the cache is only a cache: it is removed,
/// and Servo starts a new one.
pub(crate) fn repair_http_disk_cache(cache_dir: &Path) -> CacheCheck {
    let database = http_disk_cache_database(cache_dir);
    if !database.exists() {
        return CacheCheck::Usable;
    }
    match check_database(&database) {
        Ok(()) => CacheCheck::Usable,
        Err(reason) => {
            remove_http_disk_cache(cache_dir);
            CacheCheck::Removed(reason)
        }
    }
}

/// Reads what Servo reads at start, as Servo reads it (`servo-net` 0.8.0 `disk_cache.rs`).
fn check_database(database: &Path) -> Result<(), String> {
    use rusqlite::{Connection, OpenFlags};

    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("it cannot be opened: {error}"))?;
    let has_table = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'disk_cache'",
            (),
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("it cannot be read: {error}"))?;
    if has_table == 0 {
        return Ok(());
    }
    let mut statement = connection
        .prepare("SELECT key, size FROM disk_cache")
        .map_err(|error| format!("its entries cannot be read: {error}"))?;
    let mut rows = statement
        .query(())
        .map_err(|error| format!("its entries cannot be read: {error}"))?;
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => return Ok(()),
            Err(error) => return Err(format!("an entry cannot be read: {error}")),
        };
        let key = row
            .get::<_, String>("key")
            .map_err(|error| format!("an entry's address cannot be read: {error}"))?;
        url::Url::parse(&key)
            .map_err(|error| format!("an entry's address is not a URL ({error})"))?;
        row.get::<_, usize>("size")
            .map_err(|error| format!("an entry's size is not a size ({error})"))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory, removed when dropped.
    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let nanoseconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let path = std::env::temp_dir().join(format!(
                "talaria-http-cache-test-{label}-{}-{nanoseconds}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("a test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(directory: &Path) -> Vec<String> {
        let mut names = std::fs::read_dir(directory)
            .expect("the directory")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    /// A cache database in `directory` as Servo creates it, holding `rows` (key, size).
    fn database_with(directory: &Path, rows: &[(&str, rusqlite::types::Value)]) {
        let connection =
            rusqlite::Connection::open(http_disk_cache_database(directory)).expect("a database");
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL; CREATE TABLE IF NOT EXISTS \"disk_cache\" ( \
                 \"key\" text NOT NULL PRIMARY KEY, \"data\" blob NOT NULL, \
                 \"size\" integer NOT NULL, \"insertion_timestamp\" integer NOT NULL )",
            )
            .expect("the table");
        for (key, size) in rows {
            connection
                .execute(
                    "INSERT INTO disk_cache VALUES (?1, x'00', ?2, 0)",
                    rusqlite::params![key, size],
                )
                .expect("a row");
        }
    }

    use rusqlite::types::Value;

    #[test]
    fn no_cache_yet_is_usable() {
        let directory = TestDirectory::new("check-none");

        assert_eq!(repair_http_disk_cache(directory.path()), CacheCheck::Usable);
        assert_eq!(names(directory.path()), Vec::<String>::new());
    }

    #[test]
    fn a_cache_servo_can_read_is_kept() {
        let directory = TestDirectory::new("check-good");
        database_with(
            directory.path(),
            &[
                ("https://a.test/style.css", Value::Integer(10)),
                ("http://127.0.0.1:8000/", Value::Integer(0)),
            ],
        );

        assert_eq!(repair_http_disk_cache(directory.path()), CacheCheck::Usable);
        assert!(names(directory.path()).contains(&"cache.sqlite3".to_owned()));
    }

    #[test]
    fn an_empty_database_is_kept() {
        let directory = TestDirectory::new("check-empty");
        rusqlite::Connection::open(http_disk_cache_database(directory.path()))
            .and_then(|connection| connection.execute_batch("PRAGMA user_version = 1"))
            .expect("an empty database");

        assert_eq!(repair_http_disk_cache(directory.path()), CacheCheck::Usable);
    }

    fn assert_removed(directory: &TestDirectory, expected: &str) {
        match repair_http_disk_cache(directory.path()) {
            CacheCheck::Removed(reason) => assert!(reason.contains(expected), "{reason}"),
            CacheCheck::Usable => panic!("the cache should have been removed"),
        }
        assert_eq!(names(directory.path()), Vec::<String>::new());
    }

    #[test]
    fn an_entry_whose_address_servo_cannot_parse_removes_the_cache() {
        let directory = TestDirectory::new("check-url");
        database_with(
            directory.path(),
            &[
                ("https://a.test/", Value::Integer(1)),
                ("not a url", Value::Integer(1)),
            ],
        );

        assert_removed(&directory, "not a URL");
    }

    #[test]
    fn an_entry_whose_size_servo_cannot_read_removes_the_cache() {
        for (label, size) in [
            ("text", Value::Text("big".to_owned())),
            ("negative", Value::Integer(-1)),
            ("null", Value::Null),
        ] {
            let directory = TestDirectory::new(&format!("check-size-{label}"));
            // `NOT NULL` would refuse the null; the column type is what a damaged file has.
            let connection = rusqlite::Connection::open(http_disk_cache_database(directory.path()))
                .expect("a database");
            connection
                .execute_batch(
                    "CREATE TABLE disk_cache (key text, data blob, size, insertion_timestamp)",
                )
                .expect("the table");
            connection
                .execute(
                    "INSERT INTO disk_cache VALUES ('https://a.test/', x'00', ?1, 0)",
                    [size],
                )
                .expect("a row");
            drop(connection);

            assert_removed(&directory, "size");
        }
    }

    #[test]
    fn a_file_that_is_not_a_database_is_removed() {
        let directory = TestDirectory::new("check-garbage");
        std::fs::write(
            http_disk_cache_database(directory.path()),
            "not a database".repeat(100),
        )
        .expect("a file");

        assert_removed(&directory, "");
    }

    #[test]
    fn a_truncated_database_is_removed() {
        let directory = TestDirectory::new("check-truncated");
        let rows = (0..200)
            .map(|index| format!("https://a.test/{index}"))
            .collect::<Vec<_>>();
        let rows = rows
            .iter()
            .map(|key| (key.as_str(), Value::Integer(4000)))
            .collect::<Vec<_>>();
        database_with(directory.path(), &rows);
        let connection = rusqlite::Connection::open(http_disk_cache_database(directory.path()))
            .expect("the database");
        connection
            .execute("UPDATE disk_cache SET data = randomblob(4000)", ())
            .expect("bigger rows");
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("a checkpoint");
        drop(connection);
        let database = http_disk_cache_database(directory.path());
        let length = std::fs::metadata(&database).expect("the file").len();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&database)
            .and_then(|file| file.set_len(length / 2))
            .expect("a truncated file");

        assert_removed(&directory, "");
    }

    #[test]
    fn the_database_is_inside_the_cache_directory() {
        assert_eq!(
            http_disk_cache_database(Path::new("/profile/http-cache")),
            Path::new("/profile/http-cache/cache.sqlite3")
        );
    }

    #[test]
    fn removing_takes_the_database_and_its_companions_only() {
        let directory = TestDirectory::new("companions");
        for name in [
            "cache.sqlite3",
            "cache.sqlite3-wal",
            "cache.sqlite3-shm",
            "cache.sqlite3-journal",
            "unrelated",
        ] {
            std::fs::write(directory.path().join(name), "x").expect("a file");
        }

        remove_http_disk_cache(directory.path());

        assert_eq!(names(directory.path()), vec!["unrelated"]);
    }

    #[test]
    fn removing_a_cache_that_is_not_there_is_harmless() {
        let directory = TestDirectory::new("missing");

        remove_http_disk_cache(directory.path());
        remove_http_disk_cache(&directory.path().join("missing"));

        assert_eq!(names(directory.path()), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_database_is_unlinked_not_followed() {
        let directory = TestDirectory::new("symlink");
        let target = directory.path().join("target");
        std::fs::write(&target, "keep").expect("a file");
        std::os::unix::fs::symlink(&target, directory.path().join("cache.sqlite3"))
            .expect("a link");

        remove_http_disk_cache(directory.path());

        assert_eq!(names(directory.path()), vec!["target"]);
    }
}
