//! Keeps the engine's request objects for dialogs, leave prompts, sign-ins and permission
//! prompts until the user answers. Servo blocks the asking page's script (or, for a sign-in, its
//! load) until the object is answered or dropped; the browser thread is never blocked. Only an id
//! and plain data reach `talaria-core`; answers come back as `PageRequestAnswer`s.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use servo::{
    AllowOrDenyRequest, AuthenticationRequest, LoadStatus, PermissionFeature, PermissionRequest,
    SimpleDialog,
};
use talaria_core::{PagePermission, PageRequestAnswer, PageRequestId, PageRequestKind};

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static CLOCK_EPOCH: OnceLock<Instant> = OnceLock::new();

/// The clock of `PageRequest::received_at`; pass it as `now` to
/// `BrowserController::answer_page_request`.
pub fn page_request_clock() -> Duration {
    CLOCK_EPOCH.get_or_init(Instant::now).elapsed()
}

/// Something the engine waits on. Dropping it unanswered gives the engine's default answer.
pub(crate) trait EngineRequest {
    fn respond(self, answer: PageRequestAnswer);
}

/// One web view's unanswered requests.
pub(crate) struct PendingEngineRequests<R> {
    requests: HashMap<PageRequestId, R>,
    closed: bool,
}

impl<R> Default for PendingEngineRequests<R> {
    fn default() -> Self {
        Self {
            requests: HashMap::new(),
            closed: false,
        }
    }
}

impl<R: EngineRequest> PendingEngineRequests<R> {
    /// Keeps the request and returns its new id, or drops it at once (with the engine's
    /// default) if the web view is closing.
    pub(crate) fn insert(&mut self, request: R) -> Option<PageRequestId> {
        if self.closed {
            return None;
        }
        let id = PageRequestId::new(NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed));
        self.requests.insert(id, request);
        Some(id)
    }

    /// Removes the request; the caller answers it once nothing is borrowed any more.
    pub(crate) fn take(&mut self, id: PageRequestId) -> Option<R> {
        self.requests.remove(&id)
    }

    /// The web view is closing: every request is dropped and later ones are refused.
    pub(crate) fn close(&mut self) -> Vec<R> {
        self.closed = true;
        self.requests.drain().map(|(_, request)| request).collect()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.requests.len()
    }
}

/// Owns a web view's store for the browser side and closes it when dropped, so a closed or
/// replaced tab never leaves a page waiting, whenever Servo lets go of the delegate's handle.
pub(crate) struct ClosesOnDrop<R: EngineRequest> {
    shared: Rc<RefCell<PendingEngineRequests<R>>>,
}

impl<R: EngineRequest> ClosesOnDrop<R> {
    pub(crate) fn new(requests: PendingEngineRequests<R>) -> Self {
        Self {
            shared: Rc::new(RefCell::new(requests)),
        }
    }

    /// The handle the delegate keeps.
    pub(crate) fn shared(&self) -> Rc<RefCell<PendingEngineRequests<R>>> {
        Rc::clone(&self.shared)
    }

    /// Removes the request; it is answered by the caller once nothing is borrowed.
    pub(crate) fn take(&self, id: PageRequestId) -> Option<R> {
        self.shared.borrow_mut().take(id)
    }
}

impl<R: EngineRequest> Drop for ClosesOnDrop<R> {
    fn drop(&mut self) {
        let unanswered = self.shared.borrow_mut().close();
        drop(unanswered);
    }
}

/// Tells when a new document starts loading. Servo's own `Started` is not enough:
/// `WebView::reload` sets the status to `Started` without notifying
/// (`servo-0.8.0/webview.rs:552`), and a new document's `Started` is dropped as unchanged
/// whenever the status is still `Started` (`webview.rs:334-336`), as after a reload or when a page
/// never got past `Started`. So after Mercury itself loads, reloads or traverses the web view, the
/// first `HeadParsed` or `Complete` also counts as a document start. Without such a navigation
/// they never do: Servo also sends `HeadParsed` whenever a page inserts a `<body>` into its
/// top-level document (`servo-script-0.8.0/dom/html/document_structure/htmlbodyelement.rs`),
/// which must not let a page reset its own dialog blocking.
#[derive(Debug, Default)]
pub(crate) struct DocumentStartTracker {
    armed: bool,
}

impl DocumentStartTracker {
    /// Mercury asked the web view to navigate.
    pub(crate) fn arm(&mut self) {
        self.armed = true;
    }

    /// The navigation will not happen (the user chose to stay on the page).
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }

    pub(crate) fn document_started(&mut self, status: LoadStatus) -> bool {
        match status {
            LoadStatus::Started => {
                self.armed = false;
                true
            }
            LoadStatus::HeadParsed | LoadStatus::Complete => std::mem::take(&mut self.armed),
            LoadStatus::Stopped => false,
        }
    }
}

