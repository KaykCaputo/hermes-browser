use std::time::Duration;

use talaria_core::{
    BrowserCommand, BrowserController, BrowserEvent, HttpCredentials, KeyLocation, KeyState,
    KeyboardInput, KeyboardModifiers, LoadState, PagePermission, PageRequest, PageRequestAnswer,
    PageRequestId, PageRequestKind, PageRequestReply, PermissionDecision, PermissionDecisions,
    PointerButton, PointerButtonAction, SessionPermissionDecisions, TabCommand, TabId,
};

const PAGE: &str = "https://example.com/form";

fn ready_controller() -> (BrowserController, TabId) {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    commit_page(&mut controller, tab_id, PAGE);
    (controller, tab_id)
}

fn commit_page(controller: &mut BrowserController, tab_id: TabId, url: &str) {
    controller.handle_event(
        tab_id,
        BrowserEvent::AddressChanged {
            url: url.to_owned(),
        },
    );
}

fn request(id: u64, kind: PageRequestKind, at_ms: u64) -> PageRequest {
    PageRequest {
        id: PageRequestId::new(id),
        url: PAGE.to_owned(),
        kind,
        received_at: Duration::from_millis(at_ms),
    }
}

fn alert(id: u64, at_ms: u64) -> PageRequest {
    request(
        id,
        PageRequestKind::Alert {
            message: format!("alert {id}"),
        },
        at_ms,
    )
}

fn receive(controller: &mut BrowserController, tab_id: TabId, request: PageRequest) {
    assert!(controller.handle_event(tab_id, BrowserEvent::PageRequested(request)));
}

fn answer(id: u64, answer: PageRequestAnswer) -> BrowserCommand {
    BrowserCommand::AnswerPageRequest {
        id: PageRequestId::new(id),
        answer,
    }
}

fn drain(controller: &mut BrowserController) -> Vec<TabCommand> {
    std::iter::from_fn(|| controller.next_command()).collect()
}

fn pending_id(controller: &BrowserController, tab_id: TabId) -> Option<u64> {
    controller
        .pending_page_request(tab_id)
        .map(|pending| pending.id().value())
}

fn reply(answer: PageRequestAnswer) -> PageRequestReply {
    PageRequestReply::new(answer)
}

#[test]
fn requests_are_shown_one_at_a_time_in_arrival_order() {
    let (mut controller, tab_id) = ready_controller();
    receive(&mut controller, tab_id, alert(1, 0));
    receive(&mut controller, tab_id, alert(2, 10));

    assert_eq!(pending_id(&controller, tab_id), Some(1));
    assert!(controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept),
        Duration::from_millis(20),
    ));
    assert_eq!(pending_id(&controller, tab_id), Some(2));
    assert_eq!(
        drain(&mut controller),
        vec![TabCommand::new(
            tab_id,
            answer(1, PageRequestAnswer::Accept)
        )]
    );
}

#[test]
fn a_request_is_answered_exactly_once() {
    let (mut controller, tab_id) = ready_controller();
    receive(&mut controller, tab_id, alert(1, 0));

    assert!(controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept),
        Duration::ZERO,
    ));
    assert!(!controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Decline),
        Duration::ZERO,
    ));
    assert_eq!(drain(&mut controller).len(), 1);
}

#[test]
fn an_answer_goes_to_the_tab_that_asked() {
    let (mut controller, first_tab) = ready_controller();
    let second_tab = controller.open_tab();
    commit_page(&mut controller, second_tab, PAGE);
    receive(&mut controller, first_tab, alert(1, 0));
    receive(&mut controller, second_tab, alert(2, 0));

    assert!(!controller.answer_page_request(
        second_tab,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept),
        Duration::ZERO,
    ));
    assert!(controller.answer_page_request(
        first_tab,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept),
        Duration::ZERO,
    ));

    assert_eq!(
        drain(&mut controller),
        vec![TabCommand::new(
            first_tab,
            answer(1, PageRequestAnswer::Accept)
        )]
    );
    assert_eq!(pending_id(&controller, second_tab), Some(2));
}

#[test]
fn requests_of_a_background_tab_wait_for_that_tab() {
    let (mut controller, first_tab) = ready_controller();
    let second_tab = controller.open_tab();
    commit_page(&mut controller, second_tab, PAGE);
    assert_eq!(controller.active_tab_id(), second_tab);

    receive(&mut controller, first_tab, alert(1, 0));

    assert_eq!(pending_id(&controller, second_tab), None);
    assert_eq!(pending_id(&controller, first_tab), Some(1));
    assert!(drain(&mut controller).is_empty());
}

