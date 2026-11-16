//! The user's settings: loaded at start, saved off the UI thread, applied to the browser, and
//! edited in the settings window.
//!
//! Other parts of the browser read settings through `SettingsHandle`'s accessors
//! (`startup_behavior`, `block_popups`, `default_search_engine`, `settings`) and never keep a
//! copy, so a change in the window reaches them on their next read.

mod apply;
mod edit;
mod exit;
mod load;
mod saver;
mod window;

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mercury_settings::{
    SearchEngine, Settings, SettingsError, SettingsStore, StartupBehavior, Theme,
};
use mercury_ui::{ChromePalette, ChromeTheme, MainWindow};
use slint::winit_030::{WinitWindowAccessor, winit};
use slint::{ComponentHandle, Timer, TimerMode};
use talaria_core::BrowserController;

use crate::cli::LaunchOptions;
use crate::keyboard::color_scheme_from_winit;
use crate::ui_sync::sync_ui;
use saver::{SAVE_QUIET_PERIOD, SaveScheduler, WriteOutcome};

pub(crate) use apply::new_tab_hint;

thread_local! {
    /// Lets the saving thread's reports reach the handle on the UI thread.
    static HANDLE: RefCell<Weak<SettingsHandle>> = const { RefCell::new(Weak::new()) };
}

/// How often the window theme is retried until the native window exists.
const WINDOW_THEME_RETRY: Duration = Duration::from_millis(50);

/// The settings of this run. One per process, created in `main` after the profile is locked.
pub(crate) struct SettingsHandle {
    this: Weak<SettingsHandle>,
    settings: RefCell<Settings>,
    store: Option<SettingsStore>,
    scheduler: RefCell<SaveScheduler>,
    worker: RefCell<Option<SaveWorker>>,
    save_timer: Timer,
    /// The one message for the user about the settings file, until dismissed.
    notice: RefCell<Option<String>>,
    /// Why changes are not being saved, shown in the settings window.
    save_problem: RefCell<Option<String>>,
    listeners: RefCell<Vec<Rc<dyn Fn()>>>,
    window: window::SettingsWindowSlot,
    /// The theme last given to the native window.
    window_theme: Cell<Option<Theme>>,
    window_theme_timer: Timer,
}

impl SettingsHandle {
    /// Sweeps the profile's stale temporary files and loads the settings, handling every
    /// outcome as the settings crate's contract asks. Call only while holding the profile's
    /// lock; `profile_root` is `None` when the run keeps no profile.
    pub(crate) fn start(profile_root: Option<&Path>) -> Rc<Self> {
        let (store, startup) = load::load(profile_root);
        let worker = store
            .as_ref()
            .filter(|_| startup.may_save)
            .and_then(|store| {
                SaveWorker::spawn(store.clone())
                    .inspect_err(|error| log::warn!("settings will not be saved: {error}"))
                    .ok()
            });
        let may_save = worker.is_some();
        let save_problem = if !startup.may_save {
            startup.notice.clone()
        } else if store.is_some() && !may_save {
            Some("Changes to the settings cannot be saved in this session.".to_owned())
        } else {
            None
        };
        let handle = Rc::new_cyclic(|this| Self {
            this: this.clone(),
            settings: RefCell::new(startup.settings),
            store,
            scheduler: RefCell::new(SaveScheduler::new(SAVE_QUIET_PERIOD, may_save)),
            worker: RefCell::new(worker),
            save_timer: Timer::default(),
            notice: RefCell::new(startup.notice.clone()),
            save_problem: RefCell::new(save_problem),
            listeners: RefCell::default(),
            window: window::SettingsWindowSlot::default(),
            window_theme: Cell::new(None),
            window_theme_timer: Timer::default(),
        });
        HANDLE.with(|current| *current.borrow_mut() = Rc::downgrade(&handle));
        if startup.save_now {
            handle.mark_changed();
        }
        handle
    }

