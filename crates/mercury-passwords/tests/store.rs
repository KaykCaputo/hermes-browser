//! Policy in `PasswordStore`: validation and exact-origin lookup. All passwords are fake.

use mercury_passwords::{
    LoginKey, LoginKind, MAX_PASSWORD_BYTES, MAX_REALM_BYTES, MAX_USERNAME_BYTES, MemoryBackend,
    Origin, Password, PasswordError, PasswordStore, SaveOutcome,
};

fn origin(text: &str) -> Origin {
    Origin::parse(text).expect("test origin is valid")
}

fn form(origin_text: &str, username: &str) -> LoginKey {
    LoginKey::form(origin(origin_text), username)
}

fn http_auth(origin_text: &str, realm: &str, username: &str) -> LoginKey {
    LoginKey::http_auth(origin(origin_text), realm, username)
}

fn fake(text: &str) -> Password {
    Password::new(text.to_owned())
}

fn store() -> PasswordStore<MemoryBackend> {
    PasswordStore::new(MemoryBackend::new())
}

fn usernames(summaries: &[mercury_passwords::LoginSummary]) -> Vec<&str> {
    summaries
        .iter()
        .map(|summary| summary.key.username.as_str())
        .collect()
}

#[test]
fn key_constructors_fill_the_fields() {
    let key = form("https://example.com", "alice");
    assert_eq!(key.kind, LoginKind::Form);
    assert_eq!(key.username, "alice");
    let key = http_auth("https://example.com", "Staff", "bob");
    assert_eq!(
        key.kind,
        LoginKind::HttpAuth {
            realm: "Staff".to_owned()
        }
    );
}

#[test]
fn saving_and_reading_back() {
    let store = store();
    let key = form("https://example.com", "alice");
    assert_eq!(
        store.save(&key, &fake("fake-one")).expect("save"),
        SaveOutcome::Created
    );
    assert_eq!(
        store
            .password(&key)
            .expect("read")
            .expect("present")
            .expose(),
        "fake-one"
    );
    assert!(store.remove(&key).expect("remove"));
    assert!(store.password(&key).expect("read").is_none());
}

#[test]
fn empty_password_is_refused() {
    let store = store();
    let result = store.save(&form("https://example.com", "alice"), &fake(""));
    assert!(matches!(result, Err(PasswordError::InvalidInput(_))));
    assert!(store.all().expect("all").is_empty());
}

#[test]
fn empty_username_is_allowed() {
    let store = store();
    let key = form("https://example.com", "");
    assert_eq!(
        store.save(&key, &fake("fake-pin")).expect("save"),
        SaveOutcome::Created
    );
    assert_eq!(
        usernames(
            &store
                .logins_for(&origin("https://example.com"))
                .expect("list")
        ),
        [""]
    );
}

#[test]
fn length_limits_are_enforced() {
    let store = store();
    let at_limit = "u".repeat(MAX_USERNAME_BYTES);
    store
        .save(&form("https://example.com", &at_limit), &fake("fake"))
        .expect("a user name at the limit is accepted");
    let over = "u".repeat(MAX_USERNAME_BYTES + 1);
    assert!(matches!(
        store.save(&form("https://example.com", &over), &fake("fake")),
        Err(PasswordError::InvalidInput(_))
    ));
    // The limit counts bytes: 'é' is two bytes in UTF-8.
    let multibyte_over = "é".repeat(MAX_USERNAME_BYTES / 2 + 1);
    assert!(matches!(
        store.save(&form("https://example.com", &multibyte_over), &fake("fake")),
        Err(PasswordError::InvalidInput(_))
    ));

    let realm_over = "r".repeat(MAX_REALM_BYTES + 1);
    assert!(matches!(
        store.save(
            &http_auth("https://example.com", &realm_over, "u"),
            &fake("fake")
        ),
        Err(PasswordError::InvalidInput(_))
    ));
    store
        .save(
            &http_auth("https://example.com", &"r".repeat(MAX_REALM_BYTES), "u"),
            &fake("fake"),
        )
        .expect("a realm at the limit is accepted");

    let password_at_limit = "p".repeat(MAX_PASSWORD_BYTES);
    store
        .save(
            &form("https://example.com", "long"),
            &fake(&password_at_limit),
        )
        .expect("a password at the limit is accepted");
    let password_over = "p".repeat(MAX_PASSWORD_BYTES + 1);
    assert!(matches!(
        store.save(&form("https://example.com", "long"), &fake(&password_over)),
        Err(PasswordError::InvalidInput(_))
    ));
}