#[test]
fn navigating_reloading_or_traversing_drops_the_tabs_requests_first() {
    for command in [
        BrowserCommand::Reload,
        BrowserCommand::GoBack,
        BrowserCommand::GoForward,
        BrowserCommand::Navigate {
            url: "https://other.test/".to_owned(),
        },
    ] {
        let (mut controller, tab_id) = ready_controller();
        receive(&mut controller, tab_id, alert(1, 0));
        receive(
            &mut controller,
            tab_id,
            request(2, PageRequestKind::Authentication { for_proxy: false }, 0),
        );

        controller.queue_command(command.clone());

        assert_eq!(pending_id(&controller, tab_id), None);
        assert_eq!(
            drain(&mut controller),
            vec![
                TabCommand::new(tab_id, answer(1, PageRequestAnswer::Discard)),
                TabCommand::new(tab_id, answer(2, PageRequestAnswer::Discard)),
                TabCommand::new(tab_id, command),
            ]
        );
    }
}

#[test]
fn the_address_bar_navigation_drops_the_tabs_requests() {
    let (mut controller, tab_id) = ready_controller();
    receive(&mut controller, tab_id, alert(1, 0));

    controller
        .queue_navigation("other.test")
        .expect("the address resolves");

    assert_eq!(pending_id(&controller, tab_id), None);
    assert_eq!(
        drain(&mut controller)[0],
        TabCommand::new(tab_id, answer(1, PageRequestAnswer::Discard))
    );
}

#[test]
fn a_new_document_drops_the_requests_of_the_previous_one() {
    let (mut controller, tab_id) = ready_controller();
    receive(
        &mut controller,
        tab_id,
        request(1, PageRequestKind::Authentication { for_proxy: false }, 0),
    );

    controller.handle_event(tab_id, BrowserEvent::PageLoadStarted);

    assert_eq!(pending_id(&controller, tab_id), None);
    assert_eq!(
        drain(&mut controller),
        vec![TabCommand::new(
            tab_id,
            answer(1, PageRequestAnswer::Discard)
        )]
    );
}

#[test]
fn a_crash_drops_the_tabs_requests() {
    let (mut controller, tab_id) = ready_controller();
    receive(&mut controller, tab_id, alert(1, 0));

    controller.handle_event(
        tab_id,
        BrowserEvent::LoadStateChanged {
            state: LoadState::Failed {
                message: "crashed".to_owned(),
            },
        },
    );

    assert_eq!(pending_id(&controller, tab_id), None);
    assert_eq!(
        drain(&mut controller),
        vec![TabCommand::new(
            tab_id,
            answer(1, PageRequestAnswer::Discard)
        )]
    );
}

#[test]
fn closing_a_tab_forgets_its_requests_and_their_queued_answers() {
    let (mut controller, first_tab) = ready_controller();
    let second_tab = controller.open_tab();
    receive(&mut controller, first_tab, alert(1, 0));
    receive(&mut controller, first_tab, alert(2, 0));
    assert!(controller.answer_page_request(
        first_tab,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept),
        Duration::ZERO,
    ));

    assert!(controller.close_tab(first_tab));

    // The engine drops the closed tab's web view, which drops request 2 with its default.
    assert_eq!(pending_id(&controller, first_tab), None);
    assert!(drain(&mut controller).is_empty());
    assert_eq!(controller.active_tab_id(), second_tab);
}

/// A tab whose web view is loading its first page, which has not committed yet.
fn loading_first_page() -> (BrowserController, TabId) {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    controller
        .set_initial_address(tab_id, "a.test")
        .expect("the address resolves");
    assert!(controller.take_initial_url(tab_id).is_some());
    (controller, tab_id)
}

#[test]
fn a_sign_in_for_a_first_page_that_has_not_committed_is_answered_at_once() {
    let (mut controller, tab_id) = loading_first_page();
    receive(
        &mut controller,
        tab_id,
        request(1, PageRequestKind::Authentication { for_proxy: false }, 0),
    );

    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Decline),
        Duration::ZERO,
    );

    // The page cannot commit before the engine has the answer, so it must not wait for that.
    assert_eq!(
        drain(&mut controller),
        vec![TabCommand::new(
            tab_id,
            answer(1, PageRequestAnswer::Decline)
        )]
    );
}

#[test]
fn a_replaced_web_view_starts_without_the_old_ones_requests() {
    let (mut controller, tab_id) = loading_first_page();
    receive(
        &mut controller,
        tab_id,
        request(1, PageRequestKind::Authentication { for_proxy: false }, 0),
    );

    // The engine cannot navigate a page that never committed, so it replaces the web view,
    // which drops the old one's requests with their defaults.
    controller
        .queue_navigation("b.test")
        .expect("the address resolves");

    assert_eq!(pending_id(&controller, tab_id), None);
    assert!(drain(&mut controller).is_empty());
}

#[test]
fn the_first_dialog_does_not_offer_to_block_further_dialogs() {
    let (mut controller, tab_id) = ready_controller();
    receive(&mut controller, tab_id, alert(1, 0));

    let pending = controller
        .pending_page_request(tab_id)
        .expect("the alert is pending");
    assert!(!pending.offers_to_block_dialogs());
}

