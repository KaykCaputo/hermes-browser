//! Reading and writing login CSV files. Every row here is synthetic; every password is fake.

use mercury_passwords::{
    CsvImport, LoginKey, Origin, Password, PasswordError, parse_logins_csv, write_logins_csv,
};

fn parse(text: &str) -> CsvImport {
    parse_logins_csv(text).expect("the file parses")
}

fn summary(import: &CsvImport) -> Vec<(String, String, String)> {
    import
        .logins
        .iter()
        .map(|login| {
            (
                login.url.clone(),
                login.username.clone(),
                login.password.expose().to_owned(),
            )
        })
        .collect()
}

fn row(url: &str, username: &str, password: &str) -> (String, String, String) {
    (url.to_owned(), username.to_owned(), password.to_owned())
}

fn skipped(import: &CsvImport) -> Vec<(usize, &str)> {
    import
        .skipped
        .iter()
        .map(|skip| (skip.line, skip.reason.as_str()))
        .collect()
}

#[test]
fn chrome_export_with_note() {
    let import = parse(
        "name,url,username,password,note\n\
         example.com,https://example.com/login,alice,fake-one,my note\n\
         example.org,https://example.org/,,fake-two,\n",
    );
    assert_eq!(
        summary(&import),
        [
            row("https://example.com/login", "alice", "fake-one"),
            row("https://example.org/", "", "fake-two"),
        ]
    );
    assert_eq!(import.logins[0].name.as_deref(), Some("example.com"));
    assert_eq!(import.logins[0].note.as_deref(), Some("my note"));
    assert_eq!(import.logins[1].note, None);
    assert_eq!(import.logins[0].http_realm, None);
    assert!(import.skipped.is_empty());
}

#[test]
fn chrome_export_without_note_and_with_any_case_and_order() {
    let import = parse("URL,Name,Password,UserName\r\nhttps://example.com,ex,fake-pw,bob\r\n");
    assert_eq!(
        summary(&import),
        [row("https://example.com", "bob", "fake-pw")]
    );
    assert_eq!(import.logins[0].name.as_deref(), Some("ex"));
    assert_eq!(import.logins[0].note, None);
}

#[test]
fn firefox_export_with_http_realm() {
    let import = parse(
        "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\",\"timeCreated\",\"timeLastUsed\",\"timePasswordChanged\"\r\n\
         \"https://example.com\",\"alice\",\"fake-form\",,\"https://example.com\",\"{0d3b4e2a-1111-4c2b-9a77-000000000001}\",\"1700000000000\",\"1700000000000\",\"1700000000000\"\r\n\
         \"https://intranet.example.com:8443\",\"bob\",\"fake-auth\",\"Staff \"\"only\"\"\",,\"{0d3b4e2a-1111-4c2b-9a77-000000000002}\",\"1700000000000\",\"1700000000000\",\"1700000000000\"",
    );
    assert_eq!(
        summary(&import),
        [
            row("https://example.com", "alice", "fake-form"),
            row("https://intranet.example.com:8443", "bob", "fake-auth"),
        ]
    );
    assert_eq!(import.logins[0].http_realm, None);
    assert_eq!(
        import.logins[1].http_realm.as_deref(),
        Some("Staff \"only\"")
    );
    assert_eq!(import.logins[0].name, None);
    assert!(import.skipped.is_empty());
}

#[test]
fn safari_export_with_and_without_notes() {
    let import = parse(
        "Title,URL,Username,Password,Notes,OTPAuth\n\
         Example (alice),https://example.com/,alice,fake-safari,a note,otpauth://totp?secret=FAKE\n",
    );
    assert_eq!(
        summary(&import),
        [row("https://example.com/", "alice", "fake-safari")]
    );
    assert_eq!(import.logins[0].name.as_deref(), Some("Example (alice)"));
    assert_eq!(import.logins[0].note.as_deref(), Some("a note"));

    let older =
        parse("Title,Url,Username,Password,OTPAuth\nEx,https://example.com/,bob,fake-old,\n");
    assert_eq!(
        summary(&older),
        [row("https://example.com/", "bob", "fake-old")]
    );
    assert_eq!(older.logins[0].note, None);
}

