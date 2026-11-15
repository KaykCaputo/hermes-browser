//! The contract every `SecretBackend` must meet. `tests/backend_contract.rs` runs each function
//! against every backend. All passwords here are obviously fake.

use std::thread::sleep;
use std::time::Duration;

use mercury_passwords::{
    LoginKey, LoginKind, LoginSummary, Origin, Password, PasswordStore, SaveOutcome, SecretBackend,
};

pub fn origin(text: &str) -> Origin {
    Origin::parse(text).expect("test origin is valid")
}

pub fn form(origin_text: &str, username: &str) -> LoginKey {
    LoginKey {
        origin: origin(origin_text),
        kind: LoginKind::Form,
        username: username.to_owned(),
    }
}

pub fn http_auth(origin_text: &str, realm: &str, username: &str) -> LoginKey {
    LoginKey {
        origin: origin(origin_text),
        kind: LoginKind::HttpAuth {
            realm: realm.to_owned(),
        },
        username: username.to_owned(),
    }
}

pub fn fake(text: &str) -> Password {
    Password::new(text.to_owned())
}

fn read(backend: &impl SecretBackend, key: &LoginKey) -> Option<String> {
    backend
        .get(key)
        .expect("reading a login should work")
        .map(|password| password.expose().to_owned())
}

fn keys(backend: &impl SecretBackend) -> Vec<LoginKey> {
    let mut keys: Vec<LoginKey> = backend
        .list()
        .expect("listing should work")
        .into_iter()
        .map(|summary| summary.key)
        .collect();
    keys.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    keys
}

fn summary(backend: &impl SecretBackend, key: &LoginKey) -> LoginSummary {
    backend
        .list()
        .expect("listing should work")
        .into_iter()
        .find(|summary| &summary.key == key)
        .expect("the login should be listed")
}

pub fn empty_backend_lists_nothing(backend: &impl SecretBackend) {
    assert!(keys(backend).is_empty());
    assert_eq!(read(backend, &form("https://example.com", "alice")), None);
    assert!(
        !backend
            .delete(&form("https://example.com", "alice"))
            .expect("delete works")
    );
}

pub fn create_read_update_delete(backend: &impl SecretBackend) {
    let key = form("https://example.com", "alice");

    let outcome = backend
        .put(&key, &fake("fake-password-one"))
        .expect("create");
    assert_eq!(outcome, SaveOutcome::Created);
    assert_eq!(read(backend, &key).as_deref(), Some("fake-password-one"));
    assert_eq!(keys(backend), vec![key.clone()]);

    let outcome = backend
        .put(&key, &fake("fake-password-two"))
        .expect("update");
    assert_eq!(outcome, SaveOutcome::Updated);
    assert_eq!(read(backend, &key).as_deref(), Some("fake-password-two"));
    assert_eq!(keys(backend), vec![key.clone()]);

    let outcome = backend
        .put(&key, &fake("fake-password-two"))
        .expect("same value");
    assert_eq!(outcome, SaveOutcome::Unchanged);
    assert_eq!(read(backend, &key).as_deref(), Some("fake-password-two"));
    assert_eq!(keys(backend), vec![key.clone()]);

    assert!(backend.delete(&key).expect("delete"));
    assert_eq!(read(backend, &key), None);
    assert!(keys(backend).is_empty());
    assert!(!backend.delete(&key).expect("second delete"));
}

pub fn timestamps_come_from_the_item(backend: &impl SecretBackend) {
    let key = form("https://example.com", "alice");
    backend
        .put(&key, &fake("fake-password-one"))
        .expect("create");
    let created = summary(backend, &key);
    let created_at = created.created_at_ms.expect("creation time is known");
    let modified_at = created.modified_at_ms.expect("modification time is known");
    assert!(
        created_at > 1_700_000_000_000,
        "a plausible Unix time in milliseconds"
    );
    assert!(modified_at >= created_at);

    // The Keychain stores times to the second; wait long enough for a rewrite to show.
    sleep(Duration::from_millis(1_100));
    backend
        .put(&key, &fake("fake-password-one"))
        .expect("same value");
    let unchanged = summary(backend, &key);
    assert_eq!(
        unchanged.modified_at_ms,
        Some(modified_at),
        "Unchanged must not rewrite"
    );

    backend
        .put(&key, &fake("fake-password-two"))
        .expect("update");
    let updated = summary(backend, &key);
    assert_eq!(updated.created_at_ms, Some(created_at));
    assert!(updated.modified_at_ms.expect("known") > modified_at);
}

pub fn two_usernames_on_one_origin(backend: &impl SecretBackend) {
    let alice = form("https://example.com", "alice");
    let bob = form("https://example.com", "bob");
    let upper = form("https://example.com", "Alice");
    let prefix = form("https://example.com", "ali");
    backend.put(&alice, &fake("fake-alice")).expect("alice");
    backend.put(&bob, &fake("fake-bob")).expect("bob");
    backend.put(&upper, &fake("fake-upper")).expect("Alice");
    backend.put(&prefix, &fake("fake-prefix")).expect("ali");

    assert_eq!(read(backend, &alice).as_deref(), Some("fake-alice"));
    assert_eq!(read(backend, &bob).as_deref(), Some("fake-bob"));
    assert_eq!(read(backend, &upper).as_deref(), Some("fake-upper"));
    assert_eq!(read(backend, &prefix).as_deref(), Some("fake-prefix"));
    assert_eq!(keys(backend).len(), 4);

    assert!(backend.delete(&alice).expect("delete alice"));
    assert_eq!(read(backend, &bob).as_deref(), Some("fake-bob"));
    assert_eq!(read(backend, &upper).as_deref(), Some("fake-upper"));
    assert_eq!(keys(backend).len(), 3);
}

