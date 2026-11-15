//! Peak heap use while hostile files are read. A binary of its own, because it replaces the
//! global allocator with one that counts; it holds a single test so nothing else allocates
//! while it measures.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use mercury_settings::{
    Loaded, MAX_RESTORED_TABS, MAX_SESSION_FILE_BYTES, MAX_SETTINGS_FILE_BYTES, MAX_TITLE_BYTES,
    SessionStore, SettingsStore,
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
            // Counted as if the old and the new block were both alive, as they are while a
            // moving realloc copies: an upper bound on the true peak.
            let both = CURRENT.load(Ordering::Relaxed) + new_size;
            PEAK.fetch_max(both, Ordering::Relaxed);
            if new_size >= layout.size() {
                CURRENT.fetch_add(new_size - layout.size(), Ordering::Relaxed);
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

fn usize_of(bytes: u64) -> usize {
    usize::try_from(bytes).expect("fits")
}

#[test]
fn hostile_files_cost_little_more_than_their_own_size() {
    let dir = tempfile::tempdir().expect("a profile");
    let allowance = 4 * 1024 * 1024;
    let store = SessionStore::new(dir.path());

    // About 600,000 real tabs in one window: decoded, 500 of them kept.
    for (label, head) in [
        (
            "session",
            r#"{"format_version": 1, "exited_cleanly": false, "windows": [{"tabs": ["#,
        ),
        // The same without a version: the file is scanned for one before it is decoded.
        ("unversioned session", r#"{"windows": [{"tabs": ["#),
    ] {
        let size = write_repeated(
            &dir.path().join("session.json"),
            head,
            r#"{"url":"https://a.example/"}"#,
            "]}]}",
            MAX_SESSION_FILE_BYTES,
        );
        let mut tabs = 0;
        let peak = peak_growth(|| {
            let found = store.begin_run().expect("begin_run");
            tabs = found
                .last
                .map_or(0, |last| last.snapshot.windows[0].tabs.len());
        });
        eprintln!(
            "{label} file {:.1} MiB: peak heap growth {:.1} KiB, {tabs} tabs decoded",
            size as f64 / MIB,
            peak as f64 / KIB
        );
        assert_eq!(
            tabs, MAX_RESTORED_TABS,
            "{label}: decoded, not merely tolerated"
        );
        assert!(peak <= usize_of(size) + allowance, "{label}: {peak} bytes");
    }

    // One enormous title: serde_json reads it whole into its scratch buffer before the
    // decoder cuts it. The buffer grows by doubling, so at the moment it moves the old and the
    // new block are both alive: about one and a half times the string's length at the peak,
    // bounded here by twice the file's size limit.
    let head =
        r#"{"format_version": 1, "windows": [{"tabs": [{"url": "https://a.example/", "title": ""#;
    let tail = r#""}]}]}"#;
    let path = dir.path().join("session.json");
    let length = usize_of(MAX_SESSION_FILE_BYTES) - head.len() - tail.len() - 1;
    let mut text = String::with_capacity(length + head.len() + tail.len());
    text.push_str(head);
    text.extend(std::iter::repeat_n('t', length));
    text.push_str(tail);
    fs::write(&path, &text).expect("write");
    drop(text);
    let size = fs::metadata(&path).expect("size").len();
    let mut title = 0;
    let peak = peak_growth(|| {
        let found = store.begin_run().expect("begin_run");
        title = found
            .last
            .map_or(0, |last| last.snapshot.windows[0].tabs[0].title.len());
    });
    eprintln!(
        "one-string session file {:.1} MiB: peak heap growth {:.1} MiB, title kept {title} bytes",
        size as f64 / MIB,
        peak as f64 / MIB
    );
    assert_eq!(title, MAX_TITLE_BYTES, "decoded, title cut");
    assert!(
        peak <= 2 * usize_of(size) + allowance,
        "one string: {peak} bytes"
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
    assert!(
        settings_peak <= usize_of(settings_size) + allowance,
        "settings: {settings_peak} bytes"
    );
    assert_eq!(
        repairs, 1,
        "decoded, with one counted repair for the whole list"
    );
}
