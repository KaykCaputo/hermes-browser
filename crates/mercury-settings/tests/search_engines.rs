use mercury_settings::{
    DEFAULT_ENGINE_ID, MAX_TEMPLATE_BYTES, SearchEngine, SearchSettings, SettingsError,
    builtin_engines,
};
use url::Url;

fn engine(id: &str, keyword: &str, template: &str) -> SearchEngine {
    SearchEngine {
        id: id.to_owned(),
        name: format!("Engine {id}"),
        keyword: keyword.to_owned(),
        url_template: template.to_owned(),
        suggest_template: None,
    }
}

/// Queries chosen to break naive encoders.
fn awkward_queries() -> Vec<String> {
    vec![
        "rust book".to_owned(),
        "a&b=c".to_owned(),
        "c# vs f#".to_owned(),
        "1+1=2".to_owned(),
        "100% sure".to_owned(),
        "%41 is not A".to_owned(),
        "\"quoted\" and 'single'".to_owned(),
        "naïve café Ελληνικά 日本語".to_owned(),
        "emoji 🦀🚀 ok".to_owned(),
        "slash/and\\backslash?query#frag".to_owned(),
        "  leading and trailing  ".to_owned(),
        "<script>alert(1)</script>".to_owned(),
        "{searchTerms}".to_owned(),
        "a\tb\nc".to_owned(),
        "..".to_owned(),
        String::new(),
        "x".repeat(2000),
        "é🦀".repeat(500),
    ]
}