#[test]
fn a_dialog_soon_after_the_previous_one_offers_to_block_further_dialogs() {
    let (mut controller, tab_id) = ready_controller();
    receive(&mut controller, tab_id, alert(1, 0));
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept),
        Duration::from_secs(10),
    );
    receive(&mut controller, tab_id, alert(2, 12_999));

    assert!(
        controller
            .pending_page_request(tab_id)
            .expect("the second alert is pending")
            .offers_to_block_dialogs()
    );
}

#[test]
fn a_dialog_long_after_the_previous_one_does_not_offer_to_block() {
    let (mut controller, tab_id) = ready_controller();
    receive(&mut controller, tab_id, alert(1, 0));
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept),
        Duration::from_secs(10),
    );
    receive(&mut controller, tab_id, alert(2, 13_000));

    assert!(
        !controller
            .pending_page_request(tab_id)
            .expect("the second alert is pending")
            .offers_to_block_dialogs()
    );
}

#[test]
fn a_dialog_with_more_waiting_behind_it_offers_to_block() {
    let (mut controller, tab_id) = ready_controller();
    receive(&mut controller, tab_id, alert(1, 0));
    receive(&mut controller, tab_id, alert(2, 0));

    assert!(
        controller
            .pending_page_request(tab_id)
            .expect("the first alert is pending")
            .offers_to_block_dialogs()
    );
}

#[test]
fn blocking_dialogs_answers_the_pages_later_dialogs_with_their_defaults() {
    let (mut controller, tab_id) = ready_controller();
    receive(&mut controller, tab_id, alert(1, 0));
    receive(&mut controller, tab_id, alert(2, 0));
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept).blocking_further_dialogs(),
        Duration::from_millis(5),
    );
    receive(
        &mut controller,
        tab_id,
        request(
            3,
            PageRequestKind::Confirm {
                message: "again?".to_owned(),
            },
            6,
        ),
    );
    receive(
        &mut controller,
        tab_id,
        request(4, PageRequestKind::LeavePage, 7),
    );

    assert_eq!(pending_id(&controller, tab_id), None);
    assert_eq!(
        drain(&mut controller),
        vec![
            TabCommand::new(tab_id, answer(2, PageRequestAnswer::Discard)),
            TabCommand::new(tab_id, answer(1, PageRequestAnswer::Accept)),
            TabCommand::new(tab_id, answer(3, PageRequestAnswer::Discard)),
            TabCommand::new(tab_id, answer(4, PageRequestAnswer::Discard)),
        ]
    );
}

#[test]
fn blocking_dialogs_does_not_block_permission_or_sign_in_requests() {
    let (mut controller, tab_id) = ready_controller();
    receive(&mut controller, tab_id, alert(1, 0));
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept).blocking_further_dialogs(),
        Duration::ZERO,
    );
    receive(
        &mut controller,
        tab_id,
        request(2, PageRequestKind::Authentication { for_proxy: false }, 1),
    );

    assert_eq!(pending_id(&controller, tab_id), Some(2));
}

#[test]
fn dialogs_are_allowed_again_after_the_page_navigates() {
    let (mut controller, tab_id) = ready_controller();
    receive(&mut controller, tab_id, alert(1, 0));
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept).blocking_further_dialogs(),
        Duration::ZERO,
    );

    controller.handle_event(tab_id, BrowserEvent::PageLoadStarted);
    receive(&mut controller, tab_id, alert(2, 100_000));

    let pending = controller
        .pending_page_request(tab_id)
        .expect("the new page may show dialogs");
    assert_eq!(pending.id().value(), 2);
    assert!(!pending.offers_to_block_dialogs());
}

fn key_down() -> KeyboardInput {
    KeyboardInput {
        state: KeyState::Down,
        key: "a".to_owned(),
        code: "KeyA".to_owned(),
        location: KeyLocation::Standard,
        modifiers: KeyboardModifiers::default(),
        repeat: false,
        is_composing: false,
    }
}

#[test]
fn leaving_a_page_the_user_never_interacted_with_is_allowed_without_asking() {
    let (mut controller, tab_id) = ready_controller();
    receive(
        &mut controller,
        tab_id,
        request(1, PageRequestKind::LeavePage, 0),
    );

    assert_eq!(pending_id(&controller, tab_id), None);
    assert_eq!(
        drain(&mut controller),
        vec![TabCommand::new(
            tab_id,
            answer(1, PageRequestAnswer::Accept)
        )]
    );
}

