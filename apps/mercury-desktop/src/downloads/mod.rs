//! Saving files: links, images and pages the user asks for, the downloads list, and the
//! question before quitting with downloads running. The transfers are `mercury-downloads`'
//! work; their records live in the profile database through `profile_data`.
//!
//! Everything here runs on the UI thread. `mercury-downloads` reports from its own threads; its
//! events are queued and applied on the UI thread (`drain_events`).
//!
//! Entry points for other code:
//! - `start` once at start-up, after `profile_data::start`;
//! - `save_url` for "Save link as…" and "Save image as…" (the context menu);
//! - `save_page` for "Save page as…" (Cmd+S);
//! - `show_list` for the downloads shortcut;
//! - `confirm_quit` before the quit sequence starts shutting down;
//! - `handle_key` for Escape and Return while the list or the question is open;
//! - `shut_down` once the event loop has ended, before `profile_data::shut_down`.

mod format;
mod location;
mod model;
mod progress;
mod quit;
mod request;
mod view;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mercury_downloads::{DownloadEvent, DownloadManager, ResumeFrom, remove_partial_file};
use mercury_ui::{DownloadAction, DownloadRow, MainWindow};
use slint::winit_030::winit;
use slint::{ComponentHandle, ModelRc, VecModel};
use talaria_core::{BrowserController, TabId};
use url::Url;

use self::location::{SaveLocation, save_location};
use self::model::{DownloadList, NewItem, RecordOp};
pub(crate) use self::request::SaveError;
use self::request::{RequestContext, build_request, referrer_for, savable_url};
use self::view::item_view;

/// How many downloads the popover shows.
const RECENT_ROWS: usize = 6;
/// How many records the full list reads at a time.
const RECORDS_PAGE: u32 = 200;

thread_local! {
    static DOWNLOADS: RefCell<Option<Rc<Downloads>>> = const { RefCell::new(None) };
}

/// Diagnostics from the command line.
#[derive(Debug, Default)]
pub(crate) struct DownloadOptions {
    /// `--download-dir`: save into this folder in this run.
    pub(crate) folder: Option<PathBuf>,
    /// `--download`: start downloading this address once the browser is up.
    pub(crate) url: Option<String>,
}

/// `mercury-downloads` events waiting for the UI thread.
#[derive(Default)]
struct Inbox {
    events: Mutex<Vec<DownloadEvent>>,
    wake_pending: AtomicBool,
}

impl Inbox {
    fn take(&self) -> Vec<DownloadEvent> {
        self.wake_pending.store(false, Ordering::Release);
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

struct Downloads {
    window: slint::Weak<MainWindow>,
    manager: RefCell<Option<DownloadManager>>,
    inbox: Arc<Inbox>,
    list: RefCell<DownloadList>,
    popover_open: Cell<bool>,
    panel_open: Cell<bool>,
    /// How many records were asked of the database, and whether it had more.
    records_asked: Cell<u32>,
    more_records: Cell<bool>,
    /// What to do once the user agrees to quit.
    pending_quit: RefCell<Option<Box<dyn FnOnce()>>>,
}

fn current() -> Option<Rc<Downloads>> {
    DOWNLOADS.with(|current| current.borrow().clone())
}

/// Sets up the downloads list. Call once, after `profile_data::start`.
pub(crate) fn start(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    options: DownloadOptions,
) {
    location::override_folder(options.folder);
    let downloads = Rc::new(Downloads {
        window: window.as_weak(),
        manager: RefCell::new(None),
        inbox: Arc::new(Inbox::default()),
        list: RefCell::new(DownloadList::default()),
        popover_open: Cell::new(false),
        panel_open: Cell::new(false),
        records_asked: Cell::new(0),
        more_records: Cell::new(false),
        pending_quit: RefCell::new(None),
    });
    DOWNLOADS.with(|current| *current.borrow_mut() = Some(Rc::clone(&downloads)));
    window.set_downloads_show_label(show_label().into());
    connect_callbacks(window);
    downloads.load_records(RECORDS_PAGE);

    if let Some(address) = options.url {
        start_when_page_settles(controller, address);
    }
}

/// `--download`: starts once the active tab has finished loading (or failed), so that cookies
/// the page set are in the engine; polls every quarter second.
fn start_when_page_settles(controller: Rc<RefCell<BrowserController>>, address: String) {
    let timer = Rc::new(slint::Timer::default());
    let held = Rc::clone(&timer);
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            let settled = matches!(
                controller.borrow().active_state().load_state(),
                talaria_core::LoadState::Loaded | talaria_core::LoadState::Failed { .. }
            );
            if !settled || crate::engine::with_runtime(|_| ()).is_none() {
                return;
            }
            held.stop();
            let tab = controller.borrow().active_tab_id();
            let page = controller.borrow().active_state().url().to_owned();
            if let Err(error) = save_url(Some(tab), &address, Some(&page), None) {
                eprintln!("mercury-browser: cannot download '{address}': {error}");
            }
        },
    );
    // The callback holds the timer (`held`), which keeps it alive after this returns: a
    // one-off cycle of a diagnostic, left in place once stopped.
}

