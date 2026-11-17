//! The macOS menu bar, the browser menu behind the toolbar's "⋮" button, and the routing from
//! both to the existing actions.
//!
//! Keys: `shortcuts.rs` stays the only owner of keyboard shortcuts. The native menu shows their
//! key equivalents but declines them while the browser window has keyboard focus
//! (`model::window_owns_key`), so they keep arriving through winit with their press/release
//! pairing and repeat rules. The few keys only the menu handles (Cmd+H, Cmd+M, Ctrl+Cmd+F, …)
//! never reach the window as a press; the menu records them and `observe_window_event` hands
//! them to `ShortcutKeys::claim`, so their release does not reach the page either.
//!
//! Clicks: AppKit calls the menu's target while it is processing an event, so nothing there
//! touches Slint or the controller. The command is posted with `slint::invoke_from_event_loop`
//! and runs from the event loop like any other callback.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mercury_ui::{AddressBarEdit, BrowserMenuCommand, BrowserMenuEntry, MainWindow};
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::winit::keyboard::PhysicalKey;
use slint::{ComponentHandle, ModelRc, VecModel};
use talaria_core::{BrowserCommand, BrowserController, ClipboardCommand, TabActivation};

use crate::about;
use crate::shortcuts::{Platform, ShortcutKeys, run_shortcut};
use crate::ui_sync::sync_ui;

#[cfg(target_os = "macos")]
mod macos;
mod model;
mod seams;

#[cfg(target_os = "macos")]
pub(crate) use macos::reinstall_when_key;
use model::{BrowserMenuItem, EditCommand, EditTarget, MenuSnapshot};
pub(crate) use model::{Document, MenuCommand};

type Handler = Rc<dyn Fn(MenuCommand)>;

#[derive(Default)]
struct MenuState {
    snapshot: RefCell<MenuSnapshot>,
    browser_window_focused: Cell<bool>,
    address_bar_focused: Cell<bool>,
    web_view_focused: RefCell<Option<Rc<Cell<bool>>>>,
    /// Keys whose press the native menu performed; their releases must not reach the page.
    claimed_keys: RefCell<Vec<PhysicalKey>>,
    handler: RefCell<Option<Handler>>,
}

thread_local! {
    static STATE: MenuState = MenuState::default();
}

/// Builds the menus and routes them to `window`. macOS: installs the native menu bar.
pub(crate) fn connect(
    window: &MainWindow,
    controller: Rc<RefCell<BrowserController>>,
    web_view_focused: Rc<Cell<bool>>,
) {
    let weak_window = window.as_weak();
    let handler_web_view_focused = Rc::clone(&web_view_focused);
    let handler: Handler = Rc::new(move |command| {
        if let Some(window) = weak_window.upgrade() {
            run_command(&window, &controller, &handler_web_view_focused, command);
        }
    });
    STATE.with(|state| {
        state.handler.replace(Some(handler));
        state.web_view_focused.replace(Some(web_view_focused));
    });

    let (top, bottom) = model::browser_menu(Platform::current());
    window.set_browser_menu_top_entries(browser_menu_model(top));
    window.set_browser_menu_bottom_entries(browser_menu_model(bottom));
    window.on_browser_menu_activated(|command| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        dispatch(browser_menu_item(command).command());
    });
    window.on_address_bar_focus_changed(|focused| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        STATE.with(|state| state.address_bar_focused.set(focused));
    });

    #[cfg(target_os = "macos")]
    macos::install();
}

/// Drops the menus' references to the window and the controller, and the About window, before
/// the engine shuts down.
pub(crate) fn release() {
    STATE.with(|state| {
        state.handler.take();
        state.web_view_focused.take();
    });
    #[cfg(target_os = "macos")]
    macos::release();
    about::release();
}

/// Copies what the menus show from the controller (called whenever the UI syncs).
pub(crate) fn sync(controller: &BrowserController) {
    let state = controller.active_state();
    let active_tab_id = controller.active_tab_id();
    let snapshot = MenuSnapshot {
        can_go_back: state.can_go_back(),
        can_go_forward: state.can_go_forward(),
        tab_titles: controller
            .tabs()
            .map(|tab| tab.state().page_title().to_owned())
            .collect(),
        active_tab_index: controller
            .tabs()
            .position(|tab| tab.id() == active_tab_id)
            .unwrap_or(0),
        bookmarks_bar: seams::bookmarks_bar_items(controller),
    };
    STATE.with(|menu_state| {
        if *menu_state.snapshot.borrow() != snapshot {
            menu_state.snapshot.replace(snapshot);
        }
    });
}

