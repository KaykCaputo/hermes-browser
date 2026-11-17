//! Where downloads go: the one accessor for the download folder.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use mercury_settings::DownloadSettings;

thread_local! {
    static RUN: RefCell<RunFolders> = RefCell::new(RunFolders::default());
    static ASK_IGNORED_LOGGED: Cell<bool> = const { Cell::new(false) };
}

/// What this run decided at start-up.
#[derive(Debug, Default, Clone)]
struct RunFolders {
    /// `--download-dir`, a diagnostic that overrides the folder for one run.
    folder_override: Option<PathBuf>,
    /// For an automated run (`--screenshot`, `--background`): a folder inside its profile,
    /// so such a run never writes into the user's own folders.
    automated: Option<PathBuf>,
}

/// Called once at start-up. `profile_root` is the run's profile (temporary for an automated
/// run without `--profile-dir`); `None` if the run has no usable profile.
pub(crate) fn configure(
    folder_override: Option<PathBuf>,
    automated: bool,
    profile_root: Option<&Path>,
) {
    let automated = automated.then(|| automated_folder(profile_root));
    RUN.with(|run| {
        *run.borrow_mut() = RunFolders {
            folder_override,
            automated,
        };
    });
}

/// `<profile>/downloads`, or a folder of this process in the temporary directory.
fn automated_folder(profile_root: Option<&Path>) -> PathBuf {
    match profile_root {
        Some(root) => root.join("downloads"),
        None => {
            std::env::temp_dir().join(format!("mercury-browser-downloads-{}", std::process::id()))
        }
    }
}

/// The stored download settings. Until the settings work is wired into the application, these
/// are the defaults: the platform's Downloads folder, without asking.
fn download_settings() -> DownloadSettings {
    DownloadSettings::default()
}

/// The folder a new download is saved in.
pub(crate) fn download_folder() -> PathBuf {
    let run = RUN.with(|run| run.borrow().clone());
    let settings = download_settings();
    if settings.ask_where_to_save && !ASK_IGNORED_LOGGED.with(|logged| logged.replace(true)) {
        eprintln!(
            "mercury-browser: \"ask where to save\" is not available yet; downloads are saved \
             in the download folder"
        );
    }
    folder_for(
        &settings,
        run.folder_override,
        run.automated,
        platform_downloads_folder(),
    )
}

fn platform_downloads_folder() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

/// The decision, without the environment: the override, then an automated run's own folder,
/// then the stored folder, then the platform's. "Ask where to save" is not honoured until a
/// save panel exists (the settings window marks the download settings as stored only).
pub(crate) fn folder_for(
    settings: &DownloadSettings,
    folder_override: Option<PathBuf>,
    automated: Option<PathBuf>,
    platform_folder: PathBuf,
) -> PathBuf {
    folder_override
        .or(automated)
        .or_else(|| settings.directory.clone())
        .unwrap_or(platform_folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_then_the_automated_run_then_the_stored_then_the_platform_folder() {
        let platform = PathBuf::from("/Users/me/Downloads");
        let defaults = DownloadSettings::default();
        assert_eq!(
            folder_for(&defaults, None, None, platform.clone()),
            platform
        );
        let stored = DownloadSettings {
            directory: Some("/Volumes/Data/Incoming".into()),
            ask_where_to_save: false,
        };
        assert_eq!(
            folder_for(&stored, None, None, platform.clone()),
            PathBuf::from("/Volumes/Data/Incoming")
        );
        // Asking is not available yet: the folder is used.
        let asking = DownloadSettings {
            ask_where_to_save: true,
            ..stored.clone()
        };
        assert_eq!(
            folder_for(&asking, None, None, platform.clone()),
            PathBuf::from("/Volumes/Data/Incoming")
        );
        // An automated run never uses the stored or the user's folder.
        let run = PathBuf::from("/tmp/mercury-browser-profile-x/downloads");
        assert_eq!(
            folder_for(&stored, None, Some(run.clone()), platform.clone()),
            run
        );
        assert_eq!(
            folder_for(&defaults, None, Some(run.clone()), platform.clone()),
            run
        );
        assert_eq!(
            folder_for(&stored, Some("/scratch/dl".into()), Some(run), platform),
            PathBuf::from("/scratch/dl")
        );
    }

    #[test]
    fn an_automated_run_saves_inside_its_profile() {
        assert_eq!(
            automated_folder(Some(Path::new("/tmp/mercury-browser-profile-x"))),
            PathBuf::from("/tmp/mercury-browser-profile-x/downloads")
        );
        let without_profile = automated_folder(None);
        assert!(without_profile.starts_with(std::env::temp_dir()));
        assert_ne!(without_profile, platform_downloads_folder());
    }
}