fn show_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Show in Finder"
    } else {
        "Open folder"
    }
}

/// Saves `url` into the download folder: for "Save link as…" (the link's address and the
/// page's address as `referrer`) and "Save image as…" (the image's address). `suggested_name`
/// is a name the page proposed (`<a download="…">`), still sanitised by `mercury-downloads`.
/// `tab` is the tab the request came from, if any.
pub(crate) fn save_url(
    tab: Option<TabId>,
    url: &str,
    referrer: Option<&str>,
    suggested_name: Option<&str>,
) -> Result<(), SaveError> {
    let url = savable_url(url)?;
    let page = referrer.and_then(|referrer| Url::parse(referrer).ok());
    if let Some(tab) = tab {
        log::info!("tab {}: saving a file", tab.value());
    }
    let Some(downloads) = current() else {
        return Ok(());
    };
    downloads.start_download(
        url,
        referrer_for(page.as_ref()),
        suggested_name.map(str::to_owned),
    );
    Ok(())
}

/// "Save page as…": saves the active tab's address as it is served (the HTML only; images,
/// style sheets and scripts are not saved with it).
pub(crate) fn save_page(controller: &RefCell<BrowserController>) {
    let (tab, address) = {
        let controller = controller.borrow();
        (
            controller.active_tab_id(),
            controller.active_state().url().to_owned(),
        )
    };
    if let Err(error) = save_url(Some(tab), &address, None, None) {
        log::info!("cannot save the page: {error}");
    }
}

/// Opens or closes the full list of downloads (the downloads shortcut).
pub(crate) fn show_list() {
    if let Some(downloads) = current() {
        let open = !downloads.panel_open.get();
        downloads.set_panel_open(open);
    }
}

/// Before quitting: with downloads running, asks "Cancel N downloads and quit?" and runs
/// `quit` only if the user agrees; without, runs `quit` at once. The downloads themselves are
/// stopped by `shut_down`, keeping what can be resumed.
pub(crate) fn confirm_quit(quit: impl FnOnce() + 'static) {
    let Some(downloads) = current() else {
        quit();
        return;
    };
    let active = downloads.list.borrow().active_count();
    let Some(prompt) = quit::quit_prompt(active) else {
        quit();
        return;
    };
    *downloads.pending_quit.borrow_mut() = Some(Box::new(quit));
    if let Some(window) = downloads.window.upgrade() {
        window.set_quit_prompt_title(prompt.title.into());
        window.set_quit_prompt_body(prompt.body.into());
        window.set_quit_prompt_confirm_label(prompt.confirm.into());
        window.set_quit_prompt_keep_label(prompt.keep.into());
        window.set_quit_prompt_open(true);
        window.window().request_redraw();
    }
}

/// Escape closes the question (keep downloading), the popover or the list; Return answers the
/// question with its default, "Keep downloading". Returns whether the key was used.
pub(crate) fn handle_key(event: &winit::event::KeyEvent) -> bool {
    use winit::keyboard::{Key, NamedKey};
    if event.state != winit::event::ElementState::Pressed || event.repeat {
        return false;
    }
    let Some(downloads) = current() else {
        return false;
    };
    let Some(window) = downloads.window.upgrade() else {
        return false;
    };
    let escape = matches!(event.logical_key, Key::Named(NamedKey::Escape));
    let enter = matches!(event.logical_key, Key::Named(NamedKey::Enter));
    if window.get_quit_prompt_open() {
        if escape || enter {
            downloads.answer_quit(false);
            return true;
        }
        return false;
    }
    if escape && downloads.popover_open.get() {
        downloads.set_popover_open(false);
        return true;
    }
    if escape && downloads.panel_open.get() {
        downloads.set_panel_open(false);
        return true;
    }
    false
}

