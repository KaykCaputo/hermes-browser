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

use mercury_ui::{MainWindow, PageRequestStyle, PageRequestView};
use slint::ComponentHandle;
use talaria_core::{
    BrowserController, HttpCredentials, PageRequestAnswer, PageRequestId, PageRequestKind,
    PageRequestReply, PendingPageRequest,
};
use talaria_servo::page_request_clock;

use crate::ui_sync::sync_ui;

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
    let view = controller
        .pending_page_request(controller.active_tab_id())
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
        let Ok(id) = u64::try_from(id) else {
            return;
        };
        let id = PageRequestId::new(id);
        let tab_id = controller.borrow().active_tab_id();
        let pending = controller.borrow().pending_page_request(tab_id);
        let Some(pending) = pending.filter(|pending| pending.id() == id) else {
            return;
        };
        let reply = reply_for(pending.kind(), accepted, &text, &password, checked);
        controller
            .borrow_mut()
            .answer_page_request(tab_id, id, reply, page_request_clock());
        if let Some(window) = weak_window.upgrade() {
            sync_ui(&window, &controller.borrow());
            window.window().request_redraw();
        }
    });
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
        (PageRequestKind::Prompt { .. }, true) => PageRequestAnswer::PromptText(text.to_owned()),
        (PageRequestKind::Authentication { .. }, true) => PageRequestAnswer::Credentials(
            HttpCredentials::new(text.to_owned(), password.to_owned()),
        ),
        (_, true) => PageRequestAnswer::Accept,
        (_, false) => PageRequestAnswer::Decline,
    };
    let reply = PageRequestReply::new(answer);
    match kind {
        _ if !checked => reply,
        PageRequestKind::Permission(_) => reply.remembering_for_site(),
        _ => reply.blocking_further_dialogs(),
    }
}

const BLOCK_DIALOGS_CHOICE: &str = "Don't allow this page to show more dialogs";
const REMEMBER_CHOICE: &str = "Remember for this site until you quit";

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
        PageRequestKind::LeavePage => PageRequestView {
            style: PageRequestStyle::LeavePage,
            heading: "Leave this page?".into(),
            message: "Changes you made may not be saved.".into(),
            detail: format!("Asked by {site}").into(),
            accept_label: "Leave".into(),
            decline_label: "Stay".into(),
            ..PageRequestView::default()
        },
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
            let warning = if pending.origin().is_secure() {
                ""
            } else {
                "This connection is not secure (not HTTPS): your user name and password will be \
                 sent unencrypted."
            };
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
            // A page without an origin has no site to remember the decision for.
            choice_label: if pending.origin().key().is_some() {
                REMEMBER_CHOICE
            } else {
                ""
            }
            .into(),
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
    fn a_permission_prompt_reads_as_a_sentence_and_offers_to_remember() {
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
        assert_eq!(view.choice_label, REMEMBER_CHOICE);
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
            PageRequestReply::new(PageRequestAnswer::Decline).remembering_for_site()
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
}
