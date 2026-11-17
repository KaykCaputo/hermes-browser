//! Mercury's own macOS menu bar, built with AppKit.
//!
//! Slint's `MenuBar` cannot be used: on macOS its winit backend (through muda) always puts a
//! fixed application menu first, whose Quit sends `terminate:` and whose About opens AppKit's
//! standard panel, and it gives no way to keep a key equivalent from being consumed. Each Slint
//! window instead declares a hidden, empty `MenuBar`, which keeps Slint from installing its
//! default menu bar (`i-slint-backend-winit` 1.17.1, `winitwindowadapter.rs`
//! `activation_changed`: the default is only installed for a window without a `MenuBar`; a
//! hidden one builds no menu, `muda.rs` `rebuild_menu`). `ensure_installed` checks that ours is
//! still the main menu whenever one of our windows becomes key.
//!
//! The main menu is an `NSMenu` subclass: AppKit offers every key equivalent to the main menu
//! (`performKeyEquivalent:`) before the key window gets the key down. The override only gathers
//! the inputs of `model::decide_key_equivalent` and carries out its decision.
//!
//! Nothing called from AppKit here touches Slint or the controller: commands are posted with
//! `slint::invoke_from_event_loop` (`super::dispatch`), and the state read here is a snapshot.

use std::cell::RefCell;

use mercury_ui::MainWindow;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventModifierFlags,
    NSEventType, NSMenu, NSMenuDelegate, NSMenuItem, NSView, NSWindow,
};
use objc2_foundation::NSString;
use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::keyboard::{Key, PhysicalKey};
use slint::winit_030::winit::platform::scancode::PhysicalKeyExtScancode;
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::model::{
    self, KeyDecision, KeyEquivalent, KeyWindow, MatchedItem, MenuCommand, MenuItem, Modifiers,
    StandardAction, SystemAction, key_from_appkit_characters,
};
use super::seams::BookmarkId;
use crate::shortcuts::ShortcutAction;

/// Tags of the Window menu's tab items: this plus the tab's index.
const FIRST_TAB_TAG: isize = 10_000;
/// Tags of the Bookmarks menu's bar items: this plus the entry's index.
const FIRST_BOOKMARK_TAG: isize = 20_000;

struct NativeMenu {
    main_menu: Retained<MainMenu>,
    /// Target of every Mercury item and delegate of every submenu (both are weak references in
    /// AppKit, so this keeps it alive).
    target: Retained<MenuTarget>,
    window_menu: Retained<NSMenu>,
    bookmarks_menu: Retained<NSMenu>,
    /// Items that become AppKit's standard ones while a native panel is key (Edit, Close).
    routable: Vec<(Retained<NSMenuItem>, MenuCommand)>,
}

thread_local! {
    /// The browser window's `NSWindow`, once its winit window exists.
    static BROWSER_WINDOW: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
    static NATIVE: RefCell<Option<NativeMenu>> = const { RefCell::new(None) };
    /// The command of each item, by tag.
    static COMMANDS: RefCell<Vec<MenuCommand>> = const { RefCell::new(Vec::new()) };
    /// The bookmark each bar item of the Bookmarks menu opens, by position, as listed when the
    /// menu was last built.
    static BOOKMARK_IDS: RefCell<Vec<BookmarkId>> = const { RefCell::new(Vec::new()) };
}

define_class!(
    // SAFETY: NSMenu may be subclassed; the subclass adds no instance variables and no `Drop`.
    #[unsafe(super(NSMenu))]
    #[thread_kind = MainThreadOnly]
    #[name = "MercuryMainMenu"]
    struct MainMenu;

    impl MainMenu {
        // SAFETY: the signature is NSMenu's `- (BOOL)performKeyEquivalent:(NSEvent *)event`.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
            Bool::new(self.handle_key_equivalent(event))
        }
    }
);

