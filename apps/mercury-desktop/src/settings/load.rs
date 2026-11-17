//! What the browser does with each outcome of loading `settings.json`.

use std::path::Path;

use mercury_settings::{Loaded, Settings, SettingsError, SettingsStore, sweep_stale_temporaries};

/// The settings to start with, and what to do about the file.
#[derive(Debug, PartialEq)]
pub(crate) struct Startup {
    pub settings: Settings,
    /// Write the settings once now: the file needed repairs.
    pub save_now: bool,
    /// Whether this run may write the file at all.
    pub may_save: bool,
    /// One plain message for the user, shown in the window.
    pub notice: Option<String>,
}

/// Said when the stored choices cannot be used: the defaults clear nothing at quit.
pub(crate) const NOTHING_CLEARED: &str =
    "Nothing is cleared when Mercury Browser quits in this session.";

/// Interprets the result of `SettingsStore::load` as the crate's contract asks: repairs are
/// saved once; a damaged file gives the defaults, its backup is kept and the user is told; a
/// file from a newer version gives the defaults, is never written in this run, and the user is
/// told. A file that cannot be read at all is treated like a newer one, so it is not replaced.
pub(crate) fn interpret(result: Result<Loaded<Settings>, SettingsError>) -> Startup {
    match result {
        Ok(Loaded::Missing(settings)) => Startup {
            settings,
            save_now: false,
            may_save: true,
            notice: None,
        },
        Ok(Loaded::Ok { value, repairs }) => Startup {
            settings: value,
            save_now: !repairs.is_empty(),
            may_save: true,
            notice: None,
        },
        Ok(Loaded::Damaged { value, backup }) => Startup {
            settings: value,
            save_now: false,
            may_save: true,
            notice: Some(format!(
                "Your settings could not be read, so Mercury Browser is using its defaults. \
                 The old file was kept as {}.",
                backup.file_name().map_or_else(
                    || backup.display().to_string(),
                    |name| name.to_string_lossy().into_owned()
                )
            )),
        },
        Ok(Loaded::Newer { value, .. }) => Startup {
            settings: value,
            save_now: false,
            may_save: false,
            notice: Some(format!(
                "Your settings were saved by a newer version of Mercury Browser. This version \
                 is using its defaults and will not save changes, so your settings are kept \
                 for the newer version. {NOTHING_CLEARED}"
            )),
        },
        Err(error) => {
            log::warn!("settings: {error}");
            Startup {
                settings: Settings::default(),
                save_now: false,
                may_save: false,
                notice: Some(format!(
                    "Your settings could not be read, so Mercury Browser is using its defaults \
                     and will not save changes in this session. {NOTHING_CLEARED}"
                )),
            }
        }
    }
}

/// Sweeps the profile's stale temporary files and loads the settings. Call only while holding
/// the profile's lock. Without a profile the run uses the defaults and saves nothing.
pub(crate) fn load(profile_root: Option<&Path>) -> (Option<SettingsStore>, Startup) {
    let Some(root) = profile_root else {
        return (
            None,
            Startup {
                settings: Settings::default(),
                save_now: false,
                may_save: false,
                notice: None,
            },
        );
    };
    match sweep_stale_temporaries(root) {
        Ok(0) => {}
        Ok(removed) => log::info!("settings: removed {removed} stale temporary file(s)"),
        Err(error) => log::warn!("settings: could not sweep stale temporary files: {error}"),
    }
    let store = SettingsStore::new(root);
    let startup = interpret(store.load());
    (Some(store), startup)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mercury_settings::{Repair, Theme};

    use super::*;

    fn light() -> Settings {
        let mut settings = Settings::default();
        settings.appearance.theme = Theme::Light;
        settings
    }

    #[test]
    fn a_first_run_starts_from_the_defaults_quietly() {
        let startup = interpret(Ok(Loaded::Missing(Settings::default())));

        assert_eq!(
            startup,
            Startup {
                settings: Settings::default(),
                save_now: false,
                may_save: true,
                notice: None,
            }
        );
    }

    #[test]
    fn a_valid_file_is_used_and_not_rewritten() {
        let startup = interpret(Ok(Loaded::Ok {
            value: light(),
            repairs: Vec::new(),
        }));

        assert_eq!(startup.settings, light());
        assert!(!startup.save_now);
        assert!(startup.may_save);
        assert_eq!(startup.notice, None);
    }

    #[test]
    fn repairs_are_saved_once_without_bothering_the_user() {
        let startup = interpret(Ok(Loaded::Ok {
            value: light(),
            repairs: vec![Repair {
                field: "homepage",
                reason: "cleared".to_owned(),
            }],
        }));

        assert_eq!(startup.settings, light());
        assert!(startup.save_now);
        assert!(startup.may_save);
        assert_eq!(startup.notice, None);
    }

    #[test]
    fn a_damaged_file_gives_the_defaults_and_names_the_backup() {
        let startup = interpret(Ok(Loaded::Damaged {
            value: Settings::default(),
            backup: PathBuf::from("/profile/settings.json.damaged-1"),
        }));

        assert_eq!(startup.settings, Settings::default());
        assert!(!startup.save_now);
        assert!(startup.may_save);
        let notice = startup.notice.expect("the user is told");
        assert!(notice.contains("settings.json.damaged-1"), "{notice}");
        assert!(!notice.contains("/profile"), "{notice}");
    }

    #[test]
    fn a_newer_file_gives_the_defaults_and_is_never_saved() {
        let startup = interpret(Ok(Loaded::Newer {
            value: Settings::default(),
            found_version: 7,
        }));

        assert_eq!(startup.settings, Settings::default());
        assert!(!startup.save_now);
        assert!(!startup.may_save);
        let notice = startup.notice.expect("the user is told");
        assert!(notice.contains("newer version"), "{notice}");
        assert!(notice.contains(NOTHING_CLEARED), "{notice}");
    }

    #[test]
    fn an_unreadable_file_is_left_alone() {
        let startup = interpret(Err(SettingsError::UnplaceableQuery {
            id: "any".to_owned(),
        }));

        assert_eq!(startup.settings, Settings::default());
        assert!(!startup.may_save);
        let notice = startup.notice.expect("the user is told");
        assert!(notice.contains(NOTHING_CLEARED), "{notice}");
    }

    #[test]
    fn without_a_profile_nothing_is_read_or_saved() {
        let (store, startup) = load(None);

        assert!(store.is_none());
        assert_eq!(startup.settings, Settings::default());
        assert!(!startup.may_save);
        assert_eq!(startup.notice, None);
    }

    #[test]
    fn a_damaged_file_on_disk_is_moved_aside_and_the_user_told() {
        let directory = tempfile::tempdir().expect("a directory");
        std::fs::write(directory.path().join("settings.json"), "{ not json").expect("a file");

        let (store, startup) = load(Some(directory.path()));

        assert!(store.is_some());
        assert_eq!(startup.settings, Settings::default());
        assert!(directory.path().join("settings.json.damaged-1").exists());
        assert!(!directory.path().join("settings.json").exists());
        assert!(startup.notice.is_some());
    }
}
