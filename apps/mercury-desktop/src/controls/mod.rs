//! Popups for page controls: `<select>` lists, the colour picker, the file panel, the page
//! context menu, and placing the input-method window at the caret.
//!
//! The engine's requests reach the active tab's `BrowserState` (`pending_control`,
//! `input_method`); `sync` shows whatever the active tab is waiting for, and every answer goes
//! back through `BrowserController::answer_control`, which sends it at most once. Requests for
//! background tabs are dropped by the controller, and the engine drops a tab's pending request
//! when the tab stops being shown (`engine::queue_leaving_tab`).
//!
//! The page keeps keyboard focus while a popup is open; `handle_key` takes the keys from the
//! native keyboard hook instead, so the page's element is never blurred (Servo hides a control
//! whose element loses focus).

mod color;
mod context_menu;
mod file_panel;
mod geometry;
mod select;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mercury_ui::{ContextMenuRow, MainWindow, SelectPopupRow};
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::dpi::{LogicalPosition, LogicalSize};
use slint::winit_030::winit::event::{ElementState, KeyEvent};
use slint::winit_030::winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use talaria_core::{
    BrowserController, ControlAnswer, ControlId, ControlRect, ControlRequest, PointerButton,
    PointerButtonAction, Rgb, TabActivation, TabId,
};

use self::color::{ColorKey, ColorModel, SWATCHES, from_hue_lightness, hue_lightness};
use self::context_menu::{MenuCommand, MenuKey, MenuModel, MenuOutcome, MenuRow, build_menu};
use self::geometry::{Rect, element_in_window, place_at_point, place_beside};
use self::select::{SelectKey, SelectModel, SelectRow};
use crate::activation::consume_user_activation;
use crate::ui_sync::sync_ui;

/// What a key or click in a popup did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    Stay,
    Close(ControlAnswer),
}

/// Values of the `controls-popup` property of the main window.
const POPUP_NONE: i32 = 0;
const POPUP_SELECT: i32 = 1;
const POPUP_COLOR: i32 = 2;
const POPUP_MENU: i32 = 3;

/// Sizes in logical pixels, mirrored in `crates/mercury-ui/ui/controls/*.slint`.
const SELECT_CAPTION_HEIGHT: f32 = 20.0;
const SELECT_FOOTER_HEIGHT: f32 = 40.0;
const SELECT_MIN_WIDTH: f32 = 180.0;
const SELECT_MAX_WIDTH: f32 = 480.0;
const SELECT_MAX_HEIGHT: f32 = 360.0;
const COLOR_PICKER_WIDTH: f32 = 232.0;
const COLOR_PICKER_HEIGHT: f32 = 282.0;
const MENU_WIDTH: f32 = 220.0;
const MENU_PADDING: f32 = 8.0;

thread_local! {
    static CONTROLS: RefCell<Option<Rc<Controls>>> = const { RefCell::new(None) };
    static CONTROL_CLICK: Cell<ControlClick> = const { Cell::new(ControlClick::new()) };
}

pub(crate) fn connect_controls(window: &MainWindow, controller: Rc<RefCell<BrowserController>>) {
    let controls = Rc::new(Controls {
        window: window.as_weak(),
        controller,
        state: RefCell::new(State::default()),
    });
    CONTROLS.with(|current| *current.borrow_mut() = Some(controls));

    window.set_controls_color_swatches(ModelRc::new(VecModel::from(
        SWATCHES
            .iter()
            .copied()
            .map(slint_color)
            .collect::<Vec<_>>(),
    )));
    window.on_controls_dismissed(|| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        with_controls(|controls| controls.close(ControlAnswer::Dismiss));
    });
    window.on_controls_row_clicked(|row| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        if let Ok(row) = usize::try_from(row) {
            with_controls(|controls| controls.row_clicked(row));
        }
    });
    window.on_controls_row_hovered(|row| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        if let Ok(row) = usize::try_from(row) {
            with_controls(|controls| controls.row_hovered(row));
        }
    });
    window.on_controls_select_done(|| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        with_controls(Controls::select_done);
    });
    window.on_controls_color_swatch_picked(|index| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let swatch = usize::try_from(index)
            .ok()
            .and_then(|index| SWATCHES.get(index));
        if let Some(swatch) = swatch {
            with_controls(|controls| controls.colour_picked(*swatch));
        }
    });
    window.on_controls_color_area_picked(|hue, lightness| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        with_controls(|controls| controls.colour_picked(from_hue_lightness(hue, 1.0 - lightness)));
    });
    window.on_controls_color_done(|| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        with_controls(Controls::colour_done);
    });
    // The page area moved or changed size: popups and the input-method area follow. Deferred,
    // because Slint reports the change while it lays out, possibly inside a state update.
    window.on_controls_page_changed(|| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        slint::Timer::single_shot(Duration::ZERO, || {
            let _slint = crate::callback_scope::InSlintCallback::enter();
            with_controls(Controls::relayout);
        });
    });
}