    /// What the browser shows at start-up. The session work reads this to decide whether to
    /// restore the previous session.
    #[allow(dead_code, reason = "read by the session work")]
    pub(crate) fn startup_behavior(&self) -> StartupBehavior {
        self.settings.borrow().startup
    }

    /// Whether pages may open pop-up windows. For the tabs work's handling of `window.open`.
    #[allow(dead_code, reason = "read by the tabs work")]
    pub(crate) fn block_popups(&self) -> bool {
        self.settings.borrow().privacy.block_popups
    }

    /// The engine address-bar searches go to (DuckDuckGo when the chosen one no longer exists).
    #[allow(
        dead_code,
        reason = "read by the history work (the search row of suggestions)"
    )]
    pub(crate) fn default_search_engine(&self) -> SearchEngine {
        self.settings.borrow().search.default_engine()
    }

    /// A copy of every setting.
    pub(crate) fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    /// Changes the settings, applies them and schedules a save. Values must already have been
    /// checked (the window does so through `edit`); the result is validated once more, so
    /// nothing invalid is ever kept.
    pub(crate) fn update(&self, change: impl FnOnce(&mut Settings)) {
        let changed = {
            let mut settings = self.settings.borrow_mut();
            let mut candidate = settings.clone();
            change(&mut candidate);
            let (candidate, repairs) = candidate.validated();
            if !repairs.is_empty() {
                log::warn!("settings: a change needed repairs: {repairs:?}");
            }
            let changed = candidate != *settings;
            *settings = candidate;
            changed
        };
        if changed {
            self.mark_changed();
            self.notify();
        }
    }

    /// Calls `listener` after every change.
    pub(crate) fn subscribe(&self, listener: impl Fn() + 'static) {
        self.listeners.borrow_mut().push(Rc::new(listener));
    }

    pub(crate) fn notice(&self) -> Option<String> {
        self.notice.borrow().clone()
    }

    pub(crate) fn dismiss_notice(&self) {
        if self.notice.borrow_mut().take().is_some() {
            self.notify();
        }
    }

    fn save_problem(&self) -> Option<String> {
        self.save_problem.borrow().clone()
    }

    fn notify(&self) {
        let listeners = self.listeners.borrow().clone();
        for listener in listeners {
            listener();
        }
    }

    fn mark_changed(&self) {
        self.scheduler.borrow_mut().changed(Instant::now());
        self.arm_save_timer();
    }

    fn arm_save_timer(&self) {
        let Some(delay) = self.scheduler.borrow().next_check(Instant::now()) else {
            return;
        };
        let this = self.this.clone();
        self.save_timer
            .start(TimerMode::SingleShot, delay, move || {
                if let Some(handle) = this.upgrade() {
                    handle.save_if_due();
                }
            });
    }

    fn save_if_due(&self) {
        if !self.scheduler.borrow_mut().start_write(Instant::now()) {
            self.arm_save_timer();
            return;
        }
        let snapshot = self.settings();
        let sent = self
            .worker
            .borrow()
            .as_ref()
            .is_some_and(|worker| worker.sender.send(snapshot).is_ok());
        if !sent {
            self.scheduler.borrow_mut().finished(WriteOutcome::Failed);
        }
    }

    fn write_finished(&self, outcome: WriteOutcome) {
        self.scheduler.borrow_mut().finished(outcome);
        if outcome == WriteOutcome::Refused {
            let message = "Your settings were replaced by a newer version of Mercury Browser. \
                           Changes made in this session will not be saved."
                .to_owned();
            *self.save_problem.borrow_mut() = Some(message.clone());
            *self.notice.borrow_mut() = Some(message);
            self.notify();
        }
        self.arm_save_timer();
    }

    /// At quit, after the event loop has ended: waits for the write in flight, writes once
    /// more if anything is unsaved, and runs the clear-history-on-exit hook. Writing here
    /// blocks, which is fine: the window is gone.
    pub(crate) fn finish_run(&self) {
        self.save_timer.stop();
        self.window.close();
        if let Some(worker) = self.worker.borrow_mut().take() {
            let SaveWorker { sender, thread } = worker;
            drop(sender);
            match thread.join() {
                Ok(Some(outcome)) if self.scheduler.borrow().is_writing() => {
                    self.scheduler.borrow_mut().finished(outcome);
                }
                Ok(_) => {}
                Err(_) => log::error!("the settings saving thread panicked"),
            }
            if self.scheduler.borrow().needs_final_write()
                && let Some(store) = &self.store
            {
                match store.save(&self.settings.borrow()) {
                    Ok(()) => log::info!("settings saved at exit"),
                    Err(error) => log::warn!("could not save the settings at exit: {error}"),
                }
            }
        }
        let settings = self.settings.borrow();
        if settings.privacy.clear_on_exit.history {
            exit::clear_history_on_exit(&settings);
        }
    }
}

