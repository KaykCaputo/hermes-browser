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
//! (`performKeyEquivalent:`) before the key window gets the key down, and the override declines
//! the keys the window owns.

use std::cell::RefCell;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventModifierFlags,
    NSEventType, NSMenu, NSMenuDelegate, NSMenuItem,
};
use objc2_foundation::NSString;
use slint::winit_030::winit::keyboard::PhysicalKey;
use slint::winit_030::winit::platform::scancode::PhysicalKeyExtScancode;

use super::model::{
    self, KeyEquivalent, MenuCommand, MenuItem, Modifiers, SystemAction, key_from_appkit_characters,
};
use crate::shortcuts::ShortcutAction;

/// Tags of the Window menu's tab items: this plus the tab's index.
const FIRST_TAB_TAG: isize = 10_000;

struct NativeMenu {
    main_menu: Retained<MainMenu>,
    /// Target of every Mercury item and delegate of the Window menu (both are weak references
    /// in AppKit, so this keeps it alive).
    _target: Retained<MenuTarget>,
}

thread_local! {
    static NATIVE: RefCell<Option<NativeMenu>> = const { RefCell::new(None) };
    /// The command of each item, by tag.
    static COMMANDS: RefCell<Vec<MenuCommand>> = const { RefCell::new(Vec::new()) };
}

define_class!(
    #[unsafe(super(NSMenu))]
    #[thread_kind = MainThreadOnly]
    #[name = "MercuryMainMenu"]
    struct MainMenu;

    impl MainMenu {
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
            Bool::new(self.decide_key_equivalent(event))
        }
    }
);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MercuryMenuTarget"]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(mercuryMenuItemChosen:))]
        fn item_chosen(&self, item: &NSMenuItem) {
            let Some(command) = command_for_tag(item.tag()) else {
                return;
            };
            let mtm = MainThreadMarker::from(self);
            if command == MenuCommand::Shortcut(ShortcutAction::CloseTab)
                && !super::browser_window_focused()
            {
                // Cmd+W in the About window (or any other window of ours) closes that window.
                if let Some(window) = NSApplication::sharedApplication(mtm).keyWindow() {
                    window.performClose(None);
                }
                return;
            }
            super::dispatch(command);
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> Bool {
            Bool::new(validate(item))
        }
    }

    unsafe impl NSObjectProtocol for MenuTarget {}

    unsafe impl NSMenuDelegate for MenuTarget {
        /// The Window menu is about to open (or be searched for a key): list the tabs.
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            let mtm = MainThreadMarker::from(self);
            for index in (0..menu.numberOfItems()).rev() {
                if menu.itemAtIndex(index).is_some_and(|item| tab_index(item.tag()).is_some()) {
                    menu.removeItemAtIndex(index);
                }
            }
            for (index, title) in super::snapshot().tab_titles.iter().enumerate() {
                let title = if title.is_empty() { "New Tab" } else { title.as_str() };
                let item = new_item(mtm, title, Some(sel!(mercuryMenuItemChosen:)), model::tab_key(index));
                item.setTag(FIRST_TAB_TAG.saturating_add(isize::try_from(index).unwrap_or(isize::MAX)));
                unsafe { item.setTarget(Some(self)) };
                menu.addItem(&item);
            }
        }
    }
);

impl MainMenu {
    fn super_perform_key_equivalent(&self, event: &NSEvent) -> bool {
        let handled: Bool = unsafe { msg_send![super(self), performKeyEquivalent: event] };
        handled.as_bool()
    }

    fn decide_key_equivalent(&self, event: &NSEvent) -> bool {
        let Some((key, modifiers)) = key_of(event) else {
            return self.super_perform_key_equivalent(event);
        };
        let window_focused = super::browser_window_focused();
        if window_focused && model::window_owns_key(&key, modifiers) {
            // `shortcuts.rs`, the address bar or the page handles it, once.
            return false;
        }
        if model::menu_owns_key(&key, modifiers) && event.isARepeat() {
            // Menu actions do not repeat, like most `shortcuts.rs` actions.
            return true;
        }
        let handled = self.super_perform_key_equivalent(event);
        if handled && window_focused {
            super::claim_key(PhysicalKey::from_scancode(u32::from(event.keyCode())));
        }
        handled
    }
}

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

fn validate(item: &NSMenuItem) -> bool {
    let tag = item.tag();
    if let Some(index) = tab_index(tag) {
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
    if let Some(index) = tab_index(tag) {
        return Some(MenuCommand::Shortcut(ShortcutAction::SelectTab(index)));
    }
    let index = usize::try_from(tag).ok()?;
    COMMANDS.with(|commands| commands.borrow().get(index).copied())
}

fn tab_index(tag: isize) -> Option<usize> {
    tag.checked_sub(FIRST_TAB_TAG)
        .and_then(|index| usize::try_from(index).ok())
}

/// The key and modifiers of a key-down event, as `model` matches them.
fn key_of(event: &NSEvent) -> Option<(slint::winit_030::winit::keyboard::Key, Modifiers)> {
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
    let main_menu: Retained<MainMenu> =
        unsafe { msg_send![MainMenu::alloc(mtm), initWithTitle: &*NSString::from_str("")] };
    let mut commands = Vec::new();

    for menu in model::menu_bar() {
        let submenu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(menu.title));
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
                            unsafe { item.setTarget(Some(&target)) };
                            commands.push(command);
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
                MenuItem::TabList => {
                    submenu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
                }
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
        _target: target,
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
    let _ = slint::invoke_from_event_loop(move || ensure_installed(reason));
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

/// `MERCURY_MENU_DIAGNOSTICS=<directory>`: reads the main menu back after start-up, after the
/// About window opens and after it closes, and logs it; saves the About window, rendered by the
/// app's renderer, as `about-window.png` in the directory. Works in `--background` runs and
/// needs no input.
fn start_diagnostics() {
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
    slint::Timer::single_shot(Duration::from_secs(2), || {
        report("after start-up");
        crate::about::show();
        slint::Timer::single_shot(Duration::from_secs(3), move || {
            report("with the About window open");
            match crate::about::save_snapshot(&snapshot_path) {
                Ok(()) => log::info!("menu diagnostics: saved {}", snapshot_path.display()),
                Err(error) => log::warn!("menu diagnostics: no About snapshot: {error}"),
            }
            crate::about::hide();
            slint::Timer::single_shot(Duration::from_millis(1500), || {
                report("after the About window closed");
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_tags_do_not_collide_with_command_tags() {
        assert_eq!(tab_index(FIRST_TAB_TAG), Some(0));
        assert_eq!(tab_index(FIRST_TAB_TAG + 3), Some(3));
        assert_eq!(tab_index(5), None);
        let item_count = model::menu_bar()
            .iter()
            .map(|menu| menu.items.len())
            .sum::<usize>();
        assert!(isize::try_from(item_count).unwrap() < FIRST_TAB_TAG);
    }
}
