//! The page context menu: Servo's items with the browser's own link and image items.
//!
//! Servo builds the list (`document_embedder_controls.rs`, `show_context_menu`: link items,
//! image items, Cut/Copy/Paste/Select All, Back/Forward/Reload) and carries out each of its
//! actions itself. Its "Open … in New View" items would ask for a new web view
//! (`request_create_new`); here they become "Open … in new tab" through the browser's own new
//! tab path instead, and its copy items keep Servo's action under the browser's wording.

use talaria_core::{ContextMenuAction, ContextMenuEntry};

/// Row heights in logical pixels; `ui/controls/context-menu.slint` uses the same values.
pub(crate) const ITEM_HEIGHT: f32 = 26.0;
pub(crate) const SEPARATOR_HEIGHT: f32 = 9.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuCommand {
    /// Answered to the engine, which carries the action out.
    Engine(ContextMenuAction),
    /// Opened by the browser in a background tab; the engine's menu is dismissed.
    OpenInNewTab(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuRow {
    Item {
        label: String,
        enabled: bool,
        command: MenuCommand,
    },
    Separator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuKey {
    Up,
    Down,
    Home,
    End,
    Return,
    Escape,
}

/// What closing the menu does: `None` dismisses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuOutcome {
    Stay,
    Close(Option<MenuCommand>),
}

/// `page_url` is the address of the page the menu belongs to.
pub(crate) fn build_menu(
    entries: &[ContextMenuEntry],
    link_url: Option<&str>,
    image_url: Option<&str>,
    page_url: &str,
) -> Vec<MenuRow> {
    let link_url = link_url.map(|url| (url, url_use(url, page_url)));
    let image_url = image_url.map(|url| (url, url_use(url, page_url)));
    let mut rows = Vec::new();
    for entry in entries {
        let row = match entry {
            ContextMenuEntry::Separator => Some(MenuRow::Separator),
            ContextMenuEntry::Item {
                action, enabled, ..
            } => browser_item(*action, *enabled, link_url, image_url),
        };
        // Downloads: "Save link as…" goes right after "Copy link address" (the CopyLink row) and
        // "Save image as…" right after "Copy image address" (the CopyImageLink row).
        let Some(row) = row else {
            continue;
        };
        let after_separator = matches!(rows.last(), None | Some(MenuRow::Separator));
        if row == MenuRow::Separator && after_separator {
            continue;
        }
        rows.push(row);
    }
    if rows.last() == Some(&MenuRow::Separator) {
        rows.pop();
    }
    rows
}

/// What the browser may do with a link or image address from a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UrlUse {
    open: bool,
    copy: bool,
}

/// `javascript:` runs in whatever page it is used in and `blob:` means nothing outside its page,
/// so neither is opened or copied; `data:` may be copied but not opened. Other addresses open in
/// a new tab if they are `http(s)` or have the page's own scheme (a web page cannot open `file:`).
fn url_use(url: &str, page_url: &str) -> UrlUse {
    let scheme = |url: &str| {
        url.split_once(':')
            .map(|(scheme, _)| scheme.to_ascii_lowercase())
            .unwrap_or_default()
    };
    let link_scheme = scheme(url.trim_start());
    match link_scheme.as_str() {
        "javascript" | "blob" => UrlUse {
            open: false,
            copy: false,
        },
        "data" => UrlUse {
            open: false,
            copy: true,
        },
        "http" | "https" => UrlUse {
            open: true,
            copy: true,
        },
        other => UrlUse {
            open: !other.is_empty() && other == scheme(page_url),
            copy: true,
        },
    }
}

/// The browser's wording (sentence case) and handling for one of Servo's items; Servo's own
/// labels are in title case.
fn browser_item(
    action: ContextMenuAction,
    enabled: bool,
    link_url: Option<(&str, UrlUse)>,
    image_url: Option<(&str, UrlUse)>,
) -> Option<MenuRow> {
    let open_in_new_tab = |label: &str, url: Option<(&str, UrlUse)>| {
        url.filter(|(_, url_use)| url_use.open)
            .map(|(url, _)| MenuRow::Item {
                label: label.to_owned(),
                enabled,
                command: MenuCommand::OpenInNewTab(url.to_owned()),
            })
    };
    let can_copy = |url: Option<(&str, UrlUse)>| url.is_none_or(|(_, url_use)| url_use.copy);
    let label = match action {
        ContextMenuAction::OpenLinkInNewView => {
            return open_in_new_tab("Open link in new tab", link_url);
        }
        ContextMenuAction::OpenImageInNewView => {
            return open_in_new_tab("Open image in new tab", image_url);
        }
        ContextMenuAction::CopyLink if !can_copy(link_url) => return None,
        ContextMenuAction::CopyImageLink if !can_copy(image_url) => return None,
        ContextMenuAction::CopyLink => "Copy link address",
        ContextMenuAction::CopyImageLink => "Copy image address",
        ContextMenuAction::Cut => "Cut",
        ContextMenuAction::Copy => "Copy",
        ContextMenuAction::Paste => "Paste",
        ContextMenuAction::SelectAll => "Select all",
        ContextMenuAction::GoBack => "Back",
        ContextMenuAction::GoForward => "Forward",
        ContextMenuAction::Reload => "Reload",
    };
    Some(MenuRow::Item {
        label: label.to_owned(),
        enabled,
        command: MenuCommand::Engine(action),
    })
}

#[derive(Debug)]
pub(crate) struct MenuModel {
    rows: Vec<MenuRow>,
    highlighted: Option<usize>,
}

impl MenuModel {
    pub(crate) fn new(rows: Vec<MenuRow>) -> Self {
        Self {
            rows,
            highlighted: None,
        }
    }

    pub(crate) fn rows(&self) -> &[MenuRow] {
        &self.rows
    }

    pub(crate) fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    pub(crate) fn key(&mut self, key: MenuKey) -> MenuOutcome {
        let enabled = (0..self.rows.len())
            .filter(|&row| self.is_enabled(row))
            .collect::<Vec<_>>();
        match key {
            MenuKey::Down => {
                self.highlighted = match self.highlighted {
                    Some(current) => enabled
                        .iter()
                        .copied()
                        .find(|&row| row > current)
                        .or(Some(current)),
                    None => enabled.first().copied(),
                };
            }
            MenuKey::Up => {
                self.highlighted = match self.highlighted {
                    Some(current) => enabled
                        .iter()
                        .rev()
                        .copied()
                        .find(|&row| row < current)
                        .or(Some(current)),
                    None => enabled.last().copied(),
                };
            }
            MenuKey::Home => self.highlighted = enabled.first().copied(),
            MenuKey::End => self.highlighted = enabled.last().copied(),
            MenuKey::Return => {
                if let Some(row) = self.highlighted {
                    return self.click(row);
                }
            }
            MenuKey::Escape => return MenuOutcome::Close(None),
        }
        MenuOutcome::Stay
    }

    pub(crate) fn click(&mut self, row: usize) -> MenuOutcome {
        match self.rows.get(row) {
            Some(MenuRow::Item {
                enabled: true,
                command,
                ..
            }) => MenuOutcome::Close(Some(command.clone())),
            _ => MenuOutcome::Stay,
        }
    }

    pub(crate) fn hover(&mut self, row: usize) {
        self.highlighted = self.is_enabled(row).then_some(row);
    }

    pub(crate) fn row_height(&self, row: usize) -> f32 {
        match self.rows.get(row) {
            Some(MenuRow::Separator) => SEPARATOR_HEIGHT,
            Some(MenuRow::Item { .. }) => ITEM_HEIGHT,
            None => 0.0,
        }
    }

    pub(crate) fn content_height(&self) -> f32 {
        (0..self.rows.len()).map(|row| self.row_height(row)).sum()
    }

    fn is_enabled(&self, row: usize) -> bool {
        matches!(
            self.rows.get(row),
            Some(MenuRow::Item { enabled: true, .. })
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://a.test/here";

    fn item(label: &str, action: ContextMenuAction, enabled: bool) -> ContextMenuEntry {
        ContextMenuEntry::Item {
            label: label.to_owned(),
            action,
            enabled,
        }
    }

    /// Servo's list for an `<img>` inside an `<a>`, with nothing selected, on a page with history.
    fn servo_items_for_linked_image() -> Vec<ContextMenuEntry> {
        vec![
            item(
                "Open Link in New View",
                ContextMenuAction::OpenLinkInNewView,
                true,
            ),
            item("Copy Link", ContextMenuAction::CopyLink, true),
            ContextMenuEntry::Separator,
            item(
                "Open Image in New View",
                ContextMenuAction::OpenImageInNewView,
                true,
            ),
            item("Copy Image Link", ContextMenuAction::CopyImageLink, true),
            ContextMenuEntry::Separator,
            item("Cut", ContextMenuAction::Cut, false),
            item("Copy", ContextMenuAction::Copy, false),
            item("Paste", ContextMenuAction::Paste, false),
            item("Select All", ContextMenuAction::SelectAll, true),
            ContextMenuEntry::Separator,
            item("Back", ContextMenuAction::GoBack, true),
            item("Forward", ContextMenuAction::GoForward, false),
            item("Reload", ContextMenuAction::Reload, true),
        ]
    }

    fn labels(rows: &[MenuRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                MenuRow::Item { label, .. } => label.clone(),
                MenuRow::Separator => "—".to_owned(),
            })
            .collect()
    }

    fn engine(label: &str, action: ContextMenuAction, enabled: bool) -> MenuRow {
        MenuRow::Item {
            label: label.to_owned(),
            enabled,
            command: MenuCommand::Engine(action),
        }
    }

    #[test]
    fn a_link_gets_the_browsers_link_items() {
        let entries = vec![
            item(
                "Open Link in New View",
                ContextMenuAction::OpenLinkInNewView,
                true,
            ),
            item("Copy Link", ContextMenuAction::CopyLink, true),
            ContextMenuEntry::Separator,
            item("Reload", ContextMenuAction::Reload, true),
        ];

        let rows = build_menu(&entries, Some("https://a.test/next"), None, PAGE);

        assert_eq!(
            rows,
            vec![
                MenuRow::Item {
                    label: "Open link in new tab".to_owned(),
                    enabled: true,
                    command: MenuCommand::OpenInNewTab("https://a.test/next".to_owned()),
                },
                engine("Copy link address", ContextMenuAction::CopyLink, true),
                MenuRow::Separator,
                engine("Reload", ContextMenuAction::Reload, true),
            ]
        );
    }

    #[test]
    fn an_image_in_a_link_gets_link_and_image_items_then_servos_items() {
        let rows = build_menu(
            &servo_items_for_linked_image(),
            Some("https://a.test/page"),
            Some("https://a.test/i.png"),
            PAGE,
        );

        assert_eq!(
            labels(&rows),
            vec![
                "Open link in new tab",
                "Copy link address",
                "—",
                "Open image in new tab",
                "Copy image address",
                "—",
                "Cut",
                "Copy",
                "Paste",
                "Select all",
                "—",
                "Back",
                "Forward",
                "Reload",
            ]
        );
        assert_eq!(
            rows[3],
            MenuRow::Item {
                label: "Open image in new tab".to_owned(),
                enabled: true,
                command: MenuCommand::OpenInNewTab("https://a.test/i.png".to_owned()),
            }
        );
        assert_eq!(
            rows[4],
            engine("Copy image address", ContextMenuAction::CopyImageLink, true)
        );
        assert_eq!(rows[6], engine("Cut", ContextMenuAction::Cut, false));
        assert_eq!(
            rows[12],
            engine("Forward", ContextMenuAction::GoForward, false)
        );
    }

    #[test]
    fn opening_in_a_new_tab_is_left_out_without_a_url() {
        let entries = vec![
            item(
                "Open Link in New View",
                ContextMenuAction::OpenLinkInNewView,
                true,
            ),
            item("Copy Link", ContextMenuAction::CopyLink, true),
            ContextMenuEntry::Separator,
            item("Reload", ContextMenuAction::Reload, true),
        ];

        assert_eq!(
            labels(&build_menu(&entries, None, None, PAGE)),
            vec!["Copy link address", "—", "Reload"]
        );
    }

    #[test]
    fn separators_never_lead_trail_or_repeat() {
        let entries = vec![
            ContextMenuEntry::Separator,
            item(
                "Open Image in New View",
                ContextMenuAction::OpenImageInNewView,
                true,
            ),
            ContextMenuEntry::Separator,
            ContextMenuEntry::Separator,
            item("Reload", ContextMenuAction::Reload, true),
            ContextMenuEntry::Separator,
        ];

        assert_eq!(
            labels(&build_menu(&entries, None, None, PAGE)),
            vec!["Reload"]
        );
    }

    fn link_and_image_items() -> Vec<ContextMenuEntry> {
        servo_items_for_linked_image()[..6].to_vec()
    }

    #[test]
    fn script_links_get_no_open_or_copy_items() {
        let rows = build_menu(
            &link_and_image_items(),
            Some("javascript:alert(1)"),
            Some("https://a.test/i.png"),
            PAGE,
        );

        assert_eq!(
            labels(&rows),
            vec!["Open image in new tab", "Copy image address"]
        );
    }

    #[test]
    fn data_images_can_be_copied_but_not_opened_and_blob_images_neither() {
        let data = build_menu(
            &link_and_image_items(),
            None,
            Some("data:image/png;base64,AAAA"),
            PAGE,
        );
        let blob = build_menu(
            &link_and_image_items(),
            None,
            Some("blob:https://a.test/0b1c"),
            PAGE,
        );

        assert_eq!(
            labels(&data),
            vec!["Copy link address", "—", "Copy image address"]
        );
        assert_eq!(labels(&blob), vec!["Copy link address"]);
    }

    #[test]
    fn file_links_open_only_from_file_pages() {
        let from_web = build_menu(
            &link_and_image_items()[..2],
            Some("file:///etc/hosts"),
            None,
            PAGE,
        );
        let from_file = build_menu(
            &link_and_image_items()[..2],
            Some("file:///etc/hosts"),
            None,
            "file:///Users/me/index.html",
        );
        let web_from_file = build_menu(
            &link_and_image_items()[..2],
            Some("http://a.test/"),
            None,
            "file:///Users/me/index.html",
        );

        assert_eq!(labels(&from_web), vec!["Copy link address"]);
        assert_eq!(
            labels(&from_file),
            vec!["Open link in new tab", "Copy link address"]
        );
        assert_eq!(
            labels(&web_from_file),
            vec!["Open link in new tab", "Copy link address"]
        );
    }

    fn menu() -> MenuModel {
        MenuModel::new(build_menu(
            &servo_items_for_linked_image(),
            Some("https://a.test/page"),
            Some("https://a.test/i.png"),
            PAGE,
        ))
    }

    #[test]
    fn the_keyboard_moves_over_enabled_items_only() {
        let mut model = menu();
        assert_eq!(model.highlighted(), None);

        model.key(MenuKey::Down);
        assert_eq!(model.highlighted(), Some(0));
        model.key(MenuKey::Down);
        model.key(MenuKey::Down);
        assert_eq!(model.highlighted(), Some(3), "the separator is skipped");
        model.key(MenuKey::End);
        assert_eq!(model.highlighted(), Some(13));
        model.key(MenuKey::Up);
        assert_eq!(model.highlighted(), Some(11), "Forward is disabled");
        model.key(MenuKey::Home);
        assert_eq!(model.highlighted(), Some(0));
        model.key(MenuKey::Up);
        assert_eq!(model.highlighted(), Some(0));
    }

    #[test]
    fn up_from_nothing_highlights_the_last_item() {
        let mut model = menu();

        model.key(MenuKey::Up);

        assert_eq!(model.highlighted(), Some(13));
    }

    #[test]
    fn return_runs_the_highlighted_item_and_escape_dismisses() {
        let mut model = menu();
        assert_eq!(model.key(MenuKey::Return), MenuOutcome::Stay);
        model.key(MenuKey::End);

        assert_eq!(
            model.key(MenuKey::Return),
            MenuOutcome::Close(Some(MenuCommand::Engine(ContextMenuAction::Reload)))
        );
        assert_eq!(model.key(MenuKey::Escape), MenuOutcome::Close(None));
    }

    #[test]
    fn clicking_runs_enabled_items_only() {
        let mut model = menu();

        assert_eq!(model.click(2), MenuOutcome::Stay);
        assert_eq!(model.click(6), MenuOutcome::Stay);
        assert_eq!(
            model.click(0),
            MenuOutcome::Close(Some(MenuCommand::OpenInNewTab(
                "https://a.test/page".to_owned()
            )))
        );
    }

    #[test]
    fn hovering_highlights_enabled_items_and_clears_on_others() {
        let mut model = menu();

        model.hover(4);
        assert_eq!(model.highlighted(), Some(4));
        model.hover(6);
        assert_eq!(model.highlighted(), None);
    }

    #[test]
    fn the_height_adds_up_items_and_separators() {
        assert_eq!(
            menu().content_height(),
            11.0 * ITEM_HEIGHT + 3.0 * SEPARATOR_HEIGHT
        );
    }
}
