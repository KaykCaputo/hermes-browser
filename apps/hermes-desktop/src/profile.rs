use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use talaria_core::BrowserController;
use tempfile::NamedTempFile;

#[derive(Default, Serialize, Deserialize)]
pub struct ProfileData {
    #[serde(default)]
    pub bookmarks: Vec<String>,
    #[serde(default)]
    tabs: Vec<String>,
    #[serde(default)]
    active: usize,
}

pub struct Profile {
    directory: PathBuf,
    _lock: File,
    pub data: ProfileData,
}

impl Profile {
    pub fn open(directory: &Path) -> io::Result<Self> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(directory)?;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(directory.join("profile.lock"))?;
        lock.try_lock()?;
        let data = match File::open(directory.join("browser.json")) {
            Ok(file) => serde_json::from_reader(file)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => ProfileData::default(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            directory: directory.to_owned(),
            _lock: lock,
            data,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn save(&self) -> io::Result<()> {
        let file = NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(&file, &self.data)?;
        file.as_file().sync_all()?;
        file.persist(self.directory.join("browser.json"))?;
        Ok(())
    }
}

impl ProfileData {
    pub fn restore(&self) -> BrowserController {
        BrowserController::from_session(&self.tabs, self.active)
    }

    pub fn remember_tabs(&mut self, controller: &BrowserController) {
        self.tabs = controller
            .tabs()
            .map(|tab| tab.state().url().to_owned())
            .collect();
        self.active = controller
            .tabs()
            .position(|tab| tab.id() == controller.active_tab_id())
            .unwrap_or_default();
    }
}

pub fn profile_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::home_dir().map(|home| home.join(".local/state")));
    base.map(|base| base.join("hermes-browser"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_order_selection_and_bookmarks_after_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let mut profile = Profile::open(dir.path()).unwrap();
        assert!(Profile::open(dir.path()).is_err());
        let mut tabs = BrowserController::default();
        tabs.queue_navigation("https://example.org/").unwrap();
        tabs.open_tab();
        tabs.select_tab_at(0);
        profile.data.remember_tabs(&tabs);
        profile.data.bookmarks.push(tabs.state().url().to_owned());
        profile.save().unwrap();
        drop(profile);
        let profile = Profile::open(dir.path()).unwrap();
        let restored = profile.data.restore();
        assert_eq!(restored.tabs().len(), 2);
        assert_eq!(restored.state().url(), "https://example.org/");
        assert_eq!(profile.data.bookmarks, ["https://example.org/"]);
    }
}
