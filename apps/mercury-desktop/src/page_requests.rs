//! Shows the active tab's page requests (JavaScript dialogs, the leave prompt, HTTP sign-in and
//! permission prompts) and routes the user's answers back to the request that asked.
//!
//! What reaches this module, and what does not, in Servo 0.8.0:
//! - `alert()`, `confirm()`, `prompt()` from any frame of the page; Servo does not say which
//!   frame, so the tab's page is named as the asker.
//! - "Leave this page?" only where Servo runs `beforeunload` with a prompt: navigations started
//!   by the page (links, forms, `location`), reload, and `window.close()`. Servo does not run it
//!   for navigations from the address bar, back/forward, closing a tab or quitting, so those
//!   leave without asking. Servo also asks regardless of user activation; the session model
//!   applies that rule (see `talaria_core::page_request`).
//! - Sign-in requests (`401` and `407`), without the realm: Servo does not pass it on.
//! - Permission requests, see `talaria_core::PagePermission` and the lane report for which
//!   features can reach the browser with Mercury's preferences.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use mercury_ui::{MainWindow, PageRequestStyle, PageRequestView};
use slint::ComponentHandle;
use talaria_core::{
    BrowserController, HttpCredentials, PageRequestAnswer, PageRequestId, PageRequestKind,
    PageRequestReply, PendingPageRequest,
};
use talaria_servo::page_request_clock;

use crate::ui_sync::sync_ui;

/// Keys and clicks on a page-triggered sheet or prompt are ignored for this long after it
/// appears, so that a click or key press meant for the page cannot answer it (Chrome's
/// `InputEventActivationProtector` does the same).
pub(crate) const INPUT_PROTECTION: Duration = Duration::from_millis(500);

/// Remembers when the request on screen appeared. Time is passed in.
#[derive(Debug, Default)]
pub(crate) struct InputProtector {
    shown: Option<(PageRequestId, Duration)>,
}

impl InputProtector {
    /// Called whenever the window shows `shown` (or nothing); a new request restarts the time.
    pub(crate) fn show(&mut self, shown: Option<PageRequestId>, now: Duration) {
        match shown {
            None => self.shown = None,
            Some(id) if self.shown.map(|(current, _)| current) != Some(id) => {
                self.shown = Some((id, now));
            }
            Some(_) => {}
        }
    }

    /// Whether an answer to `id` given at `now` comes from the user rather than from input
    /// that was meant for the page.
    pub(crate) fn accepts(&self, id: PageRequestId, now: Duration) -> bool {
        self.shown.is_some_and(|(shown, shown_at)| {
            shown == id && now.saturating_sub(shown_at) >= INPUT_PROTECTION
        })
    }
}

thread_local! {
    /// The window's protector; the window lives on this thread.
    static PROTECTOR: RefCell<InputProtector> = RefCell::default();
}

/// The password manager's seam: credentials saved for a site's sign-in, used to fill the sheet
/// in. `origin` is the serialised origin (`https://example.com`); `realm` is the sign-in realm,
/// which Servo 0.8 does not report, so it is always `None` for now. Nothing is saved yet.
pub(crate) fn saved_http_credentials(
    _origin: &str,
    _realm: Option<&str>,
) -> Option<HttpCredentials> {
    None
}

/// Shows the active tab's oldest unanswered request, or nothing.
pub(crate) fn sync_page_request(window: &MainWindow, controller: &BrowserController) {
    let pending = controller.pending_page_request(controller.active_tab_id());
    PROTECTOR.with(|protector| {
        protector.borrow_mut().show(
            pending.as_ref().map(PendingPageRequest::id),
            page_request_clock(),
        );
    });
    let view = pending
        .map(|pending| {
            let saved = match pending.kind() {
                PageRequestKind::Authentication { .. } => pending
                    .origin()
                    .key()
                    .and_then(|origin| saved_http_credentials(origin, None)),
                _ => None,
            };
            page_request_view(&pending, saved.as_ref())
        })
        .unwrap_or_default();
    if window.get_page_request() != view {
        window.set_page_request(view);
    }
}

