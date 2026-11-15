use std::path::PathBuf;

use mercury_settings::{
    DEFAULT_ENGINE_ID, MAX_CUSTOM_ENGINES, MAX_NEVER_SAVE_ORIGINS, MAX_QUOTED_BYTES, MAX_URL_BYTES,
    Repair, SearchEngine, Settings, canonical_origin, homepage_url, is_plausible_language_tag,
};

fn custom(id: &str, keyword: &str) -> SearchEngine {
    SearchEngine {
        id: id.to_owned(),
        name: format!("Engine {id}"),
        keyword: keyword.to_owned(),
        url_template: format!("https://{id}.example/search?q={{searchTerms}}"),
        suggest_template: None,
    }
}

fn fields(repairs: &[Repair]) -> Vec<&'static str> {
    repairs.iter().map(|repair| repair.field).collect()
}

#[test]
fn valid_settings_need_no_repairs() {
    let mut settings = Settings {
        homepage: "https://example.org/".to_owned(),
        ..Settings::default()
    };
    settings.search.custom_engines = vec![custom("mine", "m")];
    settings.search.default_engine = "mine".to_owned();
    settings.downloads.directory = Some(PathBuf::from("/Users/someone/Desktop"));
    settings.appearance.default_zoom_percent = 30;
    settings.passwords.never_save_origins = vec![
        "https://bank.example".to_owned(),
        "http://localhost:8080".to_owned(),
    ];
    settings.language = Some("zh-Hant-TW".to_owned());

    let (validated, repairs) = settings.clone().validated();

    assert_eq!(repairs, Vec::new());
    assert_eq!(validated, settings);
}

#[test]
fn the_defaults_need_no_repairs() {
    let (validated, repairs) = Settings::default().validated();
    assert!(repairs.is_empty(), "{repairs:?}");
    assert_eq!(validated, Settings::default());
}

#[test]
fn a_zoom_outside_30_to_500_becomes_100() {
    for (zoom, expected) in [
        (0, 100),
        (29, 100),
        (30, 30),
        (500, 500),
        (501, 100),
        (u16::MAX, 100),
    ] {
        let mut settings = Settings::default();
        settings.appearance.default_zoom_percent = zoom;

        let (validated, repairs) = settings.validated();

        assert_eq!(
            validated.appearance.default_zoom_percent, expected,
            "{zoom}"
        );
        let expected_fields: Vec<&str> = if zoom == expected {
            Vec::new()
        } else {
            vec!["appearance.default_zoom_percent"]
        };
        assert_eq!(fields(&repairs), expected_fields, "{zoom}");
    }
}

#[test]
fn a_homepage_that_is_not_http_https_file_or_about_is_cleared() {
    let too_long = format!("https://example.org/{}", "a".repeat(MAX_URL_BYTES));
    for bad in [
        "javascript:alert(1)",
        "data:text/html,<p>hi",
        "blob:https://example.org/1234",
        "ftp://example.org/",
        "not a url at all",
        "example.org",
        too_long.as_str(),
    ] {
        let settings = Settings {
            homepage: bad.to_owned(),
            ..Settings::default()
        };

        let (validated, repairs) = settings.validated();

        assert_eq!(validated.homepage, "", "{bad}");
        assert_eq!(fields(&repairs), vec!["homepage"], "{bad}");
    }

    for good in [
        "",
        "http://example.org/",
        "https://example.org/start?x=1",
        "file:///Users/someone/start.html",
        "about:blank",
    ] {
        let settings = Settings {
            homepage: good.to_owned(),
            ..Settings::default()
        };

        let (validated, repairs) = settings.validated();

        assert_eq!(validated.homepage, good);
        assert!(repairs.is_empty(), "{good}: {repairs:?}");
    }
}

#[test]
fn an_unknown_default_engine_becomes_the_default_one() {
    let mut settings = Settings::default();
    settings.search.default_engine = "altavista".to_owned();

    let (validated, repairs) = settings.validated();

    assert_eq!(validated.search.default_engine, DEFAULT_ENGINE_ID);
    assert_eq!(fields(&repairs), vec!["search.default_engine"]);
}

#[test]
fn a_default_engine_that_was_dropped_becomes_the_default_one() {
    let mut settings = Settings::default();
    let mut broken = custom("broken", "b");
    broken.url_template = "ftp://broken.example/{searchTerms}".to_owned();
    settings.search.custom_engines = vec![broken];
    settings.search.default_engine = "broken".to_owned();

    let (validated, repairs) = settings.validated();

    assert!(validated.search.custom_engines.is_empty());
    assert_eq!(validated.search.default_engine, DEFAULT_ENGINE_ID);
    assert_eq!(
        fields(&repairs),
        vec!["search.custom_engines", "search.default_engine"]
    );
}

