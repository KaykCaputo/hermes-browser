use talaria_core::{
    BrowserCommand, BrowserController, BrowserEvent, ContextMenuAction, ControlAnswer, ControlId,
    ControlRect, ControlRequest, LoadState, Rgb, TabCommand, TabId,
};

const RECT: ControlRect = ControlRect {
    x: 10,
    y: 20,
    width: 120,
    height: 24,
};

fn colour_request() -> ControlRequest {
    ControlRequest::Color {
        rect: RECT,
        current: Some(Rgb {
            red: 1,
            green: 2,
            blue: 3,
        }),
    }
}

fn ready_controller() -> (BrowserController, TabId) {
    let mut controller = BrowserController::default();
    let tab_id = controller.active_tab_id();
    controller.handle_event(tab_id, BrowserEvent::AddressChanged { url: String::new() });
    (controller, tab_id)
}

fn queued(controller: &mut BrowserController) -> Vec<TabCommand> {
    std::iter::from_fn(|| controller.next_command()).collect()
}

fn answer(tab_id: TabId, id: u64, answer: ControlAnswer) -> TabCommand {
    TabCommand::new(
        tab_id,
        BrowserCommand::AnswerControl {
            id: ControlId::new(id),
            answer,
        },
    )
}

#[test]
fn a_control_for_the_active_tab_is_kept_until_the_engine_hides_it() {
    let (mut controller, tab_id) = ready_controller();

    controller.handle_event(
        tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(7),
            request: colour_request(),
        },
    );
    assert_eq!(
        controller.active_state().pending_control(),
        Some((ControlId::new(7), &colour_request()))
    );

    controller.handle_event(
        tab_id,
        BrowserEvent::ControlHidden {
            id: ControlId::new(7),
        },
    );
    assert_eq!(controller.active_state().pending_control(), None);
    assert!(queued(&mut controller).is_empty());
}

#[test]
fn hiding_another_control_leaves_the_pending_one() {
    let (mut controller, tab_id) = ready_controller();
    controller.handle_event(
        tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(7),
            request: colour_request(),
        },
    );

    controller.handle_event(
        tab_id,
        BrowserEvent::ControlHidden {
            id: ControlId::new(8),
        },
    );

    assert!(controller.active_state().pending_control().is_some());
}

#[test]
fn a_control_for_a_background_tab_is_dropped_at_once() {
    let (mut controller, background_tab_id) = ready_controller();
    controller.open_tab();

    controller.handle_event(
        background_tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(3),
            request: colour_request(),
        },
    );

    let background_state = controller
        .tabs()
        .find(|tab| tab.id() == background_tab_id)
        .map(|tab| tab.state().pending_control().is_some());
    assert_eq!(background_state, Some(false));
    assert_eq!(
        queued(&mut controller),
        vec![answer(background_tab_id, 3, ControlAnswer::Dismiss)]
    );
}

#[test]
fn a_new_control_replaces_and_drops_the_previous_one() {
    let (mut controller, tab_id) = ready_controller();
    controller.handle_event(
        tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(1),
            request: colour_request(),
        },
    );

    controller.handle_event(
        tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(2),
            request: colour_request(),
        },
    );

    assert_eq!(
        controller
            .active_state()
            .pending_control()
            .map(|(id, _)| id),
        Some(ControlId::new(2))
    );
    assert_eq!(
        queued(&mut controller),
        vec![answer(tab_id, 1, ControlAnswer::Dismiss)]
    );
}

#[test]
fn answering_sends_the_answer_once_and_closes_the_control() {
    let (mut controller, tab_id) = ready_controller();
    controller.handle_event(
        tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(4),
            request: colour_request(),
        },
    );
    let colour = Rgb {
        red: 9,
        green: 8,
        blue: 7,
    };

    assert!(controller.answer_control(ControlId::new(4), ControlAnswer::Color(colour)));
    assert!(!controller.answer_control(ControlId::new(4), ControlAnswer::Dismiss));

    assert_eq!(controller.active_state().pending_control(), None);
    assert_eq!(
        queued(&mut controller),
        vec![answer(tab_id, 4, ControlAnswer::Color(colour))]
    );
}

