//! What each setting does to the browser, as pure functions where possible: the controller
//! (search, default zoom, the colour scheme pages see), the engine's switches and the launch
//! options.

use mercury_settings::{
    ClearOnExit, SearchSettings, Settings, SettingsError, StartupBehavior, Theme, WebSettings,
};
use talaria_core::{BrowserController, ColorScheme, SearchUrlBuilder};
use talaria_servo::{ExitClearing, NEW_TAB_URL};

use crate::cli::LaunchOptions;

/// The colour scheme pages are told to use whatever the system's is, or `None` to follow it.
pub(crate) fn forced_color_scheme(theme: Theme) -> Option<ColorScheme> {
    match theme {
        Theme::System => None,
        Theme::Light => Some(ColorScheme::Light),
        Theme::Dark => Some(ColorScheme::Dark),
    }
}

/// Address-bar searches: a keyword as the first word chooses its engine, anything else goes
/// to the chosen engine (DuckDuckGo if that engine no longer exists). The URL is always built
/// by `SearchEngine::search_url`.
pub(crate) fn search_url_builder(search: &SearchSettings) -> SearchUrlBuilder {
    let search = search.clone();
    SearchUrlBuilder::new(move |text| {
        let (engine, query) = search
            .match_keyword(text)
            .unwrap_or_else(|| (search.default_engine(), text));
        engine
            .search_url(query)
            .map_err(|error| search_problem(&error))
    })
}

fn search_problem(error: &SettingsError) -> String {
    error.to_string()
}

/// What the engine clears at quit. History is not the engine's: see `exit::clear_history_on_exit`.
pub(crate) fn exit_clearing(clear: &ClearOnExit) -> ExitClearing {
    ExitClearing {
        cookies_and_site_data: clear.cookies,
        cache: clear.cache,
    }
}

/// The launch options for this run: a feature runs only if both the stored setting and the
/// command line allow it, so a `--disable-…` option turns it off for the run whatever the
/// setting says. Read once at start: these apply at the next start.
pub(crate) fn launch_options(options: LaunchOptions, web: &WebSettings) -> LaunchOptions {
    LaunchOptions {
        gpu_canvas: options.gpu_canvas && web.gpu_canvas,
        experimental_web_features: options.experimental_web_features
            && web.experimental_web_features,
        ..options
    }
}

/// The line under the new-tab search field.
pub(crate) fn new_tab_hint(engine_name: &str) -> String {
    format!("Searches go to {engine_name}.")
}

/// Where the home button goes: the home page, or a new-tab page when there is none.
pub(crate) fn home_address(settings: &Settings) -> String {
    if settings.homepage.is_empty() {
        NEW_TAB_URL.to_owned()
    } else {
        settings.homepage.clone()
    }
}

/// The page the first tab opens at start-up when the setting asks for the home page and the
/// command line names none. Restoring the previous session is the session work's.
pub(crate) fn start_up_address(
    settings: &Settings,
    command_line_urls: &[String],
) -> Option<String> {
    (settings.startup == StartupBehavior::Homepage
        && command_line_urls.is_empty()
        && !settings.homepage.is_empty())
    .then(|| settings.homepage.clone())
}

/// Everything the controller takes from the settings. Safe to repeat: it changes only what
/// differs.
pub(crate) fn apply_to_controller(controller: &mut BrowserController, settings: &Settings) {
    controller.set_search_url_builder(search_url_builder(&settings.search));
    controller.set_default_zoom_percent(settings.appearance.default_zoom_percent);
    controller.set_forced_color_scheme(forced_color_scheme(settings.appearance.theme));
}

#[cfg(test)]
mod tests {
    use mercury_settings::SearchEngine;
    use talaria_core::{BrowserCommand, BrowserEvent, TabCommand};

    use super::*;

    fn search_with(engine_id: &str) -> SearchSettings {
        SearchSettings {
            default_engine: engine_id.to_owned(),
            ..SearchSettings::default()
        }
    }

    fn controller_searching_with(search: &SearchSettings) -> BrowserController {
        let mut controller = BrowserController::default();
        controller.set_search_url_builder(search_url_builder(search));
        controller
    }

