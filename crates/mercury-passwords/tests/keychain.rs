//! The macOS Keychain backend: item layout, isolation, foreign items and errors. Every test runs
//! on its own temporary keychain (see `common/keychain.rs`); none touches an existing keychain.
#![cfg(target_os = "macos")]

mod common;

use std::collections::HashMap;
use std::thread::sleep;
use std::time::Duration;

use common::contract::{fake, form, http_auth};
use common::keychain::{TEST_KEYCHAIN_PASSWORD, TempKeychain, temp_keychain};
use core_foundation::data::CFData;
use mercury_passwords::{
    KEYCHAIN_SERVICE, KeychainBackend, PasswordError, PasswordStore, SaveOutcome, SecretBackend,
};
use security_framework::item::{
    ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, Limit, Location,
};
use security_framework::os::macos::keychain::{KeychainSettings, SecKeychain};

/// Attributes of every generic-password item in `keychain` (no secrets are loaded).
fn attributes(keychain: &SecKeychain) -> Vec<HashMap<String, String>> {
    match ItemSearchOptions::new()
        .keychains(std::slice::from_ref(keychain))
        .class(ItemClass::generic_password())
        .load_attributes(true)
        .limit(Limit::All)
        .search()
    {
        Ok(items) => items
            .iter()
            .map(|item| item.simplify_dict().expect("attributes are a dictionary"))
            .collect(),
        Err(error) if error.code() == -25300 => Vec::new(),
        Err(error) => panic!("search failed with status {}", error.code()),
    }
}

fn add_foreign_item(keychain: &SecKeychain, service: &str, account: &str) {
    let mut options = ItemAddOptions::new(ItemAddValue::Data {
        class: ItemClass::generic_password(),
        data: CFData::from_buffer(b"fake-foreign-secret"),
    });
    options
        .set_location(Location::FileKeychain(keychain.clone()))
        .set_service(service)
        .set_account_name(account)
        .set_label("foreign test item");
    options.add().expect("a foreign test item can be added");
}

#[test]
fn service_name_is_the_application_bundle_identifier_plus_logins() {
    assert_eq!(
        KEYCHAIN_SERVICE,
        "io.github.thepigdestroyer.mercury-browser.logins"
    );
}

#[test]
fn one_generic_password_item_per_login_with_the_documented_attributes() {
    let (guard, backend) = temp_keychain();
    backend
        .put(&form("https://example.com", "alice"), &fake("fake-one"))
        .expect("save form login");
    backend
        .put(
            &http_auth("https://example.com:8443", "Staff|Area", "bob%"),
            &fake("fake-two"),
        )
        .expect("save http login");
    backend
        .put(&form("http://localhost:3000", ""), &fake("fake-three"))
        .expect("save login without user name");

    let mut items = attributes(&guard.handle());
    items.sort_by(|a, b| a["acct"].cmp(&b["acct"]));
    assert_eq!(items.len(), 3);
    for item in &items {
        assert_eq!(item["class"], "genp");
        assert_eq!(item["svce"], KEYCHAIN_SERVICE);
        assert!(item.contains_key("cdat") && item.contains_key("mdat"));
    }

    assert_eq!(items[0]["acct"], "v1|form|http://localhost:3000||");
    assert_eq!(items[0]["labl"], "http://localhost:3000");
    assert_eq!(items[0]["desc"], "Mercury Browser web login");

    assert_eq!(items[1]["acct"], "v1|form|https://example.com||alice");
    assert_eq!(items[1]["labl"], "example.com (alice)");
    assert_eq!(items[1]["desc"], "Mercury Browser web login");

    assert_eq!(
        items[2]["acct"],
        "v1|http|https://example.com:8443|Staff%7CArea|bob%25"
    );
    assert_eq!(items[2]["labl"], "example.com:8443 (bob%) – Staff|Area");
    assert_eq!(items[2]["desc"], "Mercury Browser HTTP login");
}

#[test]
fn a_keychain_file_is_created_once_and_reopened_with_its_password() {
    let guard = TempKeychain::new();
    assert!(!guard.path().exists());
    let first = guard.backend();
    assert!(guard.path().exists());
    first
        .put(&form("https://example.com", "alice"), &fake("fake-one"))
        .expect("save");
    drop(first);

    let reopened = KeychainBackend::file(&guard.path(), TEST_KEYCHAIN_PASSWORD).expect("reopen");
    assert_eq!(
        reopened
            .get(&form("https://example.com", "alice"))
            .expect("read")
            .expect("present")
            .expose(),
        "fake-one"
    );
}

#[test]
fn a_wrong_keychain_password_is_refused_without_naming_it() {
    let guard = TempKeychain::new();
    drop(guard.backend());
    let error = match KeychainBackend::file(&guard.path(), "fake-wrong-password") {
        Ok(_) => panic!("a wrong password must not open the keychain"),
        Err(error) => error,
    };
    assert_eq!(error, PasswordError::AccessDenied);
    assert!(!format!("{error} {error:?}").contains("fake-wrong-password"));
}

