//! Policy in `PasswordStore`: validation and exact-origin lookup. All passwords are fake.

use mercury_passwords::{
    LoginKey, LoginKind, LoginSummary, MAX_PASSWORD_BYTES, MAX_REALM_BYTES, MAX_USERNAME_BYTES,
    MemoryBackend, Origin, Password, PasswordError, PasswordStore, SaveOutcome, SecretBackend,
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
    }
    assert!(store.all().expect("all").is_empty());
}

#[test]
fn every_listed_login_can_be_read_and_removed() {
    // A backend may hold keys `save` would refuse (written by another tool or an older
    // version). Whatever `all()` returns must be readable and removable.
    let backend = MemoryBackend::new();
    let odd = [
        form("https://example.com", "tab\tname"),
        form("https://example.com", "nul\0name"),
        http_auth("https://example.com", "line\nbreak", "u"),
        form("https://example.com", &"u".repeat(MAX_USERNAME_BYTES + 1)),
    ];
    for key in &odd {
        backend
            .put(key, &fake("fake-odd"))
            .expect("the backend stores anything");
    }
    let store = PasswordStore::new(backend);
    let listed = store.all().expect("all");
    assert_eq!(listed.len(), odd.len());
    for summary in &listed {
        assert_eq!(
            store
                .password(&summary.key)
                .expect("a listed login can be read")
                .expect("present")
                .expose(),
            "fake-odd"
        );
    }
    for summary in &listed {
        assert!(
            store
                .remove(&summary.key)
                .expect("a listed login can be removed")
        );
    }
    assert!(store.all().expect("all").is_empty());
}

/// A memory backend whose `delete` fails for one user name.
struct FailingDelete {
    inner: MemoryBackend,
    fail_for: &'static str,
}

impl SecretBackend for FailingDelete {
    fn put(&self, key: &LoginKey, password: &Password) -> Result<SaveOutcome, PasswordError> {
        self.inner.put(key, password)
    }
    fn get(&self, key: &LoginKey) -> Result<Option<Password>, PasswordError> {
        self.inner.get(key)
    }
    fn delete(&self, key: &LoginKey) -> Result<bool, PasswordError> {
        if key.username == self.fail_for {
            return Err(PasswordError::Os {
                code: -1,
                message: "simulated failure".to_owned(),
            });
        }
        self.inner.delete(key)
    }
    fn list(&self) -> Result<Vec<LoginSummary>, PasswordError> {
        self.inner.list()
    }
}

#[test]
fn remove_all_continues_past_an_error_and_reports_the_count_with_the_first_error() {
    let store = PasswordStore::new(FailingDelete {
        inner: MemoryBackend::new(),
        fail_for: "bob",
    });
    for username in ["alice", "bob", "carol", "dave"] {
        store
            .save(&form("https://example.com", username), &fake("fake"))
            .expect("save");
    }
    let error = store.remove_all().expect_err("one login cannot be removed");
    assert_eq!(
        error,
        PasswordError::RemoveAllIncomplete {
            removed: 3,
            first_error: Box::new(PasswordError::Os {
                code: -1,
                message: "simulated failure".to_owned(),
            }),
        }
    );
    assert!(error.to_string().contains('3'), "{error}");
    assert_eq!(usernames(&store.all().expect("all")), ["bob"]);
}

#[test]
fn debug_output_shows_origin_and_kind_but_not_the_user_name() {
    let key = http_auth("https://example.com:8443", "Staff", "alice-secret-name");
    let shown = format!("{key:?}");
    assert!(shown.contains("https://example.com:8443"), "{shown}");
    assert!(
        shown.contains("HttpAuth") && shown.contains("Staff"),
        "{shown}"
    );
    assert!(!shown.contains("alice-secret-name"), "{shown}");
    assert!(shown.contains("<redacted, 17 chars>"), "{shown}");

    let store = store();
    store
        .save(
            &form("https://example.com", "bob-secret-name"),
            &fake("fake"),
        )
        .expect("save");
    let summary = format!("{:?}", store.all().expect("all"));
    assert!(!summary.contains("bob-secret-name"), "{summary}");
    assert!(
        summary.contains("<redacted, 15 chars>") && summary.contains("Form"),
        "{summary}"
    );

    let empty = format!("{:?}", form("https://example.com", ""));
    assert!(empty.contains("<redacted, 0 chars>"), "{empty}");
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
