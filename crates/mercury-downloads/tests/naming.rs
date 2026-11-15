// Partly adapted from Zervo (https://github.com/Goddv/Zervo), commit
// 86cb71267066f3b1f402abbd5c26faaf24ddf165, `src/downloads.rs` (its tests at lines 437-539),
// licensed MPL-2.0.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use mercury_downloads::{file_name_for, format_bytes, sanitize_file_name, unique_path};
use url::Url;

fn url(text: &str) -> Url {
    Url::parse(text).expect("the test URL should be valid")
}

/// Every property a sanitised name must have, whatever went in.
fn assert_safe(input: &str, name: &str) {
    assert!(!name.is_empty(), "{input:?} gave an empty name");
    assert!(name.len() <= 255, "{input:?} gave {} bytes", name.len());
    let mut components = Path::new(name).components();
    assert!(
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none(),
        "{input:?} gave {name:?}, which is not a single path component"
    );
    assert!(!name.contains(['/', '\\', '\0']), "{input:?} gave {name:?}");
    assert!(
        !name.chars().any(char::is_control),
        "{input:?} gave {name:?} with a control character"
    );
    assert!(
        !name.starts_with('.') && !name.starts_with(char::is_whitespace),
        "{input:?} gave {name:?}, which starts with a dot or space"
    );
    assert!(
        !name.ends_with('.') && !name.ends_with(char::is_whitespace),
        "{input:?} gave {name:?}, which ends with a dot or space"
    );
    assert_ne!(name, "..");
}

// --- sanitize_file_name -------------------------------------------------------------------

#[test]
fn a_name_cannot_climb_out_of_the_download_folder() {
    assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
    assert_eq!(sanitize_file_name("..\\..\\x"), "_.._x");
    assert_eq!(sanitize_file_name("a/b"), "a_b");
    assert_eq!(sanitize_file_name("/etc/passwd"), "_etc_passwd");
    assert_eq!(
        sanitize_file_name("C:\\Windows\\evil.exe"),
        "C__Windows_evil.exe"
    );
}

#[test]
fn dots_spaces_and_nothing_become_download() {
    assert_eq!(sanitize_file_name(""), "download");
    assert_eq!(sanitize_file_name("   "), "download");
    assert_eq!(sanitize_file_name("."), "download");
    assert_eq!(sanitize_file_name(".."), "download");
    assert_eq!(sanitize_file_name("......"), "download");
    assert_eq!(sanitize_file_name(". . ."), "download");
    assert_eq!(sanitize_file_name("\u{202e}"), "download");
}

#[test]
fn nothing_lands_as_a_hidden_file_or_with_a_trailing_dot() {
    assert_eq!(sanitize_file_name("....hidden.txt"), "hidden.txt");
    assert_eq!(sanitize_file_name(" .x.txt "), "x.txt");
    assert_eq!(sanitize_file_name("evil.exe."), "evil.exe");
    assert_eq!(sanitize_file_name("evil.exe . ."), "evil.exe");
    // A name that is only an extension keeps the extension, so the file still opens as what
    // it is, instead of turning into an extensionless `pdf`.
    assert_eq!(sanitize_file_name(".pdf"), "download.pdf");
    assert_eq!(sanitize_file_name("..bashrc"), "download.bashrc");
}

#[test]
fn bidirectional_and_invisible_characters_are_removed() {
    assert_eq!(sanitize_file_name("\u{202e}gpj.exe"), "gpj.exe");
    assert_eq!(sanitize_file_name("photo\u{202e}gpj.exe"), "photogpj.exe");
    for mark in [
        '\u{061c}', '\u{200b}', '\u{200e}', '\u{200f}', '\u{2028}', '\u{2029}', '\u{202a}',
        '\u{2066}', '\u{2069}', '\u{2060}', '\u{feff}',
    ] {
        assert_eq!(
            sanitize_file_name(&format!("a{mark}b.txt")),
            "ab.txt",
            "U+{:04X} survived",
            mark as u32
        );
    }
}

