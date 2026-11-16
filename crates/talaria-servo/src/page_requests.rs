//! Keeps the engine's request objects for dialogs, leave prompts, sign-ins and permission
//! prompts until the user answers. Servo blocks the asking page's script (or, for a sign-in, its
//! load) until the object is answered or dropped; the browser thread is never blocked. Only an id
//! and plain data reach `talaria-core`; answers come back as `PageRequestAnswer`s.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use servo::{
    AllowOrDenyRequest, AuthenticationRequest, PermissionFeature, PermissionRequest, SimpleDialog,
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