/// How Mercury asks a web view to navigate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Navigation {
    Load,
    Reload,
    Traverse,
}

/// Whether Servo will swallow the `Started` of the document this navigation may load, so that the
/// tracker must infer it. `reload()` always pre-sets `Started` (`servo-0.8.0/webview.rs:552`);
/// another navigation's `Started` is only dropped as unchanged while the status is still
/// `Started`. Arming in other cases would let a navigation that loads no document (a fragment)
/// leave the tracker armed for a `<body>` the page inserts later.
pub(crate) fn arms_document_start(navigation: Navigation, status: LoadStatus) -> bool {
    navigation == Navigation::Reload || status == LoadStatus::Started
}

pub(crate) enum ServoPageRequest {
    Dialog(SimpleDialog),
    Unload(AllowOrDenyRequest),
    Authentication(AuthenticationRequest),
    Permission(PermissionRequest),
}

impl EngineRequest for ServoPageRequest {
    fn respond(self, answer: PageRequestAnswer) {
        use PageRequestAnswer::{Accept, Credentials, Decline, Discard, PromptText};

        match (self, answer) {
            (_, Discard) => {}
            (Self::Dialog(SimpleDialog::Alert(dialog)), _) => dialog.confirm(),
            (Self::Dialog(SimpleDialog::Confirm(dialog)), Decline) => dialog.dismiss(),
            (Self::Dialog(SimpleDialog::Confirm(dialog)), _) => dialog.confirm(),
            (Self::Dialog(SimpleDialog::Prompt(dialog)), Decline) => dialog.dismiss(),
            (Self::Dialog(SimpleDialog::Prompt(mut dialog)), PromptText(text)) => {
                dialog.set_current_value(&text);
                dialog.confirm();
            }
            (Self::Dialog(SimpleDialog::Prompt(dialog)), _) => dialog.confirm(),
            (Self::Unload(request), Accept) => request.allow(),
            (Self::Unload(request), Decline) => request.deny(),
            (Self::Authentication(request), Credentials(credentials)) => request.authenticate(
                credentials.username().to_owned(),
                credentials.password().to_owned(),
            ),
            (Self::Permission(request), Accept) => request.allow(),
            (Self::Permission(request), Decline) => request.deny(),
            // Anything else carries no meaning for the request: leave it to the default.
            (_, _) => {}
        }
    }
}

pub(crate) fn simple_dialog_kind(dialog: &SimpleDialog) -> PageRequestKind {
    match dialog {
        SimpleDialog::Alert(alert) => PageRequestKind::Alert {
            message: alert.message().to_owned(),
        },
        SimpleDialog::Confirm(confirm) => PageRequestKind::Confirm {
            message: confirm.message().to_owned(),
        },
        SimpleDialog::Prompt(prompt) => PageRequestKind::Prompt {
            message: prompt.message().to_owned(),
            default_value: prompt.current_value().to_owned(),
        },
    }
}