/// The thread that writes `settings.json`, one write at a time.
struct SaveWorker {
    sender: mpsc::Sender<Settings>,
    /// Returns how the last write ended.
    thread: JoinHandle<Option<WriteOutcome>>,
}

impl SaveWorker {
    fn spawn(store: SettingsStore) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::channel::<Settings>();
        let thread = std::thread::Builder::new()
            .name("settings-saver".to_owned())
            .spawn(move || {
                let mut last = None;
                for settings in receiver {
                    let outcome = match store.save(&settings) {
                        Ok(()) => WriteOutcome::Saved,
                        Err(error @ SettingsError::NewerVersion { .. }) => {
                            log::warn!("settings not saved: {error}");
                            WriteOutcome::Refused
                        }
                        Err(error) => {
                            log::warn!("settings not saved: {error}");
                            WriteOutcome::Failed
                        }
                    };
                    last = Some(outcome);
                    let _queued = slint::invoke_from_event_loop(move || {
                        if let Some(handle) = HANDLE.with(|current| current.borrow().upgrade()) {
                            handle.write_finished(outcome);
                        }
                    });
                }
                last
            })?;
        Ok(Self { sender, thread })
    }
}

/// The browser window is closing: close the settings window with it, so that the event loop,
/// which runs while any window is open, ends.
pub(crate) fn main_window_closing() {
    if let Some(handle) = HANDLE.with(|current| current.borrow().upgrade()) {
        handle.window.close();
    }
}

/// The launch options with the settings that apply at start: experimental web features and
/// the GPU canvas. See `apply::launch_options` for how the command line wins.
pub(crate) fn launch_options(options: LaunchOptions, settings: &SettingsHandle) -> LaunchOptions {
    apply::launch_options(options, &settings.settings.borrow().web)
}

/// Before the start-up tabs are opened: the controller takes the search engine, zoom and
/// colour scheme, and the first tab opens the home page if the setting asks for it.
pub(crate) fn prepare_controller(
    settings: &SettingsHandle,
    controller: &mut BrowserController,
    command_line_urls: &[String],
) {
    let current = settings.settings.borrow();
    apply::apply_to_controller(controller, &current);
    if let Some(address) = apply::start_up_address(&current, command_line_urls) {
        let first_tab_id = controller.active_tab_id();
        if let Err(error) = controller.set_initial_address(first_tab_id, &address) {
            log::warn!("could not open the home page: {error}");
        }
    }
}