#[test]
fn leaving_a_page_after_a_click_or_a_key_press_asks() {
    for interaction in [
        BrowserCommand::PointerButton {
            action: PointerButtonAction::Down,
            button: PointerButton::Left,
            x: 1.0,
            y: 1.0,
        },
        BrowserCommand::KeyboardInput(key_down()),
    ] {
        let (mut controller, tab_id) = ready_controller();
        controller.queue_command(interaction);
        drain(&mut controller);

        receive(
            &mut controller,
            tab_id,
            request(1, PageRequestKind::LeavePage, 0),
        );

        assert_eq!(pending_id(&controller, tab_id), Some(1));
        assert!(drain(&mut controller).is_empty());
    }
}

#[test]
fn moving_or_scrolling_is_not_an_interaction_and_a_new_document_forgets_one() {
    let (mut controller, tab_id) = ready_controller();
    controller.queue_command(BrowserCommand::PointerMoved { x: 1.0, y: 1.0 });
    controller.queue_command(BrowserCommand::Scrolled {
        delta_x: 0.0,
        delta_y: 4.0,
        x: 1.0,
        y: 1.0,
    });
    receive(
        &mut controller,
        tab_id,
        request(1, PageRequestKind::LeavePage, 0),
    );
    assert_eq!(pending_id(&controller, tab_id), None);

    controller.queue_command(BrowserCommand::KeyboardInput(key_down()));
    controller.handle_event(tab_id, BrowserEvent::PageLoadStarted);
    receive(
        &mut controller,
        tab_id,
        request(2, PageRequestKind::LeavePage, 0),
    );
    assert_eq!(pending_id(&controller, tab_id), None);
}

fn permission(id: u64, url: &str, permission: PagePermission) -> PageRequest {
    PageRequest {
        id: PageRequestId::new(id),
        url: url.to_owned(),
        kind: PageRequestKind::Permission(permission),
        received_at: Duration::ZERO,
    }
}

#[test]
fn notifications_are_denied_without_asking() {
    let (mut controller, tab_id) = ready_controller();
    receive(
        &mut controller,
        tab_id,
        permission(1, PAGE, PagePermission::Notifications),
    );

    assert_eq!(pending_id(&controller, tab_id), None);
    assert_eq!(
        drain(&mut controller),
        vec![TabCommand::new(
            tab_id,
            answer(1, PageRequestAnswer::Decline)
        )]
    );
}

#[test]
fn a_remembered_permission_decision_answers_later_requests_from_the_site() {
    let (mut controller, tab_id) = ready_controller();
    receive(
        &mut controller,
        tab_id,
        permission(1, PAGE, PagePermission::PersistentStorage),
    );
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept).remembering_for_site(),
        Duration::ZERO,
    );
    drain(&mut controller);

    receive(
        &mut controller,
        tab_id,
        permission(
            2,
            "https://example.com/other",
            PagePermission::PersistentStorage,
        ),
    );
    receive(
        &mut controller,
        tab_id,
        permission(3, "https://other.test/", PagePermission::PersistentStorage),
    );
    receive(
        &mut controller,
        tab_id,
        permission(4, PAGE, PagePermission::Geolocation),
    );

    // Request 4 is for geolocation, which is denied without a prompt anyway.
    assert_eq!(
        drain(&mut controller),
        vec![
            TabCommand::new(tab_id, answer(2, PageRequestAnswer::Accept)),
            TabCommand::new(tab_id, answer(4, PageRequestAnswer::Decline)),
        ]
    );
    assert_eq!(pending_id(&controller, tab_id), Some(3));
}

#[test]
fn an_unremembered_permission_decision_asks_again() {
    let (mut controller, tab_id) = ready_controller();
    receive(
        &mut controller,
        tab_id,
        permission(1, PAGE, PagePermission::PersistentStorage),
    );
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Decline),
        Duration::ZERO,
    );
    receive(
        &mut controller,
        tab_id,
        permission(2, PAGE, PagePermission::PersistentStorage),
    );

    assert_eq!(pending_id(&controller, tab_id), Some(2));
}

#[test]
fn remembering_a_decision_answers_the_same_request_waiting_in_other_tabs() {
    let (mut controller, first_tab) = ready_controller();
    let second_tab = controller.open_tab();
    commit_page(&mut controller, second_tab, PAGE);
    receive(
        &mut controller,
        first_tab,
        permission(1, PAGE, PagePermission::PersistentStorage),
    );
    receive(
        &mut controller,
        second_tab,
        permission(2, PAGE, PagePermission::PersistentStorage),
    );

    controller.answer_page_request(
        first_tab,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Decline).remembering_for_site(),
        Duration::ZERO,
    );

    assert_eq!(pending_id(&controller, second_tab), None);
    assert_eq!(
        drain(&mut controller),
        vec![
            TabCommand::new(first_tab, answer(1, PageRequestAnswer::Decline)),
            TabCommand::new(second_tab, answer(2, PageRequestAnswer::Decline)),
        ]
    );
}

