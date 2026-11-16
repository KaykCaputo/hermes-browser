//! Keeps the engine's form-control and context-menu requests until the browser answers them.
//!
//! Servo hands the delegate an owned request object that must be answered later; dropping it
//! gives the engine's default answer (`servo-0.8.0/webview_delegate.rs`: the unchanged
//! selection or colour, no files, no menu action). Only an id and plain data go to the UI.
//!
//! The kinds handled here are `SelectElement`, `ColorPicker`, `FilePicker`, `ContextMenu` and
//! `InputMethod`; the delegate sends `SimpleDialog` to the page requests instead.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use servo::{
    ContextMenuAction as ServoContextMenuAction, ContextMenuElementInformation, ContextMenuItem,
    DeviceIntRect, EmbedderControl, EmbedderControlId, FilterPattern, RgbColor,
    SelectElementOption, SelectElementOptionOrOptgroup,
};
use talaria_core::{
    BrowserEvent, ContextMenuAction, ContextMenuEntry, ControlAnswer, ControlId, ControlRect,
    ControlRequest, Rgb, SelectEntry, SelectOption,
};

/// Hands out control ids for the whole runtime, so an answer meant for a control of a closed
/// or replaced web view can never reach a control of another.
#[derive(Clone, Default)]
pub(crate) struct ControlIds(Rc<Cell<u64>>);

impl ControlIds {
    fn next(&self) -> ControlId {
        let value = self.0.get() + 1;
        self.0.set(value);
        ControlId::new(value)
    }
}

/// Entries keyed by the browser's id and by the engine's own key.
pub(crate) struct ControlStore<K, C> {
    entries: Vec<(ControlId, K, C)>,
}

impl<K, C> Default for ControlStore<K, C> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<K: PartialEq, C> ControlStore<K, C> {
    pub(crate) fn insert(&mut self, id: ControlId, key: K, control: C) {
        self.entries.push((id, key, control));
    }

    /// Removes the control so it can be answered; a second call for the same id finds nothing.
    pub(crate) fn take(&mut self, id: ControlId) -> Option<C> {
        let index = self
            .entries
            .iter()
            .position(|(entry_id, _, _)| *entry_id == id)?;
        Some(self.entries.remove(index).2)
    }

    pub(crate) fn take_by_key(&mut self, key: &K) -> Option<(ControlId, C)> {
        let index = self
            .entries
            .iter()
            .position(|(_, entry_key, _)| entry_key == key)?;
        let (id, _, control) = self.entries.remove(index);
        Some((id, control))
    }

    pub(crate) fn take_all(&mut self) -> Vec<(ControlId, C)> {
        self.entries
            .drain(..)
            .map(|(id, _, control)| (id, control))
            .collect()
    }
}

/// One web view's pending controls, shared by its delegate (which receives them) and its
/// `ServoWebView` (which answers them).
#[derive(Clone)]
pub(crate) struct EmbedderControls {
    ids: ControlIds,
    store: Rc<RefCell<ControlStore<EmbedderControlId, EmbedderControl>>>,
}

impl EmbedderControls {
    pub(crate) fn new(ids: ControlIds) -> Self {
        Self {
            ids,
            store: Rc::default(),
        }
    }

    /// Keeps a control of a kind handled here and returns the event that tells the UI about it.
    /// Simple dialogs belong to the page requests (`BrowserDelegate::show_embedder_control`
    /// sends them there); one that reaches this is dropped, which gives the engine's default.
    pub(crate) fn show(&self, control: EmbedderControl) -> Option<BrowserEvent> {
        let id = self.ids.next();
        let event = match &control {
            EmbedderControl::InputMethod(input_method) => BrowserEvent::InputMethodShown {
                id,
                rect: control_rect(input_method.position()),
            },
            _ => BrowserEvent::ControlShown {
                id,
                request: control_request(&control)?,
            },
        };
        let key = control.id();
        self.store.borrow_mut().insert(id, key, control);
        Some(event)
    }

    /// The engine closed a control itself. Returns the event for the UI if the control was
    /// one of these.
    pub(crate) fn hide(&self, key: EmbedderControlId) -> Option<BrowserEvent> {
        let (id, control) = self.store.borrow_mut().take_by_key(&key)?;
        // The engine no longer waits for an answer; dropping sends one it ignores.
        drop(control);
        Some(BrowserEvent::ControlHidden { id })
    }

