//! The list shown for a `<select>` element and its keyboard and pointer behaviour.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use talaria_core::{ControlAnswer, SelectEntry, SelectOption};

use super::Outcome;

/// Every row, heading or option, is this tall (logical pixels), so a row's position is a
/// multiplication; `ui/controls/select-popup.slint` uses the same value.
pub(crate) const ROW_HEIGHT: f32 = 24.0;

/// Typed characters within this time of each other form one search string.
const TYPEAHEAD_TIMEOUT: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectRow {
    /// An option group's label; it cannot be chosen.
    Heading(String),
    Option {
        id: usize,
        label: String,
        disabled: bool,
        in_group: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectKey {
    Up,
    Down,
    Home,
    End,
    Return,
    Space,
    Escape,
    Character(char),
}

#[derive(Debug)]
pub(crate) struct SelectModel {
    rows: Vec<SelectRow>,
    /// Ids of the selected options.
    selected: BTreeSet<usize>,
    multiple: bool,
    highlighted: Option<usize>,
    typed: String,
    typed_at: Option<Instant>,
}

impl SelectModel {
    pub(crate) fn new(entries: &[SelectEntry], selected: &[usize], multiple: bool) -> Self {
        let mut rows = Vec::new();
        for entry in entries {
            match entry {
                SelectEntry::Option(option) => rows.push(option_row(option, false)),
                SelectEntry::Group { label, options } => {
                    rows.push(SelectRow::Heading(label.clone()));
                    rows.extend(options.iter().map(|option| option_row(option, true)));
                }
            }
        }
        let mut model = Self {
            rows,
            selected: selected.iter().copied().collect(),
            multiple,
            highlighted: None,
            typed: String::new(),
            typed_at: None,
        };
        let model_selected = &model.selected;
        let highlighted = model
            .rows
            .iter()
            .position(
                |row| matches!(row, SelectRow::Option { id, .. } if model_selected.contains(id)),
            )
            .or_else(|| model.first_enabled());
        model.highlighted = highlighted;
        model
    }

    pub(crate) fn rows(&self) -> &[SelectRow] {
        &self.rows
    }

    pub(crate) fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    pub(crate) fn multiple(&self) -> bool {
        self.multiple
    }

    pub(crate) fn is_selected(&self, row: usize) -> bool {
        matches!(self.rows.get(row), Some(SelectRow::Option { id, .. }) if self.selected.contains(id))
    }

    pub(crate) fn key(&mut self, key: SelectKey, now: Instant) -> Outcome {
        match key {
            SelectKey::Up => {
                self.highlighted = self.enabled_before(self.highlighted).or(self.highlighted);
            }
            SelectKey::Down => {
                self.highlighted = self.enabled_after(self.highlighted).or(self.highlighted);
            }
            SelectKey::Home => self.highlighted = self.first_enabled(),
            SelectKey::End => self.highlighted = self.enabled_before(None),
            SelectKey::Escape => return Outcome::Close(ControlAnswer::Dismiss),
            SelectKey::Return => {
                if self.multiple {
                    return self.done();
                }
                return match self.highlighted {
                    Some(row) => self.choose(row),
                    None => Outcome::Close(ControlAnswer::Dismiss),
                };
            }
            SelectKey::Space if self.typed.is_empty() || self.typeahead_expired(now) => {
                if let Some(row) = self.highlighted {
                    return self.choose(row);
                }
            }
            SelectKey::Space => self.type_character(' ', now),
            SelectKey::Character(character) => self.type_character(character, now),
        }
        Outcome::Stay
    }

    /// A click on a row: a single-select closes with the option, a multi-select toggles it.
    pub(crate) fn click(&mut self, row: usize) -> Outcome {
        self.choose(row)
    }

    pub(crate) fn hover(&mut self, row: usize) {
        if self.is_enabled_option(row) {
            self.highlighted = Some(row);
        }
    }

    /// Closes a multi-select with the options toggled so far.
    pub(crate) fn done(&self) -> Outcome {
        Outcome::Close(ControlAnswer::Select(
            self.selected.iter().copied().collect(),
        ))
    }

    /// The row's distance from the top of the list.
    pub(crate) fn row_top(&self, row: usize) -> f32 {
        row as f32 * ROW_HEIGHT
    }

    pub(crate) fn row_height(&self, _row: usize) -> f32 {
        ROW_HEIGHT
    }

    pub(crate) fn content_height(&self) -> f32 {
        self.rows.len() as f32 * ROW_HEIGHT
    }

    fn choose(&mut self, row: usize) -> Outcome {
        let Some(SelectRow::Option {
            id,
            disabled: false,
            ..
        }) = self.rows.get(row)
        else {
            return Outcome::Stay;
        };
        let id = *id;
        self.highlighted = Some(row);
        if !self.multiple {
            return Outcome::Close(ControlAnswer::Select(vec![id]));
        }
        if !self.selected.remove(&id) {
            self.selected.insert(id);
        }
        Outcome::Stay
    }

    fn typeahead_expired(&self, now: Instant) -> bool {
        self.typed_at
            .is_none_or(|typed_at| now.saturating_duration_since(typed_at) > TYPEAHEAD_TIMEOUT)
    }

    /// Typing jumps to the next enabled option whose label starts with what was typed. Typing
    /// one letter repeatedly cycles through the options starting with it.
    fn type_character(&mut self, character: char, now: Instant) {
        if self.typeahead_expired(now) {
            self.typed.clear();
        }
        self.typed_at = Some(now);
        self.typed.extend(character.to_lowercase());

        let mut typed_characters = self.typed.chars();
        let first = typed_characters.next();
        let repeated_letter = typed_characters.all(|next| Some(next) == first);
        let needle = if repeated_letter {
            first.map(String::from).unwrap_or_default()
        } else {
            self.typed.clone()
        };
        // A new search string may match the highlighted option itself; a repeated letter moves on.
        let start = match self.highlighted {
            Some(row) if repeated_letter => row + 1,
            Some(row) => row,
            None => 0,
        };
        let count = self.rows.len();
        self.highlighted = (0..count)
            .map(|offset| (start + offset) % count)
            .find(|&row| {
                self.is_enabled_option(row)
                    && matches!(&self.rows[row], SelectRow::Option { label, .. }
                        if label.trim_start().to_lowercase().starts_with(&needle))
            })
            .or(self.highlighted);
    }

    fn is_enabled_option(&self, row: usize) -> bool {
        matches!(
            self.rows.get(row),
            Some(SelectRow::Option {
                disabled: false,
                ..
            })
        )
    }

    fn first_enabled(&self) -> Option<usize> {
        (0..self.rows.len()).find(|&row| self.is_enabled_option(row))
    }

    /// The nearest enabled option before `row`, or the last one when `row` is None.
    fn enabled_before(&self, row: Option<usize>) -> Option<usize> {
        let end = row.unwrap_or(self.rows.len());
        (0..end).rev().find(|&row| self.is_enabled_option(row))
    }

    fn enabled_after(&self, row: Option<usize>) -> Option<usize> {
        let start = row.map_or(0, |row| row + 1);
        (start..self.rows.len()).find(|&row| self.is_enabled_option(row))
    }
}

fn option_row(option: &SelectOption, in_group: bool) -> SelectRow {
    SelectRow::Option {
        id: option.id,
        label: option.label.clone(),
        disabled: option.disabled,
        in_group,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: usize, label: &str, disabled: bool) -> SelectOption {
        SelectOption {
            id,
            label: label.to_owned(),
            disabled,
        }
    }

    /// Rows: 0 "None", 1 heading "Fruit", 2 "Apple", 3 "Banana" (disabled), 4 "Blueberry",
    /// 5 "Cherry", 6 heading "Other", 7 "Bread", 8 "Zucchini" (disabled).
    fn entries() -> Vec<SelectEntry> {
        vec![
            SelectEntry::Option(option(0, "None", false)),
            SelectEntry::Group {
                label: "Fruit".to_owned(),
                options: vec![
                    option(1, "Apple", false),
                    option(2, "Banana", true),
                    option(3, "Blueberry", false),
                    option(4, "Cherry", false),
                ],
            },
            SelectEntry::Group {
                label: "Other".to_owned(),
                options: vec![option(5, "Bread", false), option(6, "Zucchini", true)],
            },
        ]
    }

    fn single(selected: &[usize]) -> SelectModel {
        SelectModel::new(&entries(), selected, false)
    }

    #[test]
    fn groups_become_a_heading_followed_by_their_options() {
        let model = single(&[]);

        assert_eq!(model.rows().len(), 9);
        assert_eq!(model.rows()[1], SelectRow::Heading("Fruit".to_owned()));
        assert_eq!(
            model.rows()[2],
            SelectRow::Option {
                id: 1,
                label: "Apple".to_owned(),
                disabled: false,
                in_group: true,
            }
        );
        assert!(matches!(
            model.rows()[0],
            SelectRow::Option {
                in_group: false,
                ..
            }
        ));
    }

    #[test]
    fn the_selected_option_starts_highlighted_and_marked() {
        let model = single(&[4]);

        assert_eq!(model.highlighted(), Some(5));
        assert!(model.is_selected(5));
        assert!(!model.is_selected(2));
        assert_eq!(single(&[]).highlighted(), Some(0));
    }

    #[test]
    fn arrows_skip_headings_and_disabled_options_and_stop_at_the_ends() {
        let mut model = single(&[]);
        let now = Instant::now();

        let mut visited = Vec::new();
        for _ in 0..6 {
            model.key(SelectKey::Down, now);
            visited.push(model.highlighted());
        }
        assert_eq!(
            visited,
            vec![Some(2), Some(4), Some(5), Some(7), Some(7), Some(7)]
        );

        model.key(SelectKey::Up, now);
        model.key(SelectKey::Up, now);
        assert_eq!(model.highlighted(), Some(4));
    }

    #[test]
    fn home_and_end_go_to_the_first_and_last_enabled_options() {
        let mut model = single(&[3]);
        let now = Instant::now();

        model.key(SelectKey::End, now);
        assert_eq!(model.highlighted(), Some(7));
        model.key(SelectKey::Home, now);
        assert_eq!(model.highlighted(), Some(0));
    }

    #[test]
    fn return_chooses_the_highlighted_option_of_a_single_select() {
        let mut model = single(&[]);
        let now = Instant::now();
        model.key(SelectKey::Down, now);

        assert_eq!(
            model.key(SelectKey::Return, now),
            Outcome::Close(ControlAnswer::Select(vec![1]))
        );
    }

    #[test]
    fn escape_cancels_with_the_selection_unchanged() {
        let mut model = single(&[3]);

        assert_eq!(
            model.key(SelectKey::Escape, Instant::now()),
            Outcome::Close(ControlAnswer::Dismiss)
        );
    }

    #[test]
    fn clicking_an_option_of_a_single_select_chooses_it() {
        let mut model = single(&[0]);

        assert_eq!(model.click(1), Outcome::Stay);
        assert_eq!(model.click(3), Outcome::Stay);
        assert_eq!(
            model.click(7),
            Outcome::Close(ControlAnswer::Select(vec![5]))
        );
    }

    #[test]
    fn a_multi_select_toggles_and_stays_open_until_return_or_done() {
        let mut model = SelectModel::new(&entries(), &[4], true);

        assert_eq!(model.click(2), Outcome::Stay);
        assert_eq!(model.click(5), Outcome::Stay);
        assert!(model.is_selected(2));
        assert!(!model.is_selected(5));
        assert_eq!(model.done(), Outcome::Close(ControlAnswer::Select(vec![1])));
        assert_eq!(
            model.key(SelectKey::Return, Instant::now()),
            Outcome::Close(ControlAnswer::Select(vec![1]))
        );
    }

    #[test]
    fn space_toggles_the_highlighted_option_of_a_multi_select() {
        let mut model = SelectModel::new(&entries(), &[], true);
        let now = Instant::now();

        model.key(SelectKey::Down, now);
        assert_eq!(model.key(SelectKey::Space, now), Outcome::Stay);
        assert!(model.is_selected(2));
        model.key(SelectKey::Space, now);
        assert!(!model.is_selected(2));
    }

    #[test]
    fn typing_jumps_to_matching_enabled_options() {
        let mut model = single(&[]);
        let start = Instant::now();

        model.key(SelectKey::Character('b'), start);
        assert_eq!(model.highlighted(), Some(4), "Banana is disabled");
        model.key(
            SelectKey::Character('r'),
            start + Duration::from_millis(300),
        );
        assert_eq!(model.highlighted(), Some(7));
        model.key(SelectKey::Character('C'), start + Duration::from_secs(3));
        assert_eq!(model.highlighted(), Some(5));
    }

    #[test]
    fn typing_one_letter_repeatedly_cycles_through_its_options() {
        let mut model = single(&[]);
        let start = Instant::now();

        let mut visited = Vec::new();
        for step in 0..3 {
            model.key(
                SelectKey::Character('b'),
                start + Duration::from_millis(100 * step),
            );
            visited.push(model.highlighted());
        }
        assert_eq!(visited, vec![Some(4), Some(7), Some(4)]);
    }

    #[test]
    fn typing_without_a_match_keeps_the_highlight() {
        let mut model = single(&[]);

        model.key(SelectKey::Character('q'), Instant::now());

        assert_eq!(model.highlighted(), Some(0));
    }

    #[test]
    fn space_while_typing_is_part_of_the_search() {
        let entries = vec![
            SelectEntry::Option(option(0, "New York", false)),
            SelectEntry::Option(option(1, "New Zealand", false)),
        ];
        let mut model = SelectModel::new(&entries, &[0], false);
        let start = Instant::now();

        for (step, key) in [
            SelectKey::Character('n'),
            SelectKey::Character('e'),
            SelectKey::Character('w'),
            SelectKey::Space,
            SelectKey::Character('z'),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                model.key(key, start + Duration::from_millis(100 * step as u64)),
                Outcome::Stay
            );
        }
        assert_eq!(model.highlighted(), Some(1));
    }

    #[test]
    fn an_empty_select_cancels_on_return() {
        let mut model = SelectModel::new(&[], &[], false);

        assert_eq!(model.highlighted(), None);
        assert_eq!(
            model.key(SelectKey::Return, Instant::now()),
            Outcome::Close(ControlAnswer::Dismiss)
        );
    }

    #[test]
    fn rows_have_one_height() {
        let model = single(&[]);

        assert_eq!(model.row_top(0), 0.0);
        assert_eq!(model.row_top(2), 2.0 * ROW_HEIGHT);
        assert_eq!(model.row_height(1), ROW_HEIGHT);
        assert_eq!(model.content_height(), 9.0 * ROW_HEIGHT);
    }

    /// A scripted list of 100,000 options with the selection near the end. Prints the time per
    /// key; the bound only catches a return to work proportional to the list per row.
    #[test]
    fn a_list_of_100000_options_stays_fast() {
        let entries = (0..100_000)
            .map(|id| SelectEntry::Option(option(id, &format!("Option {id}"), id % 7 == 3)))
            .collect::<Vec<_>>();
        let built_at = Instant::now();
        let mut model = SelectModel::new(&entries, &[99_990], true);
        let build_time = built_at.elapsed();
        let keys = [
            SelectKey::Down,
            SelectKey::Up,
            SelectKey::Space,
            SelectKey::Home,
            SelectKey::End,
            SelectKey::Down,
        ];

        let started_at = Instant::now();
        let mut now = started_at;
        for round in 0..100 {
            for key in keys {
                model.key(key, now);
                let row = model.highlighted().expect("an option is highlighted");
                let _ = (model.row_top(row), model.is_selected(row));
            }
            now += Duration::from_secs(2);
            model.key(
                SelectKey::Character(if round % 2 == 0 { 'o' } else { 'x' }),
                now,
            );
        }
        let per_key = started_at.elapsed() / 700;

        println!("100,000 options: built in {build_time:?}, {per_key:?} per key");
        assert!(per_key < Duration::from_millis(20), "{per_key:?} per key");
    }

    #[test]
    fn hovering_highlights_only_enabled_options() {
        let mut model = single(&[]);

        model.hover(3);
        assert_eq!(model.highlighted(), Some(0));
        model.hover(4);
        assert_eq!(model.highlighted(), Some(4));
    }
}