pub(crate) fn connect_page_request_callbacks(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
) {
    let weak_window = window.as_weak();
    window.on_page_request_answered(move |id, accepted, text, password, checked| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let Ok(id) = u64::try_from(id) else {
            return;
        };
        let id = PageRequestId::new(id);
        let tab_id = controller.borrow().active_tab_id();
        let pending = controller.borrow().pending_page_request(tab_id);
        let Some(pending) = pending.filter(|pending| pending.id() == id) else {
            return;
        };
        let now = page_request_clock();
        if !PROTECTOR.with(|protector| protector.borrow().accepts(id, now)) {
            log::debug!(
                "ignoring an answer to page request {} given too soon",
                id.value()
            );
            return;
        }
        let reply = reply_for(pending.kind(), accepted, &text, &password, checked);
        controller
            .borrow_mut()
            .answer_page_request(tab_id, id, reply, now);
        if let Some(window) = weak_window.upgrade() {
            sync_ui(&window, &controller.borrow());
            window.window().request_redraw();
        }
    });
}

/// Whether the active tab shows a sheet over its page (anything but a permission prompt): its
/// page then gets no keyboard or IME input.
pub(crate) fn sheet_shown(controller: &BrowserController) -> bool {
    controller
        .pending_page_request(controller.active_tab_id())
        .is_some_and(|pending| !matches!(pending.kind(), PageRequestKind::Permission(_)))
}

/// The user's answer to a request of this kind. `accepted` is OK, Leave, Sign in or Allow;
/// `checked` is the sheet's checkbox.
pub(crate) fn reply_for(
    kind: &PageRequestKind,
    accepted: bool,
    text: &str,
    password: &str,
    checked: bool,
) -> PageRequestReply {
    let answer = match (kind, accepted) {
        (PageRequestKind::Alert { .. }, _) => PageRequestAnswer::Accept,
        // Unedited: the page gets its own default value back, not the shortened, cleaned copy
        // that was shown.
        (PageRequestKind::Prompt { default_value, .. }, true) if text == default_value => {
            PageRequestAnswer::Accept
        }
        (PageRequestKind::Prompt { .. }, true) => {
            PageRequestAnswer::PromptText(text.replace(['\r', '\n'], ""))
        }
        (PageRequestKind::Authentication { .. }, true) => PageRequestAnswer::Credentials(
            HttpCredentials::new(text.to_owned(), password.to_owned()),
        ),
        (_, true) => PageRequestAnswer::Accept,
        (_, false) => PageRequestAnswer::Decline,
    };
    let reply = PageRequestReply::new(answer);
    match kind {
        // Servo 0.8 does not say which frame asked for a permission, so a decision cannot be
        // remembered for the right site; none is offered.
        PageRequestKind::Permission(_) => reply,
        _ if checked => reply.blocking_further_dialogs(),
        _ => reply,
    }
}

const BLOCK_DIALOGS_CHOICE: &str = "Don't allow this page to show more dialogs";
const BLOCK_LEAVE_PROMPTS_CHOICE: &str = "Don't allow this page to ask again";
const CREDENTIALS_REJECTED: &str = "The user name or password was not accepted.";
const NOT_HTTPS: &str = "This connection is not secure (not HTTPS): your user name and password will be sent \
     unencrypted.";

