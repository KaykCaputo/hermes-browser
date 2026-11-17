//! Saving files: links, images and pages the user asks for, pages the engine could not show
//! (offered as downloads), the downloads list, and the question before quitting with downloads
//! running. The transfers are `mercury-downloads`' work; their records live in the profile
//! database through `profile_data`.
//!
//! Everything here runs on the UI thread. `mercury-downloads` reports from its own threads; its
//! events are queued and applied on the UI thread (`drain_events`).
//!
//! Entry points for other code:
//! - `start` once at start-up, after `profile_data::start`;
//! - `save_url` for "Save link as…" and "Save image as…" (the context menu);
//! - `save_page` for "Save page as…" (Cmd+S);
//! - `show_list` for the downloads shortcut;
//! - `take_download_offers` after the engine's events are applied (the engine pump);
//! - `sync_tab` whenever the tabs change (`sync_ui`);
//! - `confirm_quit` before the quit sequence starts shutting down;
//! - `handle_key` for Escape while the list or the question is open;
//! - `shut_down` once the event loop has ended, before `profile_data::shut_down`.

mod engine_access;
mod format;
mod location;
mod model;
mod offers;
mod progress;
mod quit;
mod request;
mod slint_callback;
mod stop;
mod view;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mercury_downloads::{DownloadManager, ResumeFrom, remove_partial_file};
use mercury_ui::{DownloadAction, DownloadRow, MainWindow};
use slint::winit_030::winit;
use slint::{ComponentHandle, ModelRc, VecModel};
use talaria_core::{BrowserCommand, BrowserController, DownloadOffer, DownloadSignal, TabId};
use url::Url;

use self::model::{DownloadList, FileFacts, NewItem, RecordOp, file_still_matches};
use self::offers::{BarWatch, Decision, Navigations, OfferFacts, OfferPolicy, Refusal};
pub(crate) use self::request::SaveError;
use self::request::{RequestContext, build_request, referrer_for, savable_url};
use self::slint_callback::InSlintCallback;
use self::stop::Inbox;
use self::view::item_view;

/// How many downloads the popover shows.
const RECENT_ROWS: usize = 6;
/// How many records the full list reads at a time.
const RECORDS_PAGE: u32 = 200;
/// `--download-offer-answer`: how long a bar stays up before the diagnostic answers it.
const DIAGNOSTIC_ANSWER_DELAY: Duration = Duration::from_millis(1500);

thread_local! {
    static DOWNLOADS: RefCell<Option<Rc<Downloads>>> = const { RefCell::new(None) };
}

/// The diagnostic answer to a download bar, `--download-offer-answer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OfferAnswer {
    Download,
    Block,
}

/// Start-up settings of the downloads work.
#[derive(Debug, Default)]
pub(crate) struct DownloadOptions {
    /// `--download-dir`: save into this folder in this run.
    pub(crate) folder: Option<PathBuf>,
    /// `--download`: start downloading this address once the browser is up.
    pub(crate) url: Option<String>,
    /// `--download-offer-answer`: answer every download bar this way, after a moment.
    pub(crate) offer_answer: Option<OfferAnswer>,
    /// A `--screenshot` or `--background` run: it never saves into the user's folders.
    pub(crate) automated: bool,
    /// The run's profile folder, if it has one.
    pub(crate) profile_root: Option<PathBuf>,
}

/// A page-started download waiting for the user's answer on its tab's bar.
#[derive(Debug, Clone)]
struct PendingOffer {
    url: Url,
    referrer: Option<Url>,
    text: String,
    detail: String,
    /// The origin of the page that started it, which Block blocks.
    initiator_site: String,
    watch: BarWatch,
}

