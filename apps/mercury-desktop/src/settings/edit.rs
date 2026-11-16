//! Turning what the user does in the settings window into settings. Every value is checked
//! with the settings crate's own validators first; a refused value changes nothing and comes
//! back as a message for the field.

use std::path::PathBuf;

use mercury_settings::{
    DEFAULT_ENGINE_ID, MAX_CUSTOM_ENGINES, MAX_ENGINE_ID_BYTES, SearchEngine, Settings,
    SettingsError, StartupBehavior, Theme, homepage_url, is_plausible_language_tag,
};
use talaria_core::ZOOM_STEPS;

pub(crate) const STARTUP_CHOICES: [StartupBehavior; 3] = [
    StartupBehavior::NewTab,
    StartupBehavior::Homepage,
    StartupBehavior::RestoreSession,
];
pub(crate) const THEME_CHOICES: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];

/// The text fields of the window, by name.
pub(crate) const TEXT_FIELDS: [&str; 3] = ["homepage", "download-directory", "language"];

/// What a text field shows for the stored setting.
pub(crate) fn stored_text(settings: &Settings, name: &str) -> String {
    match name {
        "homepage" => settings.homepage.clone(),
        "download-directory" => settings
            .downloads
            .directory
            .as_ref()
            .map(|directory| directory.display().to_string())
            .unwrap_or_default(),
        "language" => settings.language.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

/// A warning for the engine form's template: searches over plain `http` can be read on the way.
pub(crate) fn template_warning(template: &str) -> Option<String> {
    template
        .trim()
        .to_ascii_lowercase()
        .starts_with("http:")
        .then(|| {
            "This address starts with http: what you search for could be read on the way. \
             Use https if the engine offers it."
                .to_owned()
        })
}

/// The zoom levels offered: the zoom steps, plus the current default if a hand-edited file
/// chose one between them.
pub(crate) fn zoom_choices(current: u16) -> Vec<u16> {
    let mut choices = ZOOM_STEPS.to_vec();
    if !choices.contains(&current) {
        choices.push(current);
        choices.sort_unstable();
    }
    choices
}

/// Applies a check box. Returns false for a name it does not know. HTTPS-only and search
/// suggestions are not among them: the window shows them switched off and disabled.
pub(crate) fn apply_toggle(settings: &mut Settings, name: &str, on: bool) -> bool {
    let field = match name {
        "show-home-button" => &mut settings.appearance.show_home_button,
        "show-bookmarks-bar" => &mut settings.appearance.show_bookmarks_bar,
        "block-ads-and-trackers" => &mut settings.privacy.block_ads_and_trackers,
        "block-popups" => &mut settings.privacy.block_popups,
        "clear-history-on-exit" => &mut settings.privacy.clear_on_exit.history,
        "clear-cookies-on-exit" => &mut settings.privacy.clear_on_exit.cookies,
        "clear-cache-on-exit" => &mut settings.privacy.clear_on_exit.cache,
        "ask-where-to-save" => &mut settings.downloads.ask_where_to_save,
        "offer-to-save-passwords" => &mut settings.passwords.offer_to_save,
        "experimental-web-features" => &mut settings.web.experimental_web_features,
        "gpu-canvas" => &mut settings.web.gpu_canvas,
        _ => return false,
    };
    *field = on;
    true
}

/// Applies a choice from a list. Returns false for a name or index it does not know.
pub(crate) fn apply_choice(settings: &mut Settings, name: &str, index: usize) -> bool {
    match name {
        "startup" => STARTUP_CHOICES
            .get(index)
            .map(|&choice| settings.startup = choice)
            .is_some(),
        "theme" => THEME_CHOICES
            .get(index)
            .map(|&choice| settings.appearance.theme = choice)
            .is_some(),
        "search-engine" => settings
            .search
            .engines()
            .get(index)
            .map(|engine| settings.search.default_engine = engine.id.clone())
            .is_some(),
        "default-zoom" => zoom_choices(settings.appearance.default_zoom_percent)
            .get(index)
            .map(|&zoom| settings.appearance.default_zoom_percent = zoom)
            .is_some(),
        _ => false,
    }
}

/// Applies a text field, or explains why its text cannot be used.
pub(crate) fn apply_text(settings: &mut Settings, name: &str, text: &str) -> Result<(), String> {
    let text = text.trim();
    match name {
        "homepage" => {
            settings.homepage = home_page(text).ok_or_else(|| {
                "Enter an http, https, file or about address, without a user name or password."
                    .to_owned()
            })?;
        }
        "language" => {
            settings.language = if text.is_empty() {
                None
            } else if is_plausible_language_tag(text) {
                Some(text.to_owned())
            } else {
                return Err("Enter a language tag such as en, en-GB or pt-BR.".to_owned());
            };
        }
        "download-directory" => {
            let directory = (!text.is_empty()).then(|| PathBuf::from(text));
            // The crate's own check of the directory is part of `validated`.
            let mut candidate = settings.clone();
            candidate.downloads.directory = directory.clone();
            let (_, repairs) = candidate.validated();
            if repairs
                .iter()
                .any(|repair| repair.field == "downloads.directory")
            {
                return Err("Enter a full path, starting with /.".to_owned());
            }
            settings.downloads.directory = directory;
        }
        _ => return Err(format!("Unknown setting {name}.")),
    }
    Ok(())
}

/// The stored form of a typed home page: empty, or what `homepage_url` accepts; an address
/// typed without a scheme (`example.org`, `localhost:8080`) is tried as `https://`, as the
/// address bar does.
fn home_page(text: &str) -> Option<String> {
    let has_scheme = text.contains("://") || text.starts_with("about:");
    homepage_url(text)
        .filter(|_| has_scheme || text.is_empty())
        .or_else(|| {
            (!has_scheme)
                .then(|| homepage_url(&format!("https://{text}")))
                .flatten()
        })
}

/// The engine the form describes, checked, or why it cannot be saved. `editing` is the id of
/// the engine being edited, `None` when adding one.
pub(crate) fn engine_from_form(
    settings: &Settings,
    editing: Option<&str>,
    name: &str,
    keyword: &str,
    url_template: &str,
) -> Result<SearchEngine, String> {
    let engine = SearchEngine {
        id: editing.map_or_else(|| new_engine_id(settings, name), str::to_owned),
        name: name.trim().to_owned(),
        keyword: keyword.trim().to_owned(),
        url_template: url_template.trim().to_owned(),
        suggest_template: None,
    };
    engine
        .validate()
        .map_err(|error| sentence(&reason(error)))?;
    if !engine.keyword.is_empty() {
        let wanted = engine.keyword.to_lowercase();
        if let Some(owner) = settings.search.engines().into_iter().find(|other| {
            Some(other.id.as_str()) != editing && other.keyword.to_lowercase() == wanted
        }) {
            return Err(format!(
                "The keyword {} is already used by {}.",
                engine.keyword, owner.name
            ));
        }
    }
    // The crate's last word: an engine it would drop on loading is never saved.
    let mut candidate = settings.clone();
    save_engine(&mut candidate, engine.clone());
    if !candidate
        .search
        .engines()
        .iter()
        .any(|usable| usable.id == engine.id)
    {
        return Err("This search engine cannot be used.".to_owned());
    }
    Ok(engine)
}

fn reason(error: SettingsError) -> String {
    match error {
        SettingsError::InvalidSearchEngine { reason, .. } => reason,
        other => other.to_string(),
    }
}

/// `reason` as a sentence: capital first letter, full stop.
fn sentence(reason: &str) -> String {
    let mut characters = reason.chars();
    let mut text = characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect::<String>())
        .unwrap_or_default();
    if !text.ends_with('.') {
        text.push('.');
    }
    text
}

/// `custom-<name in a-z, 0-9 and ->`, numbered if that is taken.
fn new_engine_id(settings: &Settings, name: &str) -> String {
    let mut slug = String::new();
    for character in name.trim().to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let mut base = if slug.is_empty() {
        "custom".to_owned()
    } else {
        format!("custom-{slug}")
    };
    // Room for a "-<number>" suffix.
    base.truncate(MAX_ENGINE_ID_BYTES - 6);
    let taken = settings
        .search
        .engines()
        .into_iter()
        .map(|engine| engine.id)
        .chain(
            settings
                .search
                .custom_engines
                .iter()
                .map(|engine| engine.id.clone()),
        )
        .collect::<Vec<_>>();
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|number| format!("{base}-{number}"))
        .find(|id| !taken.contains(id))
        .unwrap_or(base)
}

