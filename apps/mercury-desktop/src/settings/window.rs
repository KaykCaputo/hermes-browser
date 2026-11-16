//! The settings window: a separate native window built from `SettingsWindow`, created when it
//! is opened and dropped when it is closed.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use mercury_settings::{Settings, Theme, builtin_engines};
use mercury_ui::{SearchEngineRow, SettingsWindow};
use slint::language::ColorScheme as SlintColorScheme;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{SettingsHandle, edit};

/// The open settings window, if any, and the search engine being edited in it.
#[derive(Default)]
pub(crate) struct SettingsWindowSlot {
    window: RefCell<Option<SettingsWindow>>,
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
    if !visible {
        slot.close();
        match SettingsWindow::new() {
            Ok(window) => {
                connect(&window, Rc::downgrade(handle));
                fill_text_fields(&window, &handle.settings());
                *slot.window.borrow_mut() = Some(window);
            }
            Err(error) => {
                log::error!("could not create the settings window: {error}");
                return;
            }
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
    window.set_homepage(settings.homepage.clone().into());
    window.set_download_directory(
        settings
            .downloads
            .directory
            .as_ref()
            .map(|directory| directory.display().to_string())
            .unwrap_or_default()
            .into(),
    );
    window.set_language(settings.language.clone().unwrap_or_default().into());
}

fn connect(window: &SettingsWindow, handle: Weak<SettingsHandle>) {
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
    window.on_text_edited(move |name, text| {
        let (Some(handle), Some(window)) = (text_handle.upgrade(), text_window.upgrade()) else {
            return;
        };
        let mut result = Ok(());
        handle.update(|settings| result = edit::apply_text(settings, &name, &text));
        let problem = SharedString::from(result.err().unwrap_or_default());
        match name.as_str() {
            "homepage" => window.set_homepage_problem(problem),
            "download-directory" => window.set_download_directory_problem(problem),
            "language" => window.set_language_problem(problem),
            _ => log::warn!("settings window: unknown field {name}"),
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
                handle.window.close();
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
