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
        let origin = url.origin();
        if !origin.is_tuple() {
            return Self::unnamed();
        }
        let Some(host) = url.host_str() else {
            return Self::unnamed();
        };
        // `host_str` is ASCII: internationalised names appear in their punycode form, so a
        // look-alike name cannot pass for another.
        let label = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        };
        Self {
            label,
            key: Some(origin.ascii_serialization()),
            secure: url.scheme() == "https",
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

    /// Whether to offer "Don't allow this page to show more dialogs".
    pub const fn offers_to_block_dialogs(&self) -> bool {
        self.offers_to_block_dialogs
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
}

impl PageRequests {
    pub(crate) fn front(&self) -> Option<PendingPageRequest> {
        let front = self.queue.front()?;
        let mut pending = front.pending.clone();
        if pending.kind.is_simple_dialog() {
            pending.offers_to_block_dialogs |= self
                .queue
                .iter()
                .skip(1)
                .any(|queued| queued.pending.kind.is_simple_dialog());
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

        if kind.is_blockable_dialog() && self.dialogs_blocked {
            return Some(PageRequestAnswer::Discard);
        }
        match &kind {
            PageRequestKind::LeavePage if !self.user_activated => {
                return Some(PageRequestAnswer::Accept);
            }
            // There is no notification display yet, so notifications stay denied without a
            // prompt.
            PageRequestKind::Permission(PagePermission::Notifications) => {
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

        let offers_to_block_dialogs = kind.is_simple_dialog()
            && self.last_dialog_closed_at.is_some_and(|closed_at| {
                request.received_at.saturating_sub(closed_at) < SUCCESSIVE_DIALOG_INTERVAL
            });
        self.queue.push_back(QueuedRequest {
            pending: PendingPageRequest {
                id: request.id,
                kind,
                origin,
                offers_to_block_dialogs,
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
        if answered.kind.is_simple_dialog() {
            self.last_dialog_closed_at = Some(now);
        }
        let mut discarded = Vec::new();
        if reply.block_further_dialogs && answered.kind.is_blockable_dialog() {
            self.dialogs_blocked = true;
            discarded = self.take_where(|kind| kind.is_blockable_dialog());
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
        self.dialogs_blocked = false;
        self.last_dialog_closed_at = None;
        self.user_activated = false;
        self.discard_all()
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
    while let Some(character) = characters.next() {
        let replacement = match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                Some('\n')
            }
            '\n' | '\u{2028}' | '\u{2029}' | '\u{0085}' => Some('\n'),
            '\t' => Some(' '),
            _ if is_direction_control(character) => None,
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