/// After the event loop has ended: interrupts the running downloads, keeping the partial files
/// that can be resumed, records how each ended, and stops the transfers. Returns by `deadline`.
/// Call before `profile_data::shut_down`, so the records are written.
pub(crate) fn shut_down(deadline: Instant) {
    let Some(downloads) = DOWNLOADS.with(|current| current.borrow_mut().take()) else {
        return;
    };
    let manager = downloads.manager.borrow_mut().take();
    let Some(manager) = manager else {
        return;
    };
    for id in downloads.list.borrow().running_transfers() {
        manager.interrupt(id);
    }
    loop {
        downloads.apply_events();
        if downloads.list.borrow().active_count() == 0 || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let left = downloads.list.borrow().active_count();
    if left > 0 {
        log::warn!("{left} download(s) did not stop in time; their partial files are removed");
    }
    // Cancels whatever is left and waits a little for it.
    drop(manager);
    downloads.apply_events();
}

impl Downloads {
    fn manager(&self) -> Option<std::cell::Ref<'_, DownloadManager>> {
        if self.manager.borrow().is_none() {
            let inbox = Arc::clone(&self.inbox);
            let created = DownloadManager::new(move |event| {
                inbox
                    .events
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(event);
                if !inbox.wake_pending.swap(true, Ordering::AcqRel) {
                    // Fails only once the event loop has ended; `shut_down` drains the rest.
                    let _ = slint::invoke_from_event_loop(drain_events);
                }
            });
            match created {
                Ok(manager) => *self.manager.borrow_mut() = Some(manager),
                Err(error) => {
                    log::error!("cannot start downloading: {error}");
                    return None;
                }
            }
        }
        std::cell::Ref::filter_map(self.manager.borrow(), Option::as_ref).ok()
    }

    /// The parts of a request that come from the engine: its user agent and its cookies.
    fn engine_context(url: &Url) -> (String, Vec<(String, String)>) {
        crate::engine::with_runtime(|runtime| {
            (
                runtime.user_agent().to_owned(),
                runtime.cookies_for_url(url),
            )
        })
        .unwrap_or_else(|| {
            log::warn!("the engine is busy or not running; downloading without its cookies");
            (String::new(), Vec::new())
        })
    }

    fn folder(&self) -> PathBuf {
        match save_location() {
            SaveLocation::Folder(folder) => folder,
            SaveLocation::Ask { suggested_folder } => {
                // The save panel is not built yet; see the lane report.
                log::info!("asking where to save is not available yet; saving to the folder");
                suggested_folder
            }
        }
    }

    fn start_download(&self, url: Url, referrer: Option<Url>, suggested_name: Option<String>) {
        let (user_agent, cookies) = Self::engine_context(&url);
        let name_guess =
            mercury_downloads::file_name_for(None, &url, None, suggested_name.as_deref());
        let request = build_request(
            url.clone(),
            RequestContext {
                referrer: referrer.clone(),
                user_agent,
                cookies,
                destination_dir: self.folder(),
                suggested_name,
                resume: None,
            },
        );
        let Some(transfer) = self.manager().map(|manager| manager.start(request)) else {
            return;
        };
        let guid = crate::profile_data::new_guid().unwrap_or_else(fallback_guid);
        self.list.borrow_mut().begin(NewItem {
            guid,
            url: url.to_string(),
            referrer: referrer.map(String::from),
            name_guess,
            transfer,
            started_at_ms: crate::profile_data::now_ms(),
        });
        self.popover_open.set(true);
        self.sync();
    }

    fn apply_events(&self) {
        let events = self.inbox.take();
        if events.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut ops = Vec::new();
        {
            let mut list = self.list.borrow_mut();
            for event in events {
                ops.extend(list.apply(event, now));
            }
        }
        send_ops(ops);
    }

    fn load_records(&self, limit: u32) {
        let Some(data) = crate::profile_data::handle() else {
            return;
        };
        self.records_asked.set(limit);
        data.downloads(limit, move |result| {
            let Some(downloads) = current() else {
                return;
            };
            match result {
                Ok(records) => {
                    downloads
                        .more_records
                        .set(records.len() as u64 >= u64::from(limit));
                    downloads
                        .list
                        .borrow_mut()
                        .merge_records(records, |path| path.symlink_metadata().is_ok());
                    downloads.sync();
                }
                Err(error) => log::info!("download records are not available: {error}"),
            }
        });
    }

    fn set_popover_open(&self, open: bool) {
        if open {
            self.refresh_files();
        }
        self.popover_open.set(open);
        self.sync();
    }

    fn set_panel_open(&self, open: bool) {
        if open {
            self.popover_open.set(false);
            self.refresh_files();
        }
        self.panel_open.set(open);
        self.sync();
    }

    fn refresh_files(&self) {
        self.list
            .borrow_mut()
            .refresh_file_presence(|path| path.symlink_metadata().is_ok());
    }

    fn answer_quit(&self, confirmed: bool) {
        if let Some(window) = self.window.upgrade() {
            window.set_quit_prompt_open(false);
            window.window().request_redraw();
        }
        let quit = self.pending_quit.borrow_mut().take();
        if confirmed && let Some(quit) = quit {
            quit();
        }
    }

    fn act(&self, guid: &str, action: DownloadAction) {
        let item = self.list.borrow().item(guid).cloned();
        let Some(item) = item else {
            return;
        };
        match action {
            DownloadAction::Open => {
                if let Some(path) = &item.target_path
                    && !format::is_executable_type(path)
                {
                    open_file(path);
                }
            }
            DownloadAction::Show => {
                if let Some(path) = &item.target_path {
                    show_file(path);
                }
            }
            DownloadAction::Cancel => {
                if let Some(transfer) = item.transfer {
                    if let Some(manager) = self.manager.borrow().as_ref() {
                        manager.cancel(transfer);
                    }
                } else {
                    let cancelled = self.list.borrow_mut().cancel_interrupted(guid);
                    if let Some((ops, partial)) = cancelled {
                        send_ops(ops);
                        discard(partial);
                    }
                }
            }
            DownloadAction::Resume => self.resume(&item),
            DownloadAction::Retry => {
                let removed = self.list.borrow_mut().remove(guid);
                if let Some((ops, partial)) = removed {
                    send_ops(ops);
                    discard(partial);
                }
                if let Ok(url) = savable_url(&item.url) {
                    let referrer = item.referrer.as_deref().and_then(|r| Url::parse(r).ok());
                    self.start_download(url, referrer, None);
                }
            }
            DownloadAction::Remove => {
                let removed = self.list.borrow_mut().remove(guid);
                if let Some((ops, partial)) = removed {
                    send_ops(ops);
                    discard(partial);
                }
            }
        }
        self.sync();
    }

    fn resume(&self, item: &model::Item) {
        let (Some(resume), Ok(url)) = (item.resume_from(), savable_url(&item.url)) else {
            return;
        };
        let Some(folder) = resume.target_path.parent().map(Path::to_path_buf) else {
            return;
        };
        let (user_agent, cookies) = Self::engine_context(&url);
        let request = build_request(
            url,
            RequestContext {
                referrer: item.referrer.as_deref().and_then(|r| Url::parse(r).ok()),
                user_agent,
                cookies,
                destination_dir: folder,
                suggested_name: None,
                resume: Some(resume),
            },
        );
        let Some(transfer) = self.manager().map(|manager| manager.start(request)) else {
            return;
        };
        let ops = self.list.borrow_mut().resume(&item.guid, transfer);
        send_ops(ops.unwrap_or_default());
    }

    fn clear(&self) {
        let (ops, partials) = self.list.borrow_mut().clear();
        send_ops(ops);
        for partial in partials {
            discard(Some(partial));
        }
        self.sync();
    }

    fn sync(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let list = self.list.borrow();
        let rows: Vec<DownloadRow> = list
            .items()
            .iter()
            .map(|item| download_row(item_view(item)))
            .collect();
        let active = list.active_count() > 0;
        window.set_downloads_button_visible(list.has_items_this_run());
        window.set_downloads_active(active);
        window.set_downloads_progress(list.overall_progress().unwrap_or(view::INDETERMINATE_BAR));
        window.set_downloads_recent(ModelRc::new(VecModel::from(
            rows.iter().take(RECENT_ROWS).cloned().collect::<Vec<_>>(),
        )));
        window.set_downloads_popover_open(self.popover_open.get());
        window.set_downloads_panel_open(self.panel_open.get());
        window.set_downloads_can_show_more(self.more_records.get());
        window.set_downloads_all(ModelRc::new(VecModel::from(if self.panel_open.get() {
            rows
        } else {
            Vec::new()
        })));
        window.window().request_redraw();
    }
}