    /// Answers a control once. An id that is no longer pending (already answered, hidden, or
    /// dropped by a navigation) is ignored.
    pub(crate) fn answer(&self, id: ControlId, answer: ControlAnswer) {
        let control = self.store.borrow_mut().take(id);
        match control {
            Some(control) => apply_answer(control, answer),
            None => log::debug!("ignoring an answer for control {}: not pending", id.value()),
        }
    }

    /// Drops every pending control with the engine's default answer and returns the events that
    /// close them in the UI. Used when the page goes away (navigation, crash, web view closed).
    pub(crate) fn drop_all(&self) -> Vec<BrowserEvent> {
        // Taken out first: dropping a control sends a message to the engine.
        let controls = self.store.borrow_mut().take_all();
        controls
            .into_iter()
            .map(|(id, control)| {
                drop(control);
                BrowserEvent::ControlHidden { id }
            })
            .collect()
    }
}

fn apply_answer(control: EmbedderControl, answer: ControlAnswer) {
    match (control, answer) {
        (EmbedderControl::SelectElement(mut select), ControlAnswer::Select(selected)) => {
            select.select(selected);
            select.submit();
        }
        (EmbedderControl::ColorPicker(mut picker), ControlAnswer::Color(colour)) => {
            picker.select(Some(RgbColor {
                red: colour.red,
                green: colour.green,
                blue: colour.blue,
            }));
            picker.submit();
        }
        (EmbedderControl::FilePicker(mut picker), ControlAnswer::Files(paths)) => {
            let paths = chosen_paths(paths);
            if paths.is_empty() {
                picker.dismiss();
            } else {
                picker.select(&paths);
                picker.submit();
            }
        }
        (EmbedderControl::FilePicker(picker), _) => picker.dismiss(),
        (EmbedderControl::ContextMenu(menu), ControlAnswer::ContextMenu(action)) => {
            menu.select(servo_context_menu_action(action));
        }
        (EmbedderControl::ContextMenu(menu), _) => menu.dismiss(),
        (control, ControlAnswer::Dismiss) => drop(control),
        (control, answer) => {
            log::warn!(
                "dropping control {:?}: the answer {answer:?} does not fit it",
                control.id()
            );
        }
    }
}

/// The plain-data request the UI shows, for the kinds that wait for an answer.
fn control_request(control: &EmbedderControl) -> Option<ControlRequest> {
    Some(match control {
        EmbedderControl::SelectElement(select) => ControlRequest::Select {
            rect: control_rect(select.position()),
            entries: select_entries(select.options()),
            selected: select.selected_options(),
            multiple: select.allow_select_multiple(),
        },
        EmbedderControl::ColorPicker(picker) => ControlRequest::Color {
            rect: control_rect(picker.position()),
            current: picker.current_color().map(|colour| Rgb {
                red: colour.red,
                green: colour.green,
                blue: colour.blue,
            }),
        },
        EmbedderControl::FilePicker(picker) => ControlRequest::File {
            multiple: picker.allow_select_multiple(),
            extensions: file_extensions(picker.filter_patterns()),
        },
        EmbedderControl::ContextMenu(menu) => {
            let (link_url, image_url) = element_urls(menu.element_info());
            ControlRequest::ContextMenu {
                rect: control_rect(menu.position()),
                entries: context_menu_entries(menu.items()),
                link_url,
                image_url,
            }
        }
        EmbedderControl::InputMethod(_) | EmbedderControl::SimpleDialog(_) => return None,
    })
}

/// Servo 0.8.0 documents `position()` as device pixels relative to the web view, but sends
/// CSS pixels relative to the frame's viewport (`document_embedder_controls.rs`, "FIXME: This
/// is a CSS pixel rect relative to this frame"); the numbers are passed on unchanged.
fn control_rect(rect: DeviceIntRect) -> ControlRect {
    ControlRect {
        x: rect.min.x,
        y: rect.min.y,
        width: rect.width(),
        height: rect.height(),
    }
}