pub(crate) fn page_permission(feature: PermissionFeature) -> PagePermission {
    match feature {
        PermissionFeature::Geolocation => PagePermission::Geolocation,
        PermissionFeature::Notifications => PagePermission::Notifications,
        PermissionFeature::Push => PagePermission::Push,
        PermissionFeature::Midi => PagePermission::Midi,
        PermissionFeature::Camera => PagePermission::Camera,
        PermissionFeature::Microphone => PagePermission::Microphone,
        PermissionFeature::Speaker => PagePermission::Speaker,
        PermissionFeature::DeviceInfo => PagePermission::DeviceInfo,
        PermissionFeature::BackgroundSync => PagePermission::BackgroundSync,
        PermissionFeature::Bluetooth => PagePermission::Bluetooth,
        PermissionFeature::PersistentStorage => PagePermission::PersistentStorage,
        PermissionFeature::ScreenWakeLock(_) => PagePermission::ScreenWakeLock,
        PermissionFeature::Gamepad => PagePermission::Gamepad,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use servo::WakeLockType;
    use talaria_core::HttpCredentials;

    use super::*;

    /// Each request's name and its answer; `None` means it was dropped unanswered.
    type AnswerLog = Rc<RefCell<Vec<(&'static str, Option<PageRequestAnswer>)>>>;

    struct FakeRequest {
        name: &'static str,
        log: AnswerLog,
        answered: bool,
    }

    impl FakeRequest {
        fn new(name: &'static str, log: &AnswerLog) -> Self {
            Self {
                name,
                log: Rc::clone(log),
                answered: false,
            }
        }
    }

    impl EngineRequest for FakeRequest {
        fn respond(mut self, answer: PageRequestAnswer) {
            self.answered = true;
            self.log.borrow_mut().push((self.name, Some(answer)));
        }
    }

    impl Drop for FakeRequest {
        fn drop(&mut self) {
            if !self.answered {
                self.log.borrow_mut().push((self.name, None));
            }
        }
    }

    #[test]
    fn an_answer_reaches_the_request_with_that_id_only() {
        let log = Rc::default();
        let mut pending = PendingEngineRequests::default();
        let first = pending
            .insert(FakeRequest::new("first", &log))
            .expect("open");
        let second = pending
            .insert(FakeRequest::new("second", &log))
            .expect("open");
        assert_ne!(first, second);

        pending
            .take(second)
            .expect("the second request is pending")
            .respond(PageRequestAnswer::Decline);

        assert_eq!(
            *log.borrow(),
            vec![("second", Some(PageRequestAnswer::Decline))]
        );
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn a_request_can_be_taken_only_once() {
        let log = Rc::default();
        let mut pending = PendingEngineRequests::default();
        let id = pending
            .insert(FakeRequest::new("only", &log))
            .expect("open");

        pending
            .take(id)
            .expect("pending")
            .respond(PageRequestAnswer::Accept);

        assert!(pending.take(id).is_none());
        assert_eq!(log.borrow().len(), 1);
    }

    #[test]
    fn closing_drops_every_request_and_refuses_new_ones() {
        let log = Rc::default();
        let mut pending = PendingEngineRequests::default();
        pending.insert(FakeRequest::new("a", &log)).expect("open");
        pending.insert(FakeRequest::new("b", &log)).expect("open");

        drop(pending.close());
        assert_eq!(pending.insert(FakeRequest::new("late", &log)), None);

        let mut dropped = log.borrow().iter().map(|entry| entry.0).collect::<Vec<_>>();
        dropped.sort_unstable();
        assert_eq!(dropped, vec!["a", "b", "late"]);
        assert!(log.borrow().iter().all(|entry| entry.1.is_none()));
    }

    #[test]
    fn ids_are_unique_across_web_views() {
        let log = Rc::default();
        let mut first = PendingEngineRequests::default();
        let mut second = PendingEngineRequests::default();

        let a = first.insert(FakeRequest::new("a", &log)).expect("open");
        let b = second.insert(FakeRequest::new("b", &log)).expect("open");

        assert_ne!(a, b);
        assert!(first.take(b).is_none());
    }

    #[test]
    fn the_clock_does_not_go_backwards() {
        let earlier = page_request_clock();
        assert!(page_request_clock() >= earlier);
    }

    #[test]
    fn every_servo_permission_maps_to_a_page_permission() {
        let pairs = [
            (PermissionFeature::Geolocation, PagePermission::Geolocation),
            (
                PermissionFeature::Notifications,
                PagePermission::Notifications,
            ),
            (PermissionFeature::Push, PagePermission::Push),
            (PermissionFeature::Midi, PagePermission::Midi),
            (PermissionFeature::Camera, PagePermission::Camera),
            (PermissionFeature::Microphone, PagePermission::Microphone),
            (PermissionFeature::Speaker, PagePermission::Speaker),
            (PermissionFeature::DeviceInfo, PagePermission::DeviceInfo),
            (
                PermissionFeature::BackgroundSync,
                PagePermission::BackgroundSync,
            ),
            (PermissionFeature::Bluetooth, PagePermission::Bluetooth),
            (
                PermissionFeature::PersistentStorage,
                PagePermission::PersistentStorage,
            ),
            (
                PermissionFeature::ScreenWakeLock(WakeLockType::Screen),
                PagePermission::ScreenWakeLock,
            ),
            (PermissionFeature::Gamepad, PagePermission::Gamepad),
        ];
        for (feature, permission) in pairs {
            assert_eq!(page_permission(feature), permission, "{feature:?}");
        }
    }

    use servo::LoadStatus;

    /// A step of a web view's life as the tracker sees it.
    #[derive(Clone, Copy)]
    enum Step {
        /// Mercury asks the web view to load, reload or traverse.
        Navigate,
        /// The user stays on a page whose leave prompt asked.
        Stay,
        Status(LoadStatus),
    }

    use LoadStatus::{Complete, HeadParsed, Started, Stopped};
    use Step::{Navigate, Status, Stay};

    /// Feeds the steps to a tracker; returns the indexes at which a new document was reported.
    fn documents_started(steps: &[Step]) -> Vec<usize> {
        let mut tracker = DocumentStartTracker::default();
        let mut started = Vec::new();
        for (index, step) in steps.iter().enumerate() {
            match *step {
                Navigate => tracker.arm(),
                Stay => tracker.disarm(),
                Status(status) => {
                    if tracker.document_started(status) {
                        started.push(index);
                    }
                }
            }
        }
        started
    }

    #[test]
    fn a_load_that_reports_started_starts_one_document() {
        assert_eq!(
            documents_started(&[
                Status(Started),
                Status(HeadParsed),
                Status(Complete),
                Navigate,
                Status(Started),
                Status(HeadParsed),
                Status(Complete),
            ]),
            vec![0, 4]
        );
    }

    #[test]
    fn a_reload_whose_started_servo_swallowed_still_starts_a_document() {
        // `WebView::reload` sets the status to Started without notifying, so the new document's
        // own Started equals it and is not reported either.
        assert_eq!(
            documents_started(&[
                Status(Started),
                Status(HeadParsed),
                Status(Complete),
                Navigate,
                Status(HeadParsed),
                Status(Complete),
            ]),
            vec![0, 4]
        );
        assert_eq!(
            documents_started(&[Status(Complete), Navigate, Status(Complete)]),
            vec![2]
        );
    }

    #[test]
    fn a_navigation_while_a_load_is_stuck_in_started_starts_a_document() {
        // The page never got past Started (an alert loop in <head>); the next site's Started is
        // dropped as unchanged.
        assert_eq!(
            documents_started(&[
                Status(Started),
                Navigate,
                Status(HeadParsed),
                Status(Complete)
            ]),
            vec![0, 2]
        );
    }

    #[test]
    fn a_body_the_page_inserts_after_loading_starts_no_document() {
        // Servo reports HeadParsed whenever a <body> is inserted into the top-level document.
        assert_eq!(
            documents_started(&[
                Status(Started),
                Status(HeadParsed),
                Status(Complete),
                Status(HeadParsed),
                Status(Complete),
                Status(HeadParsed),
            ]),
            vec![0]
        );
    }

    #[test]
    fn a_stopped_load_and_an_inserted_body_are_one_document() {
        assert_eq!(
            documents_started(&[
                Status(Started),
                Status(Stopped),
                Status(HeadParsed),
                Status(Complete),
            ]),
            vec![0]
        );
    }

    #[test]
    fn staying_on_a_page_disarms_the_tracker() {
        assert_eq!(
            documents_started(&[
                Status(Started),
                Status(Complete),
                Navigate,
                Stay,
                Status(HeadParsed),
            ]),
            vec![0]
        );
    }

    #[test]
    fn only_navigations_whose_started_servo_will_swallow_arm_the_tracker() {
        // `reload()` pre-sets Started, so the new document's Started is always swallowed.
        for status in [Started, HeadParsed, Complete, Stopped] {
            assert!(
                arms_document_start(Navigation::Reload, status),
                "{status:?}"
            );
        }
        // Other navigations only lose their Started while the status is still Started; from any
        // other status (a fragment navigation from a loaded page, say) nothing is swallowed.
        for navigation in [Navigation::Load, Navigation::Traverse] {
            assert!(arms_document_start(navigation, Started));
            for status in [HeadParsed, Complete, Stopped] {
                assert!(!arms_document_start(navigation, status), "{status:?}");
            }
        }
    }

    #[test]
    fn a_fragment_navigation_from_a_loaded_page_leaves_an_inserted_body_alone() {
        let mut tracker = DocumentStartTracker::default();
        assert!(tracker.document_started(Started));
        assert!(!tracker.document_started(Complete));
        if arms_document_start(Navigation::Traverse, Complete) {
            tracker.arm();
        }
        // Same document: no Started; later the page inserts a <body>.
        assert!(!tracker.document_started(HeadParsed));
    }

    #[test]
    fn dropping_the_store_guard_drops_every_request_and_refuses_later_ones() {
        let log = Rc::default();
        let guard = ClosesOnDrop::new(PendingEngineRequests::default());
        let shared = guard.shared();
        shared
            .borrow_mut()
            .insert(FakeRequest::new("waiting", &log))
            .expect("open");

        drop(guard);

        assert_eq!(*log.borrow(), vec![("waiting", None)]);
        assert_eq!(
            shared.borrow_mut().insert(FakeRequest::new("late", &log)),
            None
        );
    }

    #[test]
    fn credentials_answers_keep_the_user_name_and_password() {
        let log = Rc::default();
        let mut pending = PendingEngineRequests::default();
        let id = pending
            .insert(FakeRequest::new("auth", &log))
            .expect("open");
        let credentials = HttpCredentials::new("alice".to_owned(), "secret".to_owned());

        pending
            .take(id)
            .expect("pending")
            .respond(PageRequestAnswer::Credentials(credentials.clone()));

        assert_eq!(
            *log.borrow(),
            vec![("auth", Some(PageRequestAnswer::Credentials(credentials)))]
        );
    }
}
