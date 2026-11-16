//! Requests a page makes that wait for the user: JavaScript dialogs, the "leave this page?"
//! prompt, HTTP sign-in and permission prompts.
//!
//! Each tab keeps its own queue. The front request of the active tab is the one shown; the others
//! wait, in arrival order. Every request leaves the queue exactly once, either answered by the user
//! or by a rule here, or discarded (`PageRequestAnswer::Discard`), which gives the engine's
//! default answer. A tab's requests are discarded when the user navigates, reloads or traverses
//! it, when a new document starts loading in it, when its page crashes, and when its web view is
//! replaced or closed (the engine drops those web views' requests itself).

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::time::Duration;

use url::Url;

/// A dialog that arrives within this time after the page's previous dialog was closed (or while
/// another of its dialogs is waiting) offers "Don't allow this page to show more dialogs".
/// Three seconds, as Firefox's `dom.successive_dialog_time_limit`: a page that reopens a dialog as
/// soon as it is closed always falls inside it, a person reading and answering rarely does.
pub const SUCCESSIVE_DIALOG_INTERVAL: Duration = Duration::from_secs(3);

/// A sign-in for an origin that arrives within this time after the user sent credentials to it
/// is taken as the site rejecting them.
pub const SIGN_IN_RETRY_INTERVAL: Duration = Duration::from_secs(10);

/// Combining marks kept after one base character; more are dropped so that stacked marks cannot
/// draw over the text around the message.
pub const MAX_COMBINING_MARKS: usize = 3;

/// Longest page-supplied message kept, in characters; the rest is cut off with an ellipsis.
pub const MAX_MESSAGE_CHARS: usize = 10_000;
/// Longest default value of a `prompt()` kept, in characters.
pub const MAX_PROMPT_VALUE_CHARS: usize = 2_000;

/// Identifies one request; unique for the life of the browser process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageRequestId(u64);

impl PageRequestId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// What a page asks permission to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PagePermission {
    Geolocation,
    Notifications,
    Push,
    Midi,
    Camera,
    Microphone,
    Speaker,
    DeviceInfo,
    BackgroundSync,
    Bluetooth,
    PersistentStorage,
    ScreenWakeLock,
    Gamepad,
}

impl PagePermission {
    /// Whether the user is asked. Everything else is denied without a prompt, including any
    /// variant added later. Only persistent storage does something in Mercury today:
    /// - notifications, push, background sync: no notification or service-worker UI yet;
    /// - camera, microphone, speaker, device info: WebRTC and `getUserMedia` are off;
    /// - MIDI, Bluetooth, gamepad: no device support is built or exposed;
    /// - geolocation and screen wake lock: their engine preferences (`dom_geolocation_enabled`,
    ///   `dom_wakelock_enabled`) are off. Add them here when they are turned on.
    ///
    /// A page can still name any of these in `navigator.permissions.request()`.
    pub const fn is_promptable(self) -> bool {
        matches!(self, Self::PersistentStorage)
    }