pub fn one_username_on_two_origins(backend: &impl SecretBackend) {
    let plain = form("https://example.com", "alice");
    let www = form("https://www.example.com", "alice");
    let port = form("https://example.com:8443", "alice");
    backend.put(&plain, &fake("fake-plain")).expect("plain");
    backend.put(&www, &fake("fake-www")).expect("www");
    backend.put(&port, &fake("fake-port")).expect("port");

    assert_eq!(read(backend, &plain).as_deref(), Some("fake-plain"));
    assert_eq!(read(backend, &www).as_deref(), Some("fake-www"));
    assert_eq!(read(backend, &port).as_deref(), Some("fake-port"));
    assert_eq!(keys(backend).len(), 3);
}

pub fn form_and_http_auth_logins_are_separate(backend: &impl SecretBackend) {
    let form_key = form("https://example.com", "alice");
    let auth_key = http_auth("https://example.com", "Staff area", "alice");
    let other_realm = http_auth("https://example.com", "Other area", "alice");
    let empty_realm = http_auth("https://example.com", "", "alice");
    backend.put(&form_key, &fake("fake-form")).expect("form");
    backend.put(&auth_key, &fake("fake-auth")).expect("auth");
    backend
        .put(&other_realm, &fake("fake-other"))
        .expect("other realm");
    backend
        .put(&empty_realm, &fake("fake-empty-realm"))
        .expect("empty realm");

    assert_eq!(read(backend, &form_key).as_deref(), Some("fake-form"));
    assert_eq!(read(backend, &auth_key).as_deref(), Some("fake-auth"));
    assert_eq!(read(backend, &other_realm).as_deref(), Some("fake-other"));
    assert_eq!(
        read(backend, &empty_realm).as_deref(),
        Some("fake-empty-realm")
    );
    let mut expected = vec![form_key, auth_key, other_realm, empty_realm];
    expected.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    assert_eq!(keys(backend), expected);
}

pub fn hostile_keys_round_trip(backend: &impl SecretBackend) {
    let long_name: String = "a".repeat(150) + &"é".repeat(150);
    assert_eq!(long_name.chars().count(), 300);
    let hostile = [
        form("https://example.com", ""),
        form("https://example.com", "  spaced  name  "),
        form("https://example.com", "tab\there"),
        form("https://example.com", "quote\"s and 'apostrophes'"),
        form("https://example.com", "pipe|percent%25|%7C"),
        form("https://example.com", "v1|form|https://evil.test||x"),
        form("https://example.com", "Jürgen 名前 🔑"),
        form("https://example.com", &long_name),
        http_auth("https://example.com", "realm|with|pipes", "user|name"),
        http_auth("https://example.com", "%7C not an escape", "x"),
        http_auth("https://example.com", "Ünïcödé realm \"quoted\"", ""),
    ];
    for (index, key) in hostile.iter().enumerate() {
        let password = format!("fake-hostile-{index}");
        assert_eq!(
            backend
                .put(key, &fake(&password))
                .expect("hostile key saves"),
            SaveOutcome::Created,
            "key {index} must be a new login"
        );
    }
    let mut expected = hostile.to_vec();
    expected.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    assert_eq!(keys(backend), expected);
    for (index, key) in hostile.iter().enumerate() {
        assert_eq!(read(backend, key), Some(format!("fake-hostile-{index}")));
    }
}

pub fn passwords_round_trip_byte_for_byte(backend: &impl SecretBackend) {
    let printable: String = (0x20u8..=0x7e).map(char::from).collect();
    let samples = [
        printable,
        "fake-pässwörd-密码-🔑".to_owned(),
        " leading and trailing spaces ".to_owned(),
        "line\nbreak\r\nand\ttab".to_owned(),
    ];
    for (index, sample) in samples.iter().enumerate() {
        let key = form("https://example.com", &format!("user{index}"));
        backend.put(&key, &fake(sample)).expect("save");
        let back = backend.get(&key).expect("read").expect("present");
        assert_eq!(
            back.expose().as_bytes(),
            sample.as_bytes(),
            "sample {index}"
        );
    }
}

pub fn remove_all_removes_every_login<B: SecretBackend>(backend: B) -> B {
    let store = PasswordStore::new(backend);
    store
        .save(&form("https://example.com", "alice"), &fake("fake-1"))
        .expect("save");
    store
        .save(&form("https://example.org", "bob"), &fake("fake-2"))
        .expect("save");
    store
        .save(
            &http_auth("https://example.net", "Realm", "carol"),
            &fake("fake-3"),
        )
        .expect("save");
    assert_eq!(store.remove_all().expect("remove all"), 3);
    assert!(store.all().expect("all").is_empty());
    assert_eq!(store.remove_all().expect("remove all again"), 0);
    store.into_backend()
}
