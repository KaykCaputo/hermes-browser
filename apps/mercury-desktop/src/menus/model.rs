//! What the menus contain and who owns each key: the macOS menu bar and the browser menu are
//! both built from this table, and the tests check it against `shortcuts.rs`.

use slint::winit_030::winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::shortcuts::{Platform, ShortcutAction, shortcut_action};

/// Something a menu item does. Every item calls existing action code; an item whose action
/// does not exist yet is disabled (see `is_available`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuCommand {
    /// The same action as the keyboard shortcut, run by `shortcuts::run_shortcut`.
    Shortcut(ShortcutAction),
    About,
    Edit(EditCommand),
    OpenDocument(Document),
    /// Handled entirely by AppKit (standard selectors, macOS only).
    System(SystemAction),
    // Seams other lanes fill in (`seams.rs`); disabled until they do.
    ReopenClosedTab,
    RestorePreviousSession,
    ShowHistory,
    ClearBrowsingData,
    BookmarkThisPage,
    ToggleBookmarksBar,
    ShowBookmarksManager,
    /// An entry of the bookmarks bar's first level, by its stable id.
    OpenBookmark(super::seams::BookmarkId),
    // No engine API in Servo 0.8.0: always disabled.
    Find,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SystemAction {
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    ToggleFullScreen,
}

/// Pages opened in a new tab from the Help menu and the About window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Document {
    Help,
    Licence,
    ThirdPartyNotices,
}