    /// Completes "<site> wants to …".
    pub const fn description(self) -> &'static str {
        match self {
            Self::Geolocation => "know your location",
            Self::Notifications => "show notifications",
            Self::Push => "send you push messages",
            Self::Midi => "use your MIDI devices",
            Self::Camera => "use your camera",
            Self::Microphone => "use your microphone",
            Self::Speaker => "choose your speakers",
            Self::DeviceInfo => "see your media devices",
            Self::BackgroundSync => "sync in the background",
            Self::Bluetooth => "use Bluetooth devices",
            Self::PersistentStorage => "store data permanently on this device",
            Self::ScreenWakeLock => "keep your screen on",
            Self::Gamepad => "use your game controllers",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageRequestKind {
    Alert {
        message: String,
    },
    Confirm {
        message: String,
    },
    Prompt {
        message: String,
        default_value: String,
    },
    /// The page's `beforeunload` handler asks to confirm leaving it.
    LeavePage,
    /// A site (or, with `for_proxy`, a proxy) asks for a user name and password.
    Authentication {
        for_proxy: bool,
    },
    Permission(PagePermission),
}

impl PageRequestKind {
    /// `alert()`, `confirm()` and `prompt()`.
    pub const fn is_simple_dialog(&self) -> bool {
        matches!(
            self,
            Self::Alert { .. } | Self::Confirm { .. } | Self::Prompt { .. }
        )
    }

    /// Requests "Don't allow this page to show more dialogs" stops.
    const fn is_blockable_dialog(&self) -> bool {
        self.is_simple_dialog() || matches!(self, Self::LeavePage)
    }

    fn sanitized(self) -> Self {
        match self {
            Self::Alert { message } => Self::Alert {
                message: sanitize_page_text(&message, true, MAX_MESSAGE_CHARS),
            },
            Self::Confirm { message } => Self::Confirm {
                message: sanitize_page_text(&message, true, MAX_MESSAGE_CHARS),
            },
            Self::Prompt {
                message,
                default_value,
            } => Self::Prompt {
                message: sanitize_page_text(&message, true, MAX_MESSAGE_CHARS),
                default_value: sanitize_page_text(&default_value, false, MAX_PROMPT_VALUE_CHARS),
            },
            other => other,
        }
    }
}

/// A request as the engine reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    pub id: PageRequestId,
    /// The page that asked; for a sign-in, the URL that needs the credentials.
    pub url: String,
    pub kind: PageRequestKind,
    /// When the engine received it, on the same clock as the `now` given to
    /// `BrowserController::answer_page_request`.
    pub received_at: Duration,
}

/// A user name and password for HTTP authentication. `Debug` never shows the password.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpCredentials {
    username: String,
    password: String,
}

impl HttpCredentials {
    pub const fn new(username: String, password: String) -> Self {
        Self { username, password }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for HttpCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpCredentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageRequestAnswer {
    /// OK, Leave, Allow; for a prompt, OK with its default value.
    Accept,
    /// Cancel, Stay, Block; for a sign-in, continue without credentials.
    Decline,
    /// OK on a prompt, with the text entered.
    PromptText(String),
    /// Sign in with these credentials.
    Credentials(HttpCredentials),
    /// Give the engine's default answer: OK for an alert, Cancel for confirm and prompt, Leave for
    /// a leave prompt, no credentials, permission denied.
    Discard,
}

/// The user's answer and the choices that came with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequestReply {
    answer: PageRequestAnswer,
    remember_for_site: bool,
    block_further_dialogs: bool,
}

impl PageRequestReply {
    pub const fn new(answer: PageRequestAnswer) -> Self {
        Self {
            answer,
            remember_for_site: false,
            block_further_dialogs: false,
        }
    }

    /// For a permission: answer later requests for the same permission from the same site the
    /// same way, for the rest of the session.
    pub const fn remembering_for_site(mut self) -> Self {
        self.remember_for_site = true;
        self
    }

    /// For a dialog: answer the page's further dialogs with their defaults until it navigates.
    pub const fn blocking_further_dialogs(mut self) -> Self {
        self.block_further_dialogs = true;
        self
    }

    pub(crate) fn into_answer(self) -> PageRequestAnswer {
        self.answer
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Block,
}

impl PermissionDecision {
    pub(crate) const fn answer(self) -> PageRequestAnswer {
        match self {
            Self::Allow => PageRequestAnswer::Accept,
            Self::Block => PageRequestAnswer::Decline,
        }
    }

    const fn from_answer(answer: &PageRequestAnswer) -> Option<Self> {
        match answer {
            PageRequestAnswer::Accept => Some(Self::Allow),
            PageRequestAnswer::Decline => Some(Self::Block),
            _ => None,
        }
    }
}

/// Where remembered permission decisions live. `origin` is a serialised tuple origin such as
/// `https://example.com`. The session store below forgets everything at exit; a store backed by
/// the profile can replace it through `BrowserController::set_permission_decisions`.
pub trait PermissionDecisions: fmt::Debug {
    fn decision(&self, origin: &str, permission: PagePermission) -> Option<PermissionDecision>;
    fn remember(&mut self, origin: &str, permission: PagePermission, decision: PermissionDecision);
}

/// Remembers decisions until the browser exits.
#[derive(Debug, Default)]
pub struct SessionPermissionDecisions {
    decisions: HashMap<(String, PagePermission), PermissionDecision>,
}

impl PermissionDecisions for SessionPermissionDecisions {
    fn decision(&self, origin: &str, permission: PagePermission) -> Option<PermissionDecision> {
        self.decisions
            .get(&(origin.to_owned(), permission))
            .copied()
    }