/// Shows what the active tab is waiting for and places the input-method window. Called from
/// `sync_ui` after every change of the browser state.
pub(crate) fn sync(window: &MainWindow, controller: &BrowserController) {
    with_controls(|controls| controls.sync(window, controller));
}

/// Takes a key press while a popup is open, and the release of every press it took; returns
/// true if the event was taken. Releases of keys pressed before the popup opened go on to
/// whoever had the press.
pub(crate) fn handle_key(window: &MainWindow, event: &KeyEvent, modifiers: ModifiersState) -> bool {
    let Some(controls) = CONTROLS.with(|current| current.borrow().clone()) else {
        return false;
    };
    if event.state == ElementState::Released {
        return controls
            .state
            .borrow_mut()
            .taken_keys
            .release(event.physical_key);
    }
    if window.get_text_input_focused() || modifiers.super_key() || modifiers.control_key() {
        return false;
    }
    // A page dialog's sheet wins: its keys are never taken here (`sync` also closes the popup).
    let dialog_sheet = controls.controller.try_borrow().map_or(true, |controller| {
        crate::page_requests::sheet_shown(&controller)
    });
    if dialog_sheet {
        return false;
    }
    controls.key(event)
}

/// The physical Control key went up or down (`ModifiersChanged`).
pub(crate) fn set_control_key(held: bool) {
    CONTROL_CLICK.with(|click| {
        let mut state = click.get();
        state.control_held = held;
        click.set(state);
    });
}

/// On macOS a Control-click is a secondary click; Servo only opens its context menu for the
/// secondary button, so the translation happens here, for both press and release.
pub(crate) fn pointer_button(button: PointerButton, action: PointerButtonAction) -> PointerButton {
    CONTROL_CLICK.with(|click| {
        let mut state = click.get();
        let button = state.button(button, action, cfg!(target_os = "macos"));
        click.set(state);
        button
    })
}

fn with_controls(action: impl FnOnce(&Controls)) {
    let controls = CONTROLS.with(|current| current.borrow().clone());
    if let Some(controls) = controls {
        action(&controls);
    }
}

struct Controls {
    window: slint::Weak<MainWindow>,
    controller: Rc<RefCell<BrowserController>>,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    /// The request currently shown, so each is opened once.
    shown: Option<(TabId, ControlId)>,
    popup: Popup,
    /// The input-method area last given to the window.
    input_method_area: Option<Option<Rect>>,
    taken_keys: TakenKeys,
}

#[derive(Default)]
enum Popup {
    #[default]
    None,
    Select {
        model: SelectModel,
        rows: Rc<VecModel<SelectPopupRow>>,
        anchor: ControlRect,
    },
    Color {
        model: ColorModel,
        anchor: ControlRect,
    },
    Menu {
        model: MenuModel,
        anchor: ControlRect,
    },
    /// The native file panel is open, or about to be; the sheet is filled in once it is.
    File {
        sheet: Rc<RefCell<Option<file_panel::FileSheet>>>,
    },
}

impl Popup {
    /// Takes the popup down when its request goes away (answered elsewhere, hidden by the
    /// engine, navigation, tab switch or close, crash): only the file sheet needs closing.
    fn close_native(&self) {
        if let Popup::File { sheet } = self
            && let Some(sheet) = sheet.borrow_mut().take()
        {
            sheet.end();
        }
    }
}