#[test]
fn operations_touch_only_the_targeted_keychain() {
    let (first_guard, first) = temp_keychain();
    let (second_guard, second) = temp_keychain();
    let key = form("https://example.com", "alice");
    first.put(&key, &fake("fake-first")).expect("save first");
    second.put(&key, &fake("fake-second")).expect("save second");

    assert_eq!(
        first
            .put(&key, &fake("fake-first-changed"))
            .expect("update"),
        SaveOutcome::Updated
    );
    assert_eq!(
        second.get(&key).expect("read").expect("present").expose(),
        "fake-second"
    );
    assert!(first.delete(&key).expect("delete"));
    assert_eq!(
        second.get(&key).expect("read").expect("present").expose(),
        "fake-second"
    );
    assert_eq!(second.list().expect("list").len(), 1);
    assert!(first.list().expect("list").is_empty());
    assert!(attributes(&first_guard.handle()).is_empty());
    assert_eq!(attributes(&second_guard.handle()).len(), 1);
}

#[test]
fn other_items_are_skipped_never_listed_or_removed() {
    let (guard, backend) = temp_keychain();
    let handle = guard.handle();
    add_foreign_item(
        &handle,
        "com.example.other-app",
        "v1|form|https://example.com||alice",
    );
    add_foreign_item(&handle, KEYCHAIN_SERVICE, "not a login");
    add_foreign_item(
        &handle,
        KEYCHAIN_SERVICE,
        "v2|form|https://example.com||alice",
    );
    add_foreign_item(
        &handle,
        KEYCHAIN_SERVICE,
        "v1|form|https://EXAMPLE.com||alice",
    );
    add_foreign_item(
        &handle,
        KEYCHAIN_SERVICE,
        "v1|form|http://example.com||alice",
    );
    add_foreign_item(
        &handle,
        KEYCHAIN_SERVICE,
        "v1|form|https://example.com|realm|alice",
    );
    add_foreign_item(
        &handle,
        KEYCHAIN_SERVICE,
        "v1|form|https://example.com||a%7cb",
    );

    let store = PasswordStore::new(backend);
    store
        .save(&form("https://example.com", "alice"), &fake("fake-ours"))
        .expect("save");
    store
        .save(
            &http_auth("https://example.org", "R", "bob"),
            &fake("fake-ours-2"),
        )
        .expect("save");
    assert_eq!(store.all().expect("all").len(), 2);

    assert_eq!(store.remove_all().expect("remove all"), 2);
    assert!(store.all().expect("all").is_empty());
    let remaining = attributes(&handle);
    assert_eq!(remaining.len(), 7, "every foreign item is still there");
    assert!(
        remaining
            .iter()
            .all(|item| item["labl"] == "foreign test item")
    );
}

#[test]
fn a_locked_keychain_reports_locked_and_still_lists() {
    let (guard, backend) = temp_keychain();
    let key = form("https://example.com", "locked-user");
    backend.put(&key, &fake("fake-before-lock")).expect("save");

    // Lock the temporary keychain: it locks itself after one idle second.
    let mut handle = guard.handle();
    let mut settings = KeychainSettings::new();
    settings.set_lock_interval(Some(1));
    handle.set_settings(&settings).expect("settings");
    sleep(Duration::from_millis(2_500));

    let error = backend
        .get(&key)
        .expect_err("a locked keychain cannot be read");
    assert_eq!(error, PasswordError::Locked);
    assert!(!format!("{error} {error:?}").contains("locked-user"));
    assert_eq!(
        backend
            .put(&form("https://example.com", "new"), &fake("fake-new"))
            .expect_err("a locked keychain cannot be written"),
        PasswordError::Locked
    );
    let listed = backend.list().expect("listing reads attributes only");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].key, key);

    handle.unlock(Some(TEST_KEYCHAIN_PASSWORD)).expect("unlock");
    let mut settings = KeychainSettings::new();
    settings.set_lock_interval(None);
    handle.set_settings(&settings).expect("settings");
    assert_eq!(
        backend.get(&key).expect("read").expect("present").expose(),
        "fake-before-lock"
    );
}

#[test]
fn the_guard_removes_the_keychain_file() {
    let guard = TempKeychain::new();
    drop(guard.backend());
    let dir = guard.dir().to_path_buf();
    assert!(dir.exists());
    drop(guard);
    assert!(!dir.exists());
}

#[test]
fn the_guard_removes_the_keychain_file_when_a_test_panics() {
    let mut seen = None;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let guard = TempKeychain::new();
        drop(guard.backend());
        seen = Some(guard.dir().to_path_buf());
        panic!("deliberate test panic");
    }));
    assert!(outcome.is_err());
    let dir = seen.expect("the directory was created");
    assert!(!dir.exists());
}