    fn navigate(controller: &mut BrowserController, input: &str) -> String {
        controller.queue_navigation(input).expect("an address");
        controller.active_state().url().to_owned()
    }

    #[test]
    fn searches_go_to_the_chosen_engine() {
        let mut controller = controller_searching_with(&search_with("startpage"));

        assert_eq!(
            navigate(&mut controller, "rust book"),
            "https://www.startpage.com/sp/search?query=rust+book"
        );
    }

    #[test]
    fn a_keyword_chooses_its_engine_for_one_search() {
        let mut controller = controller_searching_with(&search_with("startpage"));

        assert_eq!(
            navigate(&mut controller, "@wikipedia rust book"),
            "https://en.wikipedia.org/w/index.php?title=Special:Search&search=rust+book"
        );
        assert_eq!(
            navigate(&mut controller, "@WIKIPEDIA   ownership"),
            "https://en.wikipedia.org/w/index.php?title=Special:Search&search=ownership"
        );
    }

    #[test]
    fn a_custom_engines_keyword_works_too() {
        let mut search = search_with("duckduckgo");
        search.custom_engines.push(SearchEngine {
            id: "docs".to_owned(),
            name: "Docs".to_owned(),
            keyword: "d".to_owned(),
            url_template: "https://docs.test/search?q={searchTerms}".to_owned(),
            suggest_template: None,
        });
        let mut controller = controller_searching_with(&search);

        assert_eq!(
            navigate(&mut controller, "d vec push"),
            "https://docs.test/search?q=vec+push"
        );
    }

    #[test]
    fn an_address_that_starts_like_a_keyword_is_still_an_address() {
        let mut search = search_with("startpage");
        search.custom_engines.push(SearchEngine {
            id: "w".to_owned(),
            name: "W".to_owned(),
            keyword: "w".to_owned(),
            url_template: "https://w.test/?q={searchTerms}".to_owned(),
            suggest_template: None,
        });
        let mut controller = controller_searching_with(&search);

        assert_eq!(
            navigate(&mut controller, "w.org/rust book"),
            "https://w.org/rust%20book"
        );
        assert_eq!(
            navigate(&mut controller, "@wikipedia.org"),
            "https://wikipedia.org/"
        );
    }

    #[test]
    fn a_keyword_alone_is_searched_for_with_the_chosen_engine() {
        let mut controller = controller_searching_with(&search_with("startpage"));

        assert_eq!(
            navigate(&mut controller, "@wikipedia"),
            "https://www.startpage.com/sp/search?query=%40wikipedia"
        );
    }

    #[test]
    fn an_engine_that_no_longer_exists_falls_back_to_duckduckgo() {
        let mut controller = controller_searching_with(&search_with("removed-engine"));

        assert_eq!(
            navigate(&mut controller, "rust book"),
            "https://duckduckgo.com/?q=rust+book"
        );
    }

    #[test]
    fn a_query_an_engine_cannot_place_is_reported() {
        let mut search = search_with("path");
        search.custom_engines.push(SearchEngine {
            id: "path".to_owned(),
            name: "Path".to_owned(),
            keyword: String::new(),
            url_template: "https://path.test/s/{searchTerms}".to_owned(),
            suggest_template: None,
        });
        // The address bar never sends `..` here (it is taken for a host name), so the builder
        // is asked directly.
        let error = search_url_builder(&search)
            .build("..")
            .expect_err("`..` cannot fill a path segment");

        assert!(error.to_string().contains("cannot be placed"), "{error}");
    }

    #[test]
    fn themes_force_a_scheme_or_follow_the_system() {
        assert_eq!(forced_color_scheme(Theme::System), None);
        assert_eq!(forced_color_scheme(Theme::Light), Some(ColorScheme::Light));
        assert_eq!(forced_color_scheme(Theme::Dark), Some(ColorScheme::Dark));
    }