    fn remember(&mut self, origin: &str, permission: PagePermission, decision: PermissionDecision) {
        self.decisions
            .insert((origin.to_owned(), permission), decision);
    }
}

/// Who is asking, in the form the user reads and the form decisions are remembered under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageOrigin {
    label: String,
    key: Option<String>,
    secure: bool,
}

impl PageOrigin {
    pub fn from_url(url: &str) -> Self {
        let Ok(url) = Url::parse(url) else {
            return Self::unnamed();
        };
        // The origin, not the URL: `blob:` URLs name their creator's origin, and user info is
        // never part of it.
        let origin = url.origin();
        let url::Origin::Tuple(scheme, host, port) = &origin else {
            return Self::unnamed();
        };
        // Hosts serialise as ASCII: internationalised names appear in their punycode form, so a
        // look-alike name cannot pass for another; IPv6 addresses keep their brackets.
        let label = if url::Url::parse(&format!("{scheme}://{host}/"))
            .ok()
            .and_then(|default| default.port_or_known_default())
            == Some(*port)
        {
            host.to_string()
        } else {
            format!("{host}:{port}")
        };
        Self {
            label,
            key: Some(origin.ascii_serialization()),
            secure: scheme == "https",
        }
    }

    fn unnamed() -> Self {
        Self {
            label: "This page".to_owned(),
            key: None,
            secure: false,
        }
    }

    /// The host (and port, when given), or "This page" for a page without one (a file, `data:`).
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The serialised origin, or `None` for an opaque one; decisions are only remembered for
    /// pages that have one.
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Whether the connection to the origin is HTTPS.
    pub const fn is_secure(&self) -> bool {
        self.secure
    }
}

/// A queued request, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPageRequest {
    id: PageRequestId,
    kind: PageRequestKind,
    origin: PageOrigin,
    offers_to_block_dialogs: bool,
    for_reload: bool,
    credentials_rejected: bool,
}

impl PendingPageRequest {
    pub const fn id(&self) -> PageRequestId {
        self.id
    }

    /// The request, with page-supplied text already sanitised for display.
    pub const fn kind(&self) -> &PageRequestKind {
        &self.kind
    }

    pub const fn origin(&self) -> &PageOrigin {
        &self.origin
    }

    /// Whether to offer "Don't allow this page to show more dialogs" (for a leave prompt,
    /// "Don't allow this page to ask again").
    pub const fn offers_to_block_dialogs(&self) -> bool {
        self.offers_to_block_dialogs
    }

    /// For a leave prompt: the user asked to reload the page.
    pub const fn is_reload_prompt(&self) -> bool {
        self.for_reload
    }

    /// For a sign-in: the user sent credentials to this origin moments ago, so the site most
    /// likely rejected them.
    pub const fn credentials_were_rejected(&self) -> bool {
        self.credentials_rejected
    }
}

#[derive(Debug, Clone)]
struct QueuedRequest {
    pending: PendingPageRequest,
}

/// One tab's requests and the per-page state their rules need.
#[derive(Debug, Default)]
pub(crate) struct PageRequests {
    queue: VecDeque<QueuedRequest>,
    dialogs_blocked: bool,
    last_dialog_closed_at: Option<Duration>,
    /// Whether the user has clicked or typed in the current document; a page the user never
    /// interacted with may not ask before it is left.
    user_activated: bool,
    /// "Don't allow this page to ask again", chosen on a leave prompt.
    leave_prompts_blocked: bool,
    last_leave_prompt_closed_at: Option<Duration>,
    /// The user's latest navigation of this page was a reload.
    reload_requested: bool,
    /// The origin the user last sent credentials to, and when.
    credentials_sent: Option<(String, Duration)>,
    /// The page whose dialogs or leave prompts the user blocked (its origin, or `None` for a page
    /// without one).
    blocked_origin: Option<String>,
    /// The user navigated away from the blocked page: the blocks end once a new document starts or
    /// another site asks. They stay if the navigation produces no new document (a same-document
    /// traversal, a 204, a download) or the user stays.
    blocks_end_with_navigation: bool,
}