define_class!(
    // SAFETY: NSObject may be subclassed; no instance variables, no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MercuryMenuTarget"]
    struct MenuTarget;

    impl MenuTarget {
        // SAFETY: an action method, `- (void)action:(id)sender`; AppKit passes the menu item,
        // and a nil sender (possible through `sendAction:to:from:`) is handled.
        #[unsafe(method(mercuryMenuItemChosen:))]
        fn item_chosen(&self, item: Option<&NSMenuItem>) {
            let Some(command) = item.and_then(|item| command_for_tag(item.tag())) else {
                return;
            };
            let mtm = MainThreadMarker::from(self);
            if command == MenuCommand::Shortcut(ShortcutAction::CloseTab)
                && key_window(mtm) == KeyWindow::OtherMercury
            {
                // Cmd+W in the About window (or another window of ours) closes that window.
                close_key_window_later();
                return;
            }
            super::dispatch(command);
        }

        // SAFETY: `- (BOOL)validateMenuItem:(NSMenuItem *)item` (NSMenuItemValidation).
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> Bool {
            Bool::new(validate(item))
        }
    }

    // SAFETY: NSObject's protocol, implemented by the superclass.
    unsafe impl NSObjectProtocol for MenuTarget {}

    // SAFETY: only `menuNeedsUpdate:` is implemented, with the protocol's signature.
    unsafe impl NSMenuDelegate for MenuTarget {
        /// A menu is about to open or be searched for a key equivalent.
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            let mtm = MainThreadMarker::from(self);
            update_routing(key_window(mtm));
            let (is_window_menu, is_bookmarks_menu) = NATIVE.with(|native| {
                native.borrow().as_ref().map_or((false, false), |native| {
                    (
                        std::ptr::eq(menu, &*native.window_menu),
                        std::ptr::eq(menu, &*native.bookmarks_menu),
                    )
                })
            });
            let snapshot = super::snapshot();
            if is_window_menu {
                let items = snapshot.tab_titles.iter().enumerate().map(|(index, title)| {
                    let title = if title.is_empty() { "New Tab" } else { title.as_str() };
                    (title.to_owned(), model::tab_key(index))
                });
                replace_dynamic_items(self, menu, FIRST_TAB_TAG, items);
            }
            if is_bookmarks_menu {
                BOOKMARK_IDS.with(|ids| {
                    ids.replace(snapshot.bookmarks_bar.iter().map(|(id, _)| *id).collect())
                });
                let items = snapshot.bookmarks_bar.iter().map(|(_, title)| (title.clone(), None));
                replace_dynamic_items(self, menu, FIRST_BOOKMARK_TAG, items);
            }
        }
    }
);

impl MainMenu {
    fn super_perform_key_equivalent(&self, event: &NSEvent) -> bool {
        // SAFETY: calls NSMenu's own implementation with the same, valid event.
        let handled: Bool = unsafe { msg_send![super(self), performKeyEquivalent: event] };
        handled.as_bool()
    }

    fn handle_key_equivalent(&self, event: &NSEvent) -> bool {
        let Some((key, modifiers)) = key_of(event) else {
            return self.super_perform_key_equivalent(event);
        };
        let mtm = MainThreadMarker::from(self);
        let key_window = key_window(mtm);
        update_routing(key_window);
        let snapshot = super::snapshot();
        let item = model::item_for_key(&key, modifiers, snapshot.tab_titles.len()).map(|command| {
            MatchedItem {
                command,
                enabled: model::is_enabled(command, &snapshot, super::current_edit_target()),
            }
        });
        match model::decide_key_equivalent(&key, modifiers, key_window, event.isARepeat(), item) {
            KeyDecision::Decline => false,
            KeyDecision::Swallow => true,
            KeyDecision::Perform => {
                let handled = self.super_perform_key_equivalent(event);
                if handled && key_window == KeyWindow::Browser {
                    // The window never sees this press: swallow its release too.
                    super::claim_key(PhysicalKey::from_scancode(u32::from(event.keyCode())));
                }
                handled
            }
        }
    }
}

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: plain `init` of an NSObject subclass without instance variables.
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

