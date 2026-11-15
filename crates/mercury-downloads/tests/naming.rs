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
fn default_ignorable_characters_are_removed() {
    // The reviewer's list, then one from each remaining Default_Ignorable_Code_Point range.
    for mark in [
        '\u{00ad}',
        '\u{034f}',
        '\u{180e}',
        '\u{115f}',
        '\u{1160}',
        '\u{3164}',
        '\u{ffa0}',
        '\u{e0000}',
        '\u{e0001}',
        '\u{e0041}',
        '\u{e007f}',
        '\u{17b4}',
        '\u{17b5}',
        '\u{180b}',
        '\u{180f}',
        '\u{2064}',
        '\u{2065}',
        '\u{206f}',
        '\u{fe00}',
        '\u{fff0}',
        '\u{1bca0}',
        '\u{1bca3}',
        '\u{1d173}',
        '\u{1d17a}',
        '\u{e0100}',
        '\u{e0fff}',
    ] {
        assert_eq!(
            sanitize_file_name(&format!("a{mark}b.txt")),
            "ab.txt",
            "U+{:04X} survived",
            mark as u32
        );
    }
    // A name made only of fillers is nothing.
    assert_eq!(sanitize_file_name("\u{3164}\u{115f}.\u{ffa0}"), "download");
}

#[test]
fn joiners_and_emoji_presentation_selectors_are_kept() {
    // Default-ignorable, but part of real text: Persian words and emoji sequences need them.
    for kept in [
        "می\u{200c}خواهم.txt",
        "👨\u{200d}👩\u{200d}👧.txt",
        "❤\u{fe0f}.txt",
        "☺\u{fe0e}.txt",
    ] {
        assert_eq!(sanitize_file_name(kept), kept);
    }
}

#[test]
fn joiners_and_selectors_are_removed_from_the_extension_and_the_end() {
    // Inside or after the extension they would turn `.pdf` or `.exe` into an unknown type.
    assert_eq!(sanitize_file_name("report.pdf\u{200d}"), "report.pdf");
    assert_eq!(sanitize_file_name("setup.e\u{200d}xe"), "setup.exe");
    assert_eq!(sanitize_file_name("setup.\u{200c}exe\u{fe0f}"), "setup.exe");
    assert_eq!(sanitize_file_name("photo.j\u{fe0e}pg"), "photo.jpg");
    assert_eq!(sanitize_file_name("notes\u{200d}"), "notes");
    assert_eq!(sanitize_file_name("notes\u{200d}.\u{200d}"), "notes");
    // Kept in the stem: a Persian word with a ZWNJ and an emoji ZWJ sequence.
    assert_eq!(
        sanitize_file_name("می\u{200c}خواهم.pdf"),
        "می\u{200c}خواهم.pdf"
    );
    assert_eq!(
        sanitize_file_name("👨\u{200d}👩\u{200d}👧 photo.jpg"),
        "👨\u{200d}👩\u{200d}👧 photo.jpg"
    );
    assert_eq!(sanitize_file_name("❤\u{fe0f}.txt"), "❤\u{fe0f}.txt");
}

#[test]
fn one_extension_limit_applies_to_cutting_and_numbering() {
    // Long enough that the sanitiser has to cut, with extensions around the limit.
    for extension_len in 58..=68 {
        let extension = format!(".{}", "e".repeat(extension_len - 1));
        let name = format!("{}{extension}", "s".repeat(300));
        let sanitized = sanitize_file_name(&name);
        let kept_by_sanitiser = sanitized.ends_with(&extension);
        let numbered = unique_path(Path::new("/dl"), &name, |path: &Path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !name.contains(" ("))
        });
        let numbered = numbered.file_name().unwrap().to_str().unwrap();
        let kept_by_numbering = numbered.ends_with(&format!(" (1){extension}"));
        assert_eq!(
            kept_by_sanitiser, kept_by_numbering,
            "extension of {extension_len} bytes: sanitised {sanitized:?}, numbered {numbered:?}"
        );
        if !kept_by_numbering {
            assert!(numbered.ends_with(" (1)"), "{numbered}");
        }
    }
}