fn drain_events() {
    if let Some(downloads) = current() {
        downloads.apply_events();
        downloads.sync();
    }
}

fn connect_callbacks(window: &MainWindow) {
    window.on_downloads_toggle_popover(|| {
        if let Some(downloads) = current() {
            let open = !downloads.popover_open.get();
            downloads.set_popover_open(open);
        }
    });
    window.on_downloads_close_popover(|| {
        if let Some(downloads) = current() {
            downloads.set_popover_open(false);
        }
    });
    window.on_downloads_action(|guid, action| {
        if let Some(downloads) = current() {
            downloads.act(guid.as_str(), action);
        }
    });
    window.on_downloads_clear_list(|| {
        if let Some(downloads) = current() {
            downloads.clear();
        }
    });
    window.on_downloads_show_all(|| {
        if let Some(downloads) = current() {
            downloads.set_panel_open(true);
        }
    });
    window.on_downloads_show_more(|| {
        if let Some(downloads) = current() {
            let next = downloads.records_asked.get().saturating_add(RECORDS_PAGE);
            downloads.load_records(next);
        }
    });
    window.on_downloads_close_panel(|| {
        if let Some(downloads) = current() {
            downloads.set_panel_open(false);
        }
    });
    window.on_quit_prompt_confirm(|| {
        if let Some(downloads) = current() {
            downloads.answer_quit(true);
        }
    });
    window.on_quit_prompt_keep(|| {
        if let Some(downloads) = current() {
            downloads.answer_quit(false);
        }
    });
}