impl Controls {
    fn sync(&self, window: &MainWindow, controller: &BrowserController) {
        let tab_id = controller.active_tab_id();
        let state = controller.active_state();
        let pending = state.pending_control();
        let key = pending.map(|(id, _)| (tab_id, id));
        let page = page_rect(window);
        let zoom = state.zoom_percent();

        // A page dialog's sheet takes precedence over these popups; they do not stay open under it.
        let dialog_sheet = crate::page_requests::sheet_shown(controller);
        let mut current = self.state.borrow_mut();
        if dialog_sheet
            && current.shown == key
            && let Some((tab_id, id)) = key
            && matches!(
                current.popup,
                Popup::Select { .. } | Popup::Color { .. } | Popup::Menu { .. }
            )
        {
            log::debug!(
                "tab {}: a page dialog closes control {}",
                tab_id.value(),
                id.value()
            );
            current.popup = Popup::None;
            slint::Timer::single_shot(Duration::ZERO, move || {
                let _slint = crate::callback_scope::InSlintCallback::enter();
                with_controls(|controls| controls.reject(tab_id, id));
            });
        }
        if current.shown != key {
            current.shown = key;
            if let Some((id, request)) = pending {
                log::debug!(
                    "tab {}: showing control {}: {request:?}",
                    tab_id.value(),
                    id.value()
                );
            }
            current.popup.close_native();
            current.popup = match pending {
                None => Popup::None,
                Some((id, request))
                    if dialog_sheet
                        || !admit(
                            request,
                            tab_id,
                            Instant::now(),
                            file_panel::sheet_attached(),
                        ) =>
                {
                    log::debug!(
                        "tab {}: dropping control {}: no recent user gesture, or a sheet is up",
                        tab_id.value(),
                        id.value()
                    );
                    // Answered outside this state update.
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        let _slint = crate::callback_scope::InSlintCallback::enter();
                        with_controls(|controls| controls.reject(tab_id, id));
                    });
                    Popup::None
                }
                Some((id, request)) => {
                    let origin = page_origin(state.url());
                    window.set_controls_origin(caption(&origin).into());
                    open_popup(window, request, tab_id, id, state.url(), &origin)
                }
            };
            show_popup(window, &current.popup, page, zoom, true);
        } else {
            // The window or the page may have moved or been zoomed.
            show_popup(window, &current.popup, page, zoom, false);
        }

        // Slint places the input method itself while one of its text fields has focus.
        if window.get_text_input_focused() {
            current.input_method_area = None;
        } else {
            let area = state.input_method().map(|rect| {
                log::debug!("input-method element (from Servo): {rect:?}");
                element_in_window(rect, page, zoom)
            });
            if current.input_method_area != Some(area) {
                current.input_method_area = Some(area);
                log::debug!("input-method area: {area:?}");
                set_input_method_area(window, area);
            }
        }
    }

    fn key(&self, event: &KeyEvent) -> bool {
        let Some(window) = self.window.upgrade() else {
            return false;
        };
        let mut state = self.state.borrow_mut();
        if matches!(state.popup, Popup::None | Popup::File { .. }) {
            return false;
        }
        state.taken_keys.take(event.physical_key);
        let mut closing = None;
        match &mut state.popup {
            Popup::Select { model, rows, .. } => {
                if let Some(key) = select_key(&event.logical_key) {
                    let toggled = model.highlighted().filter(|_| key == SelectKey::Space);
                    match model.key(key, Instant::now()) {
                        Outcome::Close(answer) => closing = Some((answer, None)),
                        Outcome::Stay => {
                            refresh_select(&window, model, rows, toggled);
                            reveal_highlighted(&window, model);
                        }
                    }
                }
            }
            Popup::Color { model, .. } => {
                if let Some(key) = color_key(&event.logical_key) {
                    match model.key(key, Instant::now()) {
                        Outcome::Close(answer) => closing = Some((answer, None)),
                        Outcome::Stay => refresh_color(&window, model),
                    }
                }
            }
            Popup::Menu { model, .. } => {
                if let Some(key) = menu_key(&event.logical_key) {
                    match model.key(key) {
                        MenuOutcome::Close(command) => closing = Some(menu_answer(command)),
                        MenuOutcome::Stay => {
                            window.set_controls_highlighted(highlighted(model.highlighted()))
                        }
                    }
                }
            }
            Popup::None | Popup::File { .. } => {}
        }
        drop(state);
        if let Some((answer, new_tab)) = closing {
            self.finish(answer, new_tab);
        }
        true
    }

    fn row_clicked(&self, row: usize) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let mut state = self.state.borrow_mut();
        let closing = match &mut state.popup {
            Popup::Select { model, rows, .. } => match model.click(row) {
                Outcome::Close(answer) => Some((answer, None)),
                Outcome::Stay => {
                    refresh_select(&window, model, rows, Some(row));
                    None
                }
            },
            Popup::Menu { model, .. } => match model.click(row) {
                MenuOutcome::Close(command) => Some(menu_answer(command)),
                MenuOutcome::Stay => None,
            },
            _ => None,
        };
        drop(state);
        if let Some((answer, new_tab)) = closing {
            self.finish(answer, new_tab);
        }
    }

    fn row_hovered(&self, row: usize) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let mut state = self.state.borrow_mut();
        let highlighted_row = match &mut state.popup {
            Popup::Select { model, .. } => {
                model.hover(row);
                model.highlighted()
            }
            Popup::Menu { model, .. } => {
                model.hover(row);
                model.highlighted()
            }
            _ => return,
        };
        let value = highlighted(highlighted_row);
        if window.get_controls_highlighted() != value {
            window.set_controls_highlighted(value);
        }
    }

    fn select_done(&self) {
        let answer = match &self.state.borrow().popup {
            Popup::Select { model, .. } => match model.done() {
                Outcome::Close(answer) => answer,
                Outcome::Stay => return,
            },
            _ => return,
        };
        self.finish(answer, None);
    }

    fn colour_picked(&self, colour: Rgb) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        if let Popup::Color { model, .. } = &mut self.state.borrow_mut().popup
            && model.pick(colour, Instant::now())
        {
            refresh_color(&window, model);
        }
    }

    fn colour_done(&self) {
        let answer = match &self.state.borrow().popup {
            Popup::Color { model, .. } => match model.done(Instant::now()) {
                Outcome::Close(answer) => answer,
                Outcome::Stay => return,
            },
            _ => return,
        };
        self.finish(answer, None);
    }

    fn files_chosen(&self, tab_id: TabId, id: ControlId, paths: Vec<PathBuf>) {
        let still_shown = {
            let state = self.state.borrow();
            let Popup::File { sheet } = &state.popup else {
                return;
            };
            // The sheet has closed: nothing is left to end.
            sheet.borrow_mut().take();
            state.shown == Some((tab_id, id))
        };
        if still_shown {
            self.finish(ControlAnswer::Files(paths), None);
        }
    }

    /// Opens the file sheet for a request that is still the one shown.
    fn start_file_panel(
        &self,
        tab_id: TabId,
        id: ControlId,
        multiple: bool,
        extensions: &[String],
    ) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let sheet_slot = match &self.state.borrow().popup {
            Popup::File { sheet } if self.state.borrow().shown == Some((tab_id, id)) => {
                Rc::clone(sheet)
            }
            _ => return,
        };
        if file_panel::sheet_attached() {
            log::debug!(
                "a file sheet is already up; dropping control {}",
                id.value()
            );
            self.reject(tab_id, id);
            return;
        }
        let origin = page_origin(self.controller.borrow().active_state().url());
        let sheet = file_panel::choose_files(
            &window,
            multiple,
            extensions,
            &origin,
            Box::new(move |paths| {
                slint::Timer::single_shot(Duration::ZERO, move || {
                    let _slint = crate::callback_scope::InSlintCallback::enter();
                    with_controls(|controls| controls.files_chosen(tab_id, id, paths));
                });
            }),
        );
        *sheet_slot.borrow_mut() = sheet;
    }

    /// Drops a request that may not be shown (no recent gesture, or a sheet already up).
    fn reject(&self, tab_id: TabId, id: ControlId) {
        let still_shown = self.state.borrow().shown == Some((tab_id, id));
        if still_shown {
            self.finish(ControlAnswer::Dismiss, None);
        }
    }

    /// Places popups and the input-method area again after the page area changed.
    fn relayout(&self) {
        let (Some(window), Ok(controller)) = (self.window.upgrade(), self.controller.try_borrow())
        else {
            return;
        };
        let idle = self.state.try_borrow_mut().is_ok();
        if idle {
            self.sync(&window, &controller);
        }
    }

    fn close(&self, answer: ControlAnswer) {
        if !matches!(self.state.borrow().popup, Popup::None) {
            self.finish(answer, None);
        }
    }

    /// Sends the answer for the shown request and closes its popup; with `new_tab`, also opens
    /// that address in a background tab.
    fn finish(&self, answer: ControlAnswer, new_tab: Option<String>) {
        let Some((_, id)) = self.state.borrow().shown else {
            return;
        };
        let popup = std::mem::take(&mut self.state.borrow_mut().popup);
        popup.close_native();
        let Some(window) = self.window.upgrade() else {
            return;
        };
        window.set_controls_popup(POPUP_NONE);
        {
            let mut controller = self.controller.borrow_mut();
            if matches!(answer, ControlAnswer::ContextMenu(_)) {
                // tabs lane: mark user activation for this tab
            }
            controller.answer_control(id, answer);
            if let Some(address) = new_tab
                && let Err(error) =
                    controller.open_tab_with(Some(&address), TabActivation::Background)
            {
                log::warn!("could not open the link in a new tab: {error}");
            }
        }
        sync_ui(&window, &self.controller.borrow());
        window.window().request_redraw();
    }
}