impl PageRequests {
    pub(crate) fn front(&self) -> Option<PendingPageRequest> {
        let front = self.queue.front()?;
        let mut pending = front.pending.clone();
        let same_group = |kind: &PageRequestKind| match pending.kind {
            PageRequestKind::LeavePage => *kind == PageRequestKind::LeavePage,
            _ => kind.is_simple_dialog(),
        };
        if pending.kind.is_blockable_dialog() {
            pending.offers_to_block_dialogs |= self
                .queue
                .iter()
                .skip(1)
                .any(|queued| same_group(&queued.pending.kind));
        }
        Some(pending)
    }

    /// Queues the request, or returns the answer it gets without asking.
    pub(crate) fn receive(
        &mut self,
        request: PageRequest,
        decisions: &dyn PermissionDecisions,
    ) -> Option<PageRequestAnswer> {
        let origin = PageOrigin::from_url(&request.url);
        let kind = request.kind.sanitized();
        // Only dialogs and leave prompts name the tab's page; a sign-in names the URL that
        // answered 401, which may be a frame's on another site, and must not lift the block.
        if self.blocks_end_with_navigation
            && kind.is_blockable_dialog()
            && origin.key() != self.blocked_origin.as_deref()
        {
            self.end_blocks();
        }

        if kind.is_blockable_dialog() && self.dialogs_blocked {
            return Some(PageRequestAnswer::Discard);
        }
        match &kind {
            PageRequestKind::LeavePage if self.leave_prompts_blocked => {
                return Some(PageRequestAnswer::Discard);
            }
            PageRequestKind::LeavePage if !self.user_activated => {
                return Some(PageRequestAnswer::Accept);
            }
            // Features Mercury does not offer (notifications among them: there is no
            // notification display yet) are denied without a prompt.
            PageRequestKind::Permission(permission) if !permission.is_promptable() => {
                return Some(PageRequestAnswer::Decline);
            }
            PageRequestKind::Permission(permission) => {
                if let Some(decision) = origin
                    .key()
                    .and_then(|key| decisions.decision(key, *permission))
                {
                    return Some(decision.answer());
                }
            }
            _ => {}
        }

        let soon_after = |closed_at: Option<Duration>| {
            closed_at.is_some_and(|closed_at| {
                request.received_at.saturating_sub(closed_at) < SUCCESSIVE_DIALOG_INTERVAL
            })
        };
        let offers_to_block_dialogs = match kind {
            PageRequestKind::LeavePage => soon_after(self.last_leave_prompt_closed_at),
            _ => kind.is_simple_dialog() && soon_after(self.last_dialog_closed_at),
        };
        let for_reload = kind == PageRequestKind::LeavePage && self.reload_requested;
        let credentials_rejected = matches!(kind, PageRequestKind::Authentication { .. })
            && self
                .credentials_sent
                .as_ref()
                .is_some_and(|(sent_to, sent_at)| {
                    origin.key() == Some(sent_to.as_str())
                        && request.received_at.saturating_sub(*sent_at) < SIGN_IN_RETRY_INTERVAL
                });
        self.queue.push_back(QueuedRequest {
            pending: PendingPageRequest {
                id: request.id,
                kind,
                origin,
                offers_to_block_dialogs,
                for_reload,
                credentials_rejected,
            },
        });
        None
    }