#[test]
fn unusable_custom_engines_are_dropped() {
    let mut no_placeholder = custom("noplace", "np");
    no_placeholder.url_template = "https://noplace.example/search".to_owned();
    let mut bad_id = custom("ok", "bad");
    bad_id.id = "Has Spaces".to_owned();

    let mut settings = Settings::default();
    settings.search.custom_engines = vec![
        custom("first", "f"),
        no_placeholder,
        bad_id,
        custom("first", "other"),       // duplicate id
        custom("second", "F"),          // duplicate keyword, case-insensitively
        custom("duckduckgo", "dd"),     // shadows a built-in id
        custom("third", "@DuckDuckGo"), // a built-in's keyword
        custom("fourth", ""),
        custom("fifth", ""), // no keyword is not a duplicate keyword
    ];

    let (validated, repairs) = settings.validated();

    let kept: Vec<&str> = validated
        .search
        .custom_engines
        .iter()
        .map(|engine| engine.id.as_str())
        .collect();
    assert_eq!(kept, vec!["first", "fourth", "fifth"]);
    assert_eq!(
        fields(&repairs),
        vec!["search.custom_engines"],
        "one repair for the list"
    );
    assert!(
        repairs[0].reason.contains("dropped 6"),
        "{}",
        repairs[0].reason
    );
}

#[test]
fn custom_engines_beyond_the_limit_are_dropped() {
    let mut settings = Settings::default();
    settings.search.custom_engines = (0..MAX_CUSTOM_ENGINES + 3)
        .map(|n| custom(&format!("e{n}"), &format!("k{n}")))
        .collect();

    let (validated, repairs) = settings.validated();

    assert_eq!(validated.search.custom_engines.len(), MAX_CUSTOM_ENGINES);
    assert_eq!(repairs.len(), 1);
    assert!(
        repairs[0].reason.contains("dropped 3 beyond"),
        "{}",
        repairs[0].reason
    );
}

#[test]
fn an_implausible_language_tag_becomes_none() {
    for bad in [
        "",
        "e",
        "english please",
        "en_US",
        "123",
        "en-",
        "-en",
        "en--US",
        "en-toolongsubtag",
        "x-private",
        "en-US-\u{e9}",
        "a-very-long-but-otherwise-plausible-tag-x",
    ] {
        let settings = Settings {
            language: Some(bad.to_owned()),
            ..Settings::default()
        };

        let (validated, repairs) = settings.validated();

        assert_eq!(validated.language, None, "{bad:?}");
        assert_eq!(fields(&repairs), vec!["language"], "{bad:?}");
    }

    for good in [
        "en",
        "pt-BR",
        "zh-Hant-TW",
        "de-CH-1996",
        "es-419",
        "sr-Latn",
    ] {
        let settings = Settings {
            language: Some(good.to_owned()),
            ..Settings::default()
        };

        let (validated, repairs) = settings.validated();

        assert_eq!(validated.language.as_deref(), Some(good));
        assert!(repairs.is_empty(), "{good}: {repairs:?}");
    }
}

#[test]
fn never_save_origins_keep_only_distinct_origins() {
    let mut settings = Settings::default();
    settings.passwords.never_save_origins = [
        "https://bank.example",
        "https://bank.example/login",
        "bank.example",
        "javascript:alert(1)",
        "https://user:secret@bank.example",
        "https://bank.example?x=1",
        "file:///etc/passwd",
        "http://localhost:8080",
        "https://bank.example",
        "https://Bank.Example/",
        "https://shop.example:443",
    ]
    .map(str::to_owned)
    .to_vec();

    let (validated, repairs) = settings.validated();

    assert_eq!(
        validated.passwords.never_save_origins,
        vec![
            "https://bank.example".to_owned(),
            "http://localhost:8080".to_owned(),
            "https://shop.example".to_owned(),
        ]
    );
    assert_eq!(fields(&repairs), vec!["passwords.never_save_origins"]);
    let reason = &repairs[0].reason;
    assert!(reason.contains("dropped 6 not of the form"), "{reason}");
    assert!(reason.contains("dropped 2 listed twice"), "{reason}");
    assert!(reason.contains("wrote 1 in canonical form"), "{reason}");
}

