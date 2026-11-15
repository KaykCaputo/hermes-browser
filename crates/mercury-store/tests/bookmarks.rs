mod common;

use common::{Fixture, NOW};
use mercury_store::{BookmarkId, BookmarkKind, BookmarkRoot, Store, StoreError};

fn titles(store: &Store, folder: BookmarkId) -> Vec<String> {
    let children = store.bookmark_children(folder).unwrap();
    for (index, child) in children.iter().enumerate() {
        assert_eq!(child.position as usize, index, "positions must be dense");
        assert_eq!(child.parent, Some(folder));
    }
    children.into_iter().map(|c| c.title).collect()
}

fn setup() -> (Store, BookmarkId, BookmarkId) {
    let store = Store::open_in_memory().unwrap();
    let toolbar = store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    let other = store.bookmark_root(BookmarkRoot::Other).unwrap();
    (store, toolbar, other)
}

fn url(n: &str) -> String {
    format!("https://example.com/{n}")
}

#[test]
fn roots_are_distinct_empty_folders() {
    let (store, toolbar, other) = setup();
    assert_ne!(toolbar, other);
    let node = store.bookmark(toolbar).unwrap();
    assert_eq!(node.guid, "toolbar_____");
    assert_eq!(node.title, "Bookmarks Bar");
    assert_eq!(node.kind, BookmarkKind::Folder);
    assert_eq!(node.parent, None);
    let node = store.bookmark(other).unwrap();
    assert_eq!(node.guid, "menu________");
    assert_eq!(node.title, "Other Bookmarks");
    assert!(store.bookmark_children(toolbar).unwrap().is_empty());
}

#[test]
fn add_appends_and_returns_full_nodes() {
    let (mut store, toolbar, _) = setup();
    let a = store
        .add_bookmark(toolbar, None, "A", &url("a"), "g-a", NOW)
        .unwrap();
    let f = store
        .add_bookmark_folder(toolbar, None, "F", "g-f", NOW + 1)
        .unwrap();
    let s = store
        .add_bookmark_separator(toolbar, None, "g-s", NOW + 2)
        .unwrap();
    assert_eq!(titles(&store, toolbar), ["A", "F", ""]);

    let node = store.bookmark(a).unwrap();
    assert_eq!(node.guid, "g-a");
    assert_eq!(node.kind, BookmarkKind::Bookmark { url: url("a") });
    assert_eq!((node.added_at_ms, node.modified_at_ms), (NOW, NOW));
    assert_eq!(store.bookmark(f).unwrap().kind, BookmarkKind::Folder);
    assert_eq!(store.bookmark(s).unwrap().kind, BookmarkKind::Separator);
    assert_eq!(store.bookmark(s).unwrap().position, 2);
}

#[test]
fn insert_at_a_position_shifts_later_siblings() {
    let (mut store, toolbar, _) = setup();
    store
        .add_bookmark(toolbar, None, "A", &url("a"), "g-a", NOW)
        .unwrap();
    store
        .add_bookmark(toolbar, None, "C", &url("c"), "g-c", NOW)
        .unwrap();
    store
        .add_bookmark(toolbar, Some(1), "B", &url("b"), "g-b", NOW)
        .unwrap();
    store
        .add_bookmark_folder(toolbar, Some(0), "Start", "g-0", NOW)
        .unwrap();
    // A position past the end appends.
    store
        .add_bookmark_separator(toolbar, Some(99), "g-end", NOW)
        .unwrap();
    assert_eq!(titles(&store, toolbar), ["Start", "A", "B", "C", ""]);
}

#[test]
fn adding_under_a_non_folder_or_missing_parent_is_refused() {
    let (mut store, toolbar, _) = setup();
    let a = store
        .add_bookmark(toolbar, None, "A", &url("a"), "g-a", NOW)
        .unwrap();
    assert!(matches!(
        store.add_bookmark(a, None, "X", &url("x"), "g-x", NOW),
        Err(StoreError::NotAFolder)
    ));
    assert!(matches!(
        store.add_bookmark_folder(BookmarkId(999), None, "X", "g-y", NOW),
        Err(StoreError::NotFound)
    ));
    assert_eq!(titles(&store, toolbar), ["A"]);
}

#[test]
fn duplicate_guid_is_rejected() {
    let (mut store, toolbar, other) = setup();
    store
        .add_bookmark(toolbar, None, "A", &url("a"), "same", NOW)
        .unwrap();
    match store.add_bookmark_folder(other, None, "F", "same", NOW) {
        Err(StoreError::DuplicateGuid(guid)) => assert_eq!(guid, "same"),
        other => panic!("expected DuplicateGuid, got {other:?}"),
    }
    assert!(matches!(
        store.add_bookmark_separator(other, None, "toolbar_____", NOW),
        Err(StoreError::DuplicateGuid(_))
    ));
    assert!(store.bookmark_children(other).unwrap().is_empty());
}

