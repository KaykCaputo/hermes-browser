//! Peak heap use while hostile files are read. A binary of its own, because it replaces the
//! global allocator with one that counts; it holds a single test so nothing else allocates
//! while it measures.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use mercury_settings::{
    Loaded, MAX_SESSION_FILE_BYTES, MAX_SETTINGS_FILE_BYTES, SessionStore, SettingsStore,
};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator; only counters are added.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded with the caller's layout.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forwarded with the caller's pointer and layout.
        unsafe { System.dealloc(pointer, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded with the caller's arguments.
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            if new_size >= layout.size() {
                let grown = new_size - layout.size();
                let now = CURRENT.fetch_add(grown, Ordering::Relaxed) + grown;
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                CURRENT.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `work` and returns how far the heap grew above where it started.
fn peak_growth(work: impl FnOnce()) -> usize {
    let start = CURRENT.load(Ordering::Relaxed);
    PEAK.store(start, Ordering::Relaxed);
    work();
    PEAK.load(Ordering::Relaxed) - start
}

/// Writes `head`, then `item,` repeated, then `item` and `tail`, to just under `limit` bytes.
fn write_repeated(path: &Path, head: &str, item: &str, tail: &str, limit: u64) -> u64 {
    let limit = usize::try_from(limit).expect("fits");
    let count = (limit - head.len() - tail.len() - item.len()) / (item.len() + 1);
    let mut file = std::io::BufWriter::new(fs::File::create(path).expect("create"));
    file.write_all(head.as_bytes()).expect("write");
    for _ in 0..count {
        file.write_all(item.as_bytes()).expect("write");
        file.write_all(b",").expect("write");
    }
    file.write_all(item.as_bytes()).expect("write");
    file.write_all(tail.as_bytes()).expect("write");
    file.flush().expect("flush");
    fs::metadata(path).expect("size").len()
}

const MIB: f64 = 1024.0 * 1024.0;
const KIB: f64 = 1024.0;

#[test]
fn hostile_files_cost_little_more_than_their_own_size() {
    let dir = tempfile::tempdir().expect("a profile");
    let allowance = 4 * 1024 * 1024;

    // About 5.6 million empty tabs in one window.
    let session_size = write_repeated(
        &dir.path().join("session.json"),
        r#"{"format_version": 1, "exited_cleanly": false, "windows": [{"tabs": ["#,
        "{}",
        "]}]}",
        MAX_SESSION_FILE_BYTES,
    );
    let store = SessionStore::new(dir.path());
    let session_peak = peak_growth(|| {
        let _ = store.begin_run();
    });
    eprintln!(
        "session file {:.1} MiB: peak heap growth {:.1} KiB",
        session_size as f64 / MIB,
        session_peak as f64 / KIB
    );

    // About 350,000 empty custom engines.
    let settings_size = write_repeated(
        &dir.path().join("settings.json"),
        r#"{"format_version": 1, "search": {"custom_engines": ["#,
        "{}",
        "]}}",
        MAX_SETTINGS_FILE_BYTES,
    );
    let settings = SettingsStore::new(dir.path());
    let mut repairs = 0;
    let settings_peak = peak_growth(|| {
        if let Ok(Loaded::Ok { repairs: found, .. }) = settings.load() {
            repairs = found.len();
        }
    });
    eprintln!(
        "settings file {:.1} MiB: peak heap growth {:.1} KiB, {repairs} repairs",
        settings_size as f64 / MIB,
        settings_peak as f64 / KIB
    );

    // The same tabs in a file without a version: it is scanned for one before it is decoded.
    let unversioned_size = write_repeated(
        &dir.path().join("session.json"),
        r#"{"windows": [{"tabs": ["#,
        "{}",
        "]}]}",
        MAX_SESSION_FILE_BYTES,
    );
    let unversioned_peak = peak_growth(|| {
        let _ = store.begin_run();
    });
    eprintln!(
        "unversioned session file {:.1} MiB: peak heap growth {:.1} KiB",
        unversioned_size as f64 / MIB,
        unversioned_peak as f64 / KIB
    );

    assert!(
        unversioned_peak <= usize::try_from(unversioned_size).expect("fits") + allowance,
        "unversioned session: {unversioned_peak} bytes"
    );
    assert!(
        session_peak <= usize::try_from(session_size).expect("fits") + allowance,
        "session: {session_peak} bytes"
    );
    assert!(
        settings_peak <= usize::try_from(settings_size).expect("fits") + allowance,
        "settings: {settings_peak} bytes"
    );
    assert_eq!(repairs, 1, "one counted repair for the whole list");
}
