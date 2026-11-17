use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use mercury_ui::MainWindow;
use slint::ComponentHandle;
use talaria_core::{
    BrowserController, CloseTabOutcome, ColorScheme, DEFAULT_ZOOM_PERCENT, TabActivation, TabEvent,
    TabId, WebView as TalariaWebView, display_origin,
};
use talaria_servo::{DocumentStartedListener, NewTabAllocator, ServoRuntime, ServoWebView};

use crate::activation::{consume_user_activation, forget_tab as forget_user_activation};
use crate::ui_sync::{keep_address_draft_for, sync_ui};

/// Set to `1` to turn pop-up blocking off for this run, as the "Block pop-ups" setting will.
/// For automated checks of page-opened tabs, which cannot click.
const ALLOW_POP_UPS_VARIABLE: &str = "MERCURY_ALLOW_POP_UPS";

/// Remembers the origins the user allowed to open pop-ups. Session-scoped today; a profile
/// store can implement it later.
pub(crate) trait PopUpPermissions {
    fn allows(&self, origin: &str) -> bool;
    fn allow(&mut self, origin: &str);
}

/// Allowed origins for this run only.
#[derive(Debug, Default)]
pub(crate) struct SessionPopUpPermissions {
    origins: HashSet<String>,
}

impl PopUpPermissions for SessionPopUpPermissions {
    fn allows(&self, origin: &str) -> bool {
        self.origins.contains(origin)
    }

    fn allow(&mut self, origin: &str) {
        self.origins.insert(origin.to_owned());
    }
}

/// What happens to a tab a page asks to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopUpDecision {
    /// Opened and selected: the user just clicked or typed in the opener.
    Foreground,
    /// Opened behind the active tab: the opener's origin is allowed, without a gesture.
    Background,
    Blocked,
}

/// Decides which tabs pages may open.
///
/// Servo 0.8.0 asks the embedder for every tab a page opens and says nothing about user
/// activation (its own pop-up blocker step is a TODO in `dom/window/windowproxy.rs`), so the
/// browser tracks activation itself (`crate::activation`): a pointer press or release, or a
/// key press other than Escape or a modifier, forwarded to a tab's page lets that page open
/// one tab within `ACTIVATION_WINDOW`. Background tabs receive no input and so can open
/// nothing, unless the user allowed their origin.
pub(crate) struct PopUpPolicy {
    /// The "Block pop-ups" setting: on (the default) applies the rules above; off lets pages
    /// open tabs at will, in front.
    pub(crate) block_pop_ups: bool,
    permissions: Box<dyn PopUpPermissions>,
    /// The opener tabs that had a pop-up blocked, with the origin shown to the user.
    blocked: HashMap<TabId, String>,
    /// The indicator needs showing or hiding.
    ui_changed: bool,
}

impl PopUpPolicy {
    pub(crate) fn new(block_pop_ups: bool, permissions: Box<dyn PopUpPermissions>) -> Self {
        Self {
            block_pop_ups,
            permissions,
            blocked: HashMap::new(),
            ui_changed: false,
        }
    }

    /// Decides on a tab `opener` asks to open; `opener_origin` is its page's origin.
    /// `consume_activation` uses up the opener's user activation if it has a recent one; it is
    /// only called while pop-ups are blocked, so allowing all pop-ups leaves activations to the
    /// file and colour pickers.
    pub(crate) fn decide(
        &mut self,
        opener: TabId,
        opener_origin: Option<&str>,
        consume_activation: impl FnOnce() -> bool,
    ) -> PopUpDecision {
        if !self.block_pop_ups || consume_activation() {
            return PopUpDecision::Foreground;
        }
        let Some(origin) = opener_origin else {
            return PopUpDecision::Blocked;
        };
        if self.permissions.allows(origin) {
            return PopUpDecision::Background;
        }
        if self.blocked.insert(opener, origin.to_owned()).as_deref() != Some(origin) {
            self.ui_changed = true;
        }
        PopUpDecision::Blocked
    }

