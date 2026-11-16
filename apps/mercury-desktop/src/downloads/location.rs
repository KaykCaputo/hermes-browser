//! Where downloads go: the one accessor for the download folder and "ask where to save".

use std::cell::RefCell;
use std::path::PathBuf;

use mercury_settings::DownloadSettings;

thread_local! {
    /// `--download-dir`, a diagnostic that overrides the folder for one run.
    static FOLDER_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Where a new download is saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SaveLocation {
    Folder(PathBuf),
    /// Ask with the system's save panel. Not built yet: see `save_location`.
    Ask {
        suggested_folder: PathBuf,
    },
}

pub(crate) fn override_folder(folder: Option<PathBuf>) {
    FOLDER_OVERRIDE.with(|current| *current.borrow_mut() = folder);
}

/// The stored download settings. Until the settings work is wired into the application, these
/// are the defaults: the platform's Downloads folder, without asking.
fn download_settings() -> DownloadSettings {
    DownloadSettings::default()
}

pub(crate) fn save_location() -> SaveLocation {
    location_for(
        &download_settings(),
        FOLDER_OVERRIDE.with(|current| current.borrow().clone()),
        platform_downloads_folder(),
    )
}

fn platform_downloads_folder() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

/// The decision, without the environment. An override wins over everything (it is how
/// automated runs keep downloads out of the user's folders).
pub(crate) fn location_for(
    settings: &DownloadSettings,
    folder_override: Option<PathBuf>,
    platform_folder: PathBuf,
) -> SaveLocation {
    if let Some(folder) = folder_override {
        return SaveLocation::Folder(folder);
    }
    let folder = settings.directory.clone().unwrap_or(platform_folder);
    if settings.ask_where_to_save {
        SaveLocation::Ask {
            suggested_folder: folder,
        }
    } else {
        SaveLocation::Folder(folder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_folder_the_platform_folder_and_the_override() {
        let platform = PathBuf::from("/Users/me/Downloads");
        assert_eq!(
            location_for(&DownloadSettings::default(), None, platform.clone()),
            SaveLocation::Folder(platform.clone())
        );
        let stored = DownloadSettings {
            directory: Some("/Volumes/Data/Incoming".into()),
            ask_where_to_save: false,
        };
        assert_eq!(
            location_for(&stored, None, platform.clone()),
            SaveLocation::Folder("/Volumes/Data/Incoming".into())
        );
        let asking = DownloadSettings {
            ask_where_to_save: true,
            ..stored
        };
        assert_eq!(
            location_for(&asking, None, platform.clone()),
            SaveLocation::Ask {
                suggested_folder: "/Volumes/Data/Incoming".into()
            }
        );
        assert_eq!(
            location_for(&asking, Some("/scratch/dl".into()), platform),
            SaveLocation::Folder("/scratch/dl".into())
        );
    }
}
