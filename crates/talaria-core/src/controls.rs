//! Form controls and menus the engine asks the browser to show: `<select>` popups, colour and
//! file pickers, the page context menu and the input-method area. Each request carries an id;
//! the browser answers it exactly once, or drops it, which gives the engine's default answer.

use std::path::PathBuf;

/// Identifies one engine request; ids are never reused within a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlId(u64);

impl ControlId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// The element (or, for a context menu, the point) a control belongs to, as Servo 0.8.0
/// reports it: CSS pixels relative to the page's viewport. A point has zero size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOption {
    /// What the engine uses to select the option.
    pub id: usize,
    pub label: String,
    pub disabled: bool,
}

/// Options outside an option group come as single entries, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectEntry {
    Option(SelectOption),
    Group {
        label: String,
        options: Vec<SelectOption>,
    },
}

/// The context-menu actions the engine carries out itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuAction {
    GoBack,
    GoForward,
    Reload,
    CopyLink,
    OpenLinkInNewView,
    CopyImageLink,
    OpenImageInNewView,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuEntry {
    Item {
        label: String,
        action: ContextMenuAction,
        enabled: bool,
    },
    Separator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlRequest {
    Select {
        rect: ControlRect,
        entries: Vec<SelectEntry>,
        /// Ids of the selected options.
        selected: Vec<usize>,
        multiple: bool,
    },
    Color {
        rect: ControlRect,
        current: Option<Rgb>,
    },
    File {
        multiple: bool,
        /// File-name extensions without the dot; empty accepts every file.
        extensions: Vec<String>,
    },
    ContextMenu {
        rect: ControlRect,
        entries: Vec<ContextMenuEntry>,
        link_url: Option<String>,
        image_url: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlAnswer {
    /// Ids of the options to select.
    Select(Vec<usize>),
    Color(Rgb),
    /// Only files the user chose. An empty list leaves the input unchanged.
    Files(Vec<PathBuf>),
    ContextMenu(ContextMenuAction),
    /// Closes the control with the engine's default answer: the unchanged selection or colour,
    /// no files, no menu action.
    Dismiss,
}