    /// "Allow pop-ups from <origin>" was chosen in `tab_id`'s indicator.
    pub(crate) fn allow_blocked(&mut self, tab_id: TabId) {
        if let Some(origin) = self.blocked.remove(&tab_id) {
            self.permissions.allow(&origin);
        }
    }

    pub(crate) fn dismiss_blocked(&mut self, tab_id: TabId) {
        self.blocked.remove(&tab_id);
    }

    /// The origin to show in `tab_id`'s indicator now that its page is at `current_origin`.
    /// A tab that moved to another origin loses its indicator for good.
    pub(crate) fn indicator_for(
        &mut self,
        tab_id: TabId,
        current_origin: Option<&str>,
    ) -> Option<String> {
        let origin = self.blocked.get(&tab_id)?;
        if Some(origin.as_str()) == current_origin {
            return Some(origin.clone());
        }
        self.blocked.remove(&tab_id);
        None
    }

    /// `tab_id` was closed.
    pub(crate) fn forget_tab(&mut self, tab_id: TabId) {
        self.blocked.remove(&tab_id);
    }

    pub(crate) fn take_ui_changed(&mut self) -> bool {
        std::mem::take(&mut self.ui_changed)
    }
}

/// Whether pop-ups are blocked this run: always, unless `MERCURY_ALLOW_POP_UPS=1` is set *and*
/// the run is automated (`--screenshot` or `--background`).
pub(crate) fn blocks_pop_ups(automated: bool, allow_pop_ups_variable: Option<&str>) -> bool {
    !(automated && allow_pop_ups_variable == Some("1"))
}

/// Where the "Block pop-ups" switch is read: the settings lane's `block_popups()`, on every
/// decision, so a change in the settings window applies at once.
type BlockPopUpsSetting = Rc<dyn Fn() -> bool>;

/// Connects the switch to the user's setting (`block_popups`). `MERCURY_ALLOW_POP_UPS=1` in an
/// automated run overrides it to off; a warning is logged whenever it does, or is ignored.
pub(crate) fn configure_pop_up_blocking(
    automated: bool,
    block_popups_setting: impl Fn() -> bool + 'static,
) {
    let variable = std::env::var(ALLOW_POP_UPS_VARIABLE).ok();
    let setting: BlockPopUpsSetting = if blocks_pop_ups(automated, variable.as_deref()) {
        if variable.is_some() {
            log::warn!(
                "{ALLOW_POP_UPS_VARIABLE} is ignored outside --screenshot and --background runs"
            );
        }
        Rc::new(block_popups_setting)
    } else {
        log::warn!("{ALLOW_POP_UPS_VARIABLE}=1: pop-ups are not blocked in this automated run");
        Rc::new(|| false)
    };
    BLOCK_POP_UPS_SETTING.with(|current| current.replace(Some(setting)));
}

/// The switch now: the setting, or on until one is connected.
fn block_pop_ups_now() -> bool {
    let setting = BLOCK_POP_UPS_SETTING.with(|current| current.borrow().clone());
    setting.is_none_or(|setting| setting())
}

/// Where a tab the policy allowed opens. While the user types in the address field it never
/// comes to the front, so their draft stays with the tab they were typing in.
pub(crate) fn placement(decision: PopUpDecision, address_has_focus: bool) -> Option<TabActivation> {
    match decision {
        PopUpDecision::Blocked => None,
        PopUpDecision::Foreground if !address_has_focus => Some(TabActivation::Foreground),
        PopUpDecision::Foreground | PopUpDecision::Background => Some(TabActivation::Background),
    }
}