struct Downloads {
    window: slint::Weak<MainWindow>,
    manager: RefCell<Option<DownloadManager>>,
    inbox: Arc<Inbox>,
    list: RefCell<DownloadList>,
    policy: RefCell<OfferPolicy>,
    navigations: RefCell<Navigations>,
    bars: RefCell<HashMap<TabId, PendingOffer>>,
    active_tab: Cell<Option<TabId>>,
    offer_answer: Option<OfferAnswer>,
    popover_open: Cell<bool>,
    panel_open: Cell<bool>,
    /// Something started or ended since the list was last opened.
    attention: Cell<bool>,
    /// How many records were asked of the database, and whether it had more.
    records_asked: Cell<u32>,
    more_records: Cell<bool>,
    /// What to do once the user agrees to quit.
    pending_quit: RefCell<Option<Box<dyn FnOnce()>>>,
    /// Whether the page has the keyboard (shared with the window's input handling).
    web_view_focused: Rc<Cell<bool>>,
    /// The page had the keyboard when the popover, list or question opened.
    focus_returns_to_page: Cell<bool>,
}

fn current() -> Option<Rc<Downloads>> {
    DOWNLOADS.with(|current| current.borrow().clone())
}

/// Sets up the downloads list. Call once, after `profile_data::start`.
pub(crate) fn start(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
    options: DownloadOptions,
) {
    location::configure(
        options.folder,
        options.automated,
        options.profile_root.as_deref(),
    );
    let downloads = Rc::new(Downloads {
        window: window.as_weak(),
        manager: RefCell::new(None),
        inbox: Arc::new(Inbox::default()),
        list: RefCell::new(DownloadList::default()),
        policy: RefCell::new(OfferPolicy::default()),
        navigations: RefCell::new(Navigations::default()),
        bars: RefCell::new(HashMap::new()),
        active_tab: Cell::new(None),
        offer_answer: options.offer_answer,
        popover_open: Cell::new(false),
        panel_open: Cell::new(false),
        attention: Cell::new(false),
        records_asked: Cell::new(0),
        more_records: Cell::new(false),
        pending_quit: RefCell::new(None),
        web_view_focused,
        focus_returns_to_page: Cell::new(false),
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
            let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
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
    let Some(downloads) = current() else {
        eprintln!("mercury-browser: downloads are not available in this run");
        return Err(SaveError::NotStarted);
    };
    let url = match savable_url(url) {
        Ok(url) => url,
        Err(error) => {
            downloads.note(url, &error.to_string());
            return Err(error);
        }
    };
    let page = referrer.and_then(|referrer| Url::parse(referrer).ok());
    if let Some(tab) = tab {
        log::info!("tab {}: saving a file", tab.value());
    }
    downloads.start_download(
        url,
        referrer_for(page.as_ref()),
        suggested_name.map(str::to_owned),
        false,
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
    if address.is_empty() {
        if let Some(downloads) = current() {
            downloads.note("New tab", "There is no page to save");
        }
        return;
    }
    // A refusal is already shown in the list by `save_url`.
    let _ = save_url(Some(tab), &address, None, None);
}

/// Opens or closes the full list of downloads (the downloads shortcut).
pub(crate) fn show_list() {
    if let Some(downloads) = current() {
        let open = !downloads.panel_open.get();
        downloads.set_panel_open(open);
    }
}

/// The user typed `address` into the active tab's address bar: a download it leads to was
/// asked for. Kept here, not in the shared activation record, so typing grants nothing else.
pub(crate) fn note_typed_navigation(tab: TabId, address: &str) {
    let (Some(downloads), Ok(url)) = (current(), talaria_core::resolve_address(address)) else {
        return;
    };
    downloads
        .navigations
        .borrow_mut()
        .note_typed(tab, url.as_str(), Instant::now());
}

/// Whether a key press passed to the page counts as a gesture: anything but Escape and keys
/// pressed alone as modifiers (HTML's activation-triggering `keydown`).
pub(crate) fn counts_as_activation(key: &winit::keyboard::Key) -> bool {
    use slint::winit_030::winit::keyboard::{Key, NamedKey};
    !matches!(
        key,
        Key::Named(
            NamedKey::Escape
                | NamedKey::Shift
                | NamedKey::Control
                | NamedKey::Alt
                | NamedKey::AltGraph
                | NamedKey::Super
                | NamedKey::Meta
                | NamedKey::Hyper
                | NamedKey::Fn
                | NamedKey::FnLock
                | NamedKey::CapsLock
                | NamedKey::NumLock
                | NamedKey::ScrollLock
                | NamedKey::Symbol
                | NamedKey::SymbolLock
        )
    )
}

/// After the engine's events are applied: top-level requests (who started each navigation)
/// and pages the engine could not show, offered as downloads. The tab goes back to the page
/// before (if it is still on the offered page and can), then the file is fetched at once for a
/// navigation the user started, or the tab's bar asks.
pub(crate) fn take_download_offers(controller: &RefCell<BrowserController>) {
    let signals = controller.borrow_mut().take_download_signals();
    if signals.is_empty() {
        return;
    }
    let Some(downloads) = current() else {
        return;
    };
    let now = Instant::now();
    for (tab, signal) in signals {
        match signal {
            DownloadSignal::RequestStarted {
                url,
                redirect,
                from,
            } => downloads.navigations.borrow_mut().request_started(
                tab,
                &url,
                redirect,
                from.as_deref(),
                now,
                || crate::activation::consume_user_activation(tab, now),
            ),
            DownloadSignal::Offered(offer) => downloads.offered(controller, tab, offer, now),
        }
    }
    downloads.sync();
}

/// The tabs changed: drops the bars of closed tabs and shows the active tab's bar, if any.
pub(crate) fn sync_tab(controller: &BrowserController) {
    let Some(downloads) = current() else {
        return;
    };
    let open: Vec<TabId> = controller.tabs().map(|tab| tab.id()).collect();
    let closed: Vec<TabId> = downloads
        .bars
        .borrow()
        .keys()
        .filter(|tab| !open.contains(tab))
        .copied()
        .collect();
    for tab in closed {
        downloads.bars.borrow_mut().remove(&tab);
        downloads.policy.borrow_mut().forget_tab(tab);
        downloads.navigations.borrow_mut().forget_tab(tab);
    }
    // A bar goes when its tab moves on from the page it went back to.
    downloads.bars.borrow_mut().retain(|tab, bar| {
        controller
            .tabs()
            .find(|candidate| candidate.id() == *tab)
            .is_some_and(|candidate| bar.watch.keep(candidate.state().url()))
    });
    downloads.active_tab.set(Some(controller.active_tab_id()));
    downloads.sync_bar();
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
    // Without a window there is nobody to ask.
    let Some(window) = downloads.window.upgrade() else {
        quit();
        return;
    };
    *downloads.pending_quit.borrow_mut() = Some(Box::new(quit));
    downloads.take_keyboard();
    {
        window.set_quit_prompt_title(prompt.title.into());
        window.set_quit_prompt_body(prompt.body.into());
        window.set_quit_prompt_confirm_label(prompt.confirm.into());
        window.set_quit_prompt_keep_label(prompt.keep.into());
        window.set_quit_prompt_open(true);
        window.window().request_redraw();
    }
}

/// Escape closes the question (keep downloading), the popover or the list. Return and Tab
/// reach the question's focused button through Slint. Returns whether the key was used.
pub(crate) fn handle_key(event: &winit::event::KeyEvent) -> bool {
    use slint::winit_030::winit::keyboard::{Key, NamedKey};
    if event.state != winit::event::ElementState::Pressed || event.repeat {
        return false;
    }
    if !matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
        return false;
    }
    let Some(downloads) = current() else {
        return false;
    };
    let Some(window) = downloads.window.upgrade() else {
        return false;
    };
    if window.get_quit_prompt_open() {
        downloads.answer_quit(false);
        return true;
    }
    if downloads.popover_open.get() {
        downloads.set_popover_open(false);
        return true;
    }
    if downloads.panel_open.get() {
        downloads.set_panel_open(false);
        return true;
    }
    false
}

/// After the event loop has ended: interrupts the running downloads, keeping the partial files
/// that can be resumed, records how each ended, and stops the transfers, all by `deadline`.
/// Call before `profile_data::shut_down`, so the records are written.
pub(crate) fn shut_down(deadline: Instant) {
    let Some(downloads) = DOWNLOADS.with(|current| current.borrow_mut().take()) else {
        return;
    };
    let manager = downloads.manager.borrow_mut().take();
    let Some(manager) = manager else {
        return;
    };
    let ops = stop::finish_transfers(manager, &downloads.list, &downloads.inbox, deadline);
    send_ops(ops);
}

impl Downloads {
    fn manager(&self) -> Option<std::cell::Ref<'_, DownloadManager>> {
        if self.manager.borrow().is_none() {
            let inbox = Arc::clone(&self.inbox);
            let created = DownloadManager::new(move |event| {
                if inbox.push(event) {
                    // Fails only once the event loop has ended; `shut_down` drains the rest.
                    let _ = slint::invoke_from_event_loop(|| {
                        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
                        drain_events();
                    });
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

    /// Asks the engine for the user agent and the download URL's cookies (waiting while it is
    /// busy), then starts the transfer.
    fn start_download(
        &self,
        url: Url,
        referrer: Option<Url>,
        suggested_name: Option<String>,
        refetched: bool,
    ) {
        let name_guess =
            mercury_downloads::file_name_for(None, &url, None, suggested_name.as_deref());
        let fetched = url.clone();
        let failed_name = name_guess.clone();
        engine_access::with_engine(
            url,
            Box::new(move |user_agent, cookies| {
                let Some(downloads) = current() else {
                    return;
                };
                let request = build_request(
                    fetched.clone(),
                    RequestContext {
                        referrer: referrer.clone(),
                        user_agent,
                        cookies,
                        destination_dir: location::download_folder(),
                        suggested_name: suggested_name.clone(),
                        resume: None,
                    },
                );
                let Some(transfer) = downloads.manager().map(|manager| manager.start(request))
                else {
                    downloads.note(&name_guess, "Downloads could not be started");
                    return;
                };
                let guid = crate::profile_data::new_guid().unwrap_or_else(fallback_guid);
                downloads.list.borrow_mut().begin(NewItem {
                    guid,
                    url: fetched.to_string(),
                    referrer: referrer.map(String::from),
                    name_guess,
                    transfer,
                    started_at_ms: crate::profile_data::now_ms(),
                    suggested_name,
                    refetched,
                });
                downloads.attention.set(true);
                downloads.sync();
            }),
            Box::new(move || {
                if let Some(downloads) = current() {
                    downloads.note(
                        &failed_name,
                        "The browser engine was not available; try again",
                    );
                }
            }),
        );
    }

    /// A failure shown as one line of the list, never stored.
    fn note(&self, name: &str, message: &str) {
        let guid = crate::profile_data::new_guid().unwrap_or_else(fallback_guid);
        self.list.borrow_mut().add_note(
            guid,
            String::new(),
            name.to_owned(),
            message.to_owned(),
            crate::profile_data::now_ms(),
        );
        self.attention.set(true);
        self.sync();
    }

    /// One page the engine could not show, in `tab`.
    fn offered(
        &self,
        controller: &RefCell<BrowserController>,
        tab: TabId,
        offer: DownloadOffer,
        now: Instant,
    ) {
        let Ok(url) = Url::parse(&offer.url) else {
            return;
        };
        let navigation = self.navigations.borrow_mut().take(tab, &offer.url);
        let user_started = navigation
            .as_ref()
            .is_some_and(|navigation| navigation.user_started);
        let from = navigation
            .and_then(|navigation| navigation.from)
            .and_then(|from| Url::parse(&from).ok());
        let referrer = offer
            .referrer
            .as_deref()
            .and_then(|referrer| Url::parse(referrer).ok());
        let foreground = controller.borrow().active_tab_id() == tab;
        // Back to the page before, only if the tab still shows the offered page.
        let went_back = {
            let mut controller = controller.borrow_mut();
            let on_offer = controller.tabs().any(|candidate| {
                candidate.id() == tab
                    && candidate.state().url() == offer.url
                    && candidate.state().can_go_back()
            });
            if on_offer {
                controller.queue_command_for(tab, BrowserCommand::GoBack);
            }
            on_offer
        };
        let initiator = offers::initiator(referrer.as_ref(), from.as_ref(), &url).clone();
        let initiator_site = offers::site_of(&initiator);
        let decision = self.policy.borrow_mut().decide(
            &OfferFacts {
                tab,
                url: &url,
                posted: offer.posted,
                user_started,
                foreground,
                initiator_site: &initiator_site,
            },
            now,
        );
        let name =
            mercury_downloads::file_name_for(None, &url, Some(offer.mime_type.as_str()), None);
        log::info!(
            "tab {}: a page offered a download: {decision:?}",
            tab.value()
        );
        let referrer = referrer_for(referrer.as_ref());
        match decision {
            Decision::Download => self.start_download(url, referrer, None, true),
            Decision::Ask => {
                let text = format!(
                    "{} wants to download {name}",
                    offers::host_label(&initiator)
                );
                let detail = format!("From {}", offers::host_label(&url));
                let watch = BarWatch::new(&offer.url, went_back);
                self.ask(
                    tab,
                    PendingOffer {
                        url,
                        referrer,
                        text,
                        detail,
                        initiator_site,
                        watch,
                    },
                );
            }
            Decision::Refuse(Refusal::Posted) => self.note(
                &name,
                "This file came from a form, which Mercury Browser cannot send again; it was \
                 not saved",
            ),
            Decision::Refuse(Refusal::Unsupported(scheme)) => self.note(
                &name,
                &format!("Files from “{scheme}:” addresses cannot be saved yet"),
            ),
            Decision::Refuse(Refusal::Background | Refusal::Blocked | Refusal::RateLimited) => {}
        }
    }

    /// Puts up the tab's bar for a download the user did not start.
    fn ask(&self, tab: TabId, offer: PendingOffer) {
        self.bars.borrow_mut().insert(tab, offer);
        self.sync_bar();
        if let Some(answer) = self.offer_answer {
            slint::Timer::single_shot(DIAGNOSTIC_ANSWER_DELAY, move || {
                let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
                if let Some(downloads) = current() {
                    downloads.answer_offer(tab, answer);
                }
            });
        }
    }

    fn answer_offer(&self, tab: TabId, answer: OfferAnswer) {
        let Some(offer) = self.bars.borrow_mut().remove(&tab) else {
            return;
        };
        match answer {
            OfferAnswer::Download => {
                self.start_download(offer.url, offer.referrer, None, true);
            }
            OfferAnswer::Block => self.policy.borrow_mut().block(&offer.initiator_site),
        }
        self.sync_bar();
    }

    fn dismiss_offer(&self, tab: TabId) {
        self.bars.borrow_mut().remove(&tab);
        self.sync_bar();
    }

    fn sync_bar(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let bar = self
            .active_tab
            .get()
            .and_then(|tab| self.bars.borrow().get(&tab).cloned());
        window.set_download_offer_visible(bar.is_some());
        let (text, detail) = bar.map(|bar| (bar.text, bar.detail)).unwrap_or_default();
        window.set_download_offer_text(text.into());
        window.set_download_offer_detail(detail.into());
        window.window().request_redraw();
    }

    fn apply_events(&self) {
        let before = self.list.borrow().active_count();
        let ops = stop::apply_events(&self.list, &self.inbox);
        // A finished file's identity, read right after it was published.
        let finished: Vec<(String, PathBuf)> = ops
            .iter()
            .filter_map(|op| match op {
                RecordOp::Finish {
                    guid,
                    state: mercury_store::DownloadState::Complete,
                    ..
                } => Some(guid.clone()),
                _ => None,
            })
            .filter_map(|guid| {
                let path = self.list.borrow().item(&guid)?.target_path.clone()?;
                Some((guid, path))
            })
            .collect();
        for (guid, path) in finished {
            self.list
                .borrow_mut()
                .record_identity(&guid, file_facts(&path));
        }
        if self.list.borrow().active_count() < before {
            self.attention.set(true);
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
                        .merge_records(records, file_facts);
                    downloads.sync();
                }
                Err(error) => log::info!("download records are not available: {error}"),
            }
        });
    }

    fn set_popover_open(&self, open: bool) {
        if open {
            self.refresh_files();
            self.attention.set(false);
            self.take_keyboard();
        }
        self.popover_open.set(open);
        self.sync();
        if !open {
            self.give_keyboard_back();
        }
    }

    fn set_panel_open(&self, open: bool) {
        if open {
            self.popover_open.set(false);
            self.refresh_files();
            self.attention.set(false);
            self.take_keyboard();
        }
        self.panel_open.set(open);
        self.sync();
        if !open {
            self.give_keyboard_back();
        }
    }

    /// The popover, the list or the question opens and takes the keyboard: keys stop going to
    /// the page at once (before Slint has moved the focus), and the page gets the focus back
    /// when the last of them closes, if it had it.
    fn take_keyboard(&self) {
        if self.web_view_focused.replace(false) {
            self.focus_returns_to_page.set(true);
        }
    }

    fn give_keyboard_back(&self) {
        let still_open = self.popover_open.get()
            || self.panel_open.get()
            || self
                .window
                .upgrade()
                .is_some_and(|window| window.get_quit_prompt_open());
        if still_open || !self.focus_returns_to_page.replace(false) {
            return;
        }
        if let Some(window) = self.window.upgrade() {
            window.invoke_downloads_focus_web_view();
        }
    }

    fn refresh_files(&self) {
        self.list.borrow_mut().refresh_file_presence(file_facts);
    }

    fn answer_quit(&self, confirmed: bool) {
        if let Some(window) = self.window.upgrade() {
            window.set_quit_prompt_open(false);
            window.window().request_redraw();
        }
        if !confirmed {
            self.give_keyboard_back();
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
            DownloadAction::Open | DownloadAction::Show => {
                let Some(path) = item.target_path.clone() else {
                    return;
                };
                // Still the file that was downloaded, reached without a link?
                if !file_still_matches(&item, file_facts(&path)) {
                    self.refresh_files();
                } else if action == DownloadAction::Show {
                    show_file(&path);
                } else if !format::is_unsafe_to_open(&path) {
                    open_file(&path);
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
                let Some(source) = self.list.borrow().retry_source(guid) else {
                    return;
                };
                let removed = self.list.borrow_mut().remove(guid);
                if let Some((ops, partial)) = removed {
                    send_ops(ops);
                    discard(partial);
                }
                if let Ok(url) = savable_url(&source.url) {
                    let referrer = source.referrer.as_deref().and_then(|r| Url::parse(r).ok());
                    self.start_download(url, referrer, source.suggested_name, source.refetched);
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
        let guid = item.guid.clone();
        let referrer = item.referrer.as_deref().and_then(|r| Url::parse(r).ok());
        let fetched = url.clone();
        engine_access::with_engine(
            url,
            Box::new(move |user_agent, cookies| {
                let Some(downloads) = current() else {
                    return;
                };
                let request = build_request(
                    fetched,
                    RequestContext {
                        referrer,
                        user_agent,
                        cookies,
                        destination_dir: folder,
                        suggested_name: None,
                        resume: Some(resume),
                    },
                );
                let Some(transfer) = downloads.manager().map(|manager| manager.start(request))
                else {
                    return;
                };
                let ops = downloads.list.borrow_mut().resume(&guid, transfer);
                send_ops(ops.unwrap_or_default());
                downloads.sync();
            }),
            Box::new(|| {}),
        );
    }

    fn clear(&self) {
        let (ops, partials) = self.list.borrow_mut().clear();
        // `RecordOp::Clear` removes the partial files of every interrupted record, loaded or
        // not; these are the loaded ones, removed now in case the database is unavailable.
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
        window.set_downloads_attention(self.attention.get());
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

/// Runs `work` with the downloads state from a Slint callback.
fn with_downloads(work: impl FnOnce(&Downloads)) {
    if let Some(downloads) = current() {
        work(&downloads);
    }
}

fn connect_callbacks(window: &MainWindow) {
    window.on_downloads_toggle_popover(|| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(|downloads| downloads.set_popover_open(!downloads.popover_open.get()));
    });
    window.on_downloads_close_popover(|| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(|downloads| downloads.set_popover_open(false));
    });
    window.on_downloads_action(|guid, action| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(|downloads| downloads.act(guid.as_str(), action));
    });
    window.on_downloads_clear_list(|| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(Downloads::clear);
    });
    window.on_downloads_show_all(|| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(|downloads| downloads.set_panel_open(true));
    });
    window.on_downloads_show_more(|| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(|downloads| {
            let next = downloads.records_asked.get().saturating_add(RECORDS_PAGE);
            downloads.load_records(next);
        });
    });
    window.on_downloads_close_panel(|| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(|downloads| downloads.set_panel_open(false));
    });
    window.on_quit_prompt_confirm(|| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(|downloads| downloads.answer_quit(true));
    });
    window.on_quit_prompt_keep(|| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(|downloads| downloads.answer_quit(false));
    });
    window.on_download_offer_download(|| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(|downloads| {
            if let Some(tab) = downloads.active_tab.get() {
                downloads.answer_offer(tab, OfferAnswer::Download);
            }
        });
    });
    window.on_download_offer_block(|| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(|downloads| {
            if let Some(tab) = downloads.active_tab.get() {
                downloads.answer_offer(tab, OfferAnswer::Block);
            }
        });
    });
    window.on_download_offer_dismiss(|| {
        let _in_callback = InSlintCallback::enter(); // InSlintCallback (quit lane)
        with_downloads(|downloads| {
            if let Some(tab) = downloads.active_tab.get() {
                downloads.dismiss_offer(tab);
            }
        });
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
            RecordOp::Clear => data.clear_finished_downloads_and_partials(|result| match result {
                Ok(partials) => {
                    for (target_path, partial_path) in partials {
                        discard(Some(ResumeFrom {
                            partial_path: partial_path.into(),
                            target_path: target_path.into(),
                            etag: None,
                            last_modified: None,
                        }));
                    }
                }
                Err(error) => log::debug!("clearing the download records: {error}"),
            }),
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

/// What `lstat` says about `path`; a link is reported as not a regular file.
fn file_facts(path: &Path) -> Option<FileFacts> {
    let metadata = path.symlink_metadata().ok()?;
    #[cfg(unix)]
    let id = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let id = (0, 0);
    Some(FileFacts {
        regular: metadata.file_type().is_file(),
        len: metadata.len(),
        id,
    })
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

#[cfg(test)]
mod tests {
    use slint::winit_030::winit::keyboard::{Key, NamedKey};

    use super::counts_as_activation;

    #[test]
    fn escape_and_modifiers_alone_are_not_gestures() {
        for key in [
            NamedKey::Escape,
            NamedKey::Shift,
            NamedKey::Control,
            NamedKey::Alt,
            NamedKey::Super,
            NamedKey::Meta,
            NamedKey::CapsLock,
        ] {
            assert!(!counts_as_activation(&Key::Named(key)), "{key:?}");
        }
        for key in [
            Key::Character("a".into()),
            Key::Named(NamedKey::Enter),
            Key::Named(NamedKey::Space),
            Key::Named(NamedKey::Tab),
        ] {
            assert!(counts_as_activation(&key), "{key:?}");
        }
    }
}
