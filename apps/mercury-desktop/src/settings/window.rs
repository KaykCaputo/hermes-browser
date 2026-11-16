//! The settings window: a separate native window built from `SettingsWindow`, created when it
//! is opened and dropped when it is closed.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use mercury_settings::{Settings, Theme, builtin_engines};
use mercury_ui::{SearchEngineRow, SettingsWindow};
use slint::language::ColorScheme as SlintColorScheme;
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{SettingsHandle, edit};

/// The open settings window, if any, and the search engine being edited in it.
#[derive(Default)]
pub(crate) struct SettingsWindowSlot {
    pub(super) window: RefCell<Option<SettingsWindow>>,
    /// `Some(None)` while adding an engine, `Some(Some(id))` while editing one.
    editing: RefCell<Option<Option<String>>>,
}

impl SettingsWindowSlot {
    /// Hides and drops the window. Never call it from one of the window's own callbacks
    /// directly; `close_later` does that safely.
    pub(crate) fn close(&self) {
        if let Some(window) = self.window.borrow_mut().take()
            && let Err(error) = window.hide()
        {
            log::warn!("could not hide the settings window: {error}");
        }
        self.editing.borrow_mut().take();
    }
}

/// Opens the settings window, or brings it forward if it is open.
pub(crate) fn show(handle: &Rc<SettingsHandle>) {
    let slot = &handle.window;
    let visible = slot
        .window
        .borrow()
        .as_ref()
        .is_some_and(|window| window.window().is_visible());
    if visible {
        // Already open (Cmd+, again): bring it forward.
        if let Some(window) = slot.window.borrow().as_ref() {
            let _focused = window
                .window()
                .with_winit_window(|native| native.focus_window());
        }
        return;
    }
    slot.close();
    match SettingsWindow::new() {
        Ok(window) => {
            super::apply_chrome_theme(&window);
            connect(&window, Rc::downgrade(handle));
            fill_text_fields(&window, &handle.settings());
            *slot.window.borrow_mut() = Some(window);
        }
        Err(error) => {
            log::error!("could not create the settings window: {error}");
            return;
        }
    }
    refresh(handle);
    if let Some(window) = slot.window.borrow().as_ref()
        && let Err(error) = window.show()
    {
        log::error!("could not show the settings window: {error}");
    }
}

/// Shows the current settings in the window, if it is open. The text fields are filled only
/// when the window opens, so that nothing overwrites what the user is typing.
pub(crate) fn refresh(handle: &SettingsHandle) {
    if let Some(window) = handle.window.window.borrow().as_ref() {
        window.set_system_color_scheme(handle.system_color_scheme());
        fill(window, &handle.settings(), handle.save_problem());
    }
}