#[test]
fn decisions_for_pages_without_an_origin_are_not_remembered() {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    commit_page(&mut controller, tab_id, "file:///tmp/page.html");
    receive(
        &mut controller,
        tab_id,
        permission(
            1,
            "file:///tmp/page.html",
            PagePermission::PersistentStorage,
        ),
    );
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept).remembering_for_site(),
        Duration::ZERO,
    );
    receive(
        &mut controller,
        tab_id,
        permission(
            2,
            "file:///tmp/page.html",
            PagePermission::PersistentStorage,
        ),
    );

    assert_eq!(pending_id(&controller, tab_id), Some(2));
}

#[derive(Debug)]
struct AlwaysBlock;

impl PermissionDecisions for AlwaysBlock {
    fn decision(&self, _origin: &str, _permission: PagePermission) -> Option<PermissionDecision> {
        Some(PermissionDecision::Block)
    }

    fn remember(&mut self, _origin: &str, _permission: PagePermission, _: PermissionDecision) {}
}

#[test]
fn the_permission_store_can_be_replaced() {
    let (mut controller, tab_id) = ready_controller();
    controller.set_permission_decisions(Box::new(AlwaysBlock));
    receive(
        &mut controller,
        tab_id,
        permission(1, PAGE, PagePermission::PersistentStorage),
    );

    assert_eq!(
        drain(&mut controller),
        vec![TabCommand::new(
            tab_id,
            answer(1, PageRequestAnswer::Decline)
        )]
    );
}

#[test]
fn the_session_store_keeps_decisions_per_origin_and_permission() {
    let mut store = SessionPermissionDecisions::default();
    store.remember(
        "https://a.test",
        PagePermission::Geolocation,
        PermissionDecision::Allow,
    );
    store.remember(
        "https://a.test",
        PagePermission::Geolocation,
        PermissionDecision::Block,
    );

    assert_eq!(
        store.decision("https://a.test", PagePermission::Geolocation),
        Some(PermissionDecision::Block)
    );
    assert_eq!(
        store.decision("https://a.test", PagePermission::Camera),
        None
    );
    assert_eq!(
        store.decision("https://b.test", PagePermission::Geolocation),
        None
    );
}

#[test]
fn messages_are_shown_without_control_or_direction_override_characters() {
    let (mut controller, tab_id) = ready_controller();
    receive(
        &mut controller,
        tab_id,
        request(
            1,
            PageRequestKind::Prompt {
                message: "Pay\u{202E}moc.elgoog\u{202C}\r\nnow\u{0007}\ttoday".to_owned(),
                default_value: "line\none\u{2066}".to_owned(),
            },
            0,
        ),
    );

    let pending = controller
        .pending_page_request(tab_id)
        .expect("the prompt is pending");
    assert_eq!(
        pending.kind(),
        &PageRequestKind::Prompt {
            message: "Paymoc.elgoog\nnow today".to_owned(),
            default_value: "line one".to_owned(),
        }
    );
}

#[test]
fn the_pending_request_names_the_origin_it_came_from() {
    let (mut controller, tab_id) = ready_controller();
    receive(
        &mut controller,
        tab_id,
        PageRequest {
            id: PageRequestId::new(1),
            url: "http://127.0.0.1:8123/private/".to_owned(),
            kind: PageRequestKind::Authentication { for_proxy: false },
            received_at: Duration::ZERO,
        },
    );

    let pending = controller
        .pending_page_request(tab_id)
        .expect("the sign-in request is pending");
    assert_eq!(pending.origin().label(), "127.0.0.1:8123");
    assert!(!pending.origin().is_secure());
}

#[test]
fn credentials_never_appear_in_debug_output() {
    let credentials = HttpCredentials::new("alice".to_owned(), "hunter2".to_owned());
    let command = BrowserCommand::AnswerPageRequest {
        id: PageRequestId::new(1),
        answer: PageRequestAnswer::Credentials(credentials.clone()),
    };

    assert_eq!(credentials.username(), "alice");
    assert_eq!(credentials.password(), "hunter2");
    assert!(!format!("{command:?}").contains("hunter2"));
}

// Review round 1

const EVERY_PERMISSION: [PagePermission; 13] = [
    PagePermission::Geolocation,
    PagePermission::Notifications,
    PagePermission::Push,
    PagePermission::Midi,
    PagePermission::Camera,
    PagePermission::Microphone,
    PagePermission::Speaker,
    PagePermission::DeviceInfo,
    PagePermission::BackgroundSync,
    PagePermission::Bluetooth,
    PagePermission::PersistentStorage,
    PagePermission::ScreenWakeLock,
    PagePermission::Gamepad,
];