thread_local! {
    static BLOCK_POP_UPS_SETTING: RefCell<Option<BlockPopUpsSetting>> = const { RefCell::new(None) };
    /// Blocking is on until `configure_pop_up_blocking` says otherwise.
    static POP_UPS: RefCell<PopUpPolicy> = RefCell::new(PopUpPolicy::new(
        true,
        Box::new(SessionPopUpPermissions::default()),
    ));
}

/// Whether, and how, the page in `opener` may open a tab now: the decision point for the
/// "Block pop-ups" setting, whose switch is `PopUpPolicy::block_pop_ups`.
pub(crate) fn pages_may_open_tabs(
    opener: TabId,
    opener_origin: Option<&str>,
    now: Instant,
) -> PopUpDecision {
    let block_pop_ups = block_pop_ups_now();
    POP_UPS.with(|policy| {
        policy
            .try_borrow_mut()
            .map_or(PopUpDecision::Blocked, |mut policy| {
                policy.block_pop_ups = block_pop_ups;
                policy.decide(opener, opener_origin, || {
                    consume_user_activation(opener, now)
                })
            })
    })
}

/// The origin to name in the active tab's "Pop-up blocked" indicator.
pub(crate) fn blocked_pop_up_origin(tab_id: TabId, url: &str) -> Option<String> {
    let current_origin = display_origin(url);
    POP_UPS.with(|policy| {
        policy
            .try_borrow_mut()
            .ok()?
            .indicator_for(tab_id, current_origin.as_deref())
    })
}

/// Connects the "Pop-up blocked" indicator's buttons.
pub(crate) fn connect_pop_up_indicator(
    window: &MainWindow,
    controller: &Rc<RefCell<BrowserController>>,
) {
    let allow_controller = Rc::clone(controller);
    let allow_window = window.as_weak();
    window.on_allow_pop_ups(move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let tab_id = allow_controller.borrow().active_tab_id();
        POP_UPS.with(|policy| policy.borrow_mut().allow_blocked(tab_id));
        if let Some(window) = allow_window.upgrade() {
            sync_ui(&window, &allow_controller.borrow());
        }
    });
    let dismiss_controller = Rc::clone(controller);
    let dismiss_window = window.as_weak();
    window.on_dismiss_pop_up_blocked(move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let tab_id = dismiss_controller.borrow().active_tab_id();
        POP_UPS.with(|policy| policy.borrow_mut().dismiss_blocked(tab_id));
        if let Some(window) = dismiss_window.upgrade() {
            sync_ui(&window, &dismiss_controller.borrow());
        }
    });
}

/// Gives a page-opened web view its tab, if the pop-up policy allows it. Runs inside Servo's
/// callback, which must answer at once: the opener's script thread waits for it.
/// `address_has_focus` reports whether the user is typing in the address field.
pub(crate) fn new_tab_allocator(
    controller: &Rc<RefCell<BrowserController>>,
    address_has_focus: impl Fn() -> bool + 'static,
) -> NewTabAllocator {
    let controller = Rc::downgrade(controller);
    Rc::new(move |opener| allocate_page_tab(&controller, opener, address_has_focus()))
}

fn allocate_page_tab(
    controller: &Weak<RefCell<BrowserController>>,
    opener: TabId,
    address_has_focus: bool,
) -> Option<TabId> {
    let controller = controller.upgrade()?;
    let Ok(mut controller) = controller.try_borrow_mut() else {
        log::warn!(
            "tab {}: refused a tab opened by the page: the browser state is busy",
            opener.value()
        );
        return None;
    };
    let opener_origin = controller
        .tabs()
        .find(|tab| tab.id() == opener)
        .and_then(|tab| display_origin(tab.state().url()));
    let decision = pages_may_open_tabs(opener, opener_origin.as_deref(), Instant::now());
    let Some(activation) = placement(decision, address_has_focus) else {
        log::info!("tab {}: blocked a pop-up", opener.value());
        return None;
    };
    let tab_id = controller.adopt_tab(opener, activation)?;
    if activation == TabActivation::Foreground {
        keep_address_draft_for(tab_id);
    }
    Some(tab_id)
}