#[test]
fn control_characters_are_refused_in_usernames_and_realms() {
    let store = store();
    for bad in [
        "nul\0byte",
        "tab\tname",
        "line\nbreak",
        "cr\rname",
        "del\u{7f}",
        "c1\u{85}",
    ] {
        assert!(
            matches!(
                store.save(&form("https://example.com", bad), &fake("fake")),
                Err(PasswordError::InvalidInput(_))
            ),
            "user name {bad:?} should be refused"
        );
        assert!(
            matches!(
                store.save(&http_auth("https://example.com", bad, "u"), &fake("fake")),
                Err(PasswordError::InvalidInput(_))
            ),
            "realm {bad:?} should be refused"
        );
        assert!(matches!(
            store.password(&form("https://example.com", bad)),
            Err(PasswordError::InvalidInput(_))
        ));
        assert!(matches!(
            store.remove(&form("https://example.com", bad)),
            Err(PasswordError::InvalidInput(_))
        ));
    }
    assert!(store.all().expect("all").is_empty());
}

#[test]
fn errors_never_contain_the_input() {
    let store = store();
    let secret_name = "nul\0secret-user-name";
    let error = store
        .save(
            &form("https://example.com", secret_name),
            &fake("fake-secret-value"),
        )
        .expect_err("refused");
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains("secret"), "{shown}");
}

#[test]
fn logins_for_returns_only_form_logins_of_exactly_that_origin_sorted() {
    let store = store();
    for (origin_text, username) in [
        ("https://example.com", "carol"),
        ("https://example.com", "alice"),
        ("https://example.com", "bob"),
        ("https://www.example.com", "dave"),
        ("https://example.com:8443", "erin"),
        ("https://example.com.evil.test", "frank"),
        ("http://localhost", "gina"),
    ] {
        store
            .save(&form(origin_text, username), &fake("fake"))
            .expect("save");
    }
    store
        .save(
            &http_auth("https://example.com", "Realm", "henry"),
            &fake("fake"),
        )
        .expect("save");

    let logins = store
        .logins_for(&origin("https://example.com"))
        .expect("list");
    assert_eq!(usernames(&logins), ["alice", "bob", "carol"]);
    assert!(
        logins
            .iter()
            .all(|summary| summary.key.kind == LoginKind::Form)
    );
    assert_eq!(
        usernames(
            &store
                .logins_for(&origin("https://www.example.com"))
                .expect("list")
        ),
        ["dave"]
    );
    assert!(
        store
            .logins_for(&origin("https://example.org"))
            .expect("list")
            .is_empty()
    );
}

#[test]
fn http_auth_logins_for_matches_origin_and_realm_exactly() {
    let store = store();
    store
        .save(
            &http_auth("https://example.com", "Staff", "zoe"),
            &fake("fake"),
        )
        .expect("save");
    store
        .save(
            &http_auth("https://example.com", "Staff", "amy"),
            &fake("fake"),
        )
        .expect("save");
    store
        .save(
            &http_auth("https://example.com", "staff", "lower"),
            &fake("fake"),
        )
        .expect("save");
    store
        .save(
            &http_auth("https://www.example.com", "Staff", "www"),
            &fake("fake"),
        )
        .expect("save");
    store
        .save(&form("https://example.com", "formuser"), &fake("fake"))
        .expect("save");

    let found = store
        .http_auth_logins_for(&origin("https://example.com"), "Staff")
        .expect("list");
    assert_eq!(usernames(&found), ["amy", "zoe"]);
    assert!(
        store
            .http_auth_logins_for(&origin("https://example.com"), "Other")
            .expect("list")
            .is_empty()
    );
}

#[test]
fn all_lists_every_login_without_secrets() {
    let store = store();
    store
        .save(&form("https://b.example", "x"), &fake("fake"))
        .expect("save");
    store
        .save(&form("https://a.example", "y"), &fake("fake"))
        .expect("save");
    let all = store.all().expect("all");
    let origins: Vec<&str> = all
        .iter()
        .map(|summary| summary.key.origin.as_str())
        .collect();
    assert_eq!(origins, ["https://a.example", "https://b.example"]);
}

#[test]
fn password_debug_output_hides_the_value() {
    let password = fake("fake-do-not-print");
    let shown = format!("{password:?}");
    assert_eq!(shown, "Password(…)");
    let cloned = password.clone();
    assert_eq!(cloned.expose(), "fake-do-not-print");
}