#[test]
fn only_persistent_storage_is_asked_and_every_other_permission_is_denied_without_asking() {
    for (index, feature) in EVERY_PERMISSION.into_iter().enumerate() {
        let (mut controller, tab_id) = ready_controller();
        let id = index as u64 + 1;
        receive(&mut controller, tab_id, permission(id, PAGE, feature));

        if feature == PagePermission::PersistentStorage {
            assert!(feature.is_promptable());
            assert_eq!(pending_id(&controller, tab_id), Some(id), "{feature:?}");
            assert!(drain(&mut controller).is_empty(), "{feature:?}");
        } else {
            assert!(!feature.is_promptable(), "{feature:?}");
            assert_eq!(pending_id(&controller, tab_id), None, "{feature:?}");
            assert_eq!(
                drain(&mut controller),
                vec![TabCommand::new(
                    tab_id,
                    answer(id, PageRequestAnswer::Decline)
                )],
                "{feature:?}"
            );
        }
    }
}

#[test]
fn a_request_from_a_web_view_being_replaced_is_never_queued_on_the_new_page() {
    let (mut controller, tab_id) = loading_first_page();
    controller
        .queue_navigation("b.test")
        .expect("the address resolves");

    // Sent by the old web view before it was closed, read after the replacement was decided.
    receive(&mut controller, tab_id, alert(1, 0));
    assert_eq!(pending_id(&controller, tab_id), None);

    assert_eq!(
        controller.take_initial_url(tab_id).as_deref(),
        Some("https://b.test/")
    );
    assert_eq!(pending_id(&controller, tab_id), None);
}

fn interact(controller: &mut BrowserController) {
    controller.queue_command(BrowserCommand::KeyboardInput(key_down()));
    drain(controller);
}

#[test]
fn a_second_leave_prompt_soon_after_the_first_offers_to_stop_asking() {
    let (mut controller, tab_id) = ready_controller();
    interact(&mut controller);
    receive(
        &mut controller,
        tab_id,
        request(1, PageRequestKind::LeavePage, 0),
    );
    assert!(
        !controller
            .pending_page_request(tab_id)
            .expect("the first leave prompt is pending")
            .offers_to_block_dialogs()
    );
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept),
        Duration::from_millis(500),
    );

    receive(
        &mut controller,
        tab_id,
        request(2, PageRequestKind::LeavePage, 600),
    );
    assert!(
        controller
            .pending_page_request(tab_id)
            .expect("the second leave prompt is pending")
            .offers_to_block_dialogs()
    );
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(2),
        reply(PageRequestAnswer::Accept).blocking_further_dialogs(),
        Duration::from_millis(700),
    );
    drain(&mut controller);

    // Further leave prompts are allowed without asking; ordinary dialogs still ask.
    receive(
        &mut controller,
        tab_id,
        request(3, PageRequestKind::LeavePage, 800),
    );
    receive(&mut controller, tab_id, alert(4, 900));
    assert_eq!(
        drain(&mut controller),
        vec![TabCommand::new(
            tab_id,
            answer(3, PageRequestAnswer::Discard)
        )]
    );
    assert_eq!(pending_id(&controller, tab_id), Some(4));
}

#[test]
fn a_leave_prompt_caused_by_a_reload_says_so() {
    let (mut controller, tab_id) = ready_controller();
    interact(&mut controller);
    controller.queue_command(BrowserCommand::Reload);
    receive(
        &mut controller,
        tab_id,
        request(1, PageRequestKind::LeavePage, 0),
    );
    assert!(
        controller
            .pending_page_request(tab_id)
            .expect("pending")
            .is_reload_prompt()
    );

    let (mut controller, tab_id) = ready_controller();
    interact(&mut controller);
    receive(
        &mut controller,
        tab_id,
        request(1, PageRequestKind::LeavePage, 0),
    );
    assert!(
        !controller
            .pending_page_request(tab_id)
            .expect("pending")
            .is_reload_prompt()
    );
}

fn sign_in(id: u64, url: &str, at_ms: u64) -> PageRequest {
    PageRequest {
        id: PageRequestId::new(id),
        url: url.to_owned(),
        kind: PageRequestKind::Authentication { for_proxy: false },
        received_at: Duration::from_millis(at_ms),
    }
}

fn submit_credentials(controller: &mut BrowserController, tab_id: TabId, id: u64, at_ms: u64) {
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(id),
        reply(PageRequestAnswer::Credentials(HttpCredentials::new(
            "alice".to_owned(),
            "wrong".to_owned(),
        ))),
        Duration::from_millis(at_ms),
    );
}

#[test]
fn a_sign_in_asked_again_soon_after_credentials_were_sent_says_they_were_not_accepted() {
    let (mut controller, tab_id) = ready_controller();
    receive(
        &mut controller,
        tab_id,
        sign_in(1, "https://example.com/a", 0),
    );
    assert!(
        !controller
            .pending_page_request(tab_id)
            .expect("pending")
            .credentials_were_rejected()
    );
    submit_credentials(&mut controller, tab_id, 1, 1_000);

    receive(
        &mut controller,
        tab_id,
        sign_in(2, "https://example.com/a", 2_000),
    );
    assert!(
        controller
            .pending_page_request(tab_id)
            .expect("pending")
            .credentials_were_rejected()
    );
}

