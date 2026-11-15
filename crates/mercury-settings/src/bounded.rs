//! Limits applied while a file is decoded, so that a hostile file never makes the browser build
//! more than the documented number of items.
//!
//! Lists read from disk keep at most their limit and skip the rest without building them
//! (`IgnoredAny`); the number skipped is recorded per list for the duration of a [`scoped`]
//! decode, so the caller can report one counted repair per list. A long URL or title is not
//! *kept* whole: the value kept is at most its limit. It is still read whole once: serde_json
//! copies every string it reads from a file into a scratch buffer before the visitor sees it,
//! so one enormous string costs up to about one and a half times its own length while it is
//! read (the buffer doubles as it grows), which is at most twice the file's size limit. The
//! tree used for migrations from an older version counts its nodes.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::marker::PhantomData;

use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::search::{MAX_CUSTOM_ENGINES, SearchEngine};
use crate::session::{
    ClosedTab, MAX_CLOSED_TABS, MAX_RECOVERY_SESSIONS, MAX_RESTORED_TABS, MAX_TITLE_BYTES,
    SessionSnapshot, TabSnapshot, WindowSnapshot,
};
use crate::settings::{MAX_NEVER_SAVE_ORIGINS, MAX_URL_BYTES, truncated};

/// The most JSON values (objects, arrays, strings, numbers…) the migration tree may hold. A
/// real settings file has under 2,000 and a real session under 4,000.
pub(crate) const MAX_TREE_NODES: usize = 50_000;

/// List names, as used in [`Unread`].
pub(crate) const CUSTOM_ENGINES: &str = "search.custom_engines";
pub(crate) const NEVER_SAVE_ORIGINS: &str = "passwords.never_save_origins";
pub(crate) const WINDOWS: &str = "windows";
pub(crate) const TABS: &str = "windows.tabs";
pub(crate) const CLOSED_TABS: &str = "closed_tabs";
pub(crate) const RECOVERY_SESSIONS: &str = "sessions";

/// How many items each list had beyond its limit, skipped unread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Unread {
    counts: Vec<(&'static str, u64)>,
}

impl Unread {
    pub(crate) fn get(&self, list: &str) -> u64 {
        self.counts
            .iter()
            .filter(|(name, _)| *name == list)
            .map(|(_, count)| *count)
            .sum()
    }

    pub(crate) fn total(&self) -> u64 {
        self.counts.iter().map(|(_, count)| *count).sum()
    }

    fn add(&mut self, list: &'static str, count: u64) {
        match self.counts.iter_mut().find(|(name, _)| *name == list) {
            Some((_, total)) => *total = total.saturating_add(count),
            None => self.counts.push((list, count)),
        }
    }
}

struct Scope {
    unread: Unread,
    /// Tabs still allowed across every window of the document being decoded.
    tabs_left: usize,
}

thread_local! {
    static SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

/// Runs a decode with fresh limits and returns what it skipped. Outside a scope the lists are
/// still bounded, each on its own, but nothing is recorded.
pub(crate) fn scoped<T>(decode: impl FnOnce() -> T) -> (T, Unread) {
    let outer = SCOPE.with(|scope| {
        scope.replace(Some(Scope {
            unread: Unread::default(),
            tabs_left: MAX_RESTORED_TABS,
        }))
    });
    let value = decode();
    let inner = SCOPE.with(|scope| scope.replace(outer));
    (value, inner.map(|scope| scope.unread).unwrap_or_default())
}

fn note_unread(list: &'static str, count: u64) {
    if count > 0 {
        SCOPE.with(|scope| {
            if let Some(scope) = scope.borrow_mut().as_mut() {
                scope.unread.add(list, count);
            }
        });
    }
}

/// Reads a sequence, keeping at most `max` items and skipping the rest unbuilt.
fn bounded_vec<'de, D, T>(
    deserializer: D,
    max: usize,
    list: &'static str,
) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    bounded_vec_with(deserializer, max, list, || {})
}

/// [`bounded_vec`], calling `before_each` before each kept item is read.
fn bounded_vec_with<'de, D, T>(
    deserializer: D,
    max: usize,
    list: &'static str,
    before_each: fn(),
) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T> {
        max: usize,
        list: &'static str,
        before_each: fn(),
        item: PhantomData<T>,
    }

    impl<'de, T: Deserialize<'de>> Visitor<'de> for Bounded<T> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a list")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Vec<T>, A::Error> {
            // Grown as items arrive, never sized from a hint the file controls.
            let mut kept = Vec::new();
            while kept.len() < self.max {
                (self.before_each)();
                match items.next_element::<T>()? {
                    Some(item) => kept.push(item),
                    None => return Ok(kept),
                }
            }
            let mut skipped = 0_u64;
            while items.next_element::<IgnoredAny>()?.is_some() {
                skipped += 1;
            }
            note_unread(self.list, skipped);
            Ok(kept)
        }
    }

    deserializer.deserialize_seq(Bounded {
        max,
        list,
        before_each,
        item: PhantomData,
    })
}

pub(crate) fn custom_engines<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SearchEngine>, D::Error> {
    bounded_vec(deserializer, MAX_CUSTOM_ENGINES, CUSTOM_ENGINES)
}