/// Every value the window shows except the text fields.
pub(crate) fn fill(window: &SettingsWindow, settings: &Settings, save_problem: Option<String>) {
    window.set_forced_color_scheme(match settings.appearance.theme {
        Theme::System => SlintColorScheme::Unknown,
        Theme::Light => SlintColorScheme::Light,
        Theme::Dark => SlintColorScheme::Dark,
    });
    window.set_save_problem(save_problem.unwrap_or_default().into());

    window.set_startup_index(index_of(&edit::STARTUP_CHOICES, &settings.startup));
    let engines = settings.search.engines();
    let builtin_ids = builtin_engines()
        .into_iter()
        .map(|engine| engine.id)
        .collect::<Vec<_>>();
    let chosen = settings.search.default_engine();
    window.set_engine_choices(strings(engines.iter().map(|engine| engine.name.clone())));
    window.set_engine_index(
        engines
            .iter()
            .position(|engine| engine.id == chosen.id)
            .map_or(0, to_i32),
    );
    window.set_engines(ModelRc::new(VecModel::from(
        engines
            .iter()
            .map(|engine| SearchEngineRow {
                id: engine.id.clone().into(),
                name: engine.name.clone().into(),
                keyword: engine.keyword.clone().into(),
                custom: !builtin_ids.contains(&engine.id),
            })
            .collect::<Vec<_>>(),
    )));
    window.set_can_add_engine(edit::can_add_engine(settings));
    window.set_search_suggestions(settings.search.suggestions);
    let zooms = edit::zoom_choices(settings.appearance.default_zoom_percent);
    window.set_zoom_choices(strings(zooms.iter().map(|zoom| format!("{zoom}%"))));
    window.set_zoom_index(
        zooms
            .iter()
            .position(|&zoom| zoom == settings.appearance.default_zoom_percent)
            .map_or(0, to_i32),
    );

    window.set_theme_index(index_of(&edit::THEME_CHOICES, &settings.appearance.theme));
    window.set_show_home_button(settings.appearance.show_home_button);
    window.set_show_bookmarks_bar(settings.appearance.show_bookmarks_bar);

    window.set_block_ads_and_trackers(settings.privacy.block_ads_and_trackers);
    window.set_block_popups(settings.privacy.block_popups);
    window.set_https_only(settings.privacy.https_only);
    window.set_clear_history_on_exit(settings.privacy.clear_on_exit.history);
    window.set_clear_cookies_on_exit(settings.privacy.clear_on_exit.cookies);
    window.set_clear_cache_on_exit(settings.privacy.clear_on_exit.cache);

    window.set_ask_where_to_save(settings.downloads.ask_where_to_save);

    window.set_offer_to_save_passwords(settings.passwords.offer_to_save);
    window.set_never_save_origins(strings(
        settings.passwords.never_save_origins.iter().cloned(),
    ));

    window.set_experimental_web_features(settings.web.experimental_web_features);
    window.set_gpu_canvas(settings.web.gpu_canvas);
}

pub(crate) fn fill_text_fields(window: &SettingsWindow, settings: &Settings) {
    for name in edit::TEXT_FIELDS {
        set_text_field(window, name, edit::stored_text(settings, name).into());
    }
}

fn set_text_field(window: &SettingsWindow, name: &str, text: SharedString) {
    match name {
        "homepage" => window.set_homepage(text),
        "download-directory" => window.set_download_directory(text),
        "language" => window.set_language(text),
        _ => log::warn!("settings window: unknown field {name}"),
    }
}

fn set_problem(window: &SettingsWindow, name: &str, problem: SharedString) {
    match name {
        "homepage" => window.set_homepage_problem(problem),
        "download-directory" => window.set_download_directory_problem(problem),
        "language" => window.set_language_problem(problem),
        _ => log::warn!("settings window: unknown field {name}"),
    }
}

fn text_field(window: &SettingsWindow, name: &str) -> SharedString {
    match name {
        "homepage" => window.get_homepage(),
        "download-directory" => window.get_download_directory(),
        "language" => window.get_language(),
        _ => SharedString::new(),
    }
}

/// Commits every text field's current text (see `commit_text`).
pub(crate) fn commit_all_texts(handle: &SettingsHandle, window: &SettingsWindow) {
    for name in edit::TEXT_FIELDS {
        let text = text_field(window, name);
        commit_text(handle, window, name, &text);
    }
}

/// Applies a text field's value. Accepted, the field shows the form that was stored; refused,
/// it keeps the text and shows why.
fn commit_text(handle: &SettingsHandle, window: &SettingsWindow, name: &str, text: &str) {
    let mut result = Ok(());
    handle.update(|settings| result = edit::apply_text(settings, name, text));
    match result {
        Ok(()) => {
            set_text_field(
                window,
                name,
                edit::stored_text(&handle.settings(), name).into(),
            );
            set_problem(window, name, SharedString::new());
        }
        Err(problem) => set_problem(window, name, problem.into()),
    }
}