#[test]
fn a_later_or_other_sites_sign_in_is_not_marked_as_rejected() {
    let (mut controller, tab_id) = ready_controller();
    receive(
        &mut controller,
        tab_id,
        sign_in(1, "https://example.com/a", 0),
    );
    submit_credentials(&mut controller, tab_id, 1, 1_000);
    receive(
        &mut controller,
        tab_id,
        sign_in(2, "https://other.test/", 2_000),
    );
    assert!(
        !controller
            .pending_page_request(tab_id)
            .expect("pending")
            .credentials_were_rejected()
    );
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(2),
        reply(PageRequestAnswer::Decline),
        Duration::from_millis(2_100),
    );

    receive(
        &mut controller,
        tab_id,
        sign_in(3, "https://example.com/a", 60_000),
    );
    assert!(
        !controller
            .pending_page_request(tab_id)
            .expect("pending")
            .credentials_were_rejected()
    );
}

#[test]
fn user_names_and_passwords_never_reach_the_address() {
    let (mut controller, tab_id) = ready_controller();
    commit_page(
        &mut controller,
        tab_id,
        "http://alice:secret@127.0.0.1:8123/private?q#f",
    );

    assert_eq!(
        controller.active_state().url(),
        "http://127.0.0.1:8123/private?q#f"
    );
}

// Review round 2

#[test]
fn the_users_navigation_ends_the_blocking_of_the_page_it_leaves() {
    // The page is stuck before HeadParsed, so the engine may report no new document at all for
    // the next site; blocking must not carry over to it.
    let (mut controller, tab_id) = ready_controller();
    interact(&mut controller);
    receive(&mut controller, tab_id, alert(1, 0));
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept).blocking_further_dialogs(),
        Duration::from_millis(10),
    );
    receive(
        &mut controller,
        tab_id,
        request(2, PageRequestKind::LeavePage, 20),
    );
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(2),
        reply(PageRequestAnswer::Accept).blocking_further_dialogs(),
        Duration::from_millis(30),
    );
    controller.queue_command(BrowserCommand::Navigate {
        url: "https://other.test/".to_owned(),
    });
    drain(&mut controller);

    // The page being left keeps its blocks until it is gone: its leave prompt is allowed
    // without asking ...
    receive(
        &mut controller,
        tab_id,
        request(3, PageRequestKind::LeavePage, 40),
    );
    assert_eq!(pending_id(&controller, tab_id), None);
    // ... and the next site's dialogs are shown, even before the engine reports its document.
    receive(
        &mut controller,
        tab_id,
        PageRequest {
            url: "https://other.test/".to_owned(),
            ..alert(4, 10_000)
        },
    );
    assert_eq!(pending_id(&controller, tab_id), Some(4));
}

#[test]
fn staying_on_a_page_ends_the_reload_wording() {
    let (mut controller, tab_id) = ready_controller();
    interact(&mut controller);
    controller.queue_command(BrowserCommand::Reload);
    receive(
        &mut controller,
        tab_id,
        request(1, PageRequestKind::LeavePage, 0),
    );
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Decline),
        Duration::from_secs(1),
    );

    // Later the page itself navigates (a link): not a reload.
    receive(
        &mut controller,
        tab_id,
        request(2, PageRequestKind::LeavePage, 60_000),
    );
    assert!(
        !controller
            .pending_page_request(tab_id)
            .expect("pending")
            .is_reload_prompt()
    );
}

// Review round 3

/// A page whose dialogs and leave prompts the user has blocked.
fn blocked_page() -> (BrowserController, TabId) {
    let (mut controller, tab_id) = ready_controller();
    interact(&mut controller);
    receive(&mut controller, tab_id, alert(1, 0));
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(1),
        reply(PageRequestAnswer::Accept).blocking_further_dialogs(),
        Duration::from_millis(10),
    );
    receive(
        &mut controller,
        tab_id,
        request(2, PageRequestKind::LeavePage, 20),
    );
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(2),
        reply(PageRequestAnswer::Decline).blocking_further_dialogs(),
        Duration::from_millis(30),
    );
    drain(&mut controller);
    (controller, tab_id)
}

#[test]
fn a_block_survives_a_reload_the_user_cancels_by_staying() {
    let (mut controller, tab_id) = blocked_page();
    controller.queue_command(BrowserCommand::Reload);
    drain(&mut controller);
    // Leave prompts are blocked: the reload's prompt is answered with the default at once...
    receive(
        &mut controller,
        tab_id,
        request(3, PageRequestKind::LeavePage, 40),
    );
    assert_eq!(pending_id(&controller, tab_id), None);
    assert_eq!(
        drain(&mut controller),
        vec![TabCommand::new(
            tab_id,
            answer(3, PageRequestAnswer::Discard)
        )]
    );
    // ... and the page's next alert is still blocked.
    receive(&mut controller, tab_id, alert(4, 50));
    assert_eq!(pending_id(&controller, tab_id), None);
}