impl Document {
    pub(crate) const fn url(self) -> &'static str {
        match self {
            Self::Help => "https://thepigdestroyer.github.io/mercury-browser/",
            Self::Licence => {
                "https://github.com/thepigdestroyer/mercury-browser/blob/master/LICENSE"
            }
            Self::ThirdPartyNotices => {
                "https://github.com/thepigdestroyer/mercury-browser/blob/master/THIRD-PARTY-NOTICES.md"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Modifiers {
    pub(crate) command: bool,
    pub(crate) shift: bool,
    pub(crate) option: bool,
    pub(crate) control: bool,
}

impl Modifiers {
    pub(crate) fn state(self) -> ModifiersState {
        let mut state = ModifiersState::empty();
        state.set(ModifiersState::SUPER, self.command);
        state.set(ModifiersState::SHIFT, self.shift);
        state.set(ModifiersState::ALT, self.option);
        state.set(ModifiersState::CONTROL, self.control);
        state
    }
}

const CMD: Modifiers = Modifiers {
    command: true,
    shift: false,
    option: false,
    control: false,
};
const SHIFT_CMD: Modifiers = Modifiers { shift: true, ..CMD };
const OPTION_CMD: Modifiers = Modifiers {
    option: true,
    ..CMD
};
const CONTROL_CMD: Modifiers = Modifiers {
    control: true,
    ..CMD
};

/// The key a menu item shows, as AppKit's key equivalent: a lowercase character, or one of
/// AppKit's function-key characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyEquivalent {
    pub(crate) key: char,
    pub(crate) modifiers: Modifiers,
}

/// AppKit's character for the Delete (backspace) key.
pub(crate) const DELETE_KEY: char = '\u{8}';

impl KeyEquivalent {
    const fn new(key: char, modifiers: Modifiers) -> Self {
        Self { key, modifiers }
    }

    /// How macOS writes it in a menu, for the browser menu: "⇧⌘T".
    pub(crate) fn mac_label(self) -> String {
        let Modifiers {
            command,
            shift,
            option,
            control,
        } = self.modifiers;
        let mut label = String::new();
        for (held, symbol) in [(control, '⌃'), (option, '⌥'), (shift, '⇧'), (command, '⌘')]
        {
            if held {
                label.push(symbol);
            }
        }
        match self.key {
            DELETE_KEY => label.push('⌫'),
            key => label.extend(key.to_uppercase()),
        }
        label
    }

    /// Who handles this key when the browser window has keyboard focus.
    #[cfg(test)]
    pub(crate) fn owner(self) -> KeyOwner {
        let key = match self.key {
            DELETE_KEY => Key::Named(NamedKey::Backspace),
            key => Key::Character(key.to_string().into()),
        };
        if window_owns_key(&key, self.modifiers) {
            KeyOwner::Window
        } else {
            KeyOwner::Menu
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyOwner {
    /// The window's key path: `shortcuts.rs`, or the focused text field or page for the Edit
    /// keys. The native menu shows the key but declines it.
    Window,
    /// The native menu performs it; the window never sees the press.
    Menu,
}

/// Whether a key press belongs to the window's own key handling, and must therefore be declined
/// by the native menu bar while the browser window has keyboard focus. This is the single
/// decision behind "runs once": `shortcuts.rs` keys, and the editing keys that the address bar
/// (Slint's `TextInput`) and the page (Servo) handle themselves.
pub(crate) fn window_owns_key(key: &Key, modifiers: Modifiers) -> bool {
    shortcut_action(key, modifiers.state(), Platform::MacOs).is_some()
        || is_editing_key(key, modifiers)
}

/// The key of an AppKit key-down event, from its `charactersIgnoringModifiers`, in the form
/// `shortcuts.rs` matches (winit's logical key). AppKit writes function keys as characters in
/// a private-use range.
pub(crate) fn key_from_appkit_characters(characters: &str) -> Key {
    let named = match characters.chars().next() {
        Some('\u{f700}') => NamedKey::ArrowUp,
        Some('\u{f701}') => NamedKey::ArrowDown,
        Some('\u{f702}') => NamedKey::ArrowLeft,
        Some('\u{f703}') => NamedKey::ArrowRight,
        Some('\u{f708}') => NamedKey::F5,
        Some('\u{f72c}') => NamedKey::PageUp,
        Some('\u{f72d}') => NamedKey::PageDown,
        // Shift+Tab arrives as a back-tab character.
        Some('\t' | '\u{19}') => NamedKey::Tab,
        Some('\u{1b}') => NamedKey::Escape,
        Some('\u{7f}' | DELETE_KEY) => NamedKey::Backspace,
        _ => return Key::Character(characters.into()),
    };
    Key::Named(named)
}

/// The menu item whose key equivalent a key press matches: the table's items and the Window
/// menu's tab items (Cmd+1 to Cmd+8, as many as there are tabs).
pub(crate) fn item_for_key(
    key: &Key,
    modifiers: Modifiers,
    tab_count: usize,
) -> Option<MenuCommand> {
    let matches = |equivalent: KeyEquivalent| {
        equivalent.modifiers == modifiers
            && match (equivalent.key, key) {
                (DELETE_KEY, Key::Named(NamedKey::Backspace)) => true,
                (character, Key::Character(text)) => {
                    let text = text.to_lowercase();
                    // With Shift held AppKit reports the shifted character of these keys.
                    text == character.to_string()
                        || matches!((character, text.as_str()), ('[', "{") | (']', "}"))
                }
                _ => false,
            }
    };
    let from_table = menu_bar()
        .iter()
        .flat_map(|menu| menu.items.iter())
        .find_map(|item| match *item {
            MenuItem::Action {
                key: Some(key),
                command,
                ..
            } if matches(key) => Some(command),
            _ => None,
        });
    from_table.or_else(|| {
        (0..tab_count.min(8))
            .find(|index| tab_key(*index).is_some_and(matches))
            .map(|index| MenuCommand::Shortcut(ShortcutAction::SelectTab(index)))
    })
}

/// Which window has keyboard focus, as the menu bar sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyWindow {
    /// The browser window.
    Browser,
    /// Another of Mercury's Slint windows (About, settings).
    OtherMercury,
    /// A window AppKit made: an open or save panel, a sheet, an alert.
    Native,
    None,
}

/// A menu item a key matched, and whether it is enabled right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MatchedItem {
    pub(crate) command: MenuCommand,
    pub(crate) enabled: bool,
}

/// What the main menu does with a key equivalent AppKit offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyDecision {
    /// Not the menu's: AppKit sends the key-down to the key window.
    Decline,
    /// Taken and dropped: an auto-repeat of an action that does not repeat.
    Swallow,
    /// Let `NSMenu` find and perform the item as usual.
    Perform,
}

/// The single decision behind "runs once": who handles a key equivalent.
pub(crate) fn decide_key_equivalent(
    key: &Key,
    modifiers: Modifiers,
    key_window: KeyWindow,
    is_repeat: bool,
    item: Option<MatchedItem>,
) -> KeyDecision {
    if key_window == KeyWindow::Browser && window_owns_key(key, modifiers) {
        return KeyDecision::Decline;
    }
    let Some(item) = item else {
        // AppKit's own items (Services, Emoji & Symbols), or nothing: AppKit as usual.
        return KeyDecision::Perform;
    };
    if uses_responder_chain(item.command, key_window)
        && matches!(item.command, MenuCommand::Edit(_))
    {
        // Standard editing in a native panel; AppKit validates it and repeats it like any app.
        return KeyDecision::Perform;
    }
    if !item.enabled {
        return KeyDecision::Decline;
    }
    let repeats = matches!(item.command, MenuCommand::Shortcut(action) if action.repeats());
    if is_repeat && !repeats {
        KeyDecision::Swallow
    } else {
        KeyDecision::Perform
    }
}

/// AppKit's standard action for the Edit items and Close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StandardAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    PerformClose,
}

pub(crate) const fn standard_action(command: MenuCommand) -> Option<StandardAction> {
    match command {
        MenuCommand::Edit(EditCommand::Undo) => Some(StandardAction::Undo),
        MenuCommand::Edit(EditCommand::Redo) => Some(StandardAction::Redo),
        MenuCommand::Edit(EditCommand::Cut) => Some(StandardAction::Cut),
        MenuCommand::Edit(EditCommand::Copy) => Some(StandardAction::Copy),
        MenuCommand::Edit(EditCommand::Paste) => Some(StandardAction::Paste),
        MenuCommand::Edit(EditCommand::SelectAll) => Some(StandardAction::SelectAll),
        MenuCommand::Shortcut(ShortcutAction::CloseTab) => Some(StandardAction::PerformClose),
        _ => None,
    }
}

/// While a native panel or sheet has keyboard focus, the Edit items and Close are AppKit's
/// standard items: sent to the first responder and validated along the responder chain.
pub(crate) fn uses_responder_chain(command: MenuCommand, key_window: KeyWindow) -> bool {
    key_window == KeyWindow::Native && standard_action(command).is_some()
}

/// Cmd+Z, Shift+Cmd+Z, Cmd+X, Cmd+C, Cmd+V and Cmd+A.
fn is_editing_key(key: &Key, modifiers: Modifiers) -> bool {
    let Key::Character(character) = key else {
        return false;
    };
    if !modifiers.command || modifiers.option || modifiers.control {
        return false;
    }
    match character.to_lowercase().as_str() {
        "z" => true,
        "x" | "c" | "v" | "a" => !modifiers.shift,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuItem {
    Action {
        title: &'static str,
        key: Option<KeyEquivalent>,
        command: MenuCommand,
    },
    Separator,
    /// AppKit's Services submenu.
    Services,
    /// The open tabs, one item each (built when the Window menu opens).
    TabList,
    /// The first level of the bookmarks bar folder (built when the Bookmarks menu opens, from
    /// what the bookmarks lane provides through `seams::bookmarks_bar_items`).
    BookmarksBarFolder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Menu {
    pub(crate) title: &'static str,
    pub(crate) items: Vec<MenuItem>,
}

const fn action(title: &'static str, key: Option<KeyEquivalent>, command: MenuCommand) -> MenuItem {
    MenuItem::Action {
        title,
        key,
        command,
    }
}

const fn key(key: char, modifiers: Modifiers) -> Option<KeyEquivalent> {
    Some(KeyEquivalent::new(key, modifiers))
}

/// The macOS menu bar, top to bottom. The first menu is the application menu.
pub(crate) fn menu_bar() -> Vec<Menu> {
    use MenuCommand::*;
    use MenuItem::{BookmarksBarFolder, Separator, Services, TabList};
    use ShortcutAction as A;

    vec![
        Menu {
            title: "Mercury Browser",
            items: vec![
                action("About Mercury Browser", None, About),
                Separator,
                action("Settings…", key(',', CMD), Shortcut(A::ShowSettings)),
                Separator,
                Services,
                Separator,
                action(
                    "Hide Mercury Browser",
                    key('h', CMD),
                    System(SystemAction::Hide),
                ),
                action(
                    "Hide Others",
                    key('h', OPTION_CMD),
                    System(SystemAction::HideOthers),
                ),
                action("Show All", None, System(SystemAction::ShowAll)),
                Separator,
                action("Quit Mercury Browser", key('q', CMD), Shortcut(A::Quit)),
            ],
        },
        Menu {
            title: "File",
            items: vec![
                action("New Tab", key('t', CMD), Shortcut(A::NewTab)),
                action(
                    "Open Location…",
                    key('l', CMD),
                    Shortcut(A::FocusAddressBar),
                ),
                Separator,
                action("Close Tab", key('w', CMD), Shortcut(A::CloseTab)),
                action("Reopen Closed Tab", key('t', SHIFT_CMD), ReopenClosedTab),
            ],
        },
        Menu {
            title: "Edit",
            items: vec![
                action("Undo", key('z', CMD), Edit(EditCommand::Undo)),
                action("Redo", key('z', SHIFT_CMD), Edit(EditCommand::Redo)),
                Separator,
                action("Cut", key('x', CMD), Edit(EditCommand::Cut)),
                action("Copy", key('c', CMD), Edit(EditCommand::Copy)),
                action("Paste", key('v', CMD), Edit(EditCommand::Paste)),
                action("Select All", key('a', CMD), Edit(EditCommand::SelectAll)),
                Separator,
                // Servo 0.8.0 has no find-in-page API.
                action("Find…", key('f', CMD), Find),
            ],
        },
        Menu {
            title: "View",
            items: vec![
                action("Reload Page", key('r', CMD), Shortcut(A::Reload)),
                // Servo 0.8.0's `WebView` cannot stop a load.
                action("Stop", None, Stop),
                Separator,
                action("Actual Size", key('0', CMD), Shortcut(A::ZoomReset)),
                action("Zoom In", key('=', CMD), Shortcut(A::ZoomIn)),
                action("Zoom Out", key('-', CMD), Shortcut(A::ZoomOut)),
                Separator,
                // AppKit retitles it "Exit Full Screen" while the window is full screen.
                action(
                    "Enter Full Screen",
                    key('f', CONTROL_CMD),
                    System(SystemAction::ToggleFullScreen),
                ),
            ],
        },
        Menu {
            title: "History",
            items: vec![
                action("Back", key('[', CMD), Shortcut(A::Back)),
                action("Forward", key(']', CMD), Shortcut(A::Forward)),
                Separator,
                // History lane: these two open its window and its dialog (see `seams.rs`).
                action("Show All History", key('y', CMD), ShowHistory),
                action(
                    "Clear Browsing Data…",
                    key(DELETE_KEY, SHIFT_CMD),
                    ClearBrowsingData,
                ),
                Separator,
                // Session lane.
                action("Restore Previous Session", None, RestorePreviousSession),
            ],
        },
        Menu {
            title: "Bookmarks",
            items: vec![
                // Bookmarks lane (see `seams.rs`).
                action("Bookmark This Page…", key('d', CMD), BookmarkThisPage),
                action(
                    "Show Bookmarks Bar",
                    key('b', SHIFT_CMD),
                    ToggleBookmarksBar,
                ),
                action("Bookmarks Manager", None, ShowBookmarksManager),
                Separator,
                BookmarksBarFolder,
            ],
        },
        Menu {
            title: "Window",
            items: vec![
                action("Minimize", key('m', CMD), System(SystemAction::Minimize)),
                action("Zoom", None, System(SystemAction::Zoom)),
                Separator,
                action("Show Next Tab", key(']', SHIFT_CMD), Shortcut(A::NextTab)),
                action(
                    "Show Previous Tab",
                    key('[', SHIFT_CMD),
                    Shortcut(A::PreviousTab),
                ),
                Separator,
                TabList,
            ],
        },
        Menu {
            title: "Help",
            items: vec![action(
                "Mercury Browser Help",
                None,
                OpenDocument(Document::Help),
            )],
        },
    ]
}

/// The key shown next to a tab in the Window menu: Cmd+1 to Cmd+8 select the first eight tabs.
pub(crate) fn tab_key(index: usize) -> Option<KeyEquivalent> {
    let digit = char::from_digit(u32::try_from(index + 1).ok()?, 10)?;
    (index < 8).then(|| KeyEquivalent::new(digit, CMD))
}

/// Whether the action behind a command exists in this build.
pub(crate) const fn is_available(command: MenuCommand) -> bool {
    use MenuCommand::*;
    match command {
        Shortcut(_) | About | Edit(_) | OpenDocument(_) | System(_) => true,
        ReopenClosedTab => super::seams::REOPEN_CLOSED_TAB_AVAILABLE,
        RestorePreviousSession => super::seams::RESTORE_PREVIOUS_SESSION_AVAILABLE,
        ShowHistory => super::seams::SHOW_HISTORY_AVAILABLE,
        ClearBrowsingData => super::seams::CLEAR_BROWSING_DATA_AVAILABLE,
        BookmarkThisPage | ToggleBookmarksBar | ShowBookmarksManager | OpenBookmark(_) => {
            super::seams::BOOKMARKS_AVAILABLE
        }
        Find | Stop => false,
    }
}

/// Where an Edit menu click goes, decided the way the keys go today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditTarget {
    /// The address bar's `TextInput`: Slint's own undo, redo, cut, copy, paste, select all.
    AddressBar,
    /// The page: Servo's clipboard editing (no engine API for undo, redo or select all).
    Page,
    /// Nothing that a menu click can reach (another window, the new-tab search field, nothing).
    None,
}

pub(crate) const fn edit_target(
    browser_window_focused: bool,
    address_bar_focused: bool,
    web_view_focused: bool,
) -> EditTarget {
    if !browser_window_focused {
        EditTarget::None
    } else if address_bar_focused {
        EditTarget::AddressBar
    } else if web_view_focused {
        EditTarget::Page
    } else {
        EditTarget::None
    }
}

/// What the menus show about the browser, copied from the controller whenever the UI syncs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MenuSnapshot {
    pub(crate) can_go_back: bool,
    pub(crate) can_go_forward: bool,
    pub(crate) tab_titles: Vec<String>,
    pub(crate) active_tab_index: usize,
    /// The bookmarks bar's first level, in order: each entry's stable id and title (bookmarks
    /// lane).
    pub(crate) bookmarks_bar: Vec<(super::seams::BookmarkId, String)>,
}

pub(crate) fn is_enabled(command: MenuCommand, snapshot: &MenuSnapshot, edit: EditTarget) -> bool {
    use MenuCommand::*;
    if !is_available(command) {
        return false;
    }
    match command {
        Shortcut(ShortcutAction::Back) => snapshot.can_go_back,
        Shortcut(ShortcutAction::Forward) => snapshot.can_go_forward,
        Shortcut(ShortcutAction::NextTab | ShortcutAction::PreviousTab) => {
            snapshot.tab_titles.len() > 1
        }
        Edit(command) => match edit {
            EditTarget::AddressBar => true,
            EditTarget::Page => {
                matches!(
                    command,
                    EditCommand::Cut | EditCommand::Copy | EditCommand::Paste
                )
            }
            EditTarget::None => false,
        },
        _ => true,
    }
}

/// The browser menu's commands, matching `BrowserMenuCommand` in `browser-menu.slint`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowserMenuItem {
    NewTab,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Find,
    Settings,
    About,
    Help,
    Quit,
}

impl BrowserMenuItem {
    pub(crate) const fn command(self) -> MenuCommand {
        match self {
            Self::NewTab => MenuCommand::Shortcut(ShortcutAction::NewTab),
            Self::ZoomIn => MenuCommand::Shortcut(ShortcutAction::ZoomIn),
            Self::ZoomOut => MenuCommand::Shortcut(ShortcutAction::ZoomOut),
            Self::ZoomReset => MenuCommand::Shortcut(ShortcutAction::ZoomReset),
            Self::Find => MenuCommand::Find,
            Self::Settings => MenuCommand::Shortcut(ShortcutAction::ShowSettings),
            Self::About => MenuCommand::About,
            Self::Help => MenuCommand::OpenDocument(Document::Help),
            Self::Quit => MenuCommand::Shortcut(ShortcutAction::Quit),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BrowserMenuRow {
    pub(crate) title: &'static str,
    pub(crate) shortcut: String,
    pub(crate) enabled: bool,
    pub(crate) item: BrowserMenuItem,
    pub(crate) separator_before: bool,
}

/// The browser menu's rows above and below its zoom row.
pub(crate) fn browser_menu(platform: Platform) -> (Vec<BrowserMenuRow>, Vec<BrowserMenuRow>) {
    let row = |title, item: BrowserMenuItem, separator_before| BrowserMenuRow {
        title,
        shortcut: shortcut_label(item.command(), platform),
        enabled: is_available(item.command()),
        item,
        separator_before,
    };
    (
        vec![row("New Tab", BrowserMenuItem::NewTab, false)],
        vec![
            row("Find…", BrowserMenuItem::Find, false),
            row("Settings…", BrowserMenuItem::Settings, true),
            row("About Mercury Browser", BrowserMenuItem::About, false),
            row("Mercury Browser Help", BrowserMenuItem::Help, false),
            row("Quit Mercury Browser", BrowserMenuItem::Quit, true),
        ],
    )
}

/// The key for a command as the browser menu shows it: only keys that actually run the
/// command on this platform. On macOS that is the menu bar's key equivalent while the action
/// exists; elsewhere it is a `shortcuts.rs` key with Ctrl, written "Ctrl+T".
pub(crate) fn shortcut_label(command: MenuCommand, platform: Platform) -> String {
    if !is_available(command) {
        return String::new();
    }
    let Some(key) = menu_bar()
        .iter()
        .flat_map(|menu| menu.items.iter())
        .find_map(|item| match *item {
            MenuItem::Action {
                command: item_command,
                key,
                ..
            } if item_command == command => key,
            _ => None,
        })
    else {
        return String::new();
    };

    match platform {
        Platform::MacOs => key.mac_label(),
        Platform::Other => {
            let MenuCommand::Shortcut(action) = command else {
                return String::new();
            };
            let character = key.key.to_string();
            let modifiers = Modifiers {
                command: false,
                control: true,
                ..key.modifiers
            };
            if shortcut_action(
                &Key::Character(character.as_str().into()),
                modifiers.state(),
                platform,
            ) != Some(action)
            {
                return String::new();
            }
            let mut label = String::from("Ctrl+");
            if modifiers.shift {
                label.push_str("Shift+");
            }
            label.extend(key.key.to_uppercase());
            label
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions() -> Vec<(&'static str, Option<KeyEquivalent>, MenuCommand)> {
        menu_bar()
            .into_iter()
            .flat_map(|menu| menu.items)
            .filter_map(|item| match item {
                MenuItem::Action {
                    title,
                    key,
                    command,
                } => Some((title, key, command)),
                _ => None,
            })
            .collect()
    }

    fn winit_key(key: KeyEquivalent) -> Key {
        match key.key {
            DELETE_KEY => Key::Named(NamedKey::Backspace),
            character => Key::Character(character.to_string().into()),
        }
    }

    /// The table test: every item that shows a key runs the action that key runs.
    #[test]
    fn every_shortcut_item_shows_the_key_that_runs_its_action() {
        for (title, key, command) in actions() {
            let Some(key) = key else { continue };
            let from_keyboard =
                shortcut_action(&winit_key(key), key.modifiers.state(), Platform::MacOs);
            match command {
                MenuCommand::Shortcut(action) => {
                    assert_eq!(from_keyboard, Some(action), "{title}");
                    assert_eq!(key.owner(), KeyOwner::Window, "{title}");
                }
                MenuCommand::Edit(_) => {
                    // The address bar and the page handle these keys themselves.
                    assert_eq!(from_keyboard, None, "{title}");
                    assert_eq!(key.owner(), KeyOwner::Window, "{title}");
                }
                _ => {
                    // Nothing on the window's key path handles the key: the menu does.
                    assert_eq!(from_keyboard, None, "{title}");
                    assert_eq!(key.owner(), KeyOwner::Menu, "{title}");
                }
            }
        }
    }

    /// Runs once: a key is either declined by the menu and run by the window, or run by the
    /// menu and unknown to the window, never both.
    #[test]
    fn each_key_has_exactly_one_owner() {
        for (title, key, _) in actions() {
            let Some(key) = key else { continue };
            let window_runs_it = window_owns_key(&winit_key(key), key.modifiers);
            let menu_runs_it = key.owner() == KeyOwner::Menu;
            assert!(window_runs_it != menu_runs_it, "{title}");
        }
        for index in 0..8 {
            let key = tab_key(index).expect("the first eight tabs have keys");
            assert_eq!(
                shortcut_action(&winit_key(key), key.modifiers.state(), Platform::MacOs),
                Some(ShortcutAction::SelectTab(index))
            );
            assert_eq!(key.owner(), KeyOwner::Window);
        }
        assert_eq!(tab_key(8), None);
    }

    #[test]
    fn no_two_items_share_a_key() {
        let keys = actions()
            .into_iter()
            .filter_map(|(_, key, _)| key)
            .collect::<Vec<_>>();
        for (index, key) in keys.iter().enumerate() {
            assert!(!keys[index + 1..].contains(key), "{key:?}");
        }
    }

    #[test]
    fn the_editing_keys_belong_to_the_window() {
        let key = |text: &str| Key::Character(text.into());
        assert!(window_owns_key(&key("c"), CMD));
        assert!(window_owns_key(&key("x"), CMD));
        assert!(window_owns_key(&key("v"), CMD));
        assert!(window_owns_key(&key("a"), CMD));
        assert!(window_owns_key(&key("z"), CMD));
        assert!(window_owns_key(&key("Z"), SHIFT_CMD));
        assert!(!window_owns_key(&key("c"), CONTROL_CMD));
        assert!(!window_owns_key(&key("v"), SHIFT_CMD));
        assert!(!window_owns_key(&key("c"), Modifiers::default()));
    }

    #[test]
    fn keys_only_the_menu_handles() {
        let menu_keys = actions()
            .into_iter()
            .filter_map(|(title, key, _)| {
                key.filter(|key| key.owner() == KeyOwner::Menu)
                    .map(|key| (title, key.mac_label()))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            menu_keys,
            vec![
                ("Hide Mercury Browser", "⌘H".to_owned()),
                ("Hide Others", "⌥⌘H".to_owned()),
                ("Reopen Closed Tab", "⇧⌘T".to_owned()),
                ("Find…", "⌘F".to_owned()),
                ("Enter Full Screen", "⌃⌘F".to_owned()),
                ("Show All History", "⌘Y".to_owned()),
                ("Clear Browsing Data…", "⇧⌘⌫".to_owned()),
                ("Bookmark This Page…", "⌘D".to_owned()),
                ("Show Bookmarks Bar", "⇧⌘B".to_owned()),
                ("Minimize", "⌘M".to_owned()),
            ]
        );
    }

    #[test]
    fn every_menu_has_the_expected_title_and_the_app_menu_comes_first() {
        let titles = menu_bar().iter().map(|menu| menu.title).collect::<Vec<_>>();
        assert_eq!(
            titles,
            [
                "Mercury Browser",
                "File",
                "Edit",
                "View",
                "History",
                "Bookmarks",
                "Window",
                "Help"
            ]
        );
    }

    #[test]
    fn actions_that_do_not_exist_yet_are_disabled() {
        let snapshot = MenuSnapshot::default();
        for command in [
            MenuCommand::ReopenClosedTab,
            MenuCommand::RestorePreviousSession,
            MenuCommand::ShowHistory,
            MenuCommand::ClearBrowsingData,
            MenuCommand::BookmarkThisPage,
            MenuCommand::ToggleBookmarksBar,
            MenuCommand::ShowBookmarksManager,
            MenuCommand::OpenBookmark(crate::menus::seams::BookmarkId::default()),
            MenuCommand::Find,
            MenuCommand::Stop,
        ] {
            assert!(
                !is_enabled(command, &snapshot, EditTarget::AddressBar),
                "{command:?}"
            );
        }
        assert!(is_enabled(
            MenuCommand::Shortcut(ShortcutAction::NewTab),
            &snapshot,
            EditTarget::None
        ));
    }

    #[test]
    fn history_and_tab_items_follow_the_browser_state() {
        let mut snapshot = MenuSnapshot {
            tab_titles: vec!["A".to_owned()],
            ..MenuSnapshot::default()
        };
        let enabled = |snapshot: &MenuSnapshot, action| {
            is_enabled(MenuCommand::Shortcut(action), snapshot, EditTarget::None)
        };
        assert!(!enabled(&snapshot, ShortcutAction::Back));
        assert!(!enabled(&snapshot, ShortcutAction::Forward));
        assert!(!enabled(&snapshot, ShortcutAction::NextTab));

        snapshot.can_go_back = true;
        snapshot.can_go_forward = true;
        snapshot.tab_titles.push("B".to_owned());
        assert!(enabled(&snapshot, ShortcutAction::Back));
        assert!(enabled(&snapshot, ShortcutAction::Forward));
        assert!(enabled(&snapshot, ShortcutAction::NextTab));
        assert!(enabled(&snapshot, ShortcutAction::PreviousTab));
    }

    #[test]
    fn edit_items_are_enabled_where_a_click_can_reach() {
        let snapshot = MenuSnapshot::default();
        let all = [
            EditCommand::Undo,
            EditCommand::Redo,
            EditCommand::Cut,
            EditCommand::Copy,
            EditCommand::Paste,
            EditCommand::SelectAll,
        ];
        let enabled = |target| {
            all.into_iter()
                .filter(|command| is_enabled(MenuCommand::Edit(*command), &snapshot, target))
                .collect::<Vec<_>>()
        };
        assert_eq!(enabled(EditTarget::AddressBar), all);
        assert_eq!(
            enabled(EditTarget::Page),
            [EditCommand::Cut, EditCommand::Copy, EditCommand::Paste]
        );
        assert_eq!(enabled(EditTarget::None), []);
    }

    #[test]
    fn the_edit_target_follows_keyboard_focus() {
        assert_eq!(edit_target(true, true, false), EditTarget::AddressBar);
        assert_eq!(edit_target(true, false, true), EditTarget::Page);
        assert_eq!(edit_target(true, false, false), EditTarget::None);
        assert_eq!(edit_target(false, true, true), EditTarget::None);
    }

    #[test]
    fn browser_menu_items_run_the_same_commands_as_the_menu_bar() {
        let bar = actions();
        let (top, bottom) = browser_menu(Platform::MacOs);
        for row in top.iter().chain(&bottom) {
            assert!(
                bar.iter()
                    .any(|(_, _, command)| *command == row.item.command()),
                "{:?}",
                row.item
            );
            assert_eq!(
                row.enabled,
                is_available(row.item.command()),
                "{:?}",
                row.item
            );
        }
        for item in [
            BrowserMenuItem::ZoomIn,
            BrowserMenuItem::ZoomOut,
            BrowserMenuItem::ZoomReset,
        ] {
            assert!(bar.iter().any(|(_, _, command)| *command == item.command()));
        }
    }

    #[test]
    fn browser_menu_labels_are_the_keys_that_run_the_command() {
        let labels = |platform| {
            let (top, bottom) = browser_menu(platform);
            top.into_iter()
                .chain(bottom)
                .map(|row| (row.title, row.shortcut))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(Platform::MacOs),
            vec![
                ("New Tab", "⌘T".to_owned()),
                ("Find…", String::new()),
                ("Settings…", "⌘,".to_owned()),
                ("About Mercury Browser", String::new()),
                ("Mercury Browser Help", String::new()),
                ("Quit Mercury Browser", "⌘Q".to_owned()),
            ]
        );
        assert_eq!(
            labels(Platform::Other),
            vec![
                ("New Tab", "Ctrl+T".to_owned()),
                ("Find…", String::new()),
                ("Settings…", "Ctrl+,".to_owned()),
                ("About Mercury Browser", String::new()),
                ("Mercury Browser Help", String::new()),
                ("Quit Mercury Browser", "Ctrl+Q".to_owned()),
            ]
        );
    }

    #[test]
    fn appkit_characters_become_the_keys_shortcuts_rs_matches() {
        assert_eq!(
            key_from_appkit_characters("\u{f702}"),
            Key::Named(NamedKey::ArrowLeft)
        );
        assert_eq!(key_from_appkit_characters("\t"), Key::Named(NamedKey::Tab));
        assert_eq!(
            key_from_appkit_characters("\u{19}"),
            Key::Named(NamedKey::Tab)
        );
        assert_eq!(
            key_from_appkit_characters("\u{7f}"),
            Key::Named(NamedKey::Backspace)
        );
        assert_eq!(key_from_appkit_characters("{"), Key::Character("{".into()));
        // Shift+Cmd+] reaches AppKit as "}" and is still Next Tab.
        assert!(window_owns_key(&key_from_appkit_characters("}"), SHIFT_CMD));
        assert!(window_owns_key(
            &key_from_appkit_characters("\t"),
            Modifiers {
                control: true,
                ..Modifiers::default()
            }
        ));
        assert!(window_owns_key(
            &key_from_appkit_characters("\u{f702}"),
            CMD
        ));
    }

    #[test]
    fn a_key_finds_the_item_that_shows_it() {
        use MenuCommand::*;
        let character = |text: &str| Key::Character(text.into());
        assert_eq!(
            item_for_key(&character("t"), CMD, 1),
            Some(Shortcut(ShortcutAction::NewTab))
        );
        assert_eq!(
            item_for_key(&character("H"), OPTION_CMD, 1),
            Some(System(SystemAction::HideOthers))
        );
        assert_eq!(
            item_for_key(&key_from_appkit_characters("\u{7f}"), SHIFT_CMD, 1),
            Some(ClearBrowsingData)
        );
        // AppKit reports Shift+Cmd+] as "}".
        assert_eq!(
            item_for_key(&character("}"), SHIFT_CMD, 1),
            Some(Shortcut(ShortcutAction::NextTab))
        );
        assert_eq!(
            item_for_key(&character("2"), CMD, 3),
            Some(Shortcut(ShortcutAction::SelectTab(1)))
        );
        assert_eq!(item_for_key(&character("5"), CMD, 3), None);
        assert_eq!(item_for_key(&character("k"), CMD, 1), None);
        assert_eq!(item_for_key(&character("h"), SHIFT_CMD, 1), None);
    }

    fn matched(command: MenuCommand, enabled: bool) -> Option<MatchedItem> {
        Some(MatchedItem { command, enabled })
    }

    /// The whole key-equivalent decision, as a table: (key, modifiers, key window, repeat,
    /// matched item) → decision.
    #[test]
    fn key_equivalent_decisions() {
        use KeyDecision::*;
        use KeyWindow::*;
        use MenuCommand::*;
        let character = |text: &str| Key::Character(text.into());
        let new_tab = matched(Shortcut(ShortcutAction::NewTab), true);
        let zoom_in = matched(Shortcut(ShortcutAction::ZoomIn), true);
        let quit = matched(Shortcut(ShortcutAction::Quit), true);
        let close = matched(Shortcut(ShortcutAction::CloseTab), true);
        let hide = matched(System(SystemAction::Hide), true);
        let find = matched(Find, false);
        let copy_disabled = matched(Edit(EditCommand::Copy), false);
        let undo_disabled = matched(Edit(EditCommand::Undo), false);

        let cases = [
            // The browser window owns its shortcuts and the editing keys, repeats included.
            ("t", CMD, Browser, false, new_tab, Decline),
            ("t", CMD, Browser, true, new_tab, Decline),
            ("c", CMD, Browser, false, copy_disabled, Decline),
            // Keys only the menu has: performed once, repeats swallowed.
            ("h", CMD, Browser, false, hide, Perform),
            ("h", CMD, Browser, true, hide, Swallow),
            // A disabled item: press, repeats and release all reach the window, as before.
            ("f", CMD, Browser, false, find, Decline),
            ("f", CMD, Browser, true, find, Decline),
            // Another Mercury window (About, settings): the menu performs the item once.
            ("t", CMD, OtherMercury, false, new_tab, Perform),
            ("t", CMD, OtherMercury, true, new_tab, Swallow),
            ("q", CMD, OtherMercury, true, quit, Swallow),
            ("w", CMD, OtherMercury, true, close, Swallow),
            // ... except actions that repeat on purpose.
            ("=", CMD, OtherMercury, true, zoom_in, Perform),
            // A native panel or sheet: the Edit items are AppKit's standard ones, repeats too.
            ("c", CMD, Native, false, copy_disabled, Perform),
            ("z", CMD, Native, true, undo_disabled, Perform),
            ("w", CMD, Native, false, close, Perform),
            ("w", CMD, Native, true, close, Swallow),
            ("t", CMD, Native, true, new_tab, Swallow),
            // No item of ours: AppKit's own items (Services, Emoji & Symbols) or nothing.
            ("k", CMD, Browser, false, Option::None, Perform),
            ("k", CMD, Browser, true, Option::None, Perform),
            ("t", CMD, KeyWindow::None, false, new_tab, Perform),
            // A disabled Edit item with the About window key: the key goes to that window.
            ("c", CMD, OtherMercury, false, copy_disabled, Decline),
            // A disabled item that is not an Edit item stays disabled in a native panel.
            ("f", CMD, Native, false, find, Decline),
            // Repeats of keys no item of ours shows: AppKit as usual.
            ("k", CMD, Native, true, Option::None, Perform),
            ("k", CMD, OtherMercury, true, Option::None, Perform),
        ];
        for (text, modifiers, key_window, repeat, item, expected) in cases {
            assert_eq!(
                decide_key_equivalent(&character(text), modifiers, key_window, repeat, item),
                expected,
                "{text} {modifiers:?} {key_window:?} repeat={repeat} {item:?}"
            );
        }
    }

    #[test]
    fn edit_and_close_items_become_standard_in_native_panels() {
        use MenuCommand::*;
        for key_window in [KeyWindow::Browser, KeyWindow::OtherMercury, KeyWindow::None] {
            assert!(!uses_responder_chain(Edit(EditCommand::Copy), key_window));
            assert!(!uses_responder_chain(
                Shortcut(ShortcutAction::CloseTab),
                key_window
            ));
        }
        let native = |command| {
            uses_responder_chain(command, KeyWindow::Native).then(|| standard_action(command))
        };
        assert_eq!(
            native(Edit(EditCommand::Undo)),
            Some(Some(StandardAction::Undo))
        );
        assert_eq!(
            native(Edit(EditCommand::SelectAll)),
            Some(Some(StandardAction::SelectAll))
        );
        assert_eq!(
            native(Shortcut(ShortcutAction::CloseTab)),
            Some(Some(StandardAction::PerformClose))
        );
        assert_eq!(native(Shortcut(ShortcutAction::NewTab)), None);
        assert_eq!(native(About), None);
    }

    #[test]
    fn mac_labels_use_apples_modifier_order() {
        assert_eq!(KeyEquivalent::new('f', CONTROL_CMD).mac_label(), "⌃⌘F");
        assert_eq!(KeyEquivalent::new('t', SHIFT_CMD).mac_label(), "⇧⌘T");
        assert_eq!(KeyEquivalent::new(DELETE_KEY, SHIFT_CMD).mac_label(), "⇧⌘⌫");
    }
}