#[test]
fn never_save_origins_beyond_the_limit_are_dropped() {
    let mut settings = Settings::default();
    settings.passwords.never_save_origins = (0..MAX_NEVER_SAVE_ORIGINS + 5)
        .map(|n| format!("https://site{n}.example"))
        .collect();

    let (validated, repairs) = settings.validated();

    assert_eq!(
        validated.passwords.never_save_origins.len(),
        MAX_NEVER_SAVE_ORIGINS
    );
    assert_eq!(
        repairs.len(),
        1,
        "one repair for the cut, not one per origin"
    );
}

#[test]
fn a_relative_download_directory_becomes_none() {
    for bad in ["Downloads", "./Downloads", "~/Downloads", ""] {
        let mut settings = Settings::default();
        settings.downloads.directory = Some(PathBuf::from(bad));

        let (validated, repairs) = settings.validated();

        assert_eq!(validated.downloads.directory, None, "{bad}");
        assert_eq!(fields(&repairs), vec!["downloads.directory"], "{bad}");
    }
}

#[test]
fn every_repair_is_reported_in_field_order() {
    let mut settings = Settings {
        homepage: "javascript:void(0)".to_owned(),
        ..Settings::default()
    };
    settings.search.default_engine = "gone".to_owned();
    settings.downloads.directory = Some(PathBuf::from("relative"));
    settings.appearance.default_zoom_percent = 1000;
    settings.passwords.never_save_origins = vec!["nonsense".to_owned()];
    settings.language = Some("??".to_owned());

    let (validated, repairs) = settings.validated();

    assert_eq!(
        fields(&repairs),
        vec![
            "homepage",
            "search.default_engine",
            "downloads.directory",
            "appearance.default_zoom_percent",
            "passwords.never_save_origins",
            "language",
        ]
    );
    assert!(repairs.iter().all(|repair| !repair.reason.is_empty()));
    let (_, again) = validated.validated();
    assert!(again.is_empty(), "validating twice repairs nothing more");
}

#[test]
fn the_home_page_is_stored_as_the_parser_reads_it() {
    let settings = Settings {
        homepage: "  https://exa\tmple.org/a b\n".to_owned(),
        ..Settings::default()
    };

    let (validated, repairs) = settings.validated();

    assert_eq!(validated.homepage, "https://example.org/a%20b");
    assert_eq!(fields(&repairs), vec!["homepage"]);
    let (_, again) = validated.validated();
    assert!(again.is_empty());
}

#[test]
fn a_home_page_with_credentials_is_refused() {
    for bad in [
        "https://user:secret@example.org/",
        "https://user@example.org/",
        "http://:secret@example.org/",
    ] {
        let settings = Settings {
            homepage: bad.to_owned(),
            ..Settings::default()
        };

        let (validated, repairs) = settings.validated();

        assert_eq!(validated.homepage, "", "{bad}");
        assert_eq!(fields(&repairs), vec!["homepage"], "{bad}");
        assert!(!repairs[0].reason.contains("secret"));
    }
}

#[test]
fn the_validators_are_available_to_the_settings_window() {
    assert_eq!(homepage_url(""), Some(String::new()));
    assert_eq!(
        homepage_url("https://Example.org"),
        Some("https://example.org/".to_owned())
    );
    assert_eq!(homepage_url("javascript:alert(1)"), None);
    assert_eq!(homepage_url("https://a:b@example.org/"), None);

    assert!(is_plausible_language_tag("pt-BR"));
    assert!(!is_plausible_language_tag("pt_BR"));

    assert_eq!(
        canonical_origin("https://Bank.Example:443/"),
        Some("https://bank.example".to_owned())
    );
    assert_eq!(canonical_origin("https://bank.example/login"), None);
}

#[test]
fn repair_messages_quote_only_a_short_piece_of_the_file() {
    let long = "x".repeat(100_000);
    let settings = Settings {
        language: Some(long.clone()),
        search: mercury_settings::SearchSettings {
            default_engine: long.clone(),
            custom_engines: vec![SearchEngine {
                id: long.clone(),
                ..custom("ignored", "k")
            }],
            ..mercury_settings::SearchSettings::default()
        },
        passwords: mercury_settings::PasswordSettings {
            never_save_origins: vec![long.clone()],
            ..mercury_settings::PasswordSettings::default()
        },
        ..Settings::default()
    };

    let (_, repairs) = settings.validated();

    assert_eq!(repairs.len(), 4, "{repairs:?}");
    for repair in &repairs {
        assert!(
            repair.reason.len() < MAX_QUOTED_BYTES + 200,
            "{}: {} bytes",
            repair.field,
            repair.reason.len()
        );
    }
}