#[test]
fn console_and_clock_device_names_are_prefixed() {
    assert_eq!(sanitize_file_name("CONIN$"), "_CONIN$");
    assert_eq!(sanitize_file_name("conout$.txt"), "_conout$.txt");
    assert_eq!(sanitize_file_name("CLOCK$.log"), "_CLOCK$.log");
    assert_eq!(sanitize_file_name("CONIN$x.txt"), "CONIN$x.txt");
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
fn a_taken_name_with_a_huge_extension_is_numbered_without_overflow() {
    // What a server gets with `filename="a.<300 × x>"`: the sanitiser keeps a 254-byte "extension".
    let name = sanitize_file_name(&format!("a.{}", "x".repeat(300)));
    let first = unique_path(Path::new("/dl"), &name, taken(&[]));
    let first_name = first.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(first_name.len() + ".part".len() <= 255);

    for extension_len in [246, 247, 248, 249, 250, 253] {
        let name = format!("a.{}", "x".repeat(extension_len));
        let free = unique_path(Path::new("/dl"), &name, taken(&[]));
        let free = free.file_name().unwrap().to_str().unwrap().to_owned();
        let numbered = unique_path(Path::new("/dl"), &name, taken(&[free.as_str()]));
        let numbered = numbered.file_name().unwrap().to_str().unwrap();
        assert_ne!(numbered, free);
        assert!(
            numbered.len() + ".part".len() <= 255,
            "{extension_len}: {} bytes",
            numbered.len()
        );
    }
}

#[test]
fn numbered_names_and_their_partial_files_always_fit() {
    let dir = Path::new("/dl");
    for stem_len in [0, 1, 2, 100, 180, 240, 246, 250, 251, 255, 300] {
        for extension_len in [
            0, 1, 3, 10, 50, 62, 63, 64, 100, 200, 246, 249, 250, 254, 300,
        ] {
            let name = if extension_len == 0 {
                "s".repeat(stem_len)
            } else {
                format!("{}.{}", "s".repeat(stem_len), "e".repeat(extension_len))
            };
            for taken_count in [0_usize, 1, 2, 11] {
                let mut taken_names: Vec<String> = Vec::new();
                for _ in 0..taken_count {
                    let refs: Vec<&str> = taken_names.iter().map(String::as_str).collect();
                    let next = unique_path(dir, &name, taken(&refs));
                    taken_names.push(next.file_name().unwrap().to_str().unwrap().to_owned());
                }
                let refs: Vec<&str> = taken_names.iter().map(String::as_str).collect();
                let path = unique_path(dir, &name, taken(&refs));
                let result = path.file_name().unwrap().to_str().unwrap();
                assert!(
                    result.len() + ".part".len() <= 255,
                    "stem {stem_len}, extension {extension_len}, taken {taken_count}: {} bytes",
                    result.len()
                );
                assert!(!refs.contains(&result), "{result} was taken");
                assert_eq!(path.parent(), Some(dir));
            }
        }
    }
}

#[test]
fn numbered_multi_byte_names_stay_valid_and_within_the_limit() {
    let dir = Path::new("/dl");
    for unit in ["é", "😀"] {
        // Stems and extensions whose length in bytes lands on and around every cut point.
        for stem_chars in [
            0, 1, 40, 59, 60, 61, 62, 63, 64, 120, 123, 124, 125, 126, 200,
        ] {
            for extension_chars in [0, 1, 2, 14, 15, 16, 30, 31, 32, 60, 100] {
                let stem = unit.repeat(stem_chars);
                let name = if extension_chars == 0 {
                    stem
                } else {
                    format!("{stem}.{}", unit.repeat(extension_chars))
                };
                let mut taken_names: Vec<String> = Vec::new();
                for _ in 0..3 {
                    let refs: Vec<&str> = taken_names.iter().map(String::as_str).collect();
                    let path = unique_path(dir, &name, taken(&refs));
                    let result = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_else(|| panic!("not valid UTF-8 for {name:?}"));
                    assert!(
                        result.len() <= 250,
                        "{unit} × {stem_chars}/{extension_chars}: {} bytes",
                        result.len()
                    );
                    assert!(!refs.contains(&result), "{result} was taken");
                    taken_names.push(result.to_owned());
                }
            }
        }
    }
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
