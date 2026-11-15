use std::fs;

use mercury_settings::{
    ClearOnExit, DEFAULT_ENGINE_ID, Loaded, Settings, SettingsStore, StartupBehavior, Theme,
};

const DEFAULT_FILE: &str = r#"{
  "format_version": 1,
  "startup": "new_tab",
  "homepage": "",
  "search": {
    "default_engine": "duckduckgo",
    "custom_engines": [],
    "suggestions": false
  },
  "downloads": {
    "directory": null,
    "ask_where_to_save": false
  },
  "appearance": {
    "theme": "system",
    "default_zoom_percent": 100,
    "show_bookmarks_bar": true,
    "show_home_button": false
  },
  "privacy": {
    "block_ads_and_trackers": true,
    "https_only": false,
    "block_popups": true,
    "global_privacy_control": true,
    "clear_on_exit": {
      "history": false,
      "cookies": false,
      "cache": false
    }
  },
  "passwords": {
    "offer_to_save": true,
    "never_save_origins": []
  },
  "web": {
    "experimental_web_features": true,
    "gpu_canvas": true
  },
  "language": null
}
"#;

fn load_text(text: &str) -> Loaded<Settings> {
    let dir = tempfile::tempdir().expect("a temporary directory");
    fs::write(dir.path().join("settings.json"), text).expect("seed the file");
    SettingsStore::new(dir.path())
        .load()
        .expect("loading should not fail")
}

#[test]
fn the_defaults_are_the_documented_ones() {
    let settings = Settings::default();

    assert_eq!(settings.startup, StartupBehavior::NewTab);
    assert_eq!(settings.homepage, "");
    assert_eq!(settings.search.default_engine, DEFAULT_ENGINE_ID);
    assert!(settings.search.custom_engines.is_empty());
    assert!(!settings.search.suggestions, "suggestions leak keystrokes");
    assert_eq!(settings.downloads.directory, None);
    assert!(!settings.downloads.ask_where_to_save);
    assert_eq!(settings.appearance.theme, Theme::System);
    assert_eq!(settings.appearance.default_zoom_percent, 100);
    assert!(settings.appearance.show_bookmarks_bar);
    assert!(!settings.appearance.show_home_button);
    assert!(settings.privacy.block_ads_and_trackers);
    assert!(!settings.privacy.https_only);
    assert!(settings.privacy.block_popups);
    assert!(settings.privacy.global_privacy_control);
    assert_eq!(
        settings.privacy.clear_on_exit,
        ClearOnExit {
            history: false,
            cookies: false,
            cache: false
        }
    );
    assert!(settings.passwords.offer_to_save);
    assert!(settings.passwords.never_save_origins.is_empty());
    assert!(settings.web.experimental_web_features);
    assert!(settings.web.gpu_canvas);
    assert_eq!(settings.language, None);
}

#[test]
fn the_defaults_survive_a_serde_round_trip() {
    let json = serde_json::to_string(&Settings::default()).expect("serialise");
    let back: Settings = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(back, Settings::default());
}

#[test]
fn the_default_file_is_pretty_stable_versioned_and_ends_with_a_newline() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let store = SettingsStore::new(dir.path());
    let path = dir.path().join("settings.json");

    store.save(&Settings::default()).expect("save");
    let first = fs::read_to_string(&path).expect("read");
    store.save(&Settings::default()).expect("save again");
    let second = fs::read_to_string(&path).expect("read again");

    assert_eq!(first, DEFAULT_FILE);
    assert_eq!(
        first, second,
        "saving the same settings twice gives the same bytes"
    );
}

#[test]
fn what_is_saved_is_what_is_loaded() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let store = SettingsStore::new(dir.path());
    let mut settings = Settings {
        startup: StartupBehavior::RestoreSession,
        ..Settings::default()
    };
    settings.homepage = "https://example.org/start".to_owned();
    settings.appearance.theme = Theme::Dark;
    settings.appearance.default_zoom_percent = 125;
    settings.privacy.https_only = true;
    settings.privacy.clear_on_exit.cookies = true;
    settings.passwords.never_save_origins = vec!["https://bank.example".to_owned()];
    settings.language = Some("pt-BR".to_owned());

    store.save(&settings).expect("save");

    assert_eq!(
        store.load().expect("load"),
        Loaded::Ok {
            value: settings,
            repairs: Vec::new()
        }
    );
}

#[test]
fn an_empty_object_gives_the_defaults() {
    assert_eq!(
        load_text("{}"),
        Loaded::Ok {
            value: Settings::default(),
            repairs: Vec::new()
        }
    );
}

#[test]
fn a_partial_file_keeps_what_it_has_and_defaults_the_rest() {
    let loaded = load_text(
        r#"{"format_version": 1, "homepage": "https://example.org/",
            "appearance": {"theme": "dark"}, "privacy": {"clear_on_exit": {"cache": true}}}"#,
    );

    let mut expected = Settings {
        homepage: "https://example.org/".to_owned(),
        ..Settings::default()
    };
    expected.appearance.theme = Theme::Dark;
    expected.privacy.clear_on_exit.cache = true;
    assert_eq!(
        loaded,
        Loaded::Ok {
            value: expected,
            repairs: Vec::new()
        }
    );
}

#[test]
fn unknown_fields_are_ignored() {
    let loaded = load_text(
        r#"{"format_version": 1, "sidebar_width": 240, "homepage": "about:blank",
            "appearance": {"accent": "lavender", "theme": "light"}}"#,
    );

    let mut expected = Settings {
        homepage: "about:blank".to_owned(),
        ..Settings::default()
    };
    expected.appearance.theme = Theme::Light;
    assert_eq!(
        loaded,
        Loaded::Ok {
            value: expected,
            repairs: Vec::new()
        }
    );
}