    /// Takes the request out of the queue. Also returns the ids of requests that the reply
    /// discards (further dialogs, when it blocks them).
    pub(crate) fn answer(
        &mut self,
        id: PageRequestId,
        reply: &PageRequestReply,
        now: Duration,
    ) -> Option<(PendingPageRequest, Vec<PageRequestId>)> {
        let index = self
            .queue
            .iter()
            .position(|queued| queued.pending.id == id)?;
        let answered = self.queue.remove(index)?.pending;
        let mut discarded = Vec::new();
        match &answered.kind {
            PageRequestKind::LeavePage => {
                self.last_leave_prompt_closed_at = Some(now);
                // Answered: a later leave prompt comes from another navigation.
                self.reload_requested = false;
                // Staying: the page is not left, so its blocks stay too.
                if reply.answer == PageRequestAnswer::Decline {
                    self.blocks_end_with_navigation = false;
                }
                if reply.block_further_dialogs {
                    self.leave_prompts_blocked = true;
                    self.blocked_origin = answered.origin.key().map(str::to_owned);
                    discarded = self.take_where(|kind| *kind == PageRequestKind::LeavePage);
                }
            }
            kind if kind.is_simple_dialog() => {
                self.last_dialog_closed_at = Some(now);
                if reply.block_further_dialogs {
                    self.dialogs_blocked = true;
                    self.blocked_origin = answered.origin.key().map(str::to_owned);
                    discarded = self.take_where(PageRequestKind::is_blockable_dialog);
                }
            }
            PageRequestKind::Authentication { .. } => {
                self.credentials_sent = match (&reply.answer, answered.origin.key()) {
                    (PageRequestAnswer::Credentials(_), Some(key)) => Some((key.to_owned(), now)),
                    _ => None,
                };
            }
            _ => {}
        }
        Some((answered, discarded))
    }

    /// Removes waiting permission requests that a newly remembered decision now answers.
    pub(crate) fn take_decided(
        &mut self,
        origin_key: &str,
        permission: PagePermission,
    ) -> Vec<PageRequestId> {
        let mut taken = Vec::new();
        self.queue.retain(|queued| {
            let decided = queued.pending.kind == PageRequestKind::Permission(permission)
                && queued.pending.origin.key() == Some(origin_key);
            if decided {
                taken.push(queued.pending.id);
            }
            !decided
        });
        taken
    }

    /// Discards every request; the caller tells the engine.
    pub(crate) fn discard_all(&mut self) -> Vec<PageRequestId> {
        self.take_where(|_| true)
    }

    /// A new document started loading: the old one's requests and per-page state go.
    pub(crate) fn start_new_document(&mut self) -> Vec<PageRequestId> {
        let credentials_sent = self.credentials_sent.take();
        let discarded = self.discard_all();
        *self = Self {
            // A sign-in that loads the next document fails with a new request for it.
            credentials_sent,
            ..Self::default()
        };
        discarded
    }

    /// The user navigates the page; `reload` when they reload it. The page's blocks are only
    /// marked to end: they end when the new document starts, or when a request from another site
    /// arrives first (a page stuck before its head was parsed may never report the next
    /// document, and the next site's dialogs must not inherit its blocks).
    pub(crate) fn note_user_navigation(&mut self, reload: bool) -> Vec<PageRequestId> {
        self.reload_requested = reload;
        if self.dialogs_blocked || self.leave_prompts_blocked {
            self.blocks_end_with_navigation = true;
        }
        self.discard_all()
    }

    fn end_blocks(&mut self) {
        self.dialogs_blocked = false;
        self.leave_prompts_blocked = false;
        self.last_dialog_closed_at = None;
        self.last_leave_prompt_closed_at = None;
        self.blocked_origin = None;
        self.blocks_end_with_navigation = false;
    }