    #[test]
    fn clearing_at_exit_hands_cookies_and_cache_to_the_engine() {
        assert_eq!(
            exit_clearing(&ClearOnExit::default()),
            ExitClearing::default()
        );
        assert_eq!(
            exit_clearing(&ClearOnExit {
                history: true,
                cookies: true,
                cache: false,
            }),
            ExitClearing {
                cookies_and_site_data: true,
                cache: false,
            }
        );
        assert_eq!(
            exit_clearing(&ClearOnExit {
                history: false,
                cookies: false,
                cache: true,
            }),
            ExitClearing {
                cookies_and_site_data: false,
                cache: true,
            }
        );
    }

    #[test]
    fn the_command_line_can_only_turn_features_off() {
        let on = WebSettings {
            experimental_web_features: true,
            gpu_canvas: true,
        };
        let off = WebSettings {
            experimental_web_features: false,
            gpu_canvas: false,
        };
        let disabled_on_the_command_line = LaunchOptions {
            gpu_canvas: false,
            experimental_web_features: false,
            ..LaunchOptions::default()
        };

        let both_on = launch_options(LaunchOptions::default(), &on);
        assert!(both_on.gpu_canvas && both_on.experimental_web_features);
        let setting_off = launch_options(LaunchOptions::default(), &off);
        assert!(!setting_off.gpu_canvas && !setting_off.experimental_web_features);
        let flag_off = launch_options(disabled_on_the_command_line, &on);
        assert!(!flag_off.gpu_canvas && !flag_off.experimental_web_features);
    }

    #[test]
    fn the_launch_options_keep_everything_else() {
        let options = LaunchOptions {
            urls: vec!["a.test".to_owned()],
            background: true,
            ..LaunchOptions::default()
        };

        let effective = launch_options(options, &WebSettings::default());

        assert_eq!(effective.urls, vec!["a.test".to_owned()]);
        assert!(effective.background);
    }

    #[test]
    fn the_new_tab_hint_names_the_engine() {
        assert_eq!(new_tab_hint("Startpage"), "Searches go to Startpage.");
    }

    #[test]
    fn the_home_button_opens_the_home_page_or_a_new_tab_page() {
        assert_eq!(home_address(&Settings::default()), "about:blank");

        let settings = Settings {
            homepage: "https://home.test/".to_owned(),
            ..Settings::default()
        };
        assert_eq!(home_address(&settings), "https://home.test/");
    }

    #[test]
    fn the_home_page_opens_at_start_only_when_asked_and_nothing_else_was() {
        let mut settings = Settings {
            homepage: "https://home.test/".to_owned(),
            ..Settings::default()
        };
        assert_eq!(start_up_address(&settings, &[]), None);

        settings.startup = StartupBehavior::Homepage;
        assert_eq!(
            start_up_address(&settings, &[]).as_deref(),
            Some("https://home.test/")
        );
        assert_eq!(start_up_address(&settings, &["a.test".to_owned()]), None);

        settings.homepage.clear();
        assert_eq!(start_up_address(&settings, &[]), None);

        settings.homepage = "https://home.test/".to_owned();
        settings.startup = StartupBehavior::RestoreSession;
        assert_eq!(start_up_address(&settings, &[]), None);
    }

    #[test]
    fn the_controller_takes_search_zoom_and_scheme_from_the_settings() {
        let mut controller = BrowserController::default();
        let tab_id = controller.active_tab_id();
        controller.handle_event(tab_id, BrowserEvent::AddressChanged { url: String::new() });
        let mut settings = Settings::default();
        settings.search.default_engine = "startpage".to_owned();
        settings.appearance.default_zoom_percent = 120;
        settings.appearance.theme = Theme::Dark;

        apply_to_controller(&mut controller, &settings);
        apply_to_controller(&mut controller, &settings);

        assert_eq!(controller.default_zoom_percent(), 120);
        assert_eq!(controller.color_scheme(), ColorScheme::Dark);
        let queued = std::iter::from_fn(|| controller.next_command()).collect::<Vec<_>>();
        assert_eq!(
            queued,
            vec![
                TabCommand::new(tab_id, BrowserCommand::SetPageZoom { percent: 120 }),
                TabCommand::new(tab_id, BrowserCommand::SetColorScheme(ColorScheme::Dark)),
            ]
        );
        assert_eq!(
            navigate(&mut controller, "rust"),
            "https://www.startpage.com/sp/search?query=rust"
        );
    }
}