/// Which window has keyboard focus. AppKit's key window is compared with the browser's
/// `NSWindow` first: winit's `Focused` event can still be queued (a sheet begun from inside a
/// Slint callback is key before the browser window's `Focused(false)` arrives). Every Slint
/// window is a winit `WinitWindow`; any other key window (an open panel, a sheet, an alert) was
/// made by AppKit. winit's focus flag is used only while the browser's `NSWindow` is not known.
fn key_window(mtm: MainThreadMarker) -> KeyWindow {
    let browser = BROWSER_WINDOW.with(|browser| browser.borrow().clone());
    let Some(window) = NSApplication::sharedApplication(mtm).keyWindow() else {
        return if browser.is_none() && super::browser_window_focused() {
            KeyWindow::Browser
        } else {
            KeyWindow::None
        };
    };
    match browser {
        Some(browser) if std::ptr::eq(&*window, &*browser) => return KeyWindow::Browser,
        None if super::browser_window_focused() => return KeyWindow::Browser,
        _ => {}
    }
    let is_winit_window =
        AnyClass::get(c"WinitWindow").is_some_and(|class| window.isKindOfClass(class));
    if is_winit_window {
        KeyWindow::OtherMercury
    } else {
        KeyWindow::Native
    }
}

/// Records the browser window's `NSWindow` the first time its winit window exists. Called from
/// the browser window's winit event hook, where Slint lends the winit window (`keyboard.rs`
/// does the same).
pub(super) fn remember_browser_window(window: &MainWindow) {
    if BROWSER_WINDOW.with(|browser| browser.borrow().is_some()) {
        return;
    }
    let ns_window = window
        .window()
        .with_winit_window(|winit_window| {
            let handle = winit_window.window_handle().ok()?;
            let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                return None;
            };
            // SAFETY: winit's AppKit handle is a valid `NSView` while the winit window exists,
            // which it does for the duration of this closure.
            let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
            view.window()
        })
        .flatten();
    if let Some(ns_window) = ns_window {
        BROWSER_WINDOW.with(|browser| browser.replace(Some(ns_window)));
    }
}

/// Forgets the browser's `NSWindow` when the menus are released.
pub(super) fn release() {
    BROWSER_WINDOW.with(|browser| browser.take());
}

fn standard_selector(action: StandardAction) -> Sel {
    match action {
        StandardAction::Undo => sel!(undo:),
        StandardAction::Redo => sel!(redo:),
        StandardAction::Cut => sel!(cut:),
        StandardAction::Copy => sel!(copy:),
        StandardAction::Paste => sel!(paste:),
        StandardAction::SelectAll => sel!(selectAll:),
        StandardAction::PerformClose => sel!(performClose:),
    }
}

/// While a native panel is key, the Edit items and Close are AppKit's standard items: a nil
/// target sends the action to the first responder (`sendAction:to:nil from:`), and AppKit
/// validates them along the responder chain. Otherwise they are Mercury's.
fn update_routing(key_window: KeyWindow) {
    NATIVE.with(|native| {
        let native = native.borrow();
        let Some(native) = native.as_ref() else {
            return;
        };
        for (item, command) in &native.routable {
            let standard = model::standard_action(*command)
                .filter(|_| model::uses_responder_chain(*command, key_window));
            // SAFETY: the selectors are AppKit's standard actions or ours, which `MenuTarget`
            // implements; the target is kept alive by `NATIVE`.
            unsafe {
                match standard {
                    Some(action) => {
                        item.setAction(Some(standard_selector(action)));
                        item.setTarget(None);
                    }
                    None => {
                        item.setAction(Some(sel!(mercuryMenuItemChosen:)));
                        item.setTarget(Some(&native.target));
                    }
                }
            }
        }
    });
}

/// Closes the key window from the event loop, if it is still one of ours other than the
/// browser window.
fn close_key_window_later() {
    let _ = slint::invoke_from_event_loop(|| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if key_window(mtm) != KeyWindow::OtherMercury {
            return;
        }
        if let Some(window) = NSApplication::sharedApplication(mtm).keyWindow() {
            window.performClose(None);
        }
    });
}