    /// The tab's web view is replaced or closed; the engine drops its requests itself.
    pub(crate) fn forget_all(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn note_user_activation(&mut self) {
        self.user_activated = true;
    }

    fn take_where(
        &mut self,
        mut condition: impl FnMut(&PageRequestKind) -> bool,
    ) -> Vec<PageRequestId> {
        let mut taken = Vec::new();
        self.queue.retain(|queued| {
            let take = condition(&queued.pending.kind);
            if take {
                taken.push(queued.pending.id);
            }
            !take
        });
        taken
    }
}

/// Answers a user gives to a permission request that the decision store should remember.
pub(crate) fn decision_to_remember(
    pending: &PendingPageRequest,
    reply: &PageRequestReply,
) -> Option<(String, PagePermission, PermissionDecision)> {
    if !reply.remember_for_site {
        return None;
    }
    let PageRequestKind::Permission(permission) = pending.kind else {
        return None;
    };
    let key = pending.origin.key()?;
    let decision = PermissionDecision::from_answer(&reply.answer)?;
    Some((key.to_owned(), permission, decision))
}

/// Makes page-supplied text safe to show as plain text: no control characters other than line
/// breaks (none at all when `multiline` is false), no characters that override or isolate text
/// direction (which could make the text appear in another order than it is), CR LF and other line
/// separators as `\n`, tabs as spaces, and at most `max_chars` characters.
pub fn sanitize_page_text(text: &str, multiline: bool, max_chars: usize) -> String {
    let mut sanitized = String::with_capacity(text.len().min(max_chars * 4));
    let mut characters = text.chars().peekable();
    let mut count = 0;
    let mut combining_marks = 0;
    let mut previous_was_variation_selector = false;
    while let Some(character) = characters.next() {
        if is_variation_selector(character) {
            if previous_was_variation_selector {
                continue;
            }
            previous_was_variation_selector = true;
        } else {
            previous_was_variation_selector = false;
        }
        if is_combining_mark(character) {
            combining_marks += 1;
            if combining_marks > MAX_COMBINING_MARKS {
                continue;
            }
        } else if !is_variation_selector(character) {
            combining_marks = 0;
        }
        let replacement = match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                Some('\n')
            }
            '\n' | '\u{2028}' | '\u{2029}' | '\u{0085}' => Some('\n'),
            '\t' => Some(' '),
            _ if is_direction_control(character) || is_invisible(character) => None,
            _ if character.is_control() => None,
            _ => Some(character),
        };
        let Some(mut replacement) = replacement else {
            continue;
        };
        if replacement == '\n' && !multiline {
            replacement = ' ';
        }
        if count == max_chars {
            sanitized.push('…');
            break;
        }
        sanitized.push(replacement);
        count += 1;
    }
    sanitized
}