#[test]
fn quoted_fields_keep_commas_quotes_and_line_breaks() {
    let import = parse(
        "name,url,username,password\n\
         \"a, b\",https://example.com,\"say \"\"hi\"\"\",\"fake,with\nLF and\r\nCRLF\"\n\
         x,https://example.org,\"\",\"\"\"\"\n",
    );
    assert_eq!(
        summary(&import),
        [
            row(
                "https://example.com",
                "say \"hi\"",
                "fake,with\nLF and\r\nCRLF"
            ),
            row("https://example.org", "", "\""),
        ]
    );
    assert_eq!(import.logins[0].name.as_deref(), Some("a, b"));
}

#[test]
fn byte_order_mark_trailing_newline_and_blank_lines_are_handled() {
    let expected = [
        row("https://example.com", "alice", "fake-1"),
        row("https://example.org", "bob", "fake-2"),
    ];
    for text in [
        "name,url,username,password\nx,https://example.com,alice,fake-1\ny,https://example.org,bob,fake-2",
        "name,url,username,password\nx,https://example.com,alice,fake-1\ny,https://example.org,bob,fake-2\n",
        "\u{feff}name,url,username,password\r\nx,https://example.com,alice,fake-1\r\ny,https://example.org,bob,fake-2\r\n",
        "\n\nname,url,username,password\n\nx,https://example.com,alice,fake-1\n\r\n\ny,https://example.org,bob,fake-2\n\n",
    ] {
        let import = parse(text);
        assert_eq!(summary(&import), expected, "{text:?}");
        assert!(import.skipped.is_empty(), "{text:?}");
    }
}

#[test]
fn unusable_rows_are_skipped_with_their_line_and_a_reason_without_values() {
    let text = "name,url,username,password,note\n\
                ok,https://example.com,alice,fake-ok,\n\
                \n\
                bad-url,not a url at all,user-secret-1,fake-secret-1,\n\
                ftp,ftp://example.com,user-secret-2,fake-secret-2,\n\
                http,http://example.com,user-secret-3,fake-secret-3,\n\
                empty,https://example.com,user-secret-4,,\n\
                short,https://example.com,user-secret-5\n\
                long,https://example.com,user-secret-6,fake-secret-6,note,extra\n\
                app,android://hash-secret@com.example.app/,user-secret-7,fake-secret-7,\n\
                \"multi\nline\",https://example.org,carol,fake-multi,\n\
                last,data:text/plain,user-secret-8,fake-secret-8,\n";
    let import = parse(text);
    assert_eq!(
        summary(&import),
        [
            row("https://example.com", "alice", "fake-ok"),
            row("https://example.org", "carol", "fake-multi"),
        ]
    );
    let lines: Vec<usize> = import.skipped.iter().map(|skip| skip.line).collect();
    assert_eq!(lines, [4, 5, 6, 7, 8, 9, 10, 13]);
    for skip in &import.skipped {
        assert!(!skip.reason.is_empty());
        assert!(!skip.reason.contains("secret"), "{}", skip.reason);
        assert!(!skip.reason.contains("example"), "{}", skip.reason);
    }
    let reasons = skipped(&import);
    for (line, reason) in &reasons[..3] {
        assert!(reason.contains("URL"), "line {line}: {reason}");
    }
    assert!(
        reasons[3].1.contains("password is empty"),
        "{}",
        reasons[3].1
    );
    assert!(
        reasons[4].1.contains("number of fields"),
        "{}",
        reasons[4].1
    );
    assert!(
        reasons[5].1.contains("number of fields"),
        "{}",
        reasons[5].1
    );
    assert!(reasons[6].1.contains("Android"), "{}", reasons[6].1);
    assert!(reasons[7].1.contains("URL"), "{}", reasons[7].1);
}

#[test]
fn malformed_quoting_skips_only_that_row() {
    let import = parse(
        "name,url,username,password\n\
         a,https://example.com,\"alice\"x,fake-1\n\
         b,https://example.org,bob,fake-2\n\
         c,https://example.net,carol,\"fake-unterminated\n",
    );
    assert_eq!(
        summary(&import),
        [row("https://example.org", "bob", "fake-2")]
    );
    assert_eq!(
        import
            .skipped
            .iter()
            .map(|skip| skip.line)
            .collect::<Vec<_>>(),
        [2, 4]
    );
    for skip in &import.skipped {
        assert!(!skip.reason.contains("fake"));
        assert!(!skip.reason.contains("alice"));
    }
}