#[test]
fn a_blocked_page_cannot_make_the_user_choose_to_stay() {
    // Its leave prompts are blocked with its dialogs, so a reload never asks: the only way the
    // block could survive is a reload that produces no new document, covered above.
    let (mut controller, tab_id) = blocked_page();
    controller.queue_command(BrowserCommand::Reload);
    receive(
        &mut controller,
        tab_id,
        request(3, PageRequestKind::LeavePage, 40),
    );
    assert_eq!(pending_id(&controller, tab_id), None);
}

#[test]
fn a_block_survives_a_same_document_traversal() {
    let (mut controller, tab_id) = blocked_page();
    controller.queue_command(BrowserCommand::GoBack);
    drain(&mut controller);
    // No new document: the page keeps running and is still blocked.
    receive(&mut controller, tab_id, alert(3, 1_000));
    assert_eq!(pending_id(&controller, tab_id), None);
    receive(&mut controller, tab_id, alert(4, 100_000));
    assert_eq!(pending_id(&controller, tab_id), None);
}

#[test]
fn a_block_ends_when_another_site_asks_or_a_new_document_starts() {
    let (mut controller, tab_id) = blocked_page();
    controller.queue_command(BrowserCommand::Navigate {
        url: "https://other.test/".to_owned(),
    });
    drain(&mut controller);
    receive(
        &mut controller,
        tab_id,
        PageRequest {
            url: "https://other.test/".to_owned(),
            ..alert(3, 1_000)
        },
    );
    assert_eq!(pending_id(&controller, tab_id), Some(3));

    let (mut controller, tab_id) = blocked_page();
    controller.queue_command(BrowserCommand::Reload);
    controller.handle_event(tab_id, BrowserEvent::PageLoadStarted);
    receive(&mut controller, tab_id, alert(3, 1_000));
    assert_eq!(pending_id(&controller, tab_id), Some(3));
}

#[test]
fn another_site_does_not_end_a_block_the_user_did_not_navigate_away_from() {
    // A frame from another site asks while the user has not navigated: still blocked.
    let (mut controller, tab_id) = blocked_page();
    receive(
        &mut controller,
        tab_id,
        PageRequest {
            url: "https://other.test/".to_owned(),
            ..alert(3, 1_000)
        },
    );
    assert_eq!(pending_id(&controller, tab_id), None);
}

// Review round 4

#[test]
fn a_cross_origin_sign_in_does_not_end_a_marked_block() {
    // Sign-ins carry the URL that answered 401, which may be a frame's on another site.
    let (mut controller, tab_id) = blocked_page();
    controller.queue_command(BrowserCommand::GoBack);
    drain(&mut controller);
    receive(
        &mut controller,
        tab_id,
        sign_in(3, "https://frame.other.test/private", 1_000),
    );
    assert_eq!(pending_id(&controller, tab_id), Some(3));
    controller.answer_page_request(
        tab_id,
        PageRequestId::new(3),
        reply(PageRequestAnswer::Decline),
        Duration::from_millis(1_100),
    );
    drain(&mut controller);

    receive(&mut controller, tab_id, alert(4, 2_000));
    assert_eq!(pending_id(&controller, tab_id), None);
}

/// The order measured with `tests/pages/navigate-then-head-alert.html` (five runs, Servo 0.8):
/// the new address arrives before the new document's first dialog, and that dialog names the new
/// address. When the old page was stuck in `Started` (as after an alert loop in its head), the
/// document start is only inferred at `HeadParsed`, which comes after the alert is answered.
#[test]
fn a_new_sites_head_alert_after_a_stuck_page_is_shown_in_the_measured_order() {
    for document_start_reported_first in [false, true] {
        let (mut controller, tab_id) = blocked_page();
        controller
            .queue_navigation("https://other.test/")
            .expect("the address resolves");
        drain(&mut controller);
        // The leaving page's leave prompt is allowed without asking (it is blocked).
        receive(
            &mut controller,
            tab_id,
            request(3, PageRequestKind::LeavePage, 40),
        );
        drain(&mut controller);

        commit_page(&mut controller, tab_id, "https://other.test/");
        if document_start_reported_first {
            controller.handle_event(tab_id, BrowserEvent::PageLoadStarted);
        }
        receive(
            &mut controller,
            tab_id,
            PageRequest {
                url: "https://other.test/".to_owned(),
                ..alert(4, 1_000)
            },
        );
        assert_eq!(
            pending_id(&controller, tab_id),
            Some(4),
            "document start reported first: {document_start_reported_first}"
        );
    }
}