#[test]
fn rename_and_set_url_update_modified_at() {
    let (mut store, toolbar, _) = setup();
    let a = store
        .add_bookmark(toolbar, None, "A", &url("a"), "g-a", NOW)
        .unwrap();
    store.rename_bookmark(a, "Renamed", NOW + 10).unwrap();
    let node = store.bookmark(a).unwrap();
    assert_eq!(
        (node.title.as_str(), node.added_at_ms, node.modified_at_ms),
        ("Renamed", NOW, NOW + 10)
    );

    store.set_bookmark_url(a, &url("moved"), NOW + 20).unwrap();
    let node = store.bookmark(a).unwrap();
    assert_eq!(node.kind, BookmarkKind::Bookmark { url: url("moved") });
    assert_eq!(node.modified_at_ms, NOW + 20);
    assert_eq!(store.bookmark_for_url(&url("a")).unwrap(), None);
    assert_eq!(store.bookmark_for_url(&url("moved")).unwrap(), Some(a));

    let folder = store
        .add_bookmark_folder(toolbar, None, "F", "g-f", NOW)
        .unwrap();
    assert!(matches!(
        store.set_bookmark_url(folder, &url("x"), NOW),
        Err(StoreError::InvalidArgument(_))
    ));
    assert!(matches!(
        store.rename_bookmark(BookmarkId(999), "x", NOW),
        Err(StoreError::NotFound)
    ));
}

#[test]
fn bookmark_for_url_finds_the_first_exact_match() {
    let (mut store, toolbar, other) = setup();
    assert_eq!(store.bookmark_for_url(&url("a")).unwrap(), None);
    let first = store
        .add_bookmark(other, None, "A", &url("a"), "g-1", NOW)
        .unwrap();
    store
        .add_bookmark(toolbar, None, "A2", &url("a"), "g-2", NOW)
        .unwrap();
    assert_eq!(store.bookmark_for_url(&url("a")).unwrap(), Some(first));
    assert_eq!(store.bookmark_for_url(&url("a/")).unwrap(), None);
    assert_eq!(
        store.bookmark_for_url("https://EXAMPLE.com/a").unwrap(),
        None
    );
}

#[test]
fn move_within_a_folder_keeps_positions_dense() {
    let (mut store, toolbar, _) = setup();
    let ids: Vec<BookmarkId> = ["A", "B", "C", "D"]
        .iter()
        .map(|t| {
            store
                .add_bookmark(toolbar, None, t, &url(t), t, NOW)
                .unwrap()
        })
        .collect();
    store
        .move_bookmark(ids[0], toolbar, Some(2), NOW + 5)
        .unwrap();
    assert_eq!(titles(&store, toolbar), ["B", "C", "A", "D"]);
    store
        .move_bookmark(ids[3], toolbar, Some(0), NOW + 5)
        .unwrap();
    assert_eq!(titles(&store, toolbar), ["D", "B", "C", "A"]);
    store.move_bookmark(ids[3], toolbar, None, NOW + 5).unwrap();
    assert_eq!(titles(&store, toolbar), ["B", "C", "A", "D"]);
    store
        .move_bookmark(ids[1], toolbar, Some(1), NOW + 5)
        .unwrap();
    assert_eq!(titles(&store, toolbar), ["C", "B", "A", "D"]);
    // Moving to its own position changes nothing but the modification time.
    store
        .move_bookmark(ids[1], toolbar, Some(1), NOW + 6)
        .unwrap();
    assert_eq!(titles(&store, toolbar), ["C", "B", "A", "D"]);
    assert_eq!(store.bookmark(ids[0]).unwrap().modified_at_ms, NOW + 5);
}

#[test]
fn move_between_folders() {
    let (mut store, toolbar, other) = setup();
    let a = store
        .add_bookmark(toolbar, None, "A", &url("a"), "g-a", NOW)
        .unwrap();
    store
        .add_bookmark(toolbar, None, "B", &url("b"), "g-b", NOW)
        .unwrap();
    store
        .add_bookmark(other, None, "X", &url("x"), "g-x", NOW)
        .unwrap();
    store
        .add_bookmark(other, None, "Y", &url("y"), "g-y", NOW)
        .unwrap();

    store.move_bookmark(a, other, Some(1), NOW + 7).unwrap();
    assert_eq!(titles(&store, toolbar), ["B"]);
    assert_eq!(titles(&store, other), ["X", "A", "Y"]);
    let node = store.bookmark(a).unwrap();
    assert_eq!((node.parent, node.modified_at_ms), (Some(other), NOW + 7));

    let folder = store
        .add_bookmark_folder(toolbar, None, "F", "g-f", NOW)
        .unwrap();
    store.move_bookmark(a, folder, None, NOW + 8).unwrap();
    assert_eq!(titles(&store, other), ["X", "Y"]);
    assert_eq!(titles(&store, folder), ["A"]);
}