#[test]
fn an_unknown_or_missing_header_is_an_error_without_values() {
    for text in [
        "",
        "\n\n",
        "url,user,pass\nhttps://example.com,secret-user,secret-pass\n",
        "name,url,username,password,secret-extra\n",
        "name,url,username,password,password\n",
        "url,username,password\n",
        "Title,URL,Username,Password,Notes\n",
    ] {
        match parse_logins_csv(text) {
            Err(PasswordError::InvalidInput(reason)) => {
                assert!(!reason.contains("secret"), "{reason}")
            }
            other => panic!(
                "{text:?} gave {:?}",
                other.map(|import| import.logins.len())
            ),
        }
    }
}

#[test]
fn a_header_alone_is_an_empty_import() {
    let import = parse("name,url,username,password,note\n");
    assert!(import.logins.is_empty() && import.skipped.is_empty());
}

#[test]
fn debug_output_of_an_import_hides_passwords() {
    let import = parse("name,url,username,password\nx,https://example.com,alice,fake-hidden\n");
    let shown = format!("{import:?}");
    assert!(!shown.contains("fake-hidden"), "{shown}");
}

fn key(origin: &str, username: &str) -> LoginKey {
    LoginKey::form(Origin::parse(origin).expect("valid origin"), username)
}

fn fake(text: &str) -> Password {
    Password::new(text.to_owned())
}

#[test]
fn writer_produces_chrome_format() {
    let logins = [
        (key("https://example.com", "alice"), fake("fake-one")),
        (key("https://example.com:8443", ""), fake("fake-two")),
    ];
    let csv = write_logins_csv(logins.iter().map(|(key, password)| (key, password)));
    assert_eq!(
        csv.as_str(),
        "name,url,username,password,note\n\
         example.com,https://example.com,alice,fake-one,\n\
         example.com,https://example.com:8443,,fake-two,\n"
    );
}

#[test]
fn writer_quotes_only_when_needed() {
    let logins = [(
        key("https://example.com", "a,b"),
        fake("say \"hi\"\r\nnext line"),
    )];
    let csv = write_logins_csv(logins.iter().map(|(key, password)| (key, password)));
    assert_eq!(
        csv.as_str(),
        "name,url,username,password,note\n\
         example.com,https://example.com,\"a,b\",\"say \"\"hi\"\"\r\nnext line\",\n"
    );
}

#[test]
fn writing_then_parsing_gives_back_the_logins() {
    let printable: String = (0x20u8..=0x7e).map(char::from).collect();
    let logins = [
        (
            key("https://example.com", "comma, in name"),
            fake("fake,comma"),
        ),
        (
            key("https://example.com", "\"quoted\""),
            fake("\"fake\" \"\""),
        ),
        (
            // User names cannot hold line breaks (`save` refuses them); passwords can.
            key("https://example.org", "plain"),
            fake("fake\r\ncrlf\nlf"),
        ),
        (
            key("https://xn--bcher-kva.example", "Jürgen 名前"),
            fake("fake-pässwörd-🔑"),
        ),
        (key("http://localhost:3000", ""), fake(&printable)),
        (
            key("https://[2001:db8::1]:8443", "  spaces  "),
            fake("  fake  "),
        ),
    ];
    let csv = write_logins_csv(logins.iter().map(|(key, password)| (key, password)));
    let import = parse(&csv);
    assert!(import.skipped.is_empty());
    assert_eq!(import.logins.len(), logins.len());
    for ((key, password), login) in logins.iter().zip(&import.logins) {
        let origin = Origin::parse(&login.url).expect("the written URL parses");
        assert_eq!(origin, key.origin);
        assert_eq!(login.username, key.username);
        assert_eq!(login.password.expose(), password.expose());
        assert_eq!(login.name.as_deref(), Some(key.origin.host()));
        assert_eq!(login.note, None);
        assert_eq!(login.http_realm, None);
    }
}