/// Characters that draw nothing but can hide or pad text: zero-width space, word joiner and the
/// invisible operators, the byte-order mark, the soft hyphen, the Hangul
/// fillers and the tag characters.
const fn is_invisible(character: char) -> bool {
    matches!(
        character,
        // Not U+200C/U+200D (non-joiner, joiner): emoji sequences, Persian and Indic text need them.
        '\u{200B}'
            | '\u{2060}'..='\u{2064}'
            | '\u{FEFF}'
            | '\u{00AD}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{3164}'
            | '\u{FFA0}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// The variation selectors (VS1-VS16 and the supplement); runs keep only the first.
const fn is_variation_selector(character: char) -> bool {
    matches!(character, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

/// The combining-mark blocks (diacritical marks, their extended and supplement blocks, marks for
/// symbols, half marks). Marks of individual scripts are left alone; `talaria-core` has no Unicode
/// tables beyond these ranges.
const fn is_combining_mark(character: char) -> bool {
    matches!(
        character,
        '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// The URL without its user name and password; text that is not a URL is returned unchanged.
pub fn strip_user_info(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut parsed) if !parsed.username().is_empty() || parsed.password().is_some() => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        _ => url.to_owned(),
    }
}

/// Arabic letter mark, left-to-right and right-to-left marks, the embedding and override
/// characters, and the isolates.
const fn is_direction_control(character: char) -> bool {
    matches!(
        character,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_breaks_are_normalised_and_kept_only_when_multiline() {
        assert_eq!(
            sanitize_page_text("a\r\nb\rc\u{2028}d", true, 100),
            "a\nb\nc\nd"
        );
        assert_eq!(sanitize_page_text("a\r\nb\nc", false, 100), "a b c");
    }

    #[test]
    fn control_and_direction_characters_are_removed() {
        assert_eq!(
            sanitize_page_text(
                "\u{202E}abc\u{202C}\u{0000}\u{001B}[31m\u{2067}x\u{2069}",
                true,
                100
            ),
            "abc[31mx"
        );
    }

    #[test]
    fn long_text_is_cut_with_an_ellipsis() {
        assert_eq!(sanitize_page_text("abcdef", true, 3), "abc…");
        assert_eq!(sanitize_page_text("abc", true, 3), "abc");
    }

    #[test]
    fn origins_are_shown_as_host_and_port() {
        let origin = PageOrigin::from_url("https://example.com/path?q");
        assert_eq!(origin.label(), "example.com");
        assert_eq!(origin.key(), Some("https://example.com"));
        assert!(origin.is_secure());

        let origin = PageOrigin::from_url("http://127.0.0.1:8080/");
        assert_eq!(origin.label(), "127.0.0.1:8080");
        assert!(!origin.is_secure());
    }

    #[test]
    fn internationalised_hosts_are_shown_in_punycode() {
        assert_eq!(
            PageOrigin::from_url("https://exаmple.com/").label(),
            "xn--exmple-4nf.com"
        );
    }

    #[test]
    fn pages_without_an_origin_are_named_generically() {
        for url in [
            "file:///tmp/a.html",
            "data:text/html,hi",
            "about:blank",
            "not a url",
        ] {
            let origin = PageOrigin::from_url(url);
            assert_eq!(origin.label(), "This page", "{url}");
            assert_eq!(origin.key(), None, "{url}");
            assert!(!origin.is_secure(), "{url}");
        }
    }

    #[test]
    fn invisible_characters_are_removed() {
        let text = "a\u{200B}bc\u{2060}d\u{2064}e\u{FEFF}f\u{00AD}g\u{115F}\u{1160}\u{3164}\u{FFA0}h\u{E0041}\u{E007F}i";
        assert_eq!(sanitize_page_text(text, true, 100), "abcdefghi");
    }

    #[test]
    fn joiners_are_kept_for_emoji_sequences_and_scripts_that_need_them() {
        // Family emoji (ZWJ sequence) and Persian "می‌خواهم" (ZWNJ).
        let text = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} \u{0645}\u{06CC}\u{200C}\u{062E}";
        assert_eq!(sanitize_page_text(text, true, 100), text);
    }

    #[test]
    fn runs_of_combining_marks_and_variation_selectors_are_capped() {
        let stacked = format!("x{}y", "\u{0301}".repeat(500));
        assert_eq!(
            sanitize_page_text(&stacked, true, 10_000),
            format!("x{}y", "\u{0301}".repeat(MAX_COMBINING_MARKS))
        );
        assert_eq!(
            sanitize_page_text("a\u{FE0F}\u{FE0F}\u{FE0E}b\u{E0100}\u{E0101}", true, 100),
            "a\u{FE0F}b\u{E0100}"
        );
        // A new base character starts a new run.
        assert_eq!(
            sanitize_page_text("e\u{0301}\u{0302}e\u{0301}", true, 100),
            "e\u{0301}\u{0302}e\u{0301}"
        );
    }

    #[test]
    fn labels_never_show_user_info_and_name_ipv6_and_blob_origins() {
        assert_eq!(
            PageOrigin::from_url("https://alice:secret@example.com/").label(),
            "example.com"
        );
        let ipv6 = PageOrigin::from_url("http://[::1]:8080/a");
        assert_eq!(ipv6.label(), "[::1]:8080");
        assert_eq!(ipv6.key(), Some("http://[::1]:8080"));
        let blob = PageOrigin::from_url("blob:https://example.com/0f8c-11");
        assert_eq!(blob.label(), "example.com");
        assert_eq!(blob.key(), Some("https://example.com"));
        assert!(blob.is_secure());
        assert_eq!(
            PageOrigin::from_url("https://example.com:443/").label(),
            "example.com"
        );
    }

    #[test]
    fn user_info_is_stripped_from_urls() {
        assert_eq!(
            strip_user_info("https://alice:pw@example.com/a?b#c"),
            "https://example.com/a?b#c"
        );
        assert_eq!(
            strip_user_info("https://example.com/"),
            "https://example.com/"
        );
        assert_eq!(strip_user_info(""), "");
        assert_eq!(strip_user_info("not a url"), "not a url");
    }

    #[test]
    fn every_permission_reads_as_plain_words() {
        assert_eq!(
            PagePermission::PersistentStorage.description(),
            "store data permanently on this device"
        );
        assert_eq!(
            PagePermission::Geolocation.description(),
            "know your location"
        );
    }
}