fn open_popup(
    window: &MainWindow,
    request: &ControlRequest,
    tab_id: TabId,
    id: ControlId,
    page_url: &str,
    _origin: &str,
) -> Popup {
    match request {
        ControlRequest::Select {
            rect,
            entries,
            selected,
            multiple,
        } => {
            let model = SelectModel::new(entries, selected, *multiple);
            let rows = Rc::new(VecModel::from(
                (0..model.rows().len())
                    .map(|row| select_popup_row(&model, row))
                    .collect::<Vec<_>>(),
            ));
            window.set_controls_select_rows(ModelRc::from(rows.clone()));
            window.set_controls_select_multiple(*multiple);
            window.set_controls_highlighted(highlighted(model.highlighted()));
            window.set_controls_select_scroll_y(0.0);
            Popup::Select {
                model,
                rows,
                anchor: *rect,
            }
        }
        ControlRequest::Color { rect, current } => {
            let model = ColorModel::new(*current, Instant::now());
            refresh_color(window, &model);
            Popup::Color {
                model,
                anchor: *rect,
            }
        }
        ControlRequest::ContextMenu {
            rect,
            entries,
            link_url,
            image_url,
        } => {
            let model = MenuModel::new(build_menu(
                entries,
                link_url.as_deref(),
                image_url.as_deref(),
                page_url,
            ));
            window.set_controls_menu_rows(ModelRc::new(VecModel::from(
                model
                    .rows()
                    .iter()
                    .map(|row| match row {
                        MenuRow::Item { label, enabled, .. } => ContextMenuRow {
                            label: label.into(),
                            enabled: *enabled,
                            separator: false,
                        },
                        MenuRow::Separator => ContextMenuRow {
                            label: Default::default(),
                            enabled: false,
                            separator: true,
                        },
                    })
                    .collect::<Vec<_>>(),
            )));
            window.set_controls_highlighted(-1);
            Popup::Menu {
                model,
                anchor: *rect,
            }
        }
        ControlRequest::File {
            multiple,
            extensions,
        } => {
            let multiple = *multiple;
            let extensions = extensions.clone();
            // Started outside the current state update; AppKit calls back later, on the main
            // thread, once the user has chosen or cancelled.
            slint::Timer::single_shot(Duration::ZERO, move || {
                let _slint = crate::callback_scope::InSlintCallback::enter();
                with_controls(|controls| {
                    controls.start_file_panel(tab_id, id, multiple, &extensions);
                });
            });
            Popup::File {
                sheet: Rc::default(),
            }
        }
    }
}