#[test]
fn rows_that_save_would_refuse_are_skipped_with_their_line() {
    let long_username = "u".repeat(mercury_passwords::MAX_USERNAME_BYTES + 1);
    let long_password = "p".repeat(mercury_passwords::MAX_PASSWORD_BYTES + 1);
    let text = format!(
        "name,url,username,password\n\
         ok,https://example.com,alice,fake-ok\n\
         tab,https://example.com,\"tab\tsecret\",fake-1\n\
         nl,https://example.com,\"line\nsecret\",fake-2\n\
         long-user,https://example.com,{long_username},fake-3\n\
         long-pass,https://example.com,bob,{long_password}\n\
         also-ok,https://example.org,carol,fake-ok-2\n"
    );
    let import = parse(&text);
    assert_eq!(
        summary(&import),
        [
            row("https://example.com", "alice", "fake-ok"),
            row("https://example.org", "carol", "fake-ok-2"),
        ]
    );
    assert_eq!(
        skipped(&import),
        [
            (3, "the user name contains a control character"),
            (4, "the user name contains a control character"),
            (6, "the user name is too long"),
            (7, "the password is too long"),
        ]
    );
}

#[test]
fn firefox_rows_with_an_unacceptable_realm_are_skipped() {
    let header = "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\",\"timeCreated\",\"timeLastUsed\",\"timePasswordChanged\"";
    let long_realm = "r".repeat(mercury_passwords::MAX_REALM_BYTES + 1);
    let text = format!(
        "{header}\r\n\
         \"https://example.com\",\"a\",\"fake-1\",\"Realm\u{7}secret\",,\"g1\",\"1\",\"1\",\"1\"\r\n\
         \"https://example.com\",\"b\",\"fake-2\",\"{long_realm}\",,\"g2\",\"1\",\"1\",\"1\"\r\n\
         \"https://example.com\",\"c\",\"fake-3\",\"Fine realm\",,\"g3\",\"1\",\"1\",\"1\"\r\n"
    );
    let import = parse(&text);
    assert_eq!(
        summary(&import),
        [row("https://example.com", "c", "fake-3")]
    );
    assert_eq!(
        skipped(&import),
        [
            (2, "the realm contains a control character"),
            (3, "the realm is too long"),
        ]
    );
}

#[test]
fn nul_bytes_and_lone_carriage_returns_are_field_content() {
    // A NUL or a lone CR is not a separator. In a user name either is refused, as `save` would;
    // in a password it is kept exactly.
    let import = parse(
        "name,url,username,password\n\
         a,https://example.com,nul\0name,fake-1\n\
         b,https://example.com,cr\rname,fake-2\n\
         c,https://example.com,alice,fake\0with\rboth\n",
    );
    assert_eq!(
        summary(&import),
        [row("https://example.com", "alice", "fake\0with\rboth")]
    );
    assert_eq!(
        skipped(&import),
        [
            (2, "the user name contains a control character"),
            (3, "the user name contains a control character"),
        ]
    );

    // A file that ends its lines with lone CRs (classic Mac OS) is one line: its header does not
    // match any format, so it is refused rather than read wrongly.
    assert!(matches!(
        parse_logins_csv("name,url,username,password\rx,https://example.com,alice,fake\r"),
        Err(PasswordError::InvalidInput(_))
    ));
}

#[test]
fn values_that_look_like_spreadsheet_formulas_round_trip_unchanged() {
    let logins = [
        (
            key("https://example.com", "=user"),
            fake("=HYPERLINK(\"x\")"),
        ),
        (key("https://example.com", "+user"), fake("+1+1")),
        (key("https://example.com", "-user"), fake("-2+3")),
        (key("https://example.com", "@user"), fake("@SUM(A1)")),
    ];
    let csv = write_logins_csv(logins.iter().map(|(key, password)| (key, password)));
    let import = parse(&csv);
    assert!(import.skipped.is_empty());
    assert_eq!(import.logins.len(), logins.len());
    for ((key, password), login) in logins.iter().zip(&import.logins) {
        assert_eq!(login.username, key.username);
        assert_eq!(login.password.expose(), password.expose());
    }
}

#[test]
fn debug_output_of_a_csv_login_hides_user_name_password_and_note() {
    let import = parse(
        "name,url,username,password,note\n\
         x,https://example.com,alice-secret-name,fake-hidden,note-secret-text\n",
    );
    let shown = format!("{:?}", import.logins[0]);
    assert!(shown.contains("https://example.com"), "{shown}");
    assert!(!shown.contains("alice-secret-name"), "{shown}");
    assert!(!shown.contains("fake-hidden"), "{shown}");
    assert!(!shown.contains("note-secret-text"), "{shown}");
    assert!(shown.contains("<redacted, 17 chars>"), "{shown}");
}