fn download_row(view: view::ItemView) -> DownloadRow {
    DownloadRow {
        guid: view.guid.into(),
        name: view.name.into(),
        status: view.status.into(),
        progress: view.progress,
        failed: view.failed,
        can_open: view.actions.open,
        can_show: view.actions.show,
        can_cancel: view.actions.cancel,
        can_resume: view.actions.resume,
        can_retry: view.actions.retry,
        can_remove: view.actions.remove,
    }
}

/// Sends the database writes of a change to the profile-data service. Replies only log.
fn send_ops(ops: Vec<RecordOp>) {
    if ops.is_empty() {
        return;
    }
    let Some(data) = crate::profile_data::handle() else {
        return;
    };
    for op in ops {
        match op {
            RecordOp::Insert(start) => {
                data.insert_download(start, log_failure("recording a download"));
            }
            RecordOp::Progress {
                guid,
                received,
                total,
            } => data.update_download_progress(guid, received, total),
            RecordOp::Validators {
                guid,
                final_url,
                etag,
                last_modified,
            } => data.set_download_validators(
                guid,
                final_url,
                etag,
                last_modified,
                log_failure("recording a download's validators"),
            ),
            RecordOp::TargetPath { guid, path } => data.set_download_target_path(guid, path),
            RecordOp::Finish { guid, state, error } => data.finish_download(
                guid,
                state,
                error,
                log_failure("recording how a download ended"),
            ),
            RecordOp::Resume { guid } => {
                data.resume_download(guid, log_failure("recording a resumed download"));
            }
            RecordOp::Remove { guid } => {
                data.remove_download(guid, log_failure("removing a download record"));
            }
            RecordOp::Clear => {
                data.clear_finished_downloads(log_failure("clearing the download records"));
            }
        }
    }
}

/// A reply that only logs a failure. The records are a convenience: a download goes on whether
/// or not its record could be written.
fn log_failure<T>(
    what: &'static str,
) -> impl FnOnce(Result<T, crate::profile_data::ProfileDataError>) + 'static {
    move |result| {
        if let Err(error) = result {
            log::debug!("{what}: {error}");
        }
    }
}

/// Removes an abandoned partial file; `mercury-downloads` checks the path.
fn discard(partial: Option<ResumeFrom>) {
    if let Some(partial) = partial
        && let Err(error) = remove_partial_file(&partial)
    {
        log::warn!(
            "could not remove {}: {error}",
            partial.partial_path.display()
        );
    }
}

/// A GUID when the system's random source fails: unique within this profile in practice.
fn fallback_guid() -> String {
    format!(
        "dl-{}-{}",
        crate::profile_data::now_ms(),
        std::process::id()
    )
}

/// Opens a downloaded file with its default application, through the platform's normal open
/// (LaunchServices on macOS, which honours the quarantine attribute).
fn open_file(path: &Path) {
    #[cfg(target_os = "macos")]
    spawn_detached(std::process::Command::new("/usr/bin/open").arg(path));
    #[cfg(not(target_os = "macos"))]
    spawn_detached(std::process::Command::new("xdg-open").arg(path));
}

/// Shows a downloaded file in Finder, or opens its folder elsewhere.
fn show_file(path: &Path) {
    #[cfg(target_os = "macos")]
    spawn_detached(
        std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(path),
    );
    #[cfg(not(target_os = "macos"))]
    if let Some(folder) = path.parent() {
        spawn_detached(std::process::Command::new("xdg-open").arg(folder));
    }
}

fn spawn_detached(command: &mut std::process::Command) {
    match command.spawn() {
        // Reaped on a thread of its own, so no zombie is left.
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => log::warn!("could not open the download: {error}"),
    }
}