/// Places the open popup over the page; `opened` also scrolls a select list to its highlight.
fn show_popup(window: &MainWindow, popup: &Popup, page: Rect, zoom: u16, opened: bool) {
    let (kind, placed) = match popup {
        Popup::None | Popup::File { .. } => (POPUP_NONE, None),
        Popup::Select { model, anchor, .. } => {
            let element = element_in_window(*anchor, page, zoom);
            let footer = if model.multiple() {
                SELECT_FOOTER_HEIGHT
            } else {
                0.0
            };
            let width = element.width.clamp(SELECT_MIN_WIDTH, SELECT_MAX_WIDTH);
            let height =
                (SELECT_CAPTION_HEIGHT + model.content_height() + footer).min(SELECT_MAX_HEIGHT);
            (
                POPUP_SELECT,
                Some(place_beside(element, width, height, page)),
            )
        }
        Popup::Color { anchor, .. } => {
            let element = element_in_window(*anchor, page, zoom);
            (
                POPUP_COLOR,
                Some(place_beside(
                    element,
                    COLOR_PICKER_WIDTH,
                    COLOR_PICKER_HEIGHT,
                    page,
                )),
            )
        }
        Popup::Menu { model, anchor } => {
            let point = element_in_window(*anchor, page, zoom);
            (
                POPUP_MENU,
                Some(place_at_point(
                    point.x,
                    point.y,
                    MENU_WIDTH,
                    model.content_height() + MENU_PADDING,
                    page,
                )),
            )
        }
    };
    if let Some(placed) = placed {
        window.set_controls_popup_x(placed.x);
        window.set_controls_popup_y(placed.y);
        window.set_controls_popup_width(placed.width);
        window.set_controls_popup_height(placed.height);
    }
    if window.get_controls_popup() != kind {
        window.set_controls_popup(kind);
    }
    if opened && let Popup::Select { model, .. } = popup {
        reveal_highlighted(window, model);
    }
}

fn select_popup_row(model: &SelectModel, row: usize) -> SelectPopupRow {
    match &model.rows()[row] {
        SelectRow::Heading(label) => SelectPopupRow {
            label: label.into(),
            heading: true,
            disabled: false,
            selected: false,
            in_group: false,
        },
        SelectRow::Option {
            label,
            disabled,
            in_group,
            ..
        } => SelectPopupRow {
            label: label.into(),
            heading: false,
            disabled: *disabled,
            selected: model.is_selected(row),
            in_group: *in_group,
        },
    }
}