#[test]
fn illegal_moves_are_refused() {
    let (mut store, toolbar, other) = setup();
    let outer = store
        .add_bookmark_folder(toolbar, None, "Outer", "g-o", NOW)
        .unwrap();
    let inner = store
        .add_bookmark_folder(outer, None, "Inner", "g-i", NOW)
        .unwrap();
    let leaf = store
        .add_bookmark(inner, None, "Leaf", &url("l"), "g-l", NOW)
        .unwrap();

    assert!(matches!(
        store.move_bookmark(outer, outer, None, NOW),
        Err(StoreError::InvalidMove)
    ));
    assert!(matches!(
        store.move_bookmark(outer, inner, None, NOW),
        Err(StoreError::InvalidMove)
    ));
    assert!(matches!(
        store.move_bookmark(toolbar, other, None, NOW),
        Err(StoreError::CannotModifyRoot)
    ));
    assert!(matches!(
        store.move_bookmark(outer, leaf, None, NOW),
        Err(StoreError::NotAFolder)
    ));
    assert!(matches!(
        store.move_bookmark(BookmarkId(999), other, None, NOW),
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.move_bookmark(leaf, BookmarkId(999), None, NOW),
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.remove_bookmark(toolbar, NOW),
        Err(StoreError::CannotModifyRoot)
    ));
    assert!(matches!(
        store.remove_bookmark(BookmarkId(999), NOW),
        Err(StoreError::NotFound)
    ));

    assert_eq!(titles(&store, toolbar), ["Outer"]);
    assert_eq!(titles(&store, outer), ["Inner"]);
    assert_eq!(titles(&store, inner), ["Leaf"]);
    assert_eq!(store.bookmark(outer).unwrap().modified_at_ms, NOW);
}

#[test]
fn removing_a_folder_removes_its_subtree_and_closes_the_gap() {
    let mut f = Fixture::new("remove-subtree");
    let toolbar = f.store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    f.store
        .add_bookmark(toolbar, None, "A", &url("a"), "g-a", NOW)
        .unwrap();
    let outer = f
        .store
        .add_bookmark_folder(toolbar, None, "Outer", "g-o", NOW)
        .unwrap();
    f.store
        .add_bookmark(toolbar, None, "C", &url("c"), "g-c", NOW)
        .unwrap();
    let inner = f
        .store
        .add_bookmark_folder(outer, None, "Inner", "g-i", NOW)
        .unwrap();
    let leaf = f
        .store
        .add_bookmark(inner, None, "Leaf", &url("leaf"), "g-l", NOW)
        .unwrap();
    f.store
        .add_bookmark_separator(outer, None, "g-s", NOW)
        .unwrap();

    f.store.remove_bookmark(outer, NOW + 1).unwrap();
    assert_eq!(titles(&f.store, toolbar), ["A", "C"]);
    assert!(matches!(f.store.bookmark(inner), Err(StoreError::NotFound)));
    assert!(matches!(f.store.bookmark(leaf), Err(StoreError::NotFound)));
    assert_eq!(f.store.bookmark_for_url(&url("leaf")).unwrap(), None);
    assert_eq!(f.count("SELECT count(*) FROM bookmarks"), 4);
    // The never-visited page behind the removed bookmark is gone too.
    assert_eq!(f.place(&url("leaf")), None);
    assert!(f.place(&url("a")).is_some());
}

#[test]
fn removing_a_bookmark_keeps_a_visited_place() {
    let mut f = Fixture::new("remove-visited");
    let toolbar = f.store.bookmark_root(BookmarkRoot::Toolbar).unwrap();
    f.store
        .record_visit(
            &url("a"),
            Some("A page"),
            mercury_store::VisitTransition::Link,
            NOW,
        )
        .unwrap();
    let a = f
        .store
        .add_bookmark(toolbar, None, "A", &url("a"), "g-a", NOW)
        .unwrap();
    let b = f
        .store
        .add_bookmark(toolbar, None, "B", &url("b"), "g-b", NOW)
        .unwrap();
    f.store.remove_bookmark(a, NOW).unwrap();
    assert_eq!(f.place(&url("a")).unwrap().visit_count, 1);
    assert_eq!(f.store.bookmark(b).unwrap().position, 0);
}

#[test]
fn children_of_a_non_folder_or_missing_node() {
    let (mut store, toolbar, _) = setup();
    let a = store
        .add_bookmark(toolbar, None, "A", &url("a"), "g-a", NOW)
        .unwrap();
    assert!(matches!(
        store.bookmark_children(a),
        Err(StoreError::NotAFolder)
    ));
    assert!(matches!(
        store.bookmark_children(BookmarkId(999)),
        Err(StoreError::NotFound)
    ));
}
