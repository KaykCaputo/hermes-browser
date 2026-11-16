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
use talaria_servo::{EngineSwitches, RuntimeOptions};

use crate::cli::LaunchOptions;
use crate::keyboard::color_scheme_from_winit;
use crate::ui_sync::sync_ui;
use saver::{SAVE_QUIET_PERIOD, SaveScheduler, WriteOutcome};

pub(crate) use apply::new_tab_hint;

thread_local! {
    /// Lets the saving thread's reports reach the handle on the UI thread.
    static HANDLE: RefCell<Weak<SettingsHandle>> = const { RefCell::new(Weak::new()) };
}

/// How long `finish_run` may wait at quit for the settings write in flight. The quit work's
/// overall shutdown budget will own this number.
pub(crate) const FINISH_RUN_BUDGET: Duration = Duration::from_secs(2);

/// How often the window theme is retried until the native window exists, and for how long.
const WINDOW_THEME_RETRY: Duration = Duration::from_millis(50);
const WINDOW_THEME_ATTEMPTS: u32 = 200;

/// Called after a change; returns false when it no longer wants to be called.
type Listener = dyn Fn() -> bool;

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
    /// Called after every change; a listener that returns false is dropped.
    listeners: RefCell<Vec<(u64, Rc<Listener>)>>,
    next_listener: Cell<u64>,
    window: window::SettingsWindowSlot,
    /// The theme last given to the native window.
    window_theme: Cell<Option<Theme>>,
    window_theme_timer: Timer,
    /// The engine's switches (content blocking, what to clear at quit); the engine reads them
    /// when it acts, so setting them never waits for it.
    switches: RefCell<Option<EngineSwitches>>,
    /// The browser window: its colour scheme is the system's, for the settings window.
    main_window: RefCell<slint::Weak<MainWindow>>,
    finished: Cell<bool>,
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
            next_listener: Cell::new(0),
            window: window::SettingsWindowSlot::default(),
            window_theme: Cell::new(None),
            window_theme_timer: Timer::default(),
            switches: RefCell::new(None),
            main_window: RefCell::default(),
            finished: Cell::new(false),
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
        self.subscribe_while(move || {
            listener();
            true
        });
    }

    /// Calls `listener` after every change until it returns false.
    pub(crate) fn subscribe_while(&self, listener: impl Fn() -> bool + 'static) {
        let id = self.next_listener.get();
        self.next_listener.set(id + 1);
        self.listeners.borrow_mut().push((id, Rc::new(listener)));
    }

    pub(crate) fn notice(&self) -> Option<String> {
        self.notice.borrow().clone()
    }

    pub(crate) fn dismiss_notice(&self) {
        if self.notice.borrow_mut().take().is_some() {
            self.notify();
        }
    }

    /// The system's colour scheme, as the browser window sees it.
    fn system_color_scheme(&self) -> slint::language::ColorScheme {
        self.main_window
            .borrow()
            .upgrade()
            .map_or(slint::language::ColorScheme::Unknown, |window| {
                window.get_system_color_scheme()
            })
    }

    fn save_problem(&self) -> Option<String> {
        self.save_problem.borrow().clone()
    }

    fn notify(&self) {
        window::refresh(self);
        let listeners = self.listeners.borrow().clone();
        let finished = listeners
            .into_iter()
            .filter(|(_, listener)| !listener())
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        if !finished.is_empty() {
            self.listeners
                .borrow_mut()
                .retain(|(id, _)| !finished.contains(id));
        }
    }

    /// Closes the settings window, keeping what was typed in its text fields first. Every way
    /// the window closes goes through here or through its own close request.
    pub(crate) fn close_settings_window(&self) {
        let window = self
            .window
            .window
            .borrow()
            .as_ref()
            .map(ComponentHandle::clone_strong);
        if let Some(window) = &window {
            window::commit_all_texts(self, window);
        }
        self.window.close();
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

    /// At quit, after the event loop has ended: waits for the write in flight (until
    /// `deadline` at most), writes once more if anything is unsaved, and runs the
    /// clear-history-on-exit hook. Writing here blocks, which is fine: the window is gone.
    /// Only the first call acts.
    pub(crate) fn finish_run(&self, deadline: Instant) {
        if self.finished.replace(true) {
            return;
        }
        self.save_timer.stop();
        self.close_settings_window();
        if let Some(worker) = self.worker.borrow_mut().take() {
            let SaveWorker { sender, thread } = worker;
            drop(sender);
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if thread.is_finished() {
                match thread.join() {
                    // The thread's report of its last write never reached the event loop.
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
            } else {
                // Never two writes at once: the last changes are lost rather than racing it.
                log::warn!(
                    "a settings write did not finish in time; the last changes are not saved"
                );
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
        handle.close_settings_window();
    }
}

/// The system appearance changed: under "Follow the system" the settings window follows it.
pub(crate) fn system_theme_changed() {
    // After Slint has handled the same event, so that the browser window reports the new scheme.
    Timer::single_shot(Duration::ZERO, || {
        if let Some(handle) = HANDLE.with(|current| current.borrow().upgrade()) {
            window::refresh(&handle);
        }
    });
}

/// Gives a window the chrome theme the settings ask for, now and after every change, until the
/// window is dropped. Each window has its own copy of `ChromePalette`, so every window the
/// browser creates calls this once.
pub(crate) fn apply_chrome_theme<C>(component: &C)
where
    C: ComponentHandle + 'static,
    for<'a> ChromePalette<'a>: slint::Global<'a, C>,
{
    let Some(handle) = HANDLE.with(|current| current.borrow().upgrade()) else {
        log::warn!("settings: no settings yet; the window keeps the default theme");
        return;
    };
    let theme = chrome_theme(handle.settings.borrow().appearance.theme);
    component.global::<ChromePalette>().set_theme(theme);
    let weak = component.as_weak();
    let settings = Rc::downgrade(&handle);
    handle.subscribe_while(move || {
        let (Some(component), Some(settings)) = (weak.upgrade(), settings.upgrade()) else {
            return false;
        };
        let theme = chrome_theme(settings.settings.borrow().appearance.theme);
        component.global::<ChromePalette>().set_theme(theme);
        true
    });
}

/// The engine's options with the privacy settings it needs before it starts: content blocking
/// and what to clear (also cleared at start, in case the last quit did not get to it).
pub(crate) fn with_privacy(options: RuntimeOptions, settings: &SettingsHandle) -> RuntimeOptions {
    let current = settings.settings.borrow();
    RuntimeOptions {
        block_ads_and_trackers: current.privacy.block_ads_and_trackers,
        clear_on_exit: apply::exit_clearing(&current.privacy.clear_on_exit),
        ..options
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
    *settings.main_window.borrow_mut() = window.as_weak();
    apply_chrome_theme(window);
    // Taken once, before the event loop runs; from then on settings reach the engine through
    // the shared switches, never by borrowing it.
    let mut switches = None;
    if !crate::engine::with_runtime(|runtime| switches = Some(runtime.switches())) {
        log::error!("settings: the engine is not running; privacy changes will not reach it");
    }
    *settings.switches.borrow_mut() = switches;

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

    if let Some(switches) = settings.switches.borrow().as_ref() {
        switches.set_content_blocking(current.privacy.block_ads_and_trackers);
        switches.set_clear_on_exit(apply::exit_clearing(&current.privacy.clear_on_exit));
    }

    window.set_show_home_button(current.appearance.show_home_button);
    window.set_default_zoom_percent(i32::from(current.appearance.default_zoom_percent));
    window.set_search_engine_name(current.search.default_engine().name.into());
    window.set_settings_notice(settings.notice().unwrap_or_default().into());
    settings.apply_window_theme(window, controller, current.appearance.theme);
    sync_ui(window, &controller.borrow());
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
        self.apply_window_theme_attempt(window, controller, theme, WINDOW_THEME_ATTEMPTS);
    }

    fn apply_window_theme_attempt(
        &self,
        window: &MainWindow,
        controller: &Rc<RefCell<BrowserController>>,
        theme: Theme,
        attempts_left: u32,
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
            None if attempts_left == 0 => {
                log::warn!("settings: the window never appeared; its title bar keeps its theme");
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
                            handle.apply_window_theme_attempt(
                                &window,
                                &controller,
                                theme,
                                attempts_left - 1,
                            );
                        }
                    },
                );
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use mercury_settings::Theme;

    use super::*;

    thread_local! {
        static SLINT_READY: Cell<bool> = const { Cell::new(false) };
    }

    /// Slint's testing backend for this test thread: no display, no event loop, mock time.
    pub(crate) fn init_slint() {
        if !SLINT_READY.with(|ready| ready.replace(true)) {
            i_slint_backend_testing::init_no_event_loop();
        }
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    fn saved_theme(directory: &Path) -> String {
        std::fs::read_to_string(directory.join("settings.json")).expect("a settings file")
    }

    #[test]
    fn a_newer_file_starts_no_saving_thread() {
        init_slint();
        let directory = tempfile::tempdir().expect("a directory");
        std::fs::write(
            directory.path().join("settings.json"),
            r#"{"format_version": 99}"#,
        )
        .expect("a file");

        let handle = SettingsHandle::start(Some(directory.path()));

        assert!(handle.worker.borrow().is_none());
        assert!(handle.notice().is_some());
        assert!(handle.save_problem().is_some());
    }

    #[test]
    fn an_unreadable_file_starts_no_saving_thread() {
        init_slint();
        let directory = tempfile::tempdir().expect("a directory");
        std::fs::create_dir(directory.path().join("settings.json")).expect("a directory");

        let handle = SettingsHandle::start(Some(directory.path()));

        assert!(handle.worker.borrow().is_none());
        assert!(handle.notice().is_some());
    }

    #[test]
    fn a_change_inside_the_quiet_period_is_written_at_quit() {
        init_slint();
        let directory = tempfile::tempdir().expect("a directory");
        let handle = SettingsHandle::start(Some(directory.path()));

        handle.update(|settings| settings.appearance.theme = Theme::Light);
        handle.finish_run(deadline());

        assert!(saved_theme(directory.path()).contains(r#""theme": "light""#));
    }

    #[test]
    fn at_quit_the_write_in_flight_is_waited_for_and_its_result_taken() {
        init_slint();
        let directory = tempfile::tempdir().expect("a directory");
        let handle = SettingsHandle::start(Some(directory.path()));
        handle.update(|settings| settings.appearance.theme = Theme::Light);
        // As if the quiet period had passed: the write goes to the saving thread.
        handle
            .scheduler
            .borrow_mut()
            .changed(Instant::now() - SAVE_QUIET_PERIOD);
        handle.save_if_due();
        assert!(handle.scheduler.borrow().is_writing());

        handle.finish_run(deadline());

        // The thread's report never reached an event loop; the join handed it over.
        assert!(!handle.scheduler.borrow().is_writing());
        assert!(!handle.scheduler.borrow().needs_final_write());
        assert!(saved_theme(directory.path()).contains(r#""theme": "light""#));
    }

    #[test]
    fn a_change_made_during_the_last_write_is_written_after_it() {
        init_slint();
        let directory = tempfile::tempdir().expect("a directory");
        let handle = SettingsHandle::start(Some(directory.path()));
        handle.update(|settings| settings.appearance.theme = Theme::Light);
        handle
            .scheduler
            .borrow_mut()
            .changed(Instant::now() - SAVE_QUIET_PERIOD);
        handle.save_if_due();

        handle.update(|settings| settings.appearance.theme = Theme::Dark);
        handle.finish_run(deadline());

        assert!(saved_theme(directory.path()).contains(r#""theme": "dark""#));
    }

    #[test]
    fn finishing_twice_acts_once() {
        init_slint();
        let directory = tempfile::tempdir().expect("a directory");
        let handle = SettingsHandle::start(Some(directory.path()));
        handle.update(|settings| settings.appearance.theme = Theme::Light);
        handle.finish_run(deadline());
        std::fs::remove_file(directory.path().join("settings.json")).expect("the file");

        handle.update(|settings| settings.appearance.theme = Theme::Dark);
        handle.finish_run(deadline());

        assert!(!directory.path().join("settings.json").exists());
    }

    #[test]
    fn any_window_follows_the_chrome_theme_until_it_is_dropped() {
        init_slint();
        let handle = SettingsHandle::start(None);
        let listeners_before = handle.listeners.borrow().len();
        let window = mercury_ui::SettingsWindow::new().expect("a window");

        apply_chrome_theme(&window);
        assert_eq!(
            window.global::<ChromePalette>().get_theme(),
            ChromeTheme::System
        );
        handle.update(|settings| settings.appearance.theme = Theme::Light);
        assert_eq!(
            window.global::<ChromePalette>().get_theme(),
            ChromeTheme::Light
        );

        drop(window);
        handle.update(|settings| settings.appearance.theme = Theme::Dark);

        assert_eq!(handle.listeners.borrow().len(), listeners_before);
    }
}