#[test]
fn an_answer_for_a_control_that_is_no_longer_pending_is_ignored() {
    let (mut controller, tab_id) = ready_controller();
    controller.handle_event(
        tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(5),
            request: colour_request(),
        },
    );

    assert!(!controller.answer_control(
        ControlId::new(6),
        ControlAnswer::ContextMenu(ContextMenuAction::Reload)
    ));
    assert!(queued(&mut controller).is_empty());
    assert!(controller.active_state().pending_control().is_some());
}

#[test]
fn leaving_a_tab_drops_its_pending_control() {
    let (mut controller, tab_id) = ready_controller();
    controller.handle_event(
        tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(5),
            request: colour_request(),
        },
    );
    controller.open_tab();

    controller.dismiss_control(tab_id);
    controller.dismiss_control(tab_id);

    assert_eq!(
        queued(&mut controller),
        vec![answer(tab_id, 5, ControlAnswer::Dismiss)]
    );
}

#[test]
fn a_crash_forgets_the_pending_control_and_the_input_method() {
    let (mut controller, tab_id) = ready_controller();
    controller.handle_event(
        tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(5),
            request: colour_request(),
        },
    );
    controller.handle_event(
        tab_id,
        BrowserEvent::InputMethodShown {
            id: ControlId::new(6),
            rect: RECT,
        },
    );

    controller.handle_event(
        tab_id,
        BrowserEvent::LoadStateChanged {
            state: LoadState::Failed {
                message: "crashed".to_owned(),
            },
        },
    );

    assert_eq!(controller.active_state().pending_control(), None);
    assert_eq!(controller.active_state().input_method(), None);
}

#[test]
fn the_input_method_area_follows_show_and_hide() {
    let (mut controller, tab_id) = ready_controller();

    controller.handle_event(
        tab_id,
        BrowserEvent::InputMethodShown {
            id: ControlId::new(6),
            rect: RECT,
        },
    );
    assert_eq!(controller.active_state().input_method(), Some(RECT));

    controller.handle_event(
        tab_id,
        BrowserEvent::ControlHidden {
            id: ControlId::new(6),
        },
    );
    assert_eq!(controller.active_state().input_method(), None);
    assert!(queued(&mut controller).is_empty());
}

#[test]
fn a_background_tab_keeps_its_input_method_area() {
    let (mut controller, background_tab_id) = ready_controller();
    controller.open_tab();

    controller.handle_event(
        background_tab_id,
        BrowserEvent::InputMethodShown {
            id: ControlId::new(6),
            rect: RECT,
        },
    );

    let area = controller
        .tabs()
        .find(|tab| tab.id() == background_tab_id)
        .and_then(|tab| tab.state().input_method());
    assert_eq!(area, Some(RECT));
    assert!(queued(&mut controller).is_empty());
}

#[test]
fn a_replaced_web_view_leaves_nothing_pending() {
    let (mut controller, tab_id) = ready_controller();
    controller.handle_event(
        tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(5),
            request: colour_request(),
        },
    );
    controller.handle_event(
        tab_id,
        BrowserEvent::InputMethodShown {
            id: ControlId::new(6),
            rect: RECT,
        },
    );
    assert!(
        controller
            .set_initial_address(tab_id, "https://a.test/")
            .expect("the address resolves")
    );

    assert!(controller.take_initial_url(tab_id).is_some());

    assert_eq!(controller.active_state().pending_control(), None);
    assert_eq!(controller.active_state().input_method(), None);
}

#[test]
fn a_request_while_a_file_chooser_is_up_is_dropped_instead_of_the_chooser() {
    let (mut controller, tab_id) = ready_controller();
    let file_request = ControlRequest::File {
        multiple: false,
        extensions: Vec::new(),
    };
    controller.handle_event(
        tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(1),
            request: file_request.clone(),
        },
    );

    controller.handle_event(
        tab_id,
        BrowserEvent::ControlShown {
            id: ControlId::new(2),
            request: file_request.clone(),
        },
    );

    assert_eq!(
        controller.active_state().pending_control(),
        Some((ControlId::new(1), &file_request))
    );
    assert_eq!(
        queued(&mut controller),
        vec![answer(tab_id, 2, ControlAnswer::Dismiss)]
    );
}