/// Updates the one row whose selection may have changed and the highlight; the list itself is
/// never rebuilt, so this costs the same for any number of options.
fn refresh_select(
    window: &MainWindow,
    model: &SelectModel,
    rows: &VecModel<SelectPopupRow>,
    changed_row: Option<usize>,
) {
    if let Some(row) = changed_row.filter(|row| *row < rows.row_count()) {
        rows.set_row_data(row, select_popup_row(model, row));
    }
    window.set_controls_highlighted(highlighted(model.highlighted()));
}

fn reveal_highlighted(window: &MainWindow, model: &SelectModel) {
    let Some(row) = model.highlighted() else {
        return;
    };
    let footer = if model.multiple() {
        SELECT_FOOTER_HEIGHT
    } else {
        0.0
    };
    let viewport = window.get_controls_popup_height() - SELECT_CAPTION_HEIGHT - footer;
    let offset = -window.get_controls_select_scroll_y();
    let revealed = geometry::reveal(model.row_top(row), model.row_height(row), viewport, offset);
    if revealed != offset {
        window.set_controls_select_scroll_y(-revealed);
    }
}

fn refresh_color(window: &MainWindow, model: &ColorModel) {
    let colour = model.colour();
    let (hue, lightness) = hue_lightness(colour);
    window.set_controls_color(slint_color(colour));
    window.set_controls_color_hex(model.hex().into());
    window.set_controls_color_hex_valid(model.hex_is_valid());
    window.set_controls_color_hue(hue);
    window.set_controls_color_lightness(lightness);
}

fn menu_answer(command: Option<MenuCommand>) -> (ControlAnswer, Option<String>) {
    match command {
        Some(MenuCommand::Engine(action)) => (ControlAnswer::ContextMenu(action), None),
        Some(MenuCommand::OpenInNewTab(address)) => (ControlAnswer::Dismiss, Some(address)),
        None => (ControlAnswer::Dismiss, None),
    }
}

fn set_input_method_area(window: &MainWindow, area: Option<Rect>) {
    // Without an editable element the area goes back to winit's initial one.
    let area = area.unwrap_or(Rect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    });
    let _applied = window.window().with_winit_window(|winit_window| {
        winit_window.set_ime_cursor_area(
            LogicalPosition::new(area.x, area.y),
            LogicalSize::new(area.width, area.height),
        );
    });
}

fn page_rect(window: &MainWindow) -> Rect {
    Rect {
        x: window.get_controls_page_x(),
        y: window.get_controls_page_y(),
        width: window.get_web_view_width(),
        height: window.get_web_view_height(),
    }
}

fn highlighted(row: Option<usize>) -> i32 {
    row.and_then(|row| i32::try_from(row).ok()).unwrap_or(-1)
}

fn slint_color(colour: Rgb) -> slint::Color {
    slint::Color::from_rgb_u8(colour.red, colour.green, colour.blue)
}

/// The caption of a select list or colour picker. Servo does not say which frame asked, so it
/// names the tab's page without claiming that page asked.
fn caption(origin: &str) -> String {
    format!("A page on {origin}")
}

/// Whether a request may be shown now. A file panel or colour picker needs a gesture in the
/// tab within the last second (consumed here: one gesture, one picker), because Servo 0.8.0 opens
/// both from a script's `click()` without one (`show_the_picker_if_applicable` skips the
/// activation check); and no second file sheet while one is attached to the window. A select
/// list and the context menu only come from the user's own input (Servo checks `isTrusted` for
/// `<select>`; the context menu follows a real secondary press).
fn admit(request: &ControlRequest, tab_id: TabId, now: Instant, sheet_attached: bool) -> bool {
    match request {
        ControlRequest::File { .. } if sheet_attached => false,
        ControlRequest::File { .. } | ControlRequest::Color { .. } => {
            consume_user_activation(tab_id, now)
        }
        ControlRequest::Select { .. } | ControlRequest::ContextMenu { .. } => true,
    }
}

/// The keys whose press a popup took, so their releases are taken too and no other release is.
#[derive(Debug, Default)]
struct TakenKeys(Vec<PhysicalKey>);

impl TakenKeys {
    fn take(&mut self, key: PhysicalKey) {
        if !self.0.contains(&key) {
            self.0.push(key);
        }
    }