fn validate(item: &NSMenuItem) -> bool {
    let tag = item.tag();
    if let Some(index) = dynamic_index(tag, FIRST_TAB_TAG) {
        let snapshot = super::snapshot();
        item.setState(if index == snapshot.active_tab_index {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        return true;
    }
    command_for_tag(tag).is_some_and(|command| {
        model::is_enabled(command, &super::snapshot(), super::current_edit_target())
    })
}

fn command_for_tag(tag: isize) -> Option<MenuCommand> {
    if let Some(index) = dynamic_index(tag, FIRST_TAB_TAG) {
        return Some(MenuCommand::Shortcut(ShortcutAction::SelectTab(index)));
    }
    if let Some(index) = dynamic_index(tag, FIRST_BOOKMARK_TAG) {
        let id = BOOKMARK_IDS.with(|ids| ids.borrow().get(index).copied())?;
        return Some(MenuCommand::OpenBookmark(id));
    }
    let index = usize::try_from(tag).ok()?;
    COMMANDS.with(|commands| commands.borrow().get(index).copied())
}

/// The index of a dynamic item (tab or bookmark) from its tag.
fn dynamic_index(tag: isize, first: isize) -> Option<usize> {
    let index = usize::try_from(tag.checked_sub(first)?).ok()?;
    (index < 10_000).then_some(index)
}

/// Replaces a menu's items tagged from `first_tag` on with `items`, at the end of the menu.
fn replace_dynamic_items(
    target: &MenuTarget,
    menu: &NSMenu,
    first_tag: isize,
    items: impl Iterator<Item = (String, Option<KeyEquivalent>)>,
) {
    let mtm = MainThreadMarker::from(target);
    for index in (0..menu.numberOfItems()).rev() {
        if menu
            .itemAtIndex(index)
            .is_some_and(|item| dynamic_index(item.tag(), first_tag).is_some())
        {
            menu.removeItemAtIndex(index);
        }
    }
    for (index, (title, key)) in items.enumerate() {
        let item = new_item(mtm, &title, Some(sel!(mercuryMenuItemChosen:)), key);
        item.setTag(first_tag.saturating_add(isize::try_from(index).unwrap_or(isize::MAX)));
        // SAFETY: `target` implements the action and is kept alive by `NATIVE`.
        unsafe { item.setTarget(Some(target)) };
        menu.addItem(&item);
    }
}

/// The key and modifiers of a key-down event, as `model` matches them.
fn key_of(event: &NSEvent) -> Option<(Key, Modifiers)> {
    if event.r#type() != NSEventType::KeyDown {
        return None;
    }
    let characters = event.charactersIgnoringModifiers()?.to_string();
    let flags = event.modifierFlags();
    let modifiers = Modifiers {
        command: flags.contains(NSEventModifierFlags::Command),
        shift: flags.contains(NSEventModifierFlags::Shift),
        option: flags.contains(NSEventModifierFlags::Option),
        control: flags.contains(NSEventModifierFlags::Control),
    };
    Some((key_from_appkit_characters(&characters), modifiers))
}

fn new_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: Option<KeyEquivalent>,
) -> Retained<NSMenuItem> {
    let key_equivalent = key.map(|key| key.key.to_string()).unwrap_or_default();
    // SAFETY: a fresh item; the action is a selector AppKit or `MenuTarget` implements.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str(title),
            action,
            &NSString::from_str(&key_equivalent),
        )
    };
    if let Some(key) = key {
        let mut mask = NSEventModifierFlags::empty();
        let Modifiers {
            command,
            shift,
            option,
            control,
        } = key.modifiers;
        mask.set(NSEventModifierFlags::Command, command);
        mask.set(NSEventModifierFlags::Shift, shift);
        mask.set(NSEventModifierFlags::Option, option);
        mask.set(NSEventModifierFlags::Control, control);
        item.setKeyEquivalentModifierMask(mask);
    }
    item
}

fn system_selector(action: SystemAction) -> Sel {
    match action {
        SystemAction::Hide => sel!(hide:),
        SystemAction::HideOthers => sel!(hideOtherApplications:),
        SystemAction::ShowAll => sel!(unhideAllApplications:),
        SystemAction::Minimize => sel!(performMiniaturize:),
        SystemAction::Zoom => sel!(performZoom:),
        SystemAction::ToggleFullScreen => sel!(toggleFullScreen:),
    }
}