pub(crate) fn page_request_view(
    pending: &PendingPageRequest,
    saved_credentials: Option<&HttpCredentials>,
) -> PageRequestView {
    let site = pending.origin().label();
    let block_choice = if pending.offers_to_block_dialogs() {
        BLOCK_DIALOGS_CHOICE
    } else {
        ""
    };
    let dialog = |style, message: &str, value: &str, decline: &str| PageRequestView {
        style,
        heading: format!("{site} says").into(),
        message: message.into(),
        value: value.into(),
        accept_label: "OK".into(),
        decline_label: decline.into(),
        choice_label: block_choice.into(),
        ..PageRequestView::default()
    };

    let view = match pending.kind() {
        PageRequestKind::Alert { message } => dialog(PageRequestStyle::Alert, message, "", ""),
        PageRequestKind::Confirm { message } => {
            dialog(PageRequestStyle::Confirm, message, "", "Cancel")
        }
        PageRequestKind::Prompt {
            message,
            default_value,
        } => dialog(PageRequestStyle::Prompt, message, default_value, "Cancel"),
        PageRequestKind::LeavePage => {
            let (heading, accept) = if pending.is_reload_prompt() {
                ("Reload this page?", "Reload")
            } else {
                ("Leave this page?", "Leave")
            };
            PageRequestView {
                style: PageRequestStyle::LeavePage,
                heading: heading.into(),
                message: "Changes you made may not be saved.".into(),
                detail: format!("Asked by {site}").into(),
                accept_label: accept.into(),
                decline_label: "Stay".into(),
                choice_label: if pending.offers_to_block_dialogs() {
                    BLOCK_LEAVE_PROMPTS_CHOICE
                } else {
                    ""
                }
                .into(),
                ..PageRequestView::default()
            }
        }
        PageRequestKind::Authentication { for_proxy } => {
            let (heading, message) = if *for_proxy {
                (
                    "Sign in to the proxy server".to_owned(),
                    format!("A proxy server needs a user name and password to load {site}."),
                )
            } else {
                (
                    format!("Sign in to {site}"),
                    format!("{site} needs a user name and password."),
                )
            };
            let warning = [
                pending
                    .credentials_were_rejected()
                    .then_some(CREDENTIALS_REJECTED),
                (!pending.origin().is_secure()).then_some(NOT_HTTPS),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n");
            PageRequestView {
                style: PageRequestStyle::SignIn,
                heading: heading.into(),
                message: message.into(),
                warning: warning.into(),
                value: saved_credentials
                    .map(HttpCredentials::username)
                    .unwrap_or_default()
                    .into(),
                secret: saved_credentials
                    .map(HttpCredentials::password)
                    .unwrap_or_default()
                    .into(),
                accept_label: "Sign in".into(),
                decline_label: "Cancel".into(),
                ..PageRequestView::default()
            }
        }
        PageRequestKind::Permission(permission) => PageRequestView {
            style: PageRequestStyle::Permission,
            heading: format!("{site} wants to {}", permission.description()).into(),
            accept_label: "Allow".into(),
            decline_label: "Block".into(),
            ..PageRequestView::default()
        },
    };
    PageRequestView {
        // Ids outside `int` cannot be answered; they would need 2^31 requests in one run.
        id: i32::try_from(pending.id().value()).unwrap_or(-1),
        ..view
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use talaria_core::BrowserCommand;
    use talaria_core::{BrowserEvent, PagePermission, PageRequest, PageRequestId, TabCommand};

    use super::*;

    fn pending(url: &str, kind: PageRequestKind) -> PendingPageRequest {
        let mut controller = BrowserController::default();
        let tab_id = controller.active_tab_id();
        controller.handle_event(
            tab_id,
            BrowserEvent::AddressChanged {
                url: url.to_owned(),
            },
        );
        // A leave prompt is only shown after the user interacted with the page.
        controller.queue_command(BrowserCommand::PointerButton {
            action: talaria_core::PointerButtonAction::Down,
            button: talaria_core::PointerButton::Left,
            x: 0.0,
            y: 0.0,
        });
        controller.handle_event(
            tab_id,
            BrowserEvent::PageRequested(PageRequest {
                id: PageRequestId::new(42),
                url: url.to_owned(),
                kind,
                received_at: Duration::ZERO,
            }),
        );
        controller
            .pending_page_request(tab_id)
            .expect("the request is pending")
    }

    #[test]
    fn an_alert_names_its_site_and_has_one_button() {
        let view = page_request_view(
            &pending(
                "https://example.com/a",
                PageRequestKind::Alert {
                    message: "Hello".to_owned(),
                },
            ),
            None,
        );

        assert_eq!(view.id, 42);
        assert_eq!(view.style, PageRequestStyle::Alert);
        assert_eq!(view.heading, "example.com says");
        assert_eq!(view.message, "Hello");
        assert_eq!(view.accept_label, "OK");
        assert_eq!(view.decline_label, "");
        assert_eq!(view.choice_label, "");
    }

    #[test]
    fn a_prompt_starts_with_its_default_value() {
        let view = page_request_view(
            &pending(
                "https://example.com/a",
                PageRequestKind::Prompt {
                    message: "Name?".to_owned(),
                    default_value: "Ada".to_owned(),
                },
            ),
            None,
        );

        assert_eq!(view.style, PageRequestStyle::Prompt);
        assert_eq!(view.value, "Ada");
        assert_eq!(view.decline_label, "Cancel");
    }

    #[test]
    fn the_leave_prompt_says_who_asks() {
        let view = page_request_view(
            &pending("https://example.com/a", PageRequestKind::LeavePage),
            None,
        );

        assert_eq!(view.heading, "Leave this page?");
        assert_eq!(view.message, "Changes you made may not be saved.");
        assert_eq!(view.detail, "Asked by example.com");
        assert_eq!(view.accept_label, "Leave");
        assert_eq!(view.decline_label, "Stay");
    }

    #[test]
    fn a_sign_in_without_https_says_so_plainly() {
        let view = page_request_view(
            &pending(
                "http://127.0.0.1:8123/private",
                PageRequestKind::Authentication { for_proxy: false },
            ),
            None,
        );

        assert_eq!(view.style, PageRequestStyle::SignIn);
        assert_eq!(view.heading, "Sign in to 127.0.0.1:8123");
        assert!(view.warning.contains("not HTTPS"));
        assert_eq!(view.accept_label, "Sign in");
    }

    #[test]
    fn a_sign_in_over_https_has_no_warning_and_a_proxy_is_named_as_such() {
        let view = page_request_view(
            &pending(
                "https://example.com/",
                PageRequestKind::Authentication { for_proxy: true },
            ),
            None,
        );

        assert_eq!(view.heading, "Sign in to the proxy server");
        assert!(view.message.contains("proxy server"));
        assert_eq!(view.warning, "");
    }

    #[test]
    fn saved_credentials_fill_the_sign_in_sheet() {
        let credentials = HttpCredentials::new("alice".to_owned(), "pw".to_owned());
        let view = page_request_view(
            &pending(
                "https://example.com/",
                PageRequestKind::Authentication { for_proxy: false },
            ),
            Some(&credentials),
        );

        assert_eq!(view.value, "alice");
        assert_eq!(view.secret, "pw");
    }

    #[test]
    fn nothing_is_saved_for_sign_ins_yet() {
        assert_eq!(saved_http_credentials("https://example.com", None), None);
    }

    #[test]
    fn a_permission_prompt_reads_as_a_sentence_and_offers_nothing_to_remember() {
        let view = page_request_view(
            &pending(
                "http://127.0.0.1:8123/",
                PageRequestKind::Permission(PagePermission::PersistentStorage),
            ),
            None,
        );

        assert_eq!(view.style, PageRequestStyle::Permission);
        assert_eq!(
            view.heading,
            "127.0.0.1:8123 wants to store data permanently on this device"
        );
        assert_eq!(view.accept_label, "Allow");
        assert_eq!(view.decline_label, "Block");
        // The asking frame is unknown, so nothing is offered to remember.
        assert_eq!(view.choice_label, "");
    }

    #[test]
    fn replies_carry_the_text_the_password_and_the_choice() {
        let prompt = PageRequestKind::Prompt {
            message: String::new(),
            default_value: String::new(),
        };
        assert_eq!(
            reply_for(&prompt, true, "typed", "", false),
            PageRequestReply::new(PageRequestAnswer::PromptText("typed".to_owned()))
        );
        assert_eq!(
            reply_for(&prompt, false, "typed", "", true),
            PageRequestReply::new(PageRequestAnswer::Decline).blocking_further_dialogs()
        );
        assert_eq!(
            reply_for(
                &PageRequestKind::Authentication { for_proxy: false },
                true,
                "alice",
                "pw",
                false
            ),
            PageRequestReply::new(PageRequestAnswer::Credentials(HttpCredentials::new(
                "alice".to_owned(),
                "pw".to_owned()
            )))
        );
        assert_eq!(
            reply_for(
                &PageRequestKind::Permission(PagePermission::Camera),
                false,
                "",
                "",
                true
            ),
            PageRequestReply::new(PageRequestAnswer::Decline)
        );
        assert_eq!(
            reply_for(
                &PageRequestKind::Alert {
                    message: String::new()
                },
                false,
                "",
                "",
                false
            ),
            PageRequestReply::new(PageRequestAnswer::Accept)
        );
        assert_eq!(
            reply_for(&PageRequestKind::LeavePage, false, "", "", false),
            PageRequestReply::new(PageRequestAnswer::Decline)
        );
    }

    #[test]
    fn an_answer_from_the_window_reaches_the_engine_as_one_command() {
        let mut controller = BrowserController::default();
        let tab_id = controller.active_tab_id();
        controller.handle_event(
            tab_id,
            BrowserEvent::AddressChanged {
                url: "https://example.com/".to_owned(),
            },
        );
        controller.handle_event(
            tab_id,
            BrowserEvent::PageRequested(PageRequest {
                id: PageRequestId::new(5),
                url: "https://example.com/".to_owned(),
                kind: PageRequestKind::Confirm {
                    message: "Sure?".to_owned(),
                },
                received_at: Duration::ZERO,
            }),
        );
        let kind = controller
            .pending_page_request(tab_id)
            .expect("pending")
            .kind()
            .clone();

        controller.answer_page_request(
            tab_id,
            PageRequestId::new(5),
            reply_for(&kind, true, "", "", false),
            Duration::ZERO,
        );

        assert_eq!(
            std::iter::from_fn(|| controller.next_command()).collect::<Vec<_>>(),
            vec![TabCommand::new(
                tab_id,
                BrowserCommand::AnswerPageRequest {
                    id: PageRequestId::new(5),
                    answer: PageRequestAnswer::Accept,
                }
            )]
        );
    }

    fn prompt(default_value: &str) -> PageRequestKind {
        PageRequestKind::Prompt {
            message: String::new(),
            default_value: default_value.to_owned(),
        }
    }

    #[test]
    fn an_unedited_prompt_returns_the_pages_own_default_value() {
        // The shown default is the cleaned copy; Accept makes the engine return the original.
        assert_eq!(
            reply_for(&prompt("Ada"), true, "Ada", "", false),
            PageRequestReply::new(PageRequestAnswer::Accept)
        );
    }

    #[test]
    fn line_breaks_never_reach_the_page_from_the_prompt_field() {
        assert_eq!(
            reply_for(&prompt("x"), true, "one\r\ntwo\nthree", "", false),
            PageRequestReply::new(PageRequestAnswer::PromptText("onetwothree".to_owned()))
        );
    }

    #[test]
    fn a_reload_asks_to_reload_and_a_second_leave_prompt_offers_to_stop_asking() {
        let mut controller = BrowserController::default();
        let tab_id = controller.active_tab_id();
        controller.handle_event(
            tab_id,
            BrowserEvent::AddressChanged {
                url: "https://example.com/".to_owned(),
            },
        );
        controller.queue_command(BrowserCommand::PointerButton {
            action: talaria_core::PointerButtonAction::Down,
            button: talaria_core::PointerButton::Left,
            x: 0.0,
            y: 0.0,
        });
        controller.queue_command(BrowserCommand::Reload);
        for id in [1, 2] {
            controller.handle_event(
                tab_id,
                BrowserEvent::PageRequested(PageRequest {
                    id: PageRequestId::new(id),
                    url: "https://example.com/".to_owned(),
                    kind: PageRequestKind::LeavePage,
                    received_at: Duration::ZERO,
                }),
            );
        }

        let view = page_request_view(
            &controller.pending_page_request(tab_id).expect("pending"),
            None,
        );
        assert_eq!(view.heading, "Reload this page?");
        assert_eq!(view.message, "Changes you made may not be saved.");
        assert_eq!(view.accept_label, "Reload");
        assert_eq!(view.decline_label, "Stay");
        assert_eq!(view.choice_label, BLOCK_LEAVE_PROMPTS_CHOICE);
    }

    #[test]
    fn a_rejected_sign_in_says_so_before_the_https_warning() {
        let mut controller = BrowserController::default();
        let tab_id = controller.active_tab_id();
        let sign_in = |id| {
            BrowserEvent::PageRequested(PageRequest {
                id: PageRequestId::new(id),
                url: "http://127.0.0.1:8123/private".to_owned(),
                kind: PageRequestKind::Authentication { for_proxy: false },
                received_at: Duration::from_secs(id),
            })
        };
        controller.handle_event(tab_id, sign_in(1));
        controller.answer_page_request(
            tab_id,
            PageRequestId::new(1),
            reply_for(
                &PageRequestKind::Authentication { for_proxy: false },
                true,
                "alice",
                "wrong",
                false,
            ),
            Duration::from_secs(1),
        );
        controller.handle_event(tab_id, sign_in(2));

        let view = page_request_view(
            &controller.pending_page_request(tab_id).expect("pending"),
            None,
        );
        assert_eq!(view.warning, format!("{CREDENTIALS_REJECTED}\n{NOT_HTTPS}"));
    }

    #[test]
    fn answers_are_ignored_for_half_a_second_after_a_request_appears() {
        let mut protector = InputProtector::default();
        let first = PageRequestId::new(1);
        let second = PageRequestId::new(2);
        let at = |ms| Duration::from_millis(ms);

        protector.show(Some(first), at(1_000));
        assert!(!protector.accepts(first, at(1_000)));
        assert!(!protector.accepts(first, at(1_499)));
        assert!(protector.accepts(first, at(1_500)));
        // Showing the same request again does not restart the time.
        protector.show(Some(first), at(1_600));
        assert!(protector.accepts(first, at(1_600)));
        // A request that is not the one on screen is never answered.
        assert!(!protector.accepts(second, at(5_000)));

        protector.show(Some(second), at(2_000));
        assert!(!protector.accepts(second, at(2_100)));
        assert!(!protector.accepts(first, at(9_000)));
        protector.show(None, at(3_000));
        assert!(!protector.accepts(second, at(9_000)));
    }
}
