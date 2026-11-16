//! Keeps the GStreamer bundled with Mercury away from any other GStreamer on the machine.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A change to the process environment.
#[derive(Debug, PartialEq, Eq)]
pub enum EnvChange {
    Set(&'static str, OsString),
    Remove(&'static str),
}

/// The GStreamer version Mercury was built against (from `build.rs`).
pub const GSTREAMER_VERSION: &str = match option_env!("MERCURY_GSTREAMER_VERSION") {
    Some(version) => version,
    None => "unknown",
};

/// The bundle identifier, which names Mercury's cache directory (as in `Info.plist`).
const BUNDLE_IDENTIFIER: &str = "io.github.thepigdestroyer.mercury-browser";

/// The environment that isolates the bundled GStreamer, for an application whose cache
/// directory is `cache_dir` and whose executable lives in `executable_dir`.
///
/// Without it, `gst_init` reads (and may rewrite) the registry cache shared with every other
/// GStreamer of the user, `$XDG_CACHE_HOME/gstreamer-1.0/registry.<arch>.bin`; scans and loads,
/// in process, the plugins in `GST_PLUGIN_PATH`, `~/.local/share/gstreamer-1.0/plugins` and
/// the default system directory; and GIO, started by the `gstgio` plugin, loads the modules
/// in `GIO_EXTRA_MODULES` and in a directory compiled into the official build. Servo loads its
/// plugins itself, by file name from `<executable dir>/lib`, so none of that is needed.
///
/// GStreamer reads each `_1_0` variable before the unversioned one, so setting the versioned
/// name overrides an inherited unversioned one; those are removed as well, so that nothing
/// inherited is honoured:
///
/// * `GST_REGISTRY_1_0`: the registry cache file, here private to Mercury and named by
///   GStreamer version and architecture (`GST_REGISTRY` removed).
/// * `GST_PLUGIN_PATH_1_0` = empty: no extra plugin directories (`GST_PLUGIN_PATH` removed).
/// * `GST_PLUGIN_SYSTEM_PATH_1_0` = empty: no system or home plugin directories; an unset
///   value would mean "`~/.local/share/gstreamer-1.0/plugins` and the default directory"
///   (`GST_PLUGIN_SYSTEM_PATH` removed).
/// * `GST_REGISTRY_FORK` = `no`: a registry update never starts a helper process, so an
///   inherited `GST_PLUGIN_SCANNER_1_0` / `GST_PLUGIN_SCANNER` could not name a program to run
///   (both removed).
/// * `GIO_MODULE_DIR`: GIO's module directory, pointed at `lib/gio/modules` beside the
///   executable, which Mercury does not ship; the compiled-in default is a path on the machine
///   that built the packages (`GIO_EXTRA_MODULES` removed).
pub fn gstreamer_isolation(cache_dir: &Path, executable_dir: &Path) -> Vec<EnvChange> {
    let registry = cache_dir.join("gstreamer").join(format!(
        "registry-{GSTREAMER_VERSION}-{}.bin",
        std::env::consts::ARCH
    ));
    let gio_modules = executable_dir.join("lib").join("gio").join("modules");
    vec![
        EnvChange::Set("GST_REGISTRY_1_0", registry.into_os_string()),
        EnvChange::Remove("GST_REGISTRY"),
        EnvChange::Set("GST_PLUGIN_PATH_1_0", OsString::new()),
        EnvChange::Remove("GST_PLUGIN_PATH"),
        EnvChange::Set("GST_PLUGIN_SYSTEM_PATH_1_0", OsString::new()),
        EnvChange::Remove("GST_PLUGIN_SYSTEM_PATH"),
        EnvChange::Set("GST_REGISTRY_FORK", OsString::from("no")),
        EnvChange::Remove("GST_PLUGIN_SCANNER_1_0"),
        EnvChange::Remove("GST_PLUGIN_SCANNER"),
        EnvChange::Set("GIO_MODULE_DIR", gio_modules.into_os_string()),
        EnvChange::Remove("GIO_EXTRA_MODULES"),
    ]
}

/// Mercury's cache directory inside the platform's cache directory (`dirs::cache_dir()`,
/// `~/Library/Caches` on macOS), or inside the temporary directory if there is none.
pub fn mercury_cache_dir(platform_cache_dir: Option<PathBuf>) -> PathBuf {
    platform_cache_dir
        .unwrap_or_else(std::env::temp_dir)
        .join(BUNDLE_IDENTIFIER)
}

/// The user directories GLib (and so GStreamer) writes to, pointed into `root`, a private
/// per-run directory: `XDG_CACHE_HOME` is where GStreamer's download buffers go (copies of the
/// media being played, `<cache>/<program name>-XXXXXX`), and the data, configuration and
/// runtime directories are moved with it so that nothing GLib writes lands in the home
/// directory.
pub fn glib_user_directories(root: &Path) -> Vec<EnvChange> {
    GLIB_USER_DIRECTORIES
        .iter()
        .map(|(name, child)| EnvChange::Set(name, root.join(child).into_os_string()))
        .collect()
}

/// The GLib user-directory variables and the subdirectory of the run directory each names.
pub const GLIB_USER_DIRECTORIES: [(&str, &str); 4] = [
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_RUNTIME_DIR", "runtime"),
];

/// Applies environment changes to this process.
///
/// # Safety
///
/// Must be called before any thread exists and before anything reads the environment.
pub unsafe fn apply(changes: Vec<EnvChange>) {
    for change in changes {
        // SAFETY: the caller guarantees that no other thread exists.
        unsafe {
            match change {
                EnvChange::Set(name, value) => std::env::set_var(name, value),
                EnvChange::Remove(name) => std::env::remove_var(name),
            }
        }
    }
}

/// Applies [`gstreamer_isolation`] to this process. Content processes are started from this
/// executable and inherit the result.
pub fn isolate_gstreamer_from_user_environment() {
    let cache_dir = mercury_cache_dir(dirs::cache_dir());
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    // SAFETY: called at the top of `main`, before any thread exists and before anything
    // reads the environment.
    unsafe { apply(gstreamer_isolation(&cache_dir, &executable_dir)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes() -> Vec<EnvChange> {
        gstreamer_isolation(
            Path::new("/Users/someone/Library/Caches/mercury"),
            Path::new("/Applications/Mercury Browser.app/Contents/MacOS"),
        )
    }

    fn value_of(name: &str) -> Option<OsString> {
        changes().into_iter().find_map(|change| match change {
            EnvChange::Set(key, value) if key == name => Some(value),
            _ => None,
        })
    }

    fn removes(name: &'static str) -> bool {
        changes().contains(&EnvChange::Remove(name))
    }

    #[test]
    fn the_registry_is_a_private_file_named_by_version_and_architecture() {
        let expected =
            PathBuf::from("/Users/someone/Library/Caches/mercury/gstreamer").join(format!(
                "registry-{GSTREAMER_VERSION}-{}.bin",
                std::env::consts::ARCH
            ));
        assert_eq!(
            value_of("GST_REGISTRY_1_0"),
            Some(expected.into_os_string())
        );
        assert!(removes("GST_REGISTRY"));
    }

    #[test]
    fn no_plugin_directory_is_scanned() {
        // Empty, not unset: an unset system path means "the user's home plugins and the
        // default system directory".
        assert_eq!(value_of("GST_PLUGIN_PATH_1_0"), Some(OsString::new()));
        assert_eq!(
            value_of("GST_PLUGIN_SYSTEM_PATH_1_0"),
            Some(OsString::new())
        );
        assert!(removes("GST_PLUGIN_PATH"));
        assert!(removes("GST_PLUGIN_SYSTEM_PATH"));
    }

    #[test]
    fn no_external_plugin_scanner_is_started() {
        assert_eq!(value_of("GST_REGISTRY_FORK"), Some(OsString::from("no")));
        assert!(removes("GST_PLUGIN_SCANNER_1_0"));
        assert!(removes("GST_PLUGIN_SCANNER"));
    }

    #[test]
    fn gio_loads_modules_only_from_the_application() {
        assert_eq!(
            value_of("GIO_MODULE_DIR"),
            Some(OsString::from(
                "/Applications/Mercury Browser.app/Contents/MacOS/lib/gio/modules"
            ))
        );
        assert!(removes("GIO_EXTRA_MODULES"));
    }

    #[test]
    fn every_variable_is_named_once() {
        let mut names: Vec<&str> = changes()
            .into_iter()
            .map(|change| match change {
                EnvChange::Set(name, _) | EnvChange::Remove(name) => name,
            })
            .collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
        assert_eq!(count, 11);
    }

    #[test]
    fn the_cache_directory_is_inside_the_platform_cache_directory() {
        assert_eq!(
            mercury_cache_dir(Some(PathBuf::from("/Users/someone/Library/Caches"))),
            PathBuf::from(
                "/Users/someone/Library/Caches/io.github.thepigdestroyer.mercury-browser"
            )
        );
        assert_eq!(
            mercury_cache_dir(None),
            std::env::temp_dir().join("io.github.thepigdestroyer.mercury-browser")
        );
    }

    #[test]
    fn glib_writes_only_inside_the_run_directory() {
        let root = Path::new("/c/media-buffers/42-7");
        let changes = glib_user_directories(root);
        for (name, child) in [
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_RUNTIME_DIR", "runtime"),
        ] {
            assert!(
                changes.contains(&EnvChange::Set(name, root.join(child).into_os_string())),
                "{name}"
            );
        }
        assert_eq!(changes.len(), 4);
    }
}