/// Applies the settings to the window and the engine now and after every change, and
/// connects the window's settings callbacks (`show-settings`, `go-home`,
/// `dismiss-settings-notice`).
pub(crate) fn connect(
    settings: &Rc<SettingsHandle>,
    window: &MainWindow,
    controller: &Rc<RefCell<BrowserController>>,
) {
    let apply = {
        let settings = Rc::downgrade(settings);
        let window = window.as_weak();
        let controller = Rc::clone(controller);
        move || {
            let (Some(settings), Some(window)) = (settings.upgrade(), window.upgrade()) else {
                return;
            };
            apply_everywhere(&settings, &window, &controller);
        }
    };
    apply();
    settings.subscribe(apply);

    let show_settings = Rc::downgrade(settings);
    window.on_show_settings(move || {
        if let Some(settings) = show_settings.upgrade() {
            window::show(&settings);
        }
    });

    let go_home_settings = Rc::downgrade(settings);
    let go_home_window = window.as_weak();
    window.on_go_home(move || {
        let (Some(settings), Some(window)) = (go_home_settings.upgrade(), go_home_window.upgrade())
        else {
            return;
        };
        let address = apply::home_address(&settings.settings.borrow());
        window.invoke_navigate(address.into());
    });

    let dismiss_settings = Rc::downgrade(settings);
    window.on_dismiss_settings_notice(move || {
        if let Some(settings) = dismiss_settings.upgrade() {
            settings.dismiss_notice();
        }
    });
}

fn apply_everywhere(
    settings: &SettingsHandle,
    window: &MainWindow,
    controller: &Rc<RefCell<BrowserController>>,
) {
    let current = settings.settings();
    apply::apply_to_controller(&mut controller.borrow_mut(), &current);

    let applied = crate::engine::with_runtime(|runtime| {
        runtime.set_content_blocking(current.privacy.block_ads_and_trackers);
        runtime.set_clear_on_exit(apply::exit_clearing(&current.privacy.clear_on_exit));
    });
    if !applied {
        log::warn!("the engine was busy; privacy settings apply at the next change");
    }

    window
        .global::<ChromePalette>()
        .set_theme(chrome_theme(current.appearance.theme));
    window.set_show_home_button(current.appearance.show_home_button);
    window.set_search_engine_name(current.search.default_engine().name.into());
    window.set_settings_notice(settings.notice().unwrap_or_default().into());
    settings.apply_window_theme(window, controller, current.appearance.theme);
    sync_ui(window, &controller.borrow());
    window::refresh(settings);
    window.window().request_redraw();
}

pub(crate) fn chrome_theme(theme: Theme) -> ChromeTheme {
    match theme {
        Theme::System => ChromeTheme::System,
        Theme::Light => ChromeTheme::Light,
        Theme::Dark => ChromeTheme::Dark,
    }
}

impl SettingsHandle {
    /// Gives the native window (its title bar) the chosen theme. The native window exists only
    /// once the event loop runs, so this retries until it does. Following the system again,
    /// the system's scheme is read back for pages, since the window reported the forced one.
    fn apply_window_theme(
        &self,
        window: &MainWindow,
        controller: &Rc<RefCell<BrowserController>>,
        theme: Theme,
    ) {
        if self.window_theme.get() == Some(theme) {
            return;
        }
        let native_theme = match theme {
            Theme::System => None,
            Theme::Light => Some(winit::window::Theme::Light),
            Theme::Dark => Some(winit::window::Theme::Dark),
        };
        let applied = window.window().with_winit_window(|native| {
            native.set_theme(native_theme);
            native.theme()
        });
        match applied {
            Some(reported) => {
                self.window_theme.set(Some(theme));
                self.window_theme_timer.stop();
                if theme == Theme::System
                    && let Some(reported) = reported
                {
                    controller
                        .borrow_mut()
                        .set_color_scheme(color_scheme_from_winit(reported));
                }
            }
            None => {
                let this = self.this.clone();
                let weak_window = window.as_weak();
                let controller = Rc::clone(controller);
                self.window_theme_timer.start(
                    TimerMode::SingleShot,
                    WINDOW_THEME_RETRY,
                    move || {
                        if let (Some(handle), Some(window)) =
                            (this.upgrade(), weak_window.upgrade())
                        {
                            handle.apply_window_theme(&window, &controller, theme);
                        }
                    },
                );
            }
        }
    }
}
