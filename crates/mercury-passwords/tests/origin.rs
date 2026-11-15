use mercury_passwords::{Origin, PasswordError};
use url::Url;

fn origin(text: &str) -> Result<Origin, PasswordError> {
    Origin::parse(text)
}

fn origin_text(text: &str) -> String {
    origin(text)
        .unwrap_or_else(|error| panic!("{text} should be a valid origin: {error}"))
        .as_str()
        .to_owned()
}

#[test]
fn https_origin_keeps_scheme_and_host() {
    assert_eq!(origin_text("https://example.com"), "https://example.com");
}

#[test]
fn host_and_scheme_are_lower_cased() {
    assert_eq!(origin_text("HTTPS://Example.COM/"), "https://example.com");
}

#[test]
fn default_port_is_omitted_and_other_ports_kept() {
    assert_eq!(
        origin_text("https://example.com:443/"),
        "https://example.com"
    );
    assert_eq!(
        origin_text("https://example.com:8443/"),
        "https://example.com:8443"
    );
    assert_eq!(origin_text("http://localhost:80/"), "http://localhost");
    assert_eq!(
        origin_text("http://localhost:8080/"),
        "http://localhost:8080"
    );
}

#[test]
fn one_trailing_dot_is_removed() {
    assert_eq!(origin_text("https://example.com./"), "https://example.com");
    assert_eq!(origin_text("http://localhost./"), "http://localhost");
}

#[test]
fn empty_labels_are_refused() {
    assert!(matches!(
        origin("https://example.com../"),
        Err(PasswordError::InvalidOrigin(_))
    ));
    assert!(matches!(
        origin("https://a..example.com/"),
        Err(PasswordError::InvalidOrigin(_))
    ));
}

#[test]
fn internationalised_hosts_become_ascii() {
    assert_eq!(
        origin_text("https://Bücher.example/"),
        "https://xn--bcher-kva.example"
    );
    assert_eq!(
        origin_text("https://xn--bcher-kva.example/"),
        "https://xn--bcher-kva.example"
    );
}

#[test]
fn user_info_path_query_and_fragment_are_dropped() {
    assert_eq!(
        origin_text("https://user:secret@example.com:8443/a/b?c=d#e"),
        "https://example.com:8443"
    );
}

#[test]
fn ip_addresses_are_accepted_for_https() {
    assert_eq!(origin_text("https://192.0.2.7/"), "https://192.0.2.7");
    assert_eq!(
        origin_text("https://[2001:db8::1]:8443/"),
        "https://[2001:db8::1]:8443"
    );
}

#[test]
fn http_is_accepted_for_loopback_hosts_only() {
    assert_eq!(origin_text("http://localhost/"), "http://localhost");
    assert_eq!(
        origin_text("http://LOCALHOST:3000/"),
        "http://localhost:3000"
    );
    assert_eq!(
        origin_text("http://127.0.0.1:8000/"),
        "http://127.0.0.1:8000"
    );
    assert_eq!(origin_text("http://127.255.0.9/"), "http://127.255.0.9");
    assert_eq!(origin_text("http://[::1]:8000/"), "http://[::1]:8000");

    for refused in [
        "http://example.com/",
        "http://128.0.0.1/",
        "http://10.0.0.1/",
        "http://[::2]/",
        "http://localhost.example.com/",
        "http://foo.localhost/",
    ] {
        assert!(
            matches!(origin(refused), Err(PasswordError::InvalidOrigin(_))),
            "{refused} should be refused"
        );
    }
}

#[test]
fn other_schemes_and_origin_less_urls_are_refused() {
    for refused in [
        "file:///etc/hosts",
        "data:text/html,hello",
        "about:blank",
        "blob:https://example.com/0d6b8d1e-6c5a-4f43-8a65-4cb0a4a3e2a1",
        "ftp://example.com/",
        "ws://localhost/",
        "wss://example.com/",
        "javascript:alert(1)",
        "mailto:someone@example.com",
        "android://hash@com.example.app/",
    ] {
        assert!(
            matches!(origin(refused), Err(PasswordError::InvalidOrigin(_))),
            "{refused} should be refused"
        );
    }
}

#[test]
fn text_that_is_not_a_url_is_refused() {
    for refused in [
        "",
        "example.com",
        "https://",
        "https://exa mple.com/",
        "//example.com",
    ] {
        assert!(
            matches!(origin(refused), Err(PasswordError::InvalidOrigin(_))),
            "{refused:?} should be refused"
        );
    }
}

#[test]
fn similar_looking_sites_are_different_origins() {
    let texts = [
        "https://example.com",
        "https://www.example.com",
        "http://example.com",
        "https://example.com:8443",
        "https://example.com.evil.test",
    ];
    let results: Vec<_> = texts.iter().map(|text| origin(text)).collect();
    for (i, left) in results.iter().enumerate() {
        for (j, right) in results.iter().enumerate() {
            if i == j {
                continue;
            }
            if let (Ok(left), Ok(right)) = (left, right) {
                assert_ne!(left, right, "{} and {} must differ", texts[i], texts[j]);
            }
        }
    }
    assert!(
        results[2].is_err(),
        "plain http to a public host is refused"
    );
}

#[test]
fn from_url_matches_parse() {
    let url = Url::parse("https://Example.com:8443/login?next=/").expect("test URL is valid");
    let from_url = Origin::from_url(&url).expect("an https URL has an origin");
    assert_eq!(
        from_url,
        origin("https://example.com:8443").expect("valid origin")
    );
}

#[test]
fn host_returns_the_host_part() {
    assert_eq!(
        origin("https://Example.com:8443/").expect("valid").host(),
        "example.com"
    );
    assert_eq!(origin("http://[::1]:8000/").expect("valid").host(), "[::1]");
    assert_eq!(
        origin("https://192.0.2.7/").expect("valid").host(),
        "192.0.2.7"
    );
}

#[test]
fn parsing_an_origin_string_again_gives_the_same_origin() {
    for text in [
        "https://example.com",
        "https://example.com:8443",
        "http://[::1]:8000",
        "https://xn--bcher-kva.example",
    ] {
        let first = origin(text).expect("valid origin");
        let second = origin(first.as_str()).expect("an origin string parses");
        assert_eq!(first, second);
        assert_eq!(first.as_str(), text);
    }
}

#[test]
fn display_shows_the_origin_text() {
    let origin = origin("https://example.com:8443/").expect("valid");
    assert_eq!(origin.to_string(), "https://example.com:8443");
}