    /// True if the press of `key` was taken; that release is taken then too.
    fn release(&mut self, key: PhysicalKey) -> bool {
        let Some(index) = self.0.iter().position(|taken| *taken == key) else {
            return false;
        };
        self.0.remove(index);
        true
    }
}

/// `scheme://host[:port]` of the page's address, without any user name or password.
fn page_origin(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.split(':').next().unwrap_or_default().to_owned();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    format!("{scheme}://{host}")
}

fn select_key(key: &Key) -> Option<SelectKey> {
    Some(match key {
        Key::Named(NamedKey::ArrowUp) => SelectKey::Up,
        Key::Named(NamedKey::ArrowDown) => SelectKey::Down,
        Key::Named(NamedKey::Home | NamedKey::PageUp) => SelectKey::Home,
        Key::Named(NamedKey::End | NamedKey::PageDown) => SelectKey::End,
        Key::Named(NamedKey::Enter) => SelectKey::Return,
        Key::Named(NamedKey::Escape) => SelectKey::Escape,
        Key::Named(NamedKey::Space) => SelectKey::Space,
        Key::Character(text) => SelectKey::Character(text.chars().next()?),
        _ => return None,
    })
}

fn color_key(key: &Key) -> Option<ColorKey> {
    Some(match key {
        Key::Named(NamedKey::Enter) => ColorKey::Return,
        Key::Named(NamedKey::Escape) => ColorKey::Escape,
        Key::Named(NamedKey::Backspace) => ColorKey::Backspace,
        Key::Character(text) => ColorKey::Character(text.chars().next()?),
        _ => return None,
    })
}

fn menu_key(key: &Key) -> Option<MenuKey> {
    Some(match key {
        Key::Named(NamedKey::ArrowUp) => MenuKey::Up,
        Key::Named(NamedKey::ArrowDown) => MenuKey::Down,
        Key::Named(NamedKey::Home) => MenuKey::Home,
        Key::Named(NamedKey::End) => MenuKey::End,
        Key::Named(NamedKey::Enter | NamedKey::Space) => MenuKey::Return,
        Key::Named(NamedKey::Escape) => MenuKey::Escape,
        _ => return None,
    })
}

/// Turns a macOS Control-click into a secondary click, press and release alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ControlClick {
    control_held: bool,
    /// A Control-click was pressed and its release must be a secondary release too.
    translating: bool,
}

impl ControlClick {
    const fn new() -> Self {
        Self {
            control_held: false,
            translating: false,
        }
    }