/// Called by the engine, synchronously inside its event loop, whenever a tab starts loading a
/// new document (Servo's `LoadStatus::Started`, also when it interrupts a load that had parsed
/// its head). The activation from the last document, often the click on the link, is dropped
/// before any script of the new document runs, so the new page cannot use it.
pub(crate) fn document_started_listener() -> DocumentStartedListener {
    Rc::new(forget_user_activation)
}

/// `tab_id` was closed: its blocked-pop-up indicator goes too.
pub(crate) fn forget_tab(tab_id: TabId) {
    POP_UPS.with(|policy| {
        if let Ok(mut policy) = policy.try_borrow_mut() {
            policy.forget_tab(tab_id);
        }
    });
}

/// What `adopt_page_web_views` changed.
pub(crate) struct Adoption {
    /// Web views pages opened were added.
    pub(crate) web_views_added: bool,
    /// The tab strip or the pop-up indicator must be redrawn.
    pub(crate) ui_changed: bool,
}

/// Takes the web views pages opened during the last spin, so the creation loop sees that
/// their tabs have one, and gives each the tab's zoom and the colour scheme.
pub(crate) fn adopt_page_web_views(
    runtime: &ServoRuntime,
    controller: &RefCell<BrowserController>,
    web_views: &mut HashMap<TabId, ServoWebView>,
    event_receivers: &mut HashMap<TabId, Receiver<TabEvent>>,
) -> Adoption {
    let adopted = runtime.take_adopted_web_views();
    let web_views_added = !adopted.is_empty();
    for (tab_id, mut web_view, event_receiver) in adopted {
        let Some((zoom_percent, color_scheme)) = tab_settings(&controller.borrow(), tab_id) else {
            continue;
        };
        if zoom_percent != DEFAULT_ZOOM_PERCENT {
            let _zoom_result = web_view.set_page_zoom(zoom_percent);
        }
        let _scheme_result = web_view.set_color_scheme(color_scheme);
        web_views.insert(tab_id, web_view);
        event_receivers.insert(tab_id, event_receiver);
    }
    let ui_changed = POP_UPS.with(|policy| {
        policy
            .try_borrow_mut()
            .is_ok_and(|mut policy| policy.take_ui_changed())
    });
    Adoption {
        web_views_added,
        ui_changed: web_views_added || ui_changed,
    }
}

fn tab_settings(controller: &BrowserController, tab_id: TabId) -> Option<(u16, ColorScheme)> {
    let tab = controller.tabs().find(|tab| tab.id() == tab_id)?;
    Some((tab.state().zoom_percent(), controller.color_scheme()))
}