pub(crate) fn never_save_origins<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    bounded_vec(deserializer, MAX_NEVER_SAVE_ORIGINS, NEVER_SAVE_ORIGINS)
}

pub(crate) fn windows<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<WindowSnapshot>, D::Error> {
    // A window without a tab is dropped, so more windows than tabs is never useful.
    bounded_vec(deserializer, MAX_RESTORED_TABS, WINDOWS)
}

pub(crate) fn tabs<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<TabSnapshot>, D::Error> {
    let max = SCOPE.with(|scope| {
        scope
            .borrow()
            .as_ref()
            .map_or(MAX_RESTORED_TABS, |scope| scope.tabs_left)
    });
    let tabs = bounded_vec(deserializer, max, TABS)?;
    SCOPE.with(|scope| {
        if let Some(scope) = scope.borrow_mut().as_mut() {
            scope.tabs_left = scope.tabs_left.saturating_sub(tabs.len());
        }
    });
    Ok(tabs)
}

/// The sessions of the recovery file: at most [`MAX_RECOVERY_SESSIONS`], each with a tab
/// budget of its own.
pub(crate) fn recovery_sessions<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SessionSnapshot>, D::Error> {
    bounded_vec_with(
        deserializer,
        MAX_RECOVERY_SESSIONS,
        RECOVERY_SESSIONS,
        || {
            SCOPE.with(|scope| {
                if let Some(scope) = scope.borrow_mut().as_mut() {
                    scope.tabs_left = MAX_RESTORED_TABS;
                }
            });
        },
    )
}

pub(crate) fn closed_tabs<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ClosedTab>, D::Error> {
    bounded_vec(deserializer, MAX_CLOSED_TABS, CLOSED_TABS)
}

/// A string kept only if it fits; a longer one becomes empty (for URLs, which are then
/// dropped as unrestorable) or is cut at a character boundary (for titles).
struct BoundedString {
    max: usize,
    cut: bool,
}

impl Visitor<'_> for BoundedString {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<String, E> {
        Ok(if text.len() <= self.max {
            text.to_owned()
        } else if self.cut {
            truncated(text, self.max).to_owned()
        } else {
            String::new()
        })
    }
}

pub(crate) fn url<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    deserializer.deserialize_str(BoundedString {
        max: MAX_URL_BYTES,
        cut: false,
    })
}

pub(crate) fn title<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    deserializer.deserialize_str(BoundedString {
        max: MAX_TITLE_BYTES,
        cut: true,
    })
}

/// Parses a whole document into a tree of at most [`MAX_TREE_NODES`] values, for migrations.
pub(crate) fn parse_tree(reader: impl std::io::Read) -> Result<Value, String> {
    let budget = Cell::new(MAX_TREE_NODES);
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let tree = TreeSeed(&budget)
        .deserialize(&mut deserializer)
        .map_err(|error| error.to_string())?;
    deserializer.end().map_err(|error| error.to_string())?;
    Ok(tree)
}

#[derive(Clone, Copy)]
struct TreeSeed<'a>(&'a Cell<usize>);

impl TreeSeed<'_> {
    fn spend<E: de::Error>(self) -> Result<(), E> {
        let left = self.0.get();
        if left == 0 {
            return Err(E::custom(format!(
                "more than {MAX_TREE_NODES} values; too large to migrate"
            )));
        }
        self.0.set(left - 1);
        Ok(())
    }
}

impl<'de> DeserializeSeed<'de> for TreeSeed<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for TreeSeed<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        self.spend()?;
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        self.spend()?;
        Ok(Value::from(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        self.spend()?;
        Ok(Value::from(value))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        self.spend()?;
        Ok(serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        self.spend()?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        self.spend()?;
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Value, A::Error> {
        self.spend()?;
        let mut values = Vec::new();
        while let Some(value) = items.next_element_seed(self)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut members: A) -> Result<Value, A::Error> {
        self.spend()?;
        let mut map = Map::new();
        while let Some(key) = members.next_key::<String>()? {
            let value = members.next_value_seed(self)?;
            map.insert(key, value);
        }
        Ok(Value::Object(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_refuses_a_node_bomb() {
        let bomb = format!("[{}0]", "0,".repeat(MAX_TREE_NODES));
        let error = parse_tree(bomb.as_bytes()).unwrap_err();
        assert!(error.contains("too large to migrate"), "{error}");
    }

    #[test]
    fn the_tree_holds_an_ordinary_document() {
        let tree =
            parse_tree(br#"{"a": [1, -2, 3.5, "x", null, true, {"b": {}}]}"#.as_slice()).unwrap();
        assert_eq!(
            tree,
            serde_json::json!({"a": [1, -2, 3.5, "x", null, true, {"b": {}}]})
        );
    }

    #[test]
    fn lists_are_cut_and_the_rest_counted() {
        let json = format!("[{}]", vec!["1"; 70].join(","));
        let (kept, unread) = scoped(|| {
            let mut deserializer = serde_json::Deserializer::from_str(&json);
            bounded_vec::<_, u8>(&mut deserializer, 50, CUSTOM_ENGINES).unwrap()
        });
        assert_eq!(kept.len(), 50);
        assert_eq!(unread.get(CUSTOM_ENGINES), 20);
        assert_eq!(unread.total(), 20);
    }
}