fn build(mtm: MainThreadMarker) -> NativeMenu {
    let application = NSApplication::sharedApplication(mtm);
    let target = MenuTarget::new(mtm);
    // SAFETY: NSMenu's designated initialiser, on our subclass, which adds no state.
    let main_menu: Retained<MainMenu> =
        unsafe { msg_send![MainMenu::alloc(mtm), initWithTitle: &*NSString::from_str("")] };
    let mut commands = Vec::new();
    let mut routable = Vec::new();
    let mut window_menu = None;
    let mut bookmarks_menu = None;

    for menu in model::menu_bar() {
        let submenu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(menu.title));
        submenu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        for entry in &menu.items {
            match *entry {
                MenuItem::Action {
                    title,
                    key,
                    command,
                } => {
                    let item = match command {
                        MenuCommand::System(action) => {
                            // Standard AppKit item: sent along the responder chain, validated by
                            // AppKit.
                            new_item(mtm, title, Some(system_selector(action)), key)
                        }
                        _ => {
                            let item =
                                new_item(mtm, title, Some(sel!(mercuryMenuItemChosen:)), key);
                            item.setTag(isize::try_from(commands.len()).unwrap_or(isize::MAX));
                            // SAFETY: `target` implements the action and is kept alive with
                            // the menu in `NATIVE`.
                            unsafe { item.setTarget(Some(&target)) };
                            commands.push(command);
                            if model::standard_action(command).is_some() {
                                routable.push((item.clone(), command));
                            }
                            item
                        }
                    };
                    submenu.addItem(&item);
                }
                MenuItem::Separator => submenu.addItem(&NSMenuItem::separatorItem(mtm)),
                MenuItem::Services => {
                    let services =
                        NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("Services"));
                    let item = new_item(mtm, "Services", None, None);
                    item.setSubmenu(Some(&services));
                    submenu.addItem(&item);
                    application.setServicesMenu(Some(&services));
                }
                MenuItem::TabList => window_menu = Some(submenu.clone()),
                MenuItem::BookmarksBarFolder => bookmarks_menu = Some(submenu.clone()),
            }
        }
        let top_item = new_item(mtm, menu.title, None, None);
        top_item.setSubmenu(Some(&submenu));
        main_menu.addItem(&top_item);
        if menu.title == "Help" {
            application.setHelpMenu(Some(&submenu));
        }
    }

    COMMANDS.with(|stored| stored.replace(commands));
    NativeMenu {
        main_menu,
        target,
        window_menu: window_menu.expect("the Window menu lists the tabs"),
        bookmarks_menu: bookmarks_menu.expect("the Bookmarks menu lists the bar"),
        routable,
    }
}

/// Builds the menu bar and makes it the application's main menu.
pub(super) fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("the menu bar can only be built on the main thread");
        return;
    };
    NATIVE.with(|native| {
        if native.borrow().is_none() {
            native.replace(Some(build(mtm)));
        }
    });
    ensure_installed("start-up");
    // Again once the event loop runs, after anything that happens while it starts.
    ensure_installed_later("the event loop started");
    #[cfg(feature = "menu-diagnostics")]
    start_diagnostics();
}

/// Makes Mercury's menu the main menu if something replaced it, and logs the menu bar.
pub(crate) fn ensure_installed(reason: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(ours) = NATIVE.with(|native| {
        native
            .borrow()
            .as_ref()
            .map(|native| native.main_menu.clone())
    }) else {
        return;
    };
    let application = NSApplication::sharedApplication(mtm);
    let current = application.mainMenu();
    let is_ours = current
        .as_deref()
        .is_some_and(|current| std::ptr::eq(current, &**ours));
    if !is_ours {
        if current.is_some() {
            log::warn!("another menu bar replaced Mercury's ({reason}); reinstalling it");
        }
        application.setMainMenu(Some(&ours));
    }
    log::debug!(
        "menu bar after {reason}: [{}] (Mercury's: {})",
        main_menu_titles(&application).join(", "),
        main_menu_is_ours(&application),
    );
}

pub(crate) fn ensure_installed_later(reason: &'static str) {
    let _ = slint::invoke_from_event_loop(move || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        ensure_installed(reason);
    });
}