/// Called first for every winit event of the browser window.
pub(crate) fn observe_window_event(
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))] window: &MainWindow,
    event: &WindowEvent,
    shortcut_keys: &mut ShortcutKeys,
) {
    #[cfg(target_os = "macos")]
    macos::remember_browser_window(window);
    observe_event(event, shortcut_keys);
}

fn observe_event(event: &WindowEvent, shortcut_keys: &mut ShortcutKeys) {
    STATE.with(|state| {
        for key in state.claimed_keys.take() {
            shortcut_keys.claim(key);
        }
        if let WindowEvent::Focused(focused) = event {
            state.browser_window_focused.set(*focused);
        }
    });
    #[cfg(target_os = "macos")]
    if matches!(event, WindowEvent::Focused(true)) {
        macos::ensure_installed_later("the browser window became key");
    }
}

/// Runs a menu command from the event loop, outside whatever callback chose it.
pub(crate) fn dispatch(command: MenuCommand) {
    if let Err(error) = slint::invoke_from_event_loop(move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        run(command);
    }) {
        log::warn!("could not run the menu command {command:?}: {error}");
    }
}

fn run(command: MenuCommand) {
    let handler = STATE.with(|state| state.handler.borrow().clone());
    if let Some(handler) = handler {
        handler(command);
    }
}

fn run_command(
    window: &MainWindow,
    controller: &RefCell<BrowserController>,
    web_view_focused: &Cell<bool>,
    command: MenuCommand,
) {
    if !model::is_available(command) {
        return;
    }
    match command {
        MenuCommand::Shortcut(action) => {
            run_shortcut(window, controller, web_view_focused, action);
            window.window().request_redraw();
        }
        MenuCommand::About => about::show(),
        MenuCommand::Edit(edit) => run_edit(window, controller, edit),
        MenuCommand::OpenDocument(document) => {
            let opened = controller
                .borrow_mut()
                .open_tab_with(Some(document.url()), TabActivation::Foreground);
            if let Err(error) = opened {
                log::warn!("could not open {}: {error}", document.url());
                return;
            }
            sync_ui(window, &controller.borrow());
            window.window().request_redraw();
        }
        // Performed by AppKit itself; never dispatched.
        MenuCommand::System(_) => {}
        MenuCommand::ReopenClosedTab => seams::reopen_closed_tab(window),
        MenuCommand::RestorePreviousSession => seams::restore_previous_session(window),
        MenuCommand::ShowHistory => seams::show_history(window),
        MenuCommand::ClearBrowsingData => seams::clear_browsing_data(window),
        MenuCommand::BookmarkThisPage => seams::bookmark_this_page(window),
        MenuCommand::ToggleBookmarksBar => seams::toggle_bookmarks_bar(window),
        MenuCommand::ShowBookmarksManager => seams::show_bookmarks_manager(window),
        MenuCommand::OpenBookmark(id) => seams::open_bookmark(window, id),
        MenuCommand::Find | MenuCommand::Stop => {}
    }
}

/// An Edit menu click goes where the key would have gone.
fn run_edit(window: &MainWindow, controller: &RefCell<BrowserController>, edit: EditCommand) {
    match current_edit_target() {
        EditTarget::AddressBar => window.invoke_edit_address_bar(address_bar_edit(edit)),
        EditTarget::Page => {
            let command = match edit {
                EditCommand::Cut => ClipboardCommand::Cut,
                EditCommand::Copy => ClipboardCommand::Copy,
                EditCommand::Paste => ClipboardCommand::Paste,
                // No engine API: the item is disabled while the page has focus.
                EditCommand::Undo | EditCommand::Redo | EditCommand::SelectAll => return,
            };
            controller
                .borrow_mut()
                .queue_command(BrowserCommand::Clipboard(command));
            window.window().request_redraw();
        }
        EditTarget::None => {}
    }
}

fn current_edit_target() -> EditTarget {
    STATE.with(|state| {
        let web_view_focused = state
            .web_view_focused
            .borrow()
            .as_ref()
            .is_some_and(|focused| focused.get());
        model::edit_target(
            state.browser_window_focused.get(),
            state.address_bar_focused.get(),
            web_view_focused,
        )
    })
}