fn connect(window: &SettingsWindow, handle: Weak<SettingsHandle>) {
    // menus lane: reinstall Mercury's menu when this window becomes key
    let toggled_handle = handle.clone();
    window.on_toggled(move |name, on| {
        if let Some(handle) = toggled_handle.upgrade() {
            handle.update(|settings| {
                if !edit::apply_toggle(settings, &name, on) {
                    log::warn!("settings window: unknown check box {name}");
                }
            });
        }
    });

    let chosen_handle = handle.clone();
    window.on_chosen(move |name, index| {
        let (Some(handle), Ok(index)) = (chosen_handle.upgrade(), usize::try_from(index)) else {
            return;
        };
        handle.update(|settings| {
            if !edit::apply_choice(settings, &name, index) {
                log::warn!("settings window: unknown choice {name} {index}");
            }
        });
    });

    let text_handle = handle.clone();
    let text_window = window.as_weak();
    window.on_text_committed(move |name, text| {
        if let (Some(handle), Some(window)) = (text_handle.upgrade(), text_window.upgrade()) {
            commit_text(&handle, &window, &name, &text);
        }
    });

    let commit_handle = handle.clone();
    let commit_window = window.as_weak();
    window.on_commit_texts(move || {
        if let (Some(handle), Some(window)) = (commit_handle.upgrade(), commit_window.upgrade()) {
            commit_all_texts(&handle, &window);
        }
    });

    // The native close button: keep what was typed, then hide as usual.
    let close_request_handle = handle.clone();
    let close_request_window = window.as_weak();
    window.window().on_close_requested(move || {
        if let (Some(handle), Some(window)) = (
            close_request_handle.upgrade(),
            close_request_window.upgrade(),
        ) {
            commit_all_texts(&handle, &window);
        }
        slint::CloseRequestResponse::HideWindow
    });

    let warning_window = window.as_weak();
    window.on_editor_url_edited(move |text| {
        if let Some(window) = warning_window.upgrade() {
            window.set_editor_warning(edit::template_warning(&text).unwrap_or_default().into());
        }
    });

    window.on_quit_browser(|| {
        // What the main window's quit shortcut does (`ShortcutAction::Quit`).
        if let Err(error) = slint::quit_event_loop() {
            log::warn!("could not quit: {error}");
        }
    });

    let add_handle = handle.clone();
    let add_window = window.as_weak();
    window.on_add_engine(move || {
        let (Some(handle), Some(window)) = (add_handle.upgrade(), add_window.upgrade()) else {
            return;
        };
        *handle.window.editing.borrow_mut() = Some(None);
        open_editor(&window, "", "", "", "");
    });

    let edit_handle = handle.clone();
    let edit_window = window.as_weak();
    window.on_edit_engine(move |id| {
        let (Some(handle), Some(window)) = (edit_handle.upgrade(), edit_window.upgrade()) else {
            return;
        };
        let settings = handle.settings();
        let Some(engine) = settings
            .search
            .custom_engines
            .iter()
            .find(|engine| engine.id == id.as_str())
        else {
            return;
        };
        *handle.window.editing.borrow_mut() = Some(Some(engine.id.clone()));
        open_editor(
            &window,
            &engine.id,
            &engine.name,
            &engine.keyword,
            &engine.url_template,
        );
    });

    let remove_handle = handle.clone();
    window.on_remove_engine(move |id| {
        if let Some(handle) = remove_handle.upgrade() {
            handle.update(|settings| edit::remove_engine(settings, &id));
        }
    });

    let save_handle = handle.clone();
    let save_window = window.as_weak();
    window.on_save_engine(move || {
        let (Some(handle), Some(window)) = (save_handle.upgrade(), save_window.upgrade()) else {
            return;
        };
        let Some(editing) = handle.window.editing.borrow().clone() else {
            return;
        };
        let checked = edit::engine_from_form(
            &handle.settings(),
            editing.as_deref(),
            &window.get_editor_name(),
            &window.get_editor_keyword(),
            &window.get_editor_url(),
        );
        match checked {
            Ok(engine) => {
                handle.window.editing.borrow_mut().take();
                window.set_editor_open(false);
                window.set_editor_problem(SharedString::new());
                handle.update(|settings| edit::save_engine(settings, engine));
            }
            Err(problem) => window.set_editor_problem(problem.into()),
        }
    });

    let cancel_handle = handle.clone();
    let cancel_window = window.as_weak();
    window.on_cancel_engine(move || {
        if let Some(handle) = cancel_handle.upgrade() {
            handle.window.editing.borrow_mut().take();
        }
        if let Some(window) = cancel_window.upgrade() {
            window.set_editor_open(false);
            window.set_editor_problem(SharedString::new());
        }
    });

    let never_save_handle = handle.clone();
    window.on_remove_never_save(move |index| {
        let (Some(handle), Ok(index)) = (never_save_handle.upgrade(), usize::try_from(index))
        else {
            return;
        };
        handle.update(|settings| {
            if index < settings.passwords.never_save_origins.len() {
                settings.passwords.never_save_origins.remove(index);
            }
        });
    });

    window.on_clear_browsing_data(clear_browsing_data);

    let close_handle = handle;
    window.on_close_settings(move || {
        let close_handle = close_handle.clone();
        // Dropping the window inside its own callback is not safe; do it right after.
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(handle) = close_handle.upgrade() {
                handle.close_settings_window();
            }
        });
    });
}