/// Adds the engine, or replaces the one with its id.
pub(crate) fn save_engine(settings: &mut Settings, engine: SearchEngine) {
    let custom = &mut settings.search.custom_engines;
    match custom.iter_mut().find(|existing| existing.id == engine.id) {
        Some(existing) => *existing = engine,
        None => custom.push(engine),
    }
}

/// Removes a custom engine; if it was the chosen one, DuckDuckGo is chosen again.
pub(crate) fn remove_engine(settings: &mut Settings, id: &str) {
    settings
        .search
        .custom_engines
        .retain(|engine| engine.id != id);
    if settings.search.default_engine == id {
        settings.search.default_engine = DEFAULT_ENGINE_ID.to_owned();
    }
}

/// Whether another custom engine can be added.
pub(crate) fn can_add_engine(settings: &Settings) -> bool {
    settings.search.custom_engines.len() < MAX_CUSTOM_ENGINES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zoom_choices_are_the_steps_plus_an_odd_current_value() {
        assert_eq!(zoom_choices(100), ZOOM_STEPS.to_vec());
        let with_odd = zoom_choices(125);
        assert_eq!(with_odd.len(), ZOOM_STEPS.len() + 1);
        assert!(with_odd.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(with_odd.contains(&125));
    }

    #[test]
    fn check_boxes_set_their_settings() {
        let mut settings = Settings::default();
        for (name, read) in [
            (
                "show-home-button",
                (|s: &Settings| s.appearance.show_home_button) as fn(&Settings) -> bool,
            ),
            ("show-bookmarks-bar", |s| s.appearance.show_bookmarks_bar),
            ("block-ads-and-trackers", |s| {
                s.privacy.block_ads_and_trackers
            }),
            ("block-popups", |s| s.privacy.block_popups),
            ("clear-history-on-exit", |s| s.privacy.clear_on_exit.history),
            ("clear-cookies-on-exit", |s| s.privacy.clear_on_exit.cookies),
            ("clear-cache-on-exit", |s| s.privacy.clear_on_exit.cache),
            ("ask-where-to-save", |s| s.downloads.ask_where_to_save),
            ("offer-to-save-passwords", |s| s.passwords.offer_to_save),
            ("experimental-web-features", |s| {
                s.web.experimental_web_features
            }),
            ("gpu-canvas", |s| s.web.gpu_canvas),
        ] {
            for value in [true, false] {
                assert!(apply_toggle(&mut settings, name, value), "{name}");
                assert_eq!(read(&settings), value, "{name}");
            }
        }
        assert!(!apply_toggle(&mut settings, "no-such-setting", true));
        assert!(!settings.privacy.https_only);
    }

    #[test]
    fn https_only_and_suggestions_cannot_be_turned_on_from_the_window() {
        let mut settings = Settings::default();

        assert!(!apply_toggle(&mut settings, "https-only", true));
        assert!(!apply_toggle(&mut settings, "search-suggestions", true));
        assert!(!settings.privacy.https_only);
        assert!(!settings.search.suggestions);
    }

    #[test]
    fn choices_set_start_up_theme_engine_and_zoom() {
        let mut settings = Settings::default();

        assert!(apply_choice(&mut settings, "startup", 1));
        assert_eq!(settings.startup, StartupBehavior::Homepage);
        assert!(apply_choice(&mut settings, "theme", 2));
        assert_eq!(settings.appearance.theme, Theme::Dark);
        let startpage = settings
            .search
            .engines()
            .iter()
            .position(|engine| engine.id == "startpage")
            .expect("a built-in engine");
        assert!(apply_choice(&mut settings, "search-engine", startpage));
        assert_eq!(settings.search.default_engine, "startpage");
        let index_of_150 = zoom_choices(100)
            .iter()
            .position(|&zoom| zoom == 150)
            .expect("a zoom step");
        assert!(apply_choice(&mut settings, "default-zoom", index_of_150));
        assert_eq!(settings.appearance.default_zoom_percent, 150);

        let before = settings.clone();
        assert!(!apply_choice(&mut settings, "theme", 3));
        assert!(!apply_choice(&mut settings, "search-engine", 999));
        assert!(!apply_choice(&mut settings, "default-zoom", 999));
        assert!(!apply_choice(&mut settings, "nothing", 0));
        assert_eq!(settings, before);
    }

    #[test]
    fn a_home_page_is_stored_in_its_canonical_form() {
        let mut settings = Settings::default();

        assert_eq!(
            apply_text(&mut settings, "homepage", "https://Example.COM"),
            Ok(())
        );
        assert_eq!(settings.homepage, "https://example.com/");
        assert_eq!(
            apply_text(&mut settings, "homepage", "example.org/start"),
            Ok(())
        );
        assert_eq!(settings.homepage, "https://example.org/start");
        assert_eq!(apply_text(&mut settings, "homepage", "  "), Ok(()));
        assert_eq!(settings.homepage, "");
    }

    #[test]
    fn a_refused_home_page_changes_nothing_and_says_why() {
        let mut settings = Settings {
            homepage: "https://kept.test/".to_owned(),
            ..Settings::default()
        };

        for refused in [
            "ftp://files.test/",
            "https://user:secret@bank.test/",
            "javascript:alert(1)",
        ] {
            let problem = apply_text(&mut settings, "homepage", refused).expect_err(refused);
            assert!(!problem.is_empty());
            assert_eq!(settings.homepage, "https://kept.test/", "{refused}");
        }
    }

    #[test]
    fn a_language_must_look_like_a_language_tag() {
        let mut settings = Settings::default();

        assert_eq!(apply_text(&mut settings, "language", "en-GB"), Ok(()));
        assert_eq!(settings.language.as_deref(), Some("en-GB"));
        assert!(apply_text(&mut settings, "language", "english please").is_err());
        assert_eq!(settings.language.as_deref(), Some("en-GB"));
        assert_eq!(apply_text(&mut settings, "language", ""), Ok(()));
        assert_eq!(settings.language, None);
    }

    #[test]
    fn a_download_directory_must_be_absolute() {
        let mut settings = Settings::default();

        assert_eq!(
            apply_text(
                &mut settings,
                "download-directory",
                "/Users/me/Downloads/web"
            ),
            Ok(())
        );
        assert_eq!(
            settings.downloads.directory,
            Some(PathBuf::from("/Users/me/Downloads/web"))
        );
        assert!(apply_text(&mut settings, "download-directory", "Downloads").is_err());
        assert_eq!(
            settings.downloads.directory,
            Some(PathBuf::from("/Users/me/Downloads/web"))
        );
        assert_eq!(apply_text(&mut settings, "download-directory", ""), Ok(()));
        assert_eq!(settings.downloads.directory, None);
    }

    #[test]
    fn an_unknown_text_field_is_refused() {
        let mut settings = Settings::default();
        assert!(apply_text(&mut settings, "nothing", "x").is_err());
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn a_new_engine_gets_a_fresh_id_and_is_checked_by_the_crate() {
        let settings = Settings::default();

        let engine = engine_from_form(
            &settings,
            None,
            "Rust Docs",
            "@rs",
            "https://docs.rs/releases/search?query={searchTerms}",
        )
        .expect("a usable engine");

        assert_eq!(engine.id, "custom-rust-docs");
        assert_eq!(engine.name, "Rust Docs");
        assert_eq!(engine.keyword, "@rs");
        assert_eq!(engine.suggest_template, None);
        assert_eq!(engine.validate().ok(), Some(()));
    }

    #[test]
    fn a_new_engines_id_never_repeats_one_in_use() {
        let mut settings = Settings::default();
        let first = engine_from_form(
            &settings,
            None,
            "Docs",
            "",
            "https://a.test/?q={searchTerms}",
        )
        .expect("usable");
        save_engine(&mut settings, first);

        let second = engine_from_form(
            &settings,
            None,
            "Docs",
            "",
            "https://b.test/?q={searchTerms}",
        )
        .expect("usable");

        assert_eq!(second.id, "custom-docs-2");
    }

    #[test]
    fn a_name_without_letters_still_gives_a_valid_id() {
        let engine = engine_from_form(
            &Settings::default(),
            None,
            "日本語",
            "",
            "https://a.test/?q={searchTerms}",
        )
        .expect("usable");

        assert_eq!(engine.id, "custom");
    }

    #[test]
    fn the_form_reports_the_crates_reason() {
        let problem = engine_from_form(
            &Settings::default(),
            None,
            "Broken",
            "",
            "https://a.test/search",
        )
        .expect_err("no placeholder");

        assert_eq!(problem, "The template has no {searchTerms}.");
    }

    #[test]
    fn a_keyword_in_use_is_refused_whatever_its_case() {
        let problem = engine_from_form(
            &Settings::default(),
            None,
            "Mine",
            "@Wikipedia",
            "https://a.test/?q={searchTerms}",
        )
        .expect_err("the keyword is taken");

        assert_eq!(
            problem,
            "The keyword @Wikipedia is already used by Wikipedia (en)."
        );
    }

    #[test]
    fn editing_keeps_the_id_and_may_keep_the_keyword() {
        let mut settings = Settings::default();
        let engine = engine_from_form(
            &settings,
            None,
            "Docs",
            "@d",
            "https://a.test/?q={searchTerms}",
        )
        .expect("usable");
        save_engine(&mut settings, engine);

        let edited = engine_from_form(
            &settings,
            Some("custom-docs"),
            "Docs, renamed",
            "@d",
            "https://b.test/?q={searchTerms}",
        )
        .expect("usable");
        save_engine(&mut settings, edited);

        assert_eq!(settings.search.custom_engines.len(), 1);
        assert_eq!(settings.search.custom_engines[0].id, "custom-docs");
        assert_eq!(settings.search.custom_engines[0].name, "Docs, renamed");
    }

    #[test]
    fn removing_the_chosen_engine_chooses_duckduckgo_again() {
        let mut settings = Settings::default();
        let engine = engine_from_form(
            &settings,
            None,
            "Docs",
            "",
            "https://a.test/?q={searchTerms}",
        )
        .expect("usable");
        save_engine(&mut settings, engine);
        settings.search.default_engine = "custom-docs".to_owned();

        remove_engine(&mut settings, "custom-docs");

        assert!(settings.search.custom_engines.is_empty());
        assert_eq!(settings.search.default_engine, DEFAULT_ENGINE_ID);
    }

    #[test]
    fn built_in_engines_cannot_be_removed() {
        let mut settings = Settings::default();

        remove_engine(&mut settings, "google");

        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn a_text_field_shows_what_was_stored() {
        let mut settings = Settings::default();
        assert_eq!(stored_text(&settings, "homepage"), "");
        assert_eq!(stored_text(&settings, "download-directory"), "");
        assert_eq!(stored_text(&settings, "language"), "");

        apply_text(&mut settings, "homepage", "example.org").expect("accepted");
        apply_text(&mut settings, "download-directory", "/tmp/downloads").expect("accepted");
        apply_text(&mut settings, "language", " en-GB ").expect("accepted");

        assert_eq!(stored_text(&settings, "homepage"), "https://example.org/");
        assert_eq!(
            stored_text(&settings, "download-directory"),
            "/tmp/downloads"
        );
        assert_eq!(stored_text(&settings, "language"), "en-GB");
    }

    #[test]
    fn a_host_with_a_port_is_a_home_page() {
        let mut settings = Settings::default();

        assert_eq!(
            apply_text(&mut settings, "homepage", "localhost:8080"),
            Ok(())
        );
        assert_eq!(settings.homepage, "https://localhost:8080/");
        assert_eq!(
            apply_text(&mut settings, "homepage", "example.org:8443/a"),
            Ok(())
        );
        assert_eq!(settings.homepage, "https://example.org:8443/a");
        assert_eq!(apply_text(&mut settings, "homepage", "about:blank"), Ok(()));
        assert_eq!(settings.homepage, "about:blank");
    }

    #[test]
    fn a_plain_http_template_is_warned_about() {
        assert!(template_warning("http://search.test/?q={searchTerms}").is_some());
        assert!(template_warning("  HTTP://search.test/?q={searchTerms}").is_some());
        assert_eq!(
            template_warning("https://search.test/?q={searchTerms}"),
            None
        );
        assert_eq!(template_warning(""), None);
    }
}