/// The page in `tab_id` closed its tab. Returns whether the tabs changed. A page never quits
/// the browser: if it closes the last tab, a blank tab takes its place.
pub(crate) fn close_page_tab(controller: &mut BrowserController, tab_id: TabId) -> bool {
    match controller.close_tab(tab_id) {
        CloseTabOutcome::Closed => true,
        CloseTabOutcome::LastTab => {
            controller.open_tab();
            controller.close_tab(tab_id) == CloseTabOutcome::Closed
        }
        CloseTabOutcome::NotFound => false,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use talaria_core::{BrowserEvent, LoadState};

    use super::*;
    use crate::activation::{ACTIVATION_WINDOW, Activations};

    const OPENER: TabId = TabId::new(1);
    const ORIGIN: Option<&str> = Some("https://a.test");

    fn policy() -> PopUpPolicy {
        PopUpPolicy::new(true, Box::new(SessionPopUpPermissions::default()))
    }

    /// Decides with `activations` standing in for the per-tab activation state.
    fn decide(
        policy: &mut PopUpPolicy,
        activations: &mut Activations,
        opener: TabId,
        now: Instant,
    ) -> PopUpDecision {
        policy.decide(opener, ORIGIN, || activations.consume(opener, now))
    }

    #[test]
    fn a_gesture_lets_the_page_open_one_tab_in_front() {
        let (mut policy, mut activations) = (policy(), Activations::default());
        let now = Instant::now();
        activations.note(OPENER, now);

        assert_eq!(
            decide(
                &mut policy,
                &mut activations,
                OPENER,
                now + Duration::from_millis(200)
            ),
            PopUpDecision::Foreground
        );
    }

    #[test]
    fn a_second_pop_up_from_one_gesture_is_refused() {
        let (mut policy, mut activations) = (policy(), Activations::default());
        let now = Instant::now();
        activations.note(OPENER, now);

        assert_eq!(
            decide(&mut policy, &mut activations, OPENER, now),
            PopUpDecision::Foreground
        );
        assert_eq!(
            decide(&mut policy, &mut activations, OPENER, now),
            PopUpDecision::Blocked
        );
    }

    #[test]
    fn a_background_tab_cannot_open_a_tab() {
        let (mut policy, mut activations) = (policy(), Activations::default());
        let now = Instant::now();
        let active = TabId::new(2);
        // Only the active tab ever receives input.
        activations.note(active, now);

        assert_eq!(
            decide(&mut policy, &mut activations, OPENER, now),
            PopUpDecision::Blocked
        );
        assert_eq!(
            decide(&mut policy, &mut activations, active, now),
            PopUpDecision::Foreground
        );
    }

    #[test]
    fn an_activation_older_than_a_second_is_refused() {
        let (mut policy, mut activations) = (policy(), Activations::default());
        let now = Instant::now();
        activations.note(OPENER, now);

        assert_eq!(
            decide(
                &mut policy,
                &mut activations,
                OPENER,
                now + ACTIVATION_WINDOW
            ),
            PopUpDecision::Blocked
        );
    }

    #[test]
    fn an_allowed_origin_opens_in_the_background() {
        let (mut policy, mut activations) = (policy(), Activations::default());
        let now = Instant::now();
        assert_eq!(
            decide(&mut policy, &mut activations, OPENER, now),
            PopUpDecision::Blocked
        );
        assert_eq!(policy.indicator_for(OPENER, ORIGIN).as_deref(), ORIGIN);

        policy.allow_blocked(OPENER);

        assert_eq!(policy.indicator_for(OPENER, ORIGIN).as_deref(), None);
        assert_eq!(
            decide(&mut policy, &mut activations, OPENER, now),
            PopUpDecision::Background
        );
        // Another tab of the same origin is allowed too; a gesture still opens in front.
        assert_eq!(
            decide(&mut policy, &mut activations, TabId::new(3), now),
            PopUpDecision::Background
        );
        activations.note(OPENER, now);
        assert_eq!(
            decide(&mut policy, &mut activations, OPENER, now),
            PopUpDecision::Foreground
        );
    }

    #[test]
    fn a_blocked_pop_up_is_reported_for_the_openers_origin_until_dismissed_or_it_navigates_away() {
        let (mut policy, mut activations) = (policy(), Activations::default());
        let now = Instant::now();
        assert_eq!(
            decide(&mut policy, &mut activations, OPENER, now),
            PopUpDecision::Blocked
        );
        assert!(policy.take_ui_changed());
        assert!(!policy.take_ui_changed());

        assert_eq!(policy.indicator_for(OPENER, ORIGIN).as_deref(), ORIGIN);
        assert_eq!(policy.indicator_for(TabId::new(2), ORIGIN).as_deref(), None);

        policy.dismiss_blocked(OPENER);
        assert_eq!(policy.indicator_for(OPENER, ORIGIN).as_deref(), None);
        assert_eq!(
            decide(&mut policy, &mut activations, OPENER, now),
            PopUpDecision::Blocked
        );
    }

    #[test]
    fn repeated_blocked_attempts_only_redraw_when_the_origin_shown_changes() {
        let mut policy = policy();
        assert_eq!(
            policy.decide(OPENER, ORIGIN, || false),
            PopUpDecision::Blocked
        );
        assert!(policy.take_ui_changed());

        for _ in 0..10 {
            assert_eq!(
                policy.decide(OPENER, ORIGIN, || false),
                PopUpDecision::Blocked
            );
        }
        assert!(!policy.take_ui_changed());

        let other = Some("https://b.test");
        assert_eq!(
            policy.decide(OPENER, other, || false),
            PopUpDecision::Blocked
        );
        assert!(policy.take_ui_changed());
    }

    #[test]
    fn closing_or_leaving_the_origin_drops_the_indicator() {
        let mut policy = policy();
        let _ = policy.decide(OPENER, ORIGIN, || false);
        let _ = policy.decide(TabId::new(2), ORIGIN, || false);

        assert_eq!(
            policy.indicator_for(OPENER, ORIGIN),
            ORIGIN.map(str::to_owned)
        );
        // The tab navigated elsewhere: gone, also after coming back.
        assert_eq!(policy.indicator_for(OPENER, Some("https://b.test")), None);
        assert_eq!(policy.indicator_for(OPENER, ORIGIN), None);

        policy.forget_tab(TabId::new(2));
        assert_eq!(policy.indicator_for(TabId::new(2), ORIGIN), None);
    }

    #[test]
    fn a_pop_up_opens_behind_while_the_address_field_has_focus() {
        assert_eq!(
            placement(PopUpDecision::Foreground, false),
            Some(TabActivation::Foreground)
        );
        assert_eq!(
            placement(PopUpDecision::Foreground, true),
            Some(TabActivation::Background)
        );
        assert_eq!(
            placement(PopUpDecision::Background, false),
            Some(TabActivation::Background)
        );
        assert_eq!(placement(PopUpDecision::Blocked, true), None);
    }

    #[test]
    fn pop_ups_are_allowed_only_in_automated_runs_with_the_variable_set() {
        assert!(blocks_pop_ups(false, None));
        assert!(blocks_pop_ups(false, Some("1")));
        assert!(blocks_pop_ups(true, None));
        assert!(blocks_pop_ups(true, Some("0")));
        assert!(!blocks_pop_ups(true, Some("1")));
    }

    #[test]
    fn a_page_without_an_origin_is_blocked_without_an_indicator() {
        let mut policy = policy();

        assert_eq!(
            policy.decide(OPENER, None, || false),
            PopUpDecision::Blocked
        );
        assert_eq!(policy.indicator_for(OPENER, None).as_deref(), None);
    }

    #[test]
    fn with_blocking_off_every_page_opens_tabs_in_front_and_keeps_its_activation() {
        let mut policy = PopUpPolicy::new(false, Box::new(SessionPopUpPermissions::default()));
        let mut activations = Activations::default();
        let now = Instant::now();
        activations.note(OPENER, now);

        assert_eq!(
            decide(&mut policy, &mut activations, OPENER, now),
            PopUpDecision::Foreground
        );
        assert_eq!(
            policy.decide(OPENER, None, || false),
            PopUpDecision::Foreground
        );
        assert!(activations.consume(OPENER, now));
    }

    #[test]
    fn the_decision_point_uses_up_the_tabs_user_activation() {
        POP_UPS.with(|current| *current.borrow_mut() = policy());
        let tab_id = TabId::new(77);
        let now = Instant::now();
        crate::activation::note_user_activation(tab_id, now);

        assert_eq!(
            pages_may_open_tabs(tab_id, ORIGIN, now),
            PopUpDecision::Foreground
        );
        assert_eq!(
            pages_may_open_tabs(tab_id, ORIGIN, now),
            PopUpDecision::Blocked
        );
    }

    #[test]
    fn an_activation_is_forgotten_when_a_document_starts_while_another_is_loading() {
        let mut controller = BrowserController::default();
        let tab_id = controller.active_tab_id();
        // The current page has parsed its head but is still loading (ads, say).
        controller.handle_event(
            tab_id,
            BrowserEvent::LoadStateChanged {
                state: LoadState::Loading,
            },
        );
        let now = Instant::now();
        crate::activation::note_user_activation(tab_id, now);

        // The user's click on a link starts the next document.
        document_started_listener()(tab_id);

        assert!(!crate::activation::consume_user_activation(tab_id, now));
    }

    #[test]
    fn a_document_starting_within_the_spin_reaches_the_activation_before_the_allocator() {
        POP_UPS.with(|current| *current.borrow_mut() = policy());
        let controller = Rc::new(RefCell::new(BrowserController::default()));
        let opener = controller.borrow().active_tab_id();
        let allocator = new_tab_allocator(&controller, || false);
        let listener = document_started_listener();

        // Within one engine spin: the click's document starts, then its script opens a tab.
        crate::activation::note_user_activation(opener, Instant::now());
        listener(opener);
        assert_eq!(allocator(opener), None);

        // Without a new document, the same click lets the page open its tab.
        crate::activation::note_user_activation(opener, Instant::now());
        assert!(allocator(opener).is_some());
    }

    #[test]
    fn the_switch_follows_the_users_setting_at_each_decision() {
        POP_UPS.with(|current| *current.borrow_mut() = policy());
        let setting = Rc::new(std::cell::Cell::new(true));
        let read = Rc::clone(&setting);
        configure_pop_up_blocking(false, move || read.get());
        let opener = TabId::new(88);

        assert_eq!(
            pages_may_open_tabs(opener, ORIGIN, Instant::now()),
            PopUpDecision::Blocked
        );
        setting.set(false);
        assert_eq!(
            pages_may_open_tabs(opener, ORIGIN, Instant::now()),
            PopUpDecision::Foreground
        );
    }

    fn ready_controller() -> (BrowserController, TabId) {
        let mut controller = BrowserController::default();
        let tab_id = controller.active_tab_id();
        controller.handle_event(tab_id, BrowserEvent::AddressChanged { url: String::new() });
        (controller, tab_id)
    }

    #[test]
    fn a_page_closing_its_tab_returns_to_the_opener() {
        let (mut controller, opener) = ready_controller();
        let adopted = controller
            .adopt_tab(opener, TabActivation::Foreground)
            .expect("the opener exists");

        assert!(close_page_tab(&mut controller, adopted));
        assert_eq!(controller.active_tab_id(), opener);
        assert_eq!(controller.tabs().len(), 1);
    }

    #[test]
    fn a_page_closing_the_last_tab_leaves_a_blank_tab_instead_of_quitting() {
        let (mut controller, tab_id) = ready_controller();

        assert!(close_page_tab(&mut controller, tab_id));

        assert_eq!(controller.tabs().len(), 1);
        let blank = controller.active_tab_id();
        assert_ne!(blank, tab_id);
        assert_eq!(controller.state().url(), "");
        assert_eq!(controller.state().page_title(), "New Tab");
    }

    #[test]
    fn the_allocator_refuses_while_the_browser_state_is_borrowed() {
        let controller = Rc::new(RefCell::new(BrowserController::default()));
        let opener = controller.borrow().active_tab_id();
        let allocator = new_tab_allocator(&controller, || false);

        let _borrowed = controller.borrow();
        assert_eq!(allocator(opener), None);
    }

    #[test]
    fn the_allocator_refuses_once_the_browser_state_is_gone() {
        let controller = Rc::new(RefCell::new(BrowserController::default()));
        let opener = controller.borrow().active_tab_id();
        let allocator = new_tab_allocator(&controller, || false);
        drop(controller);

        assert_eq!(allocator(opener), None);
    }
}