#[test]
fn control_characters_are_removed() {
    assert_eq!(sanitize_file_name("re\nport.pdf"), "report.pdf");
    assert_eq!(sanitize_file_name("re\0port.pdf"), "report.pdf");
    assert_eq!(sanitize_file_name("re\u{7f}port\u{85}.pdf"), "report.pdf");
    assert_eq!(sanitize_file_name("tab\there.txt"), "tabhere.txt");
}

#[test]
fn colons_are_replaced() {
    assert_eq!(sanitize_file_name("12:30 notes.txt"), "12_30 notes.txt");
}

#[test]
fn windows_device_names_are_prefixed() {
    assert_eq!(sanitize_file_name("CON.txt"), "_CON.txt");
    assert_eq!(sanitize_file_name("con"), "_con");
    assert_eq!(sanitize_file_name("nul.tar.gz"), "_nul.tar.gz");
    assert_eq!(sanitize_file_name("COM1.zip"), "_COM1.zip");
    assert_eq!(sanitize_file_name("lpt9"), "_lpt9");
    assert_eq!(sanitize_file_name("COM\u{b9}.txt"), "_COM\u{b9}.txt");
    // Names that only start the same way are fine.
    assert_eq!(sanitize_file_name("console.log"), "console.log");
    assert_eq!(sanitize_file_name("COM10.txt"), "COM10.txt");
}

#[test]
fn long_names_are_cut_to_255_bytes_keeping_the_extension() {
    let long = format!("{}.pdf", "a".repeat(300));
    let cut = sanitize_file_name(&long);
    assert_eq!(cut.len(), 255);
    assert_eq!(cut, format!("{}.pdf", "a".repeat(251)));

    let exactly = format!("{}.pdf", "a".repeat(251));
    assert_eq!(sanitize_file_name(&exactly), exactly);
}

#[test]
fn multi_byte_names_are_cut_on_a_character_boundary() {
    // 62 emoji (248 bytes) plus `.txt` is 252 bytes: it fits.
    let fits = format!("{}.txt", "😀".repeat(62));
    assert_eq!(sanitize_file_name(&fits), fits);

    // 63 emoji plus `.txt` is 256 bytes; a 255-byte cut would land inside an emoji.
    let over = format!("{}.txt", "😀".repeat(63));
    assert_eq!(sanitize_file_name(&over), fits);

    let accented = format!("{}.txt", "é".repeat(200));
    let cut = sanitize_file_name(&accented);
    assert!(cut.len() <= 255);
    assert!(cut.ends_with(".txt"));
    assert_eq!(cut, format!("{}.txt", "é".repeat(125)));
}

#[test]
fn a_huge_extension_is_not_kept_at_the_cost_of_the_whole_name() {
    let name = format!("a.{}", "b".repeat(400));
    let cut = sanitize_file_name(&name);
    assert!(cut.len() <= 255);
    assert!(cut.starts_with("a."));
}

#[test]
fn a_cut_does_not_leave_a_trailing_dot_or_space() {
    // The stem may keep 251 bytes, which ends exactly after `  .`.
    let name = format!("{}  .{}.txt", "a".repeat(248), "b".repeat(30));
    let cut = sanitize_file_name(&name);
    assert_safe(&name, &cut);
    assert_eq!(cut, format!("{}.txt", "a".repeat(248)));
}