fn decode_component(text: &str) -> String {
    // Path and fragment encoding never leaves a literal '+', so form decoding is exact here.
    url::form_urlencoded::parse(format!("v={text}").as_bytes())
        .next()
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

#[test]
fn the_built_in_table_has_the_well_known_engines_in_a_stable_order() {
    let ids: Vec<String> = builtin_engines()
        .into_iter()
        .map(|engine| engine.id)
        .collect();
    assert_eq!(
        ids,
        vec![
            "duckduckgo",
            "bing",
            "brave",
            "ecosia",
            "google",
            "startpage",
            "wikipedia"
        ]
    );
    assert_eq!(ids[0], DEFAULT_ENGINE_ID);
}

#[test]
fn the_built_in_templates_are_the_published_ones() {
    let templates: Vec<(String, String, Option<String>)> = builtin_engines()
        .into_iter()
        .map(|engine| (engine.id, engine.url_template, engine.suggest_template))
        .collect();
    let expected = [
        (
            "duckduckgo",
            "https://duckduckgo.com/?q={searchTerms}",
            Some("https://duckduckgo.com/ac/?q={searchTerms}&type=list"),
        ),
        ("bing", "https://www.bing.com/search?q={searchTerms}", None),
        (
            "brave",
            "https://search.brave.com/search?q={searchTerms}",
            Some("https://search.brave.com/api/suggest?q={searchTerms}"),
        ),
        (
            "ecosia",
            "https://www.ecosia.org/search?q={searchTerms}",
            Some("https://ac.ecosia.org/autocomplete?q={searchTerms}&type=list"),
        ),
        (
            "google",
            "https://www.google.com/search?q={searchTerms}",
            None,
        ),
        (
            "startpage",
            "https://www.startpage.com/sp/search?query={searchTerms}",
            Some("https://www.startpage.com/osuggestions?q={searchTerms}"),
        ),
        (
            "wikipedia",
            "https://en.wikipedia.org/w/index.php?title=Special:Search&search={searchTerms}",
            Some(concat!(
                "https://en.wikipedia.org/w/api.php",
                "?action=opensearch&search={searchTerms}&namespace=0"
            )),
        ),
    ]
    .map(|(id, url, suggest)| (id.to_owned(), url.to_owned(), suggest.map(str::to_owned)))
    .to_vec();
    assert_eq!(templates, expected);
}

#[test]
fn every_built_in_engine_is_valid_with_a_unique_id_and_keyword() {
    let engines = builtin_engines();
    for engine in &engines {
        engine
            .validate()
            .unwrap_or_else(|error| panic!("{}: {error}", engine.id));
        assert!(!engine.name.is_empty());
        assert!(engine.keyword.starts_with('@'), "{}", engine.keyword);
    }
    let mut keywords: Vec<String> = engines
        .iter()
        .map(|engine| engine.keyword.to_lowercase())
        .collect();
    keywords.sort();
    keywords.dedup();
    assert_eq!(keywords.len(), engines.len());
}

/// How the address bar built a search before the settings existed (talaria-core at
/// `546ed1e`, `duckduckgo_search_url`), kept here as the reference the default engine must match.
fn address_bar_before_settings(query: &str) -> Url {
    let mut url = Url::parse("https://duckduckgo.com/").expect("a URL");
    url.query_pairs_mut().append_pair("q", query);
    url
}

#[test]
fn duckduckgo_builds_exactly_what_the_address_bar_builds_today() {
    let duckduckgo = SearchSettings::default().default_engine();
    assert_eq!(duckduckgo.id, "duckduckgo");

    for query in awkward_queries() {
        let ours = duckduckgo.search_url(&query).expect("a search URL");
        let today = address_bar_before_settings(&query);
        assert_eq!(ours, today, "{query:?}");
        assert_eq!(ours.as_str(), today.as_str(), "{query:?}");
    }
}

#[test]
fn every_built_in_engine_round_trips_awkward_queries() {
    for engine in builtin_engines() {
        let parameter = if engine.id == "startpage" {
            "query"
        } else if engine.id == "wikipedia" {
            "search"
        } else {
            "q"
        };
        for query in awkward_queries() {
            let url = engine.search_url(&query).expect("a search URL");
            let reparsed = Url::parse(url.as_str()).expect("the URL parses again");
            let found: Vec<String> = reparsed
                .query_pairs()
                .filter(|(name, _)| name == parameter)
                .map(|(_, value)| value.into_owned())
                .collect();
            assert_eq!(found, vec![query.clone()], "{} {query:?}", engine.id);
            assert_eq!(reparsed.fragment(), None, "{} {query:?}", engine.id);
        }
    }
}

#[test]
fn a_placeholder_in_the_path_is_encoded_for_the_path() {
    let engine = engine(
        "pathy",
        "p",
        "https://pathy.example/search/{searchTerms}/results",
    );

    for query in awkward_queries() {
        if query == ".." {
            continue;
        }
        let url = engine.search_url(&query).expect("a search URL");
        let reparsed = Url::parse(url.as_str()).expect("the URL parses again");
        let segments: Vec<&str> = reparsed.path_segments().expect("segments").collect();
        assert_eq!(segments.len(), 3, "{query:?} -> {url}");
        assert_eq!(segments[0], "search");
        assert_eq!(segments[2], "results");
        assert_eq!(decode_component(segments[1]), query, "{query:?} -> {url}");
        assert_eq!(reparsed.query(), None, "{query:?}");
        assert_eq!(reparsed.fragment(), None, "{query:?}");
    }
}

#[test]
fn a_dot_segment_query_cannot_be_placed_in_a_path_segment_of_its_own() {
    let engine = engine("pathy", "p", "https://pathy.example/search/{searchTerms}");

    for query in [".", ".."] {
        assert!(
            matches!(
                engine.search_url(query),
                Err(SettingsError::UnplaceableQuery { .. })
            ),
            "{query:?}"
        );
    }
    let prefixed = mercury_settings::SearchEngine {
        url_template: "https://pathy.example/search/q-{searchTerms}".to_owned(),
        ..engine
    };
    assert_eq!(
        prefixed
            .search_url("..")
            .expect("not a dot segment")
            .as_str(),
        "https://pathy.example/search/q-.."
    );
}

#[test]
fn a_placeholder_in_the_fragment_is_encoded_for_the_fragment() {
    let engine = engine("frag", "f", "https://frag.example/app#q={searchTerms}");

    for query in awkward_queries() {
        let url = engine.search_url(&query).expect("a search URL");
        let reparsed = Url::parse(url.as_str()).expect("the URL parses again");
        let fragment = reparsed.fragment().expect("a fragment");
        let value = fragment.strip_prefix("q=").expect("the prefix stays");
        assert_eq!(decode_component(value), query, "{query:?} -> {url}");
    }
}

#[test]
fn bad_templates_are_refused() {
    let too_long = format!(
        "https://long.example/?q={{searchTerms}}&pad={}",
        "a".repeat(MAX_TEMPLATE_BYTES)
    );
    for template in [
        "ftp://files.example/{searchTerms}",
        "javascript:alert('{searchTerms}')",
        "data:text/html,{searchTerms}",
        "file:///search?q={searchTerms}",
        "https://none.example/search?q=",
        "https://twice.example/?q={searchTerms}&again={searchTerms}",
        "https://other.example/?q={searchTerms}&page={startPage}",
        "https://other.example/?q={searchTerms}&page={startPage?}",
        "https://{searchTerms}.example/",
        "https://host.example{searchTerms}",
        "{searchTerms}://example.org/",
        "https:///?q={searchTerms}",
        "https://user:secret@creds.example/?q={searchTerms}",
        "https://exa mple.org/?q={searchTerms}",
        "not a url {searchTerms}",
        "https://brace.example/?q={searchTerms}}",
        too_long.as_str(),
    ] {
        let engine = engine("bad", "b", template);
        assert!(
            matches!(
                engine.validate(),
                Err(SettingsError::InvalidSearchEngine { .. })
            ),
            "{template} should be refused"
        );
        assert!(engine.search_url("x").is_err(), "{template}");
    }
}

#[test]
fn good_templates_are_accepted() {
    for template in [
        "http://intranet.example/search?q={searchTerms}",
        "https://example.org/?q={searchTerms}&lang=en",
        "https://example.org/search/{searchTerms}",
        "https://example.org/#/search/{searchTerms}",
        "https://localhost:8443/s?q={searchTerms}",
        "HTTPS://Example.ORG/?q={searchTerms}",
    ] {
        engine("good", "g", template)
            .validate()
            .unwrap_or_else(|error| panic!("{template}: {error}"));
    }
}

#[test]
fn a_bad_suggestion_template_is_refused() {
    let mut engine = engine("sugg", "s", "https://sugg.example/?q={searchTerms}");
    engine.suggest_template = Some("https://sugg.example/complete".to_owned());
    assert!(engine.validate().is_err());
    engine.suggest_template = Some("https://sugg.example/complete?q={searchTerms}".to_owned());
    assert!(engine.validate().is_ok());
}

#[test]
fn ids_names_and_keywords_are_checked() {
    let good = engine("ok-1_a.b", "ok", "https://ok.example/?q={searchTerms}");
    assert!(good.validate().is_ok());

    for id in ["", "UPPER", "with space", "slash/", &"a".repeat(65)] {
        let bad = SearchEngine {
            id: id.to_owned(),
            ..good.clone()
        };
        assert!(bad.validate().is_err(), "id {id:?}");
    }
    for name in ["", "   ", "tab\there", &"n".repeat(101)] {
        let bad = SearchEngine {
            name: name.to_owned(),
            ..good.clone()
        };
        assert!(bad.validate().is_err(), "name {name:?}");
    }
    for keyword in ["two words", "tab\t", &"k".repeat(33)] {
        let bad = SearchEngine {
            keyword: keyword.to_owned(),
            ..good.clone()
        };
        assert!(bad.validate().is_err(), "keyword {keyword:?}");
    }
}

fn settings_with(custom: Vec<SearchEngine>) -> SearchSettings {
    SearchSettings {
        custom_engines: custom,
        ..SearchSettings::default()
    }
}

#[test]
fn engines_are_the_built_ins_then_the_usable_custom_ones() {
    let settings = settings_with(vec![
        engine("mine", "m", "https://mine.example/?q={searchTerms}"),
        engine("broken", "x", "https://broken.example/"),
        engine("mine", "m2", "https://again.example/?q={searchTerms}"),
    ]);

    let ids: Vec<String> = settings
        .engines()
        .into_iter()
        .map(|engine| engine.id)
        .collect();

    let mut expected: Vec<String> = builtin_engines()
        .into_iter()
        .map(|engine| engine.id)
        .collect();
    expected.push("mine".to_owned());
    assert_eq!(ids, expected);
}

#[test]
fn the_default_engine_falls_back_to_duckduckgo() {
    let mut settings = settings_with(vec![engine(
        "mine",
        "m",
        "https://mine.example/?q={searchTerms}",
    )]);
    assert_eq!(settings.default_engine().id, "duckduckgo");

    settings.default_engine = "mine".to_owned();
    assert_eq!(settings.default_engine().id, "mine");

    settings.default_engine = "google".to_owned();
    assert_eq!(settings.default_engine().id, "google");

    settings.default_engine = "missing".to_owned();
    assert_eq!(settings.default_engine().id, "duckduckgo");
}

#[test]
fn a_keyword_followed_by_a_query_selects_its_engine() {
    let settings = settings_with(vec![engine(
        "wiki-short",
        "w",
        "https://en.wikipedia.org/w/index.php?search={searchTerms}",
    )]);

    let (engine, query) = settings.match_keyword("w rust book").expect("a match");
    assert_eq!(engine.id, "wiki-short");
    assert_eq!(query, "rust book");

    let (engine, query) = settings
        .match_keyword("@wikipedia rust book")
        .expect("a match");
    assert_eq!(engine.id, "wikipedia");
    assert_eq!(query, "rust book");

    let (engine, query) = settings
        .match_keyword("  @Google   Rust  ")
        .expect("a match");
    assert_eq!(engine.id, "google");
    assert_eq!(query, "Rust");

    let (engine, query) = settings.match_keyword("W\tsearch me").expect("a match");
    assert_eq!(engine.id, "wiki-short");
    assert_eq!(query, "search me");
}

#[test]
fn a_keyword_alone_or_at_the_start_of_a_word_is_not_a_match() {
    let settings = settings_with(vec![engine(
        "wiki-short",
        "w",
        "https://en.wikipedia.org/w/index.php?search={searchTerms}",
    )]);

    for input in [
        "",
        "w",
        "w ",
        "w    ",
        "@wikipedia",
        "w.org",
        "w.org rust",
        "wikipedia.org/wiki/Rust",
        "www.example.org",
        "wrust book",
        "rust w book",
        "@duckduckgo.com",
        "https://w.example/ x",
    ] {
        assert_eq!(settings.match_keyword(input), None, "{input:?}");
    }
}

#[test]
fn a_dropped_engine_has_no_keyword() {
    let settings = settings_with(vec![engine(
        "broken",
        "b",
        "ftp://broken.example/{searchTerms}",
    )]);
    assert_eq!(settings.match_keyword("b anything"), None);
}

#[test]
fn names_and_keywords_with_invisible_characters_are_refused() {
    let good = engine("ok", "ok", "https://ok.example/?q={searchTerms}");
    for sneaky in [
        "evil\u{202E}moc.elgoog", // right-to-left override
        "Goo\u{200B}gle",         // zero-width space
        "a\u{2066}b\u{2069}",     // isolates
        "soft\u{00AD}hyphen",
        "\u{FEFF}bom",
        "tag\u{E0041}",
        "line\u{2028}break",
        "\u{3164}",
    ] {
        let bad_name = SearchEngine {
            name: sneaky.to_owned(),
            ..good.clone()
        };
        assert!(bad_name.validate().is_err(), "name {sneaky:?}");
        let bad_keyword = SearchEngine {
            keyword: sneaky.to_owned(),
            ..good.clone()
        };
        assert!(bad_keyword.validate().is_err(), "keyword {sneaky:?}");
    }
    let fine = SearchEngine {
        name: "Recherche • Ελληνικά 日本語 🦀".to_owned(),
        keyword: "@récherche".to_owned(),
        ..good
    };
    assert!(fine.validate().is_ok());
}