    fn button(
        &mut self,
        button: PointerButton,
        action: PointerButtonAction,
        on_macos: bool,
    ) -> PointerButton {
        if !on_macos || button != PointerButton::Left {
            return button;
        }
        match action {
            PointerButtonAction::Down => {
                self.translating = self.control_held;
                if self.translating {
                    PointerButton::Right
                } else {
                    button
                }
            }
            PointerButtonAction::Up => {
                if std::mem::take(&mut self.translating) {
                    PointerButton::Right
                } else {
                    button
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use slint::winit_030::winit::keyboard::KeyCode;

    use crate::activation::note_user_activation;

    fn file_request() -> ControlRequest {
        ControlRequest::File {
            multiple: false,
            extensions: Vec::new(),
        }
    }

    fn colour_request() -> ControlRequest {
        ControlRequest::Color {
            rect: ControlRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            current: None,
        }
    }

    #[test]
    fn a_picker_without_a_gesture_is_dropped() {
        let tab = TabId::new(41);
        let now = Instant::now();

        assert!(!admit(&file_request(), tab, now, false));
        assert!(!admit(&colour_request(), tab, now, false));
    }

    #[test]
    fn one_gesture_opens_one_picker() {
        let tab = TabId::new(42);
        let now = Instant::now();
        note_user_activation(tab, now);

        assert!(admit(
            &file_request(),
            tab,
            now + Duration::from_millis(200),
            false
        ));
        assert!(!admit(
            &file_request(),
            tab,
            now + Duration::from_millis(300),
            false
        ));
        note_user_activation(tab, now);
        assert!(admit(
            &colour_request(),
            tab,
            now + Duration::from_millis(200),
            false
        ));
        assert!(!admit(
            &colour_request(),
            tab,
            now + Duration::from_millis(300),
            false
        ));
    }

    #[test]
    fn a_stale_gesture_opens_nothing() {
        let tab = TabId::new(43);
        let now = Instant::now();
        note_user_activation(tab, now);

        assert!(!admit(
            &file_request(),
            tab,
            now + Duration::from_millis(1500),
            false
        ));
    }

    #[test]
    fn a_gesture_in_another_tab_does_not_count() {
        let now = Instant::now();
        note_user_activation(TabId::new(44), now);

        assert!(!admit(&file_request(), TabId::new(45), now, false));
    }

    #[test]
    fn no_second_file_sheet_while_one_is_attached() {
        let tab = TabId::new(46);
        let now = Instant::now();
        note_user_activation(tab, now);

        assert!(!admit(&file_request(), tab, now, true));
    }

    #[test]
    fn select_lists_and_menus_need_no_gesture() {
        let tab = TabId::new(47);
        let select = ControlRequest::Select {
            rect: ControlRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            entries: Vec::new(),
            selected: Vec::new(),
            multiple: false,
        };

        assert!(admit(&select, tab, Instant::now(), true));
    }

    #[test]
    fn only_releases_of_taken_presses_are_taken() {
        let mut taken = TakenKeys::default();
        let enter = PhysicalKey::Code(KeyCode::Enter);
        let shift = PhysicalKey::Code(KeyCode::ShiftLeft);

        assert!(!taken.release(shift), "pressed before the popup opened");
        taken.take(enter);
        taken.take(enter);
        assert!(taken.release(enter));
        assert!(!taken.release(enter));
    }

    #[test]
    fn captions_name_the_page_without_claiming_it_asked() {
        assert_eq!(
            caption("https://shop.example"),
            "A page on https://shop.example"
        );
    }

    #[test]
    fn a_control_click_is_a_secondary_click_on_macos() {
        let mut click = ControlClick::new();
        click.control_held = true;

        assert_eq!(
            click.button(PointerButton::Left, PointerButtonAction::Down, true),
            PointerButton::Right
        );
        click.control_held = false;
        assert_eq!(
            click.button(PointerButton::Left, PointerButtonAction::Up, true),
            PointerButton::Right,
            "the release matches the press even after Control was let go"
        );
        assert_eq!(
            click.button(PointerButton::Left, PointerButtonAction::Down, true),
            PointerButton::Left
        );
        assert_eq!(
            click.button(PointerButton::Left, PointerButtonAction::Up, true),
            PointerButton::Left
        );
    }

    #[test]
    fn a_control_click_stays_a_primary_click_elsewhere() {
        let mut click = ControlClick::new();
        click.control_held = true;

        assert_eq!(
            click.button(PointerButton::Left, PointerButtonAction::Down, false),
            PointerButton::Left
        );
        assert_eq!(
            click.button(PointerButton::Right, PointerButtonAction::Down, true),
            PointerButton::Right
        );
    }

    #[test]
    fn a_release_after_control_was_pressed_mid_click_stays_primary() {
        let mut click = ControlClick::new();

        click.button(PointerButton::Left, PointerButtonAction::Down, true);
        click.control_held = true;

        assert_eq!(
            click.button(PointerButton::Left, PointerButtonAction::Up, true),
            PointerButton::Left
        );
    }

    #[test]
    fn the_origin_leaves_out_the_path_and_any_credentials() {
        assert_eq!(
            page_origin("https://user:secret@shop.example:8443/cart?x=1#top"),
            "https://shop.example:8443"
        );
        assert_eq!(page_origin("https://a.test"), "https://a.test");
        assert_eq!(page_origin("file:///Users/me/page.html"), "file://");
        assert_eq!(page_origin("about:blank"), "about");
    }

    #[test]
    fn keys_map_to_popup_keys() {
        assert_eq!(
            select_key(&Key::Named(NamedKey::ArrowDown)),
            Some(SelectKey::Down)
        );
        assert_eq!(
            select_key(&Key::Character("é".into())),
            Some(SelectKey::Character('é'))
        );
        assert_eq!(select_key(&Key::Named(NamedKey::Tab)), None);
        assert_eq!(
            color_key(&Key::Named(NamedKey::Backspace)),
            Some(ColorKey::Backspace)
        );
        assert_eq!(
            menu_key(&Key::Named(NamedKey::Escape)),
            Some(MenuKey::Escape)
        );
    }

    #[test]
    fn menu_commands_become_engine_answers_or_new_tabs() {
        assert_eq!(
            menu_answer(Some(MenuCommand::OpenInNewTab(
                "https://a.test/".to_owned()
            ))),
            (ControlAnswer::Dismiss, Some("https://a.test/".to_owned()))
        );
        assert_eq!(
            menu_answer(Some(MenuCommand::Engine(
                talaria_core::ContextMenuAction::CopyLink
            ))),
            (
                ControlAnswer::ContextMenu(talaria_core::ContextMenuAction::CopyLink),
                None
            )
        );
        assert_eq!(menu_answer(None), (ControlAnswer::Dismiss, None));
    }
}