const fn address_bar_edit(edit: EditCommand) -> AddressBarEdit {
    match edit {
        EditCommand::Undo => AddressBarEdit::Undo,
        EditCommand::Redo => AddressBarEdit::Redo,
        EditCommand::Cut => AddressBarEdit::Cut,
        EditCommand::Copy => AddressBarEdit::Copy,
        EditCommand::Paste => AddressBarEdit::Paste,
        EditCommand::SelectAll => AddressBarEdit::SelectAll,
    }
}

const fn browser_menu_item(command: BrowserMenuCommand) -> BrowserMenuItem {
    match command {
        BrowserMenuCommand::NewTab => BrowserMenuItem::NewTab,
        BrowserMenuCommand::ZoomIn => BrowserMenuItem::ZoomIn,
        BrowserMenuCommand::ZoomOut => BrowserMenuItem::ZoomOut,
        BrowserMenuCommand::ZoomReset => BrowserMenuItem::ZoomReset,
        BrowserMenuCommand::Find => BrowserMenuItem::Find,
        BrowserMenuCommand::Settings => BrowserMenuItem::Settings,
        BrowserMenuCommand::About => BrowserMenuItem::About,
        BrowserMenuCommand::Help => BrowserMenuItem::Help,
        BrowserMenuCommand::Quit => BrowserMenuItem::Quit,
    }
}

const fn browser_menu_command(item: BrowserMenuItem) -> BrowserMenuCommand {
    match item {
        BrowserMenuItem::NewTab => BrowserMenuCommand::NewTab,
        BrowserMenuItem::ZoomIn => BrowserMenuCommand::ZoomIn,
        BrowserMenuItem::ZoomOut => BrowserMenuCommand::ZoomOut,
        BrowserMenuItem::ZoomReset => BrowserMenuCommand::ZoomReset,
        BrowserMenuItem::Find => BrowserMenuCommand::Find,
        BrowserMenuItem::Settings => BrowserMenuCommand::Settings,
        BrowserMenuItem::About => BrowserMenuCommand::About,
        BrowserMenuItem::Help => BrowserMenuCommand::Help,
        BrowserMenuItem::Quit => BrowserMenuCommand::Quit,
    }
}

fn browser_menu_model(rows: Vec<model::BrowserMenuRow>) -> ModelRc<BrowserMenuEntry> {
    let entries = rows
        .into_iter()
        .map(|row| BrowserMenuEntry {
            title: row.title.into(),
            shortcut: row.shortcut.into(),
            enabled: row.enabled,
            command: browser_menu_command(row.item),
            separator_before: row.separator_before,
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(entries))
}

/// The native menu performed a key's press (macOS).
fn claim_key(key: PhysicalKey) {
    STATE.with(|state| state.claimed_keys.borrow_mut().push(key));
}

fn browser_window_focused() -> bool {
    STATE.with(|state| state.browser_window_focused.get())
}

fn snapshot() -> MenuSnapshot {
    STATE.with(|state| state.snapshot.borrow().clone())
}

#[cfg(test)]
mod tests {
    use slint::winit_030::winit::keyboard::KeyCode;

    use super::*;
    use crate::shortcuts::KeyDisposition;

    #[test]
    fn browser_menu_commands_survive_the_trip_through_slint() {
        for item in [
            BrowserMenuItem::NewTab,
            BrowserMenuItem::ZoomIn,
            BrowserMenuItem::ZoomOut,
            BrowserMenuItem::ZoomReset,
            BrowserMenuItem::Find,
            BrowserMenuItem::Settings,
            BrowserMenuItem::About,
            BrowserMenuItem::Help,
            BrowserMenuItem::Quit,
        ] {
            assert_eq!(browser_menu_item(browser_menu_command(item)), item);
        }
    }

    #[test]
    fn a_key_the_menu_performed_is_handed_to_the_shortcut_keys() {
        let key = PhysicalKey::Code(KeyCode::KeyH);
        let mut shortcut_keys = ShortcutKeys::default();
        claim_key(key);

        observe_event(&WindowEvent::Occluded(false), &mut shortcut_keys);

        assert_eq!(shortcut_keys.release(key), KeyDisposition::Consume);
        assert_eq!(shortcut_keys.release(key), KeyDisposition::Forward);
    }

    #[test]
    fn edit_clicks_follow_window_focus() {
        observe_event(&WindowEvent::Focused(false), &mut ShortcutKeys::default());
        STATE.with(|state| state.address_bar_focused.set(true));
        assert_eq!(current_edit_target(), EditTarget::None);

        STATE.with(|state| state.browser_window_focused.set(true));
        assert_eq!(current_edit_target(), EditTarget::AddressBar);
        STATE.with(|state| state.address_bar_focused.set(false));
    }
}