/// The history lane's seam: "Clear browsing data…" in the privacy section. The button stays
/// disabled (`clear-browsing-data-available` is false) until this opens that dialog.
fn clear_browsing_data() {
    log::info!("clearing browsing data is not available yet");
}

fn open_editor(window: &SettingsWindow, id: &str, name: &str, keyword: &str, url: &str) {
    window.set_editor_id(id.into());
    window.set_editor_name(name.into());
    window.set_editor_keyword(keyword.into());
    window.set_editor_url(url.into());
    window.set_editor_problem(SharedString::new());
    window.set_editor_warning(edit::template_warning(url).unwrap_or_default().into());
    window.set_editor_open(true);
}

fn strings(values: impl Iterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values.map(SharedString::from).collect::<Vec<_>>(),
    ))
}

fn index_of<T: PartialEq>(choices: &[T], value: &T) -> i32 {
    choices
        .iter()
        .position(|choice| choice == value)
        .map_or(0, to_i32)
}

fn to_i32(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    use mercury_settings::SearchEngine;
    use slint::platform::PointerEventButton;

    use super::super::tests::init_slint;
    use super::*;

    fn open(handle: &Rc<SettingsHandle>) -> SettingsWindow {
        show(handle);
        let window = handle
            .window
            .window
            .borrow()
            .as_ref()
            .expect("the window is open")
            .clone_strong();
        window
            .window()
            .set_size(slint::LogicalSize::new(900.0, 1400.0));
        window
    }

    /// The control labelled `label`; the text beside a control carries the same label.
    fn element(window: &SettingsWindow, label: &str) -> ElementHandle {
        ElementHandle::find_by_accessible_label(window, label)
            .find(|element| element.accessible_role() != Some(AccessibleRole::Text))
            .unwrap_or_else(|| panic!("no control labelled {label:?}"))
    }

    #[test]
    fn a_check_box_follows_the_setting_after_the_user_has_clicked_it() {
        init_slint();
        let handle = SettingsHandle::start(None);
        let window = open(&handle);
        element(&window, "Privacy and security").invoke_accessible_default_action();

        element(&window, "Block ads and trackers").mock_single_click(PointerEventButton::Left);
        assert!(!handle.settings().privacy.block_ads_and_trackers);
        assert_eq!(
            element(&window, "Block ads and trackers").accessible_checked(),
            Some(false)
        );

        handle.update(|settings| settings.privacy.block_ads_and_trackers = true);

        assert_eq!(
            element(&window, "Block ads and trackers").accessible_checked(),
            Some(true)
        );
    }

    #[test]
    fn the_engine_choice_follows_when_the_chosen_engine_is_removed() {
        init_slint();
        let handle = SettingsHandle::start(None);
        handle.update(|settings| {
            settings.search.custom_engines.push(SearchEngine {
                id: "custom-docs".to_owned(),
                name: "Docs".to_owned(),
                keyword: String::new(),
                url_template: "https://docs.test/?q={searchTerms}".to_owned(),
                suggest_template: None,
            });
            settings.search.default_engine = "custom-docs".to_owned();
        });
        let window = open(&handle);
        assert_eq!(
            element(&window, "Search engine")
                .accessible_value()
                .as_deref(),
            Some("Docs")
        );

        element(&window, "Remove Docs").mock_single_click(PointerEventButton::Left);

        assert_eq!(handle.settings().search.default_engine, "duckduckgo");
        assert_eq!(
            element(&window, "Search engine")
                .accessible_value()
                .as_deref(),
            Some("DuckDuckGo")
        );
    }

    #[test]
    fn a_typed_home_page_survives_a_section_change_in_its_stored_form() {
        init_slint();
        let handle = SettingsHandle::start(None);
        let window = open(&handle);

        element(&window, "Home page").set_accessible_value("example.org");
        assert_eq!(
            handle.settings().homepage,
            "",
            "nothing is saved while typing"
        );
        element(&window, "Appearance").mock_single_click(PointerEventButton::Left);
        element(&window, "General").mock_single_click(PointerEventButton::Left);

        assert_eq!(handle.settings().homepage, "https://example.org/");
        assert_eq!(
            element(&window, "Home page").accessible_value().as_deref(),
            Some("https://example.org/")
        );
        assert_eq!(window.get_homepage_problem(), "");
    }

    #[test]
    fn a_refused_home_page_keeps_its_text_and_says_why() {
        init_slint();
        let handle = SettingsHandle::start(None);
        let window = open(&handle);

        element(&window, "Home page").set_accessible_value("ftp://files.test/");
        element(&window, "Appearance").invoke_accessible_default_action();
        element(&window, "General").invoke_accessible_default_action();

        assert_eq!(handle.settings().homepage, "");
        assert_eq!(
            element(&window, "Home page").accessible_value().as_deref(),
            Some("ftp://files.test/")
        );
        assert_ne!(window.get_homepage_problem(), "");
    }

    #[test]
    fn sections_are_tabs_reachable_by_their_names() {
        init_slint();
        let handle = SettingsHandle::start(None);
        let window = open(&handle);
        let downloads = element(&window, "Downloads");

        assert_eq!(downloads.accessible_role(), Some(AccessibleRole::Tab));
        downloads.invoke_accessible_default_action();

        assert_eq!(window.get_section(), 3);
        assert_eq!(downloads.accessible_item_selected(), Some(true));
    }

    #[test]
    fn typed_text_is_kept_when_the_browser_window_closes_the_settings() {
        init_slint();
        let handle = SettingsHandle::start(None);
        let window = open(&handle);
        element(&window, "Home page").set_accessible_value("example.org");

        handle.close_settings_window();

        assert_eq!(handle.settings().homepage, "https://example.org/");
        assert!(handle.window.window.borrow().is_none());
    }

    #[test]
    fn typed_text_is_kept_when_the_native_close_button_is_used() {
        init_slint();
        let handle = SettingsHandle::start(None);
        let window = open(&handle);
        element(&window, "Home page").set_accessible_value("example.org");

        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::CloseRequested);

        assert_eq!(handle.settings().homepage, "https://example.org/");
        assert!(!window.window().is_visible());
    }

    #[test]
    fn typed_text_is_saved_at_quit() {
        init_slint();
        let directory = tempfile::tempdir().expect("a directory");
        let handle = SettingsHandle::start(Some(directory.path()));
        let window = open(&handle);
        element(&window, "Advanced").invoke_accessible_default_action();
        element(&window, "Language").set_accessible_value("pt-BR");
        drop(window);

        handle.finish_run(std::time::Instant::now() + super::super::FINISH_RUN_BUDGET);

        let saved = std::fs::read_to_string(directory.path().join("settings.json"))
            .expect("a settings file");
        assert!(saved.contains(r#""language": "pt-BR""#), "{saved}");
    }

    #[test]
    fn the_settings_window_takes_the_chrome_theme_and_follows_it() {
        init_slint();
        let handle = SettingsHandle::start(None);
        handle.update(|settings| settings.appearance.theme = mercury_settings::Theme::Light);
        let window = open(&handle);
        let theme =
            |window: &SettingsWindow| window.global::<mercury_ui::ChromePalette>().get_theme();
        assert_eq!(theme(&window), mercury_ui::ChromeTheme::Light);

        handle.update(|settings| settings.appearance.theme = mercury_settings::Theme::Dark);

        assert_eq!(theme(&window), mercury_ui::ChromeTheme::Dark);
    }
}