/// For a window of ours other than the browser window (About, settings): reinstall Mercury's
/// menu whenever it becomes key. Takes the window's winit event hook.
pub(crate) fn reinstall_when_key(window: &slint::Window, reason: &'static str) {
    use slint::winit_030::{EventResult, winit::event::WindowEvent};
    window.on_winit_window_event(move |_window, event| {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        if matches!(event, WindowEvent::Focused(true)) {
            ensure_installed_later(reason);
        }
        EventResult::Propagate
    });
}

fn main_menu_titles(application: &NSApplication) -> Vec<String> {
    application
        .mainMenu()
        .map(|menu| {
            menu.itemArray()
                .iter()
                .map(|item| match item.submenu() {
                    Some(submenu) => submenu.title().to_string(),
                    None => item.title().to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn main_menu_is_ours(application: &NSApplication) -> bool {
    application
        .mainMenu()
        .is_some_and(|menu| menu.isKindOfClass(MainMenu::class()))
}

/// `MERCURY_MENU_DIAGNOSTICS=<directory>`, in builds with the `menu-diagnostics` feature
/// (documented in docs/developer-guide/testing.md): reads the main menu back after start-up,
/// with the About window open and after it closes, and the same for the settings window, and
/// logs it; saves the About window, rendered by the app's renderer, as `about-window.png` in
/// the directory. Works in `--background` runs and needs no input.
#[cfg(feature = "menu-diagnostics")]
fn start_diagnostics() {
    use std::time::Duration;

    let Some(directory) = std::env::var_os("MERCURY_MENU_DIAGNOSTICS") else {
        return;
    };
    let snapshot_path = std::path::PathBuf::from(directory).join("about-window.png");
    fn report(stage: &str) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        ensure_installed(stage);
        let application = NSApplication::sharedApplication(mtm);
        log::info!(
            "menu diagnostics, {stage}: main menu is Mercury's: {}; menus: [{}]",
            main_menu_is_ours(&application),
            main_menu_titles(&application).join(", "),
        );
    }
    let delay = Duration::from_secs(2);
    slint::Timer::single_shot(delay, || {
        let _slint = crate::callback_scope::InSlintCallback::enter();
        report("after start-up");
        crate::about::show();
        let delay = Duration::from_secs(3);
        slint::Timer::single_shot(delay, move || {
            let _slint = crate::callback_scope::InSlintCallback::enter();
            report("with the About window open");
            match crate::about::save_snapshot(&snapshot_path) {
                Ok(()) => log::info!("menu diagnostics: saved {}", snapshot_path.display()),
                Err(error) => log::warn!("menu diagnostics: no About snapshot: {error}"),
            }
            crate::about::hide();
            let delay = Duration::from_millis(1500);
            slint::Timer::single_shot(delay, || {
                let _slint = crate::callback_scope::InSlintCallback::enter();
                report("after the About window closed");
                // The settings window, opened the way its menu item opens it.
                super::dispatch(MenuCommand::Shortcut(ShortcutAction::ShowSettings));
                let delay = Duration::from_secs(3);
                slint::Timer::single_shot(delay, || {
                    let _slint = crate::callback_scope::InSlintCallback::enter();
                    report("with the settings window open");
                    // Closes the settings window (what the browser does when it closes).
                    crate::settings::main_window_closing();
                    let delay = Duration::from_millis(1500);
                    slint::Timer::single_shot(delay, || {
                        let _slint = crate::callback_scope::InSlintCallback::enter();
                        report("after the settings window closed");
                    });
                });
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_tags_do_not_collide_with_command_tags() {
        assert_eq!(dynamic_index(FIRST_TAB_TAG, FIRST_TAB_TAG), Some(0));
        assert_eq!(dynamic_index(FIRST_TAB_TAG + 3, FIRST_TAB_TAG), Some(3));
        assert_eq!(dynamic_index(5, FIRST_TAB_TAG), None);
        assert_eq!(dynamic_index(FIRST_BOOKMARK_TAG, FIRST_TAB_TAG), None);
        assert_eq!(
            dynamic_index(FIRST_BOOKMARK_TAG + 2, FIRST_BOOKMARK_TAG),
            Some(2)
        );
        let item_count = model::menu_bar()
            .iter()
            .map(|menu| menu.items.len())
            .sum::<usize>();
        assert!(isize::try_from(item_count).unwrap() < FIRST_TAB_TAG);
    }
}