#[test]
fn every_hostile_name_comes_out_as_one_safe_component() {
    let long_dots = ".".repeat(300);
    let long_bidi = "\u{202e}".repeat(300);
    let long_emoji = "👨‍👩‍👧".repeat(40);
    for input in [
        "../../etc/passwd",
        "..\\..\\x",
        "a/b",
        "/",
        "\\",
        "\0",
        "..",
        ".",
        "",
        "   ",
        "\u{202e}gpj.exe",
        "CON",
        "CON.txt",
        "aux.",
        "file\r\n.txt",
        long_dots.as_str(),
        long_bidi.as_str(),
        long_emoji.as_str(),
        "\u{3000}wide space\u{3000}",
        "\u{a0}.nbsp\u{a0}",
        "%2e%2e%2fescape",
    ] {
        let name = sanitize_file_name(input);
        assert_safe(input, &name);
        // Sanitising is idempotent: a sanitised name is left alone.
        assert_eq!(sanitize_file_name(&name), name, "for {input:?}");
    }
}

// --- file_name_for ------------------------------------------------------------------------

#[test]
fn content_disposition_names_the_file_first() {
    let final_url = url("https://example.com/files/ignored.bin");
    assert_eq!(
        file_name_for(
            Some("attachment; filename=\"plain.txt\"; filename*=UTF-8''%E6%B8%AC%E8%A9%A6.txt"),
            &final_url,
            Some("text/plain"),
            Some("suggested.txt"),
        ),
        "測試.txt"
    );
    assert_eq!(
        file_name_for(
            Some("attachment; filename=report.pdf"),
            &final_url,
            None,
            Some("suggested.txt")
        ),
        "report.pdf"
    );
}

#[test]
fn the_suggested_name_comes_next() {
    let final_url = url("https://example.com/files/ignored.bin");
    assert_eq!(
        file_name_for(Some("inline"), &final_url, None, Some("chosen.txt")),
        "chosen.txt"
    );
    assert_eq!(
        file_name_for(
            Some("attachment; filename=\"\""),
            &final_url,
            None,
            Some("chosen.txt")
        ),
        "chosen.txt"
    );
    // A blank suggestion is no suggestion.
    assert_eq!(
        file_name_for(None, &final_url, None, Some("   ")),
        "ignored.bin"
    );
}

#[test]
fn the_final_url_comes_last() {
    assert_eq!(
        file_name_for(None, &url("https://x.test/dir/a%20b.zip?x=1#y"), None, None),
        "a b.zip"
    );
    assert_eq!(
        file_name_for(None, &url("https://x.test/dir/sub/"), None, None),
        "sub"
    );
    assert_eq!(
        file_name_for(None, &url("https://x.test/"), None, None),
        "download"
    );
    assert_eq!(
        file_name_for(None, &url("https://x.test"), None, None),
        "download"
    );
}

#[test]
fn every_source_is_sanitised() {
    let final_url = url("https://x.test/%2Fetc%2Fpasswd");
    assert_eq!(file_name_for(None, &final_url, None, None), "_etc_passwd");
    assert_eq!(
        file_name_for(None, &url("https://x.test/..%2F..%2Fx.sh"), None, None),
        "_.._x.sh"
    );
    assert_eq!(
        file_name_for(
            Some("attachment; filename=\"../../etc/passwd\""),
            &final_url,
            None,
            None
        ),
        "_.._etc_passwd"
    );
    assert_eq!(
        file_name_for(None, &final_url, None, Some("../evil.sh")),
        "_evil.sh"
    );
    assert_eq!(
        file_name_for(
            Some("attachment; filename*=UTF-8''%E2%80%AEgpj.exe"),
            &final_url,
            None,
            None
        ),
        "gpj.exe"
    );
}

