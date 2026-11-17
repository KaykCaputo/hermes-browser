//! Text the downloads list shows, and which files get an "Open" button.

use std::path::Path;

use mercury_downloads::format_bytes;

/// "12 s left", "3 min left", "2 h left"; `None` without a known total or speed.
pub(crate) fn time_left(
    received: u64,
    total: Option<u64>,
    bytes_per_second: u64,
) -> Option<String> {
    let remaining = total?.checked_sub(received).filter(|left| *left > 0)?;
    if bytes_per_second == 0 {
        return None;
    }
    let seconds = remaining.div_ceil(bytes_per_second);
    Some(if seconds < 60 {
        format!("{seconds} s left")
    } else if seconds < 3600 {
        format!("{} min left", seconds.div_ceil(60).clamp(1, 59))
    } else {
        format!("{} h left", seconds.div_ceil(3600))
    })
}

/// "1.2 MB of 5.0 MB", or "1.2 MB" without a total.
pub(crate) fn amount(received: u64, total: Option<u64>) -> String {
    match total {
        Some(total) => format!("{} of {}", format_bytes(received), format_bytes(total)),
        None => format_bytes(received),
    }
}

/// The status line of a download in progress: amount, speed and time left.
pub(crate) fn progress_line(received: u64, total: Option<u64>, bytes_per_second: u64) -> String {
    let mut line = amount(received, total);
    if bytes_per_second > 0 {
        line.push_str(&format!(" — {}/s", format_bytes(bytes_per_second)));
        if let Some(left) = time_left(received, total, bytes_per_second) {
            line.push_str(", ");
            line.push_str(&left);
        }
    }
    line
}

/// Files that run code, change the system or open an address when opened, and documents that
/// run scripts in a browser. The list offers only "Show in Finder" for them, never a direct
/// "Open", so a download is never one click from running.
pub(crate) fn is_unsafe_to_open(path: &Path) -> bool {
    const EXECUTABLE: &[&str] = &[
        // macOS applications, scripts and installers.
        "app",
        "command",
        "tool",
        "terminal",
        "pkg",
        "mpkg",
        "scpt",
        "scptd",
        "applescript",
        "workflow",
        "action",
        "prefpane",
        "kext",
        "osx",
        "dylib",
        "bundle",
        "plugin",
        "service",
        "shortcut",
        "mobileconfig",
        // Address files that open whatever they point at.
        "fileloc",
        "inetloc",
        "webloc",
        "url",
        // Documents that run scripts when a browser opens them.
        "html",
        "htm",
        "xhtml",
        "shtml",
        "svg",
        // Scripts any system runs.
        "sh",
        "bash",
        "zsh",
        "csh",
        "ksh",
        "fish",
        "py",
        "pyw",
        "pl",
        "rb",
        "php",
        "jar",
        "jnlp",
        // Linux.
        "appimage",
        "desktop",
        "run",
        "deb",
        "rpm",
        "flatpak",
        "snap",
        // Windows (harmless here, but never one click from running anywhere).
        "exe",
        "msi",
        "bat",
        "cmd",
        "com",
        "scr",
        "ps1",
        "vbs",
        "vbe",
        "wsf",
        "hta",
        "lnk",
        "reg",
        "cpl",
        "msc",
    ];
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            EXECUTABLE
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_is_rounded_up_to_a_readable_unit() {
        assert_eq!(time_left(0, None, 1000), None);
        assert_eq!(time_left(0, Some(1000), 0), None);
        assert_eq!(time_left(1000, Some(1000), 10).as_deref(), None);
        assert_eq!(time_left(0, Some(1000), 100).as_deref(), Some("10 s left"));
        assert_eq!(time_left(0, Some(1001), 100).as_deref(), Some("11 s left"));
        assert_eq!(
            time_left(0, Some(6_000), 100).as_deref(),
            Some("1 min left")
        );
        assert_eq!(
            time_left(0, Some(9_100), 100).as_deref(),
            Some("2 min left")
        );
        assert_eq!(
            time_left(0, Some(7_200_000), 1000).as_deref(),
            Some("2 h left")
        );
        // More received than promised: nothing sensible to say.
        assert_eq!(time_left(2000, Some(1000), 100), None);
    }

    #[test]
    fn amounts_and_progress_lines() {
        assert_eq!(amount(512, None), "512 B");
        assert_eq!(amount(1536, Some(5 * 1024 * 1024)), "1.5 KB of 5.0 MB");
        assert_eq!(
            progress_line(1024 * 1024, Some(2 * 1024 * 1024), 512 * 1024),
            "1.0 MB of 2.0 MB — 512.0 KB/s, 2 s left"
        );
        assert_eq!(progress_line(2048, None, 1024), "2.0 KB — 1.0 KB/s");
        assert_eq!(progress_line(0, Some(10), 0), "0 B of 10 B");
    }

    #[test]
    fn programs_and_installers_are_executable_types() {
        for name in [
            "Setup.app",
            "run.command",
            "install.sh",
            "tool.PKG",
            "x.mpkg",
            "game.exe",
            "a.msi",
            "b.bat",
            "c.jar",
            "d.scpt",
            "e.workflow",
            "f.AppImage",
            "g.desktop",
            "h.py",
            "i.ps1",
            "j.vbs",
            "k.terminal",
            "l.fileloc",
            "m.inetloc",
            "n.webloc",
            "o.mobileconfig",
            "p.dylib",
            "q.bundle",
            "r.plugin",
            "s.service",
            "t.shortcut",
            "u.jnlp",
            "v.url",
            "page.html",
            "page.HTM",
            "drawing.svg",
        ] {
            assert!(is_unsafe_to_open(Path::new(name)), "{name}");
        }
        for name in [
            "report.pdf",
            "photo.png",
            "archive.zip",
            "disk.dmg",
            "notes.txt",
            "no-extension",
        ] {
            assert!(!is_unsafe_to_open(Path::new(name)), "{name}");
        }
    }
}