/// The link and image under the pointer; an `<img>` inside an `<a>` has both.
fn element_urls(info: &ContextMenuElementInformation) -> (Option<String>, Option<String>) {
    (
        info.link_url.as_ref().map(ToString::to_string),
        info.image_url.as_ref().map(ToString::to_string),
    )
}

/// A label the page wrote, for showing in browser UI: control characters and line or paragraph
/// separators become spaces and
/// bidirectional formatting characters are removed, so a label cannot reorder or break the text
/// around it.
fn page_label(label: &str) -> String {
    label
        .chars()
        .filter(|character| !is_bidi_control(*character))
        .map(|character| {
            // U+2028 and U+2029 are line and paragraph separators, not control characters.
            if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// Unicode's explicit directional formatting characters (marks, embeddings, overrides,
/// isolates).
fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

fn select_option(option: &SelectElementOption) -> SelectOption {
    SelectOption {
        id: option.id,
        label: page_label(&option.label),
        disabled: option.is_disabled,
    }
}

fn select_entries(options: &[SelectElementOptionOrOptgroup]) -> Vec<SelectEntry> {
    options
        .iter()
        .map(|entry| match entry {
            SelectElementOptionOrOptgroup::Option(option) => {
                SelectEntry::Option(select_option(option))
            }
            SelectElementOptionOrOptgroup::Optgroup { label, options } => SelectEntry::Group {
                label: page_label(label),
                options: options.iter().map(select_option).collect(),
            },
        })
        .collect()
}

fn context_menu_entries(items: &[ContextMenuItem]) -> Vec<ContextMenuEntry> {
    items
        .iter()
        .map(|item| match item {
            ContextMenuItem::Item {
                label,
                action,
                enabled,
            } => ContextMenuEntry::Item {
                label: label.clone(),
                action: context_menu_action(*action),
                enabled: *enabled,
            },
            ContextMenuItem::Separator => ContextMenuEntry::Separator,
        })
        .collect()
}

fn context_menu_action(action: ServoContextMenuAction) -> ContextMenuAction {
    match action {
        ServoContextMenuAction::GoBack => ContextMenuAction::GoBack,
        ServoContextMenuAction::GoForward => ContextMenuAction::GoForward,
        ServoContextMenuAction::Reload => ContextMenuAction::Reload,
        ServoContextMenuAction::CopyLink => ContextMenuAction::CopyLink,
        ServoContextMenuAction::OpenLinkInNewWebView => ContextMenuAction::OpenLinkInNewView,
        ServoContextMenuAction::CopyImageLink => ContextMenuAction::CopyImageLink,
        ServoContextMenuAction::OpenImageInNewView => ContextMenuAction::OpenImageInNewView,
        ServoContextMenuAction::Cut => ContextMenuAction::Cut,
        ServoContextMenuAction::Copy => ContextMenuAction::Copy,
        ServoContextMenuAction::Paste => ContextMenuAction::Paste,
        ServoContextMenuAction::SelectAll => ContextMenuAction::SelectAll,
    }
}

fn servo_context_menu_action(action: ContextMenuAction) -> ServoContextMenuAction {
    match action {
        ContextMenuAction::GoBack => ServoContextMenuAction::GoBack,
        ContextMenuAction::GoForward => ServoContextMenuAction::GoForward,
        ContextMenuAction::Reload => ServoContextMenuAction::Reload,
        ContextMenuAction::CopyLink => ServoContextMenuAction::CopyLink,
        ContextMenuAction::OpenLinkInNewView => ServoContextMenuAction::OpenLinkInNewWebView,
        ContextMenuAction::CopyImageLink => ServoContextMenuAction::CopyImageLink,
        ContextMenuAction::OpenImageInNewView => ServoContextMenuAction::OpenImageInNewView,
        ContextMenuAction::Cut => ServoContextMenuAction::Cut,
        ContextMenuAction::Copy => ServoContextMenuAction::Copy,
        ContextMenuAction::Paste => ServoContextMenuAction::Paste,
        ContextMenuAction::SelectAll => ServoContextMenuAction::SelectAll,
    }
}

/// Servo turns the `accept` attribute into extensions without the dot: `.png` stays `png`, and a
/// MIME type such as `image/png` becomes its extensions through `mime_guess`
/// (`file_input_type.rs`, `filter_from_accept`). Duplicates and empty entries are removed here.
fn file_extensions(patterns: &[FilterPattern]) -> Vec<String> {
    let mut extensions = Vec::<String>::new();
    for pattern in patterns {
        let extension = pattern.0.trim().trim_start_matches('.').to_lowercase();
        if !extension.is_empty() && !extensions.contains(&extension) {
            extensions.push(extension);
        }
    }
    extensions
}

/// Keeps only absolute paths; the open panel never returns anything else, but the engine
/// would read whatever it is given.
fn chosen_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter(|path| path.is_absolute())
        .collect()
}

#[cfg(test)]
mod tests {
    use euclid::{Box2D, Point2D};
    use servo::ContextMenuElementInformationFlags;

    use super::*;

    #[test]
    fn ids_are_unique_across_web_views_of_one_runtime() {
        let ids = ControlIds::default();
        let first = EmbedderControls::new(ids.clone());
        let second = EmbedderControls::new(ids);

        let a = first.ids.next();
        let b = second.ids.next();
        let c = first.ids.next();

        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn a_control_is_answered_at_most_once() {
        let mut store = ControlStore::<u8, &str>::default();
        store.insert(ControlId::new(1), 10, "select");

        assert_eq!(store.take(ControlId::new(1)), Some("select"));
        assert_eq!(store.take(ControlId::new(1)), None);
    }

    #[test]
    fn an_unknown_answer_leaves_pending_controls_alone() {
        let mut store = ControlStore::<u8, &str>::default();
        store.insert(ControlId::new(1), 10, "select");

        assert_eq!(store.take(ControlId::new(2)), None);
        assert_eq!(store.take_by_key(&11), None);
        assert_eq!(store.take_all(), vec![(ControlId::new(1), "select")]);
    }

    #[test]
    fn the_engine_hides_a_control_by_its_own_key() {
        let mut store = ControlStore::<u8, &str>::default();
        store.insert(ControlId::new(1), 10, "select");
        store.insert(ControlId::new(2), 20, "input method");

        assert_eq!(
            store.take_by_key(&20),
            Some((ControlId::new(2), "input method"))
        );
        assert_eq!(store.take(ControlId::new(2)), None);
        assert_eq!(store.take(ControlId::new(1)), Some("select"));
    }

    #[test]
    fn dropping_all_empties_the_store() {
        let mut store = ControlStore::<u8, &str>::default();
        store.insert(ControlId::new(1), 10, "a");
        store.insert(ControlId::new(2), 20, "b");

        assert_eq!(store.take_all().len(), 2);
        assert!(store.take_all().is_empty());
    }

    #[test]
    fn page_labels_lose_control_and_bidi_characters() {
        assert_eq!(page_label("Apple"), "Apple");
        assert_eq!(page_label("a\tb\nc\u{7}"), "a b c ");
        assert_eq!(
            page_label("pay\u{202e}gpj.exe\u{202c} \u{2066}x\u{2069}\u{200f}\u{61c}"),
            "paygpj.exe x"
        );
        assert_eq!(page_label("Ünïcödé ✓"), "Ünïcödé ✓");
        assert_eq!(page_label("one\u{2028}two\u{2029}three"), "one two three");
    }

    #[test]
    fn select_labels_from_the_page_are_cleaned() {
        let entries = select_entries(&[SelectElementOptionOrOptgroup::Optgroup {
            label: "\u{202e}Fruit".to_owned(),
            options: vec![option(1, "Ap\u{0}ple", false)],
        }]);

        assert_eq!(
            entries,
            vec![SelectEntry::Group {
                label: "Fruit".to_owned(),
                options: vec![SelectOption {
                    id: 1,
                    label: "Ap ple".to_owned(),
                    disabled: false,
                }],
            }]
        );
    }

    #[test]
    fn rectangles_keep_servos_numbers() {
        let rect = Box2D::new(Point2D::new(10, 20), Point2D::new(130, 44));

        assert_eq!(
            control_rect(rect),
            ControlRect {
                x: 10,
                y: 20,
                width: 120,
                height: 24,
            }
        );
    }

    fn option(id: usize, label: &str, is_disabled: bool) -> SelectElementOption {
        SelectElementOption {
            id,
            label: label.to_owned(),
            is_disabled,
        }
    }

    #[test]
    fn select_options_and_groups_keep_their_ids_labels_and_state() {
        let entries = select_entries(&[
            SelectElementOptionOrOptgroup::Option(option(0, "None", false)),
            SelectElementOptionOrOptgroup::Optgroup {
                label: "Fruit".to_owned(),
                options: vec![option(1, "Apple", false), option(2, "Durian", true)],
            },
        ]);

        assert_eq!(
            entries,
            vec![
                SelectEntry::Option(SelectOption {
                    id: 0,
                    label: "None".to_owned(),
                    disabled: false,
                }),
                SelectEntry::Group {
                    label: "Fruit".to_owned(),
                    options: vec![
                        SelectOption {
                            id: 1,
                            label: "Apple".to_owned(),
                            disabled: false,
                        },
                        SelectOption {
                            id: 2,
                            label: "Durian".to_owned(),
                            disabled: true,
                        },
                    ],
                },
            ]
        );
    }

    #[test]
    fn context_menu_items_keep_labels_actions_state_and_separators() {
        let entries = context_menu_entries(&[
            ContextMenuItem::Item {
                label: "Copy".to_owned(),
                action: ServoContextMenuAction::Copy,
                enabled: false,
            },
            ContextMenuItem::Separator,
            ContextMenuItem::Item {
                label: "Back".to_owned(),
                action: ServoContextMenuAction::GoBack,
                enabled: true,
            },
        ]);

        assert_eq!(
            entries,
            vec![
                ContextMenuEntry::Item {
                    label: "Copy".to_owned(),
                    action: ContextMenuAction::Copy,
                    enabled: false,
                },
                ContextMenuEntry::Separator,
                ContextMenuEntry::Item {
                    label: "Back".to_owned(),
                    action: ContextMenuAction::GoBack,
                    enabled: true,
                },
            ]
        );
    }

    #[test]
    fn every_context_menu_action_survives_the_round_trip() {
        let actions = [
            ServoContextMenuAction::GoBack,
            ServoContextMenuAction::GoForward,
            ServoContextMenuAction::Reload,
            ServoContextMenuAction::CopyLink,
            ServoContextMenuAction::OpenLinkInNewWebView,
            ServoContextMenuAction::CopyImageLink,
            ServoContextMenuAction::OpenImageInNewView,
            ServoContextMenuAction::Cut,
            ServoContextMenuAction::Copy,
            ServoContextMenuAction::Paste,
            ServoContextMenuAction::SelectAll,
        ];
        for action in actions {
            assert_eq!(
                servo_context_menu_action(context_menu_action(action)),
                action
            );
        }
    }

    #[test]
    fn an_image_inside_a_link_reports_both_urls() {
        let info = ContextMenuElementInformation {
            flags: ContextMenuElementInformationFlags::Link
                | ContextMenuElementInformationFlags::Image,
            link_url: Some(url::Url::parse("https://a.test/page").expect("valid URL")),
            image_url: Some(url::Url::parse("https://a.test/i.png").expect("valid URL")),
        };

        assert_eq!(
            element_urls(&info),
            (
                Some("https://a.test/page".to_owned()),
                Some("https://a.test/i.png".to_owned())
            )
        );
        assert_eq!(
            element_urls(&ContextMenuElementInformation::default()),
            (None, None)
        );
    }

    #[test]
    fn file_filters_become_unique_lowercase_extensions() {
        let extensions = file_extensions(&[
            FilterPattern("PNG".to_owned()),
            FilterPattern(".jpg".to_owned()),
            FilterPattern("png".to_owned()),
            FilterPattern(" ".to_owned()),
        ]);

        assert_eq!(extensions, vec!["png".to_owned(), "jpg".to_owned()]);
    }

    #[test]
    fn only_absolute_chosen_paths_are_passed_on() {
        assert_eq!(
            chosen_paths(vec![PathBuf::from("/a/b.txt"), PathBuf::from("c.txt")]),
            vec![PathBuf::from("/a/b.txt")]
        );
    }
}