#[test]
fn a_name_without_an_extension_gets_one_from_the_mime_type() {
    let getfile = url("https://x.test/getfile");
    assert_eq!(
        file_name_for(None, &getfile, Some("text/plain; charset=utf-8"), None),
        "getfile.txt"
    );
    assert_eq!(
        file_name_for(None, &getfile, Some("application/pdf"), None),
        "getfile.pdf"
    );
    assert_eq!(
        file_name_for(None, &getfile, Some("IMAGE/JPEG"), None),
        "getfile.jpg"
    );
    assert_eq!(
        file_name_for(None, &url("https://x.test/"), Some("application/zip"), None),
        "download.zip"
    );
    // Nothing useful to add for an opaque or unknown type.
    assert_eq!(
        file_name_for(None, &getfile, Some("application/octet-stream"), None),
        "getfile"
    );
    assert_eq!(
        file_name_for(None, &getfile, Some("application/x-unheard-of"), None),
        "getfile"
    );
    assert_eq!(
        file_name_for(None, &getfile, Some("garbage"), None),
        "getfile"
    );
    // A name that has an extension keeps it, even when the type disagrees.
    assert_eq!(
        file_name_for(None, &url("https://x.test/a.zip"), Some("text/html"), None),
        "a.zip"
    );
}

// --- unique_path --------------------------------------------------------------------------

fn taken(names: &[&str]) -> impl Fn(&Path) -> bool {
    let dir = PathBuf::from("/dl");
    let set: HashSet<PathBuf> = names.iter().map(|name| dir.join(name)).collect();
    move |path: &Path| set.contains(path)
}

#[test]
fn a_free_name_is_used_as_is() {
    assert_eq!(
        unique_path(Path::new("/dl"), "report.pdf", taken(&[])),
        PathBuf::from("/dl/report.pdf")
    );
}

#[test]
fn taken_names_are_numbered() {
    let dir = Path::new("/dl");
    assert_eq!(
        unique_path(dir, "report.pdf", taken(&["report.pdf"])),
        PathBuf::from("/dl/report (1).pdf")
    );
    assert_eq!(
        unique_path(dir, "report.pdf", taken(&["report.pdf", "report (1).pdf"])),
        PathBuf::from("/dl/report (2).pdf")
    );
    assert_eq!(
        unique_path(dir, "README", taken(&["README"])),
        PathBuf::from("/dl/README (1)")
    );
    assert_eq!(
        unique_path(dir, "archive.tar.gz", taken(&["archive.tar.gz"])),
        PathBuf::from("/dl/archive (1).tar.gz")
    );
}

#[test]
fn an_in_flight_partial_file_counts_as_taken() {
    let dir = Path::new("/dl");
    assert_eq!(
        unique_path(dir, "report.pdf", taken(&["report.pdf.part"])),
        PathBuf::from("/dl/report (1).pdf")
    );
    assert_eq!(
        unique_path(dir, "README", taken(&["README.part", "README (1)"])),
        PathBuf::from("/dl/README (2)")
    );
}

#[test]
fn numbered_names_still_fit_with_the_partial_suffix() {
    let dir = Path::new("/dl");
    let long = sanitize_file_name(&format!("{}.pdf", "a".repeat(300)));
    let path = unique_path(dir, &long, taken(&[]));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(name.len() + ".part".len() <= 255, "{} bytes", name.len());
    assert!(name.ends_with(".pdf"));

    let first = name.to_owned();
    let numbered = unique_path(dir, &long, taken(&[first.as_str()]));
    let numbered = numbered.file_name().unwrap().to_str().unwrap();
    assert!(numbered.len() + ".part".len() <= 255);
    assert!(numbered.ends_with(" (1).pdf"), "{numbered}");
}

#[test]
fn a_directory_where_everything_is_taken_still_terminates() {
    let path = unique_path(Path::new("/dl"), "report.pdf", |_: &Path| true);
    assert_ne!(path, PathBuf::from("/dl/report.pdf"));
    assert_eq!(path.parent(), Some(Path::new("/dl")));
}

// --- format_bytes -------------------------------------------------------------------------

#[test]
fn bytes_are_reported_in_units_a_person_reads() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(1023), "1023 B");
    assert_eq!(format_bytes(1024), "1.0 KB");
    assert_eq!(format_bytes(1536), "1.5 KB");
    assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    assert_eq!(format_bytes(u64::MAX), "16.0 EB");
}
