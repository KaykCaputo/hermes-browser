use std::error::Error;

use talaria_core::{AddressResolutionError, NavigationTarget, classify_address, resolve_address};
use url::Url;

#[test]
fn explicit_https_url_is_preserved() {
    let target = classify_address("  https://example.com/docs  ")
        .expect("a valid HTTPS URL should be accepted");

    assert_eq!(
        target,
        NavigationTarget::Url(
            Url::parse("https://example.com/docs").expect("the test URL should be valid")
        )
    );
}

#[test]
fn about_url_is_preserved() {
    let url = resolve_address("about:blank").expect("about:blank should be supported");

    assert_eq!(url.as_str(), "about:blank");
}

#[test]
fn hostname_without_scheme_uses_https() {
    let url =
        resolve_address("servo.org/downloads").expect("a hostname with a path should be accepted");

    assert_eq!(url.as_str(), "https://servo.org/downloads");
}

#[test]
fn localhost_with_port_uses_https() {
    let url =
        resolve_address("localhost:3000/status").expect("localhost with a port should be accepted");

    assert_eq!(url.as_str(), "https://localhost:3000/status");
}

#[test]
fn ip_address_with_port_uses_https() {
    let url = resolve_address("127.0.0.1:8080/status")
        .expect("an IP address with a port should be accepted");

    assert_eq!(url.as_str(), "https://127.0.0.1:8080/status");
}

#[test]
fn plain_text_becomes_a_duckduckgo_search() {
    let target = classify_address("  ownership in rust  ")
        .expect("plain text should be classified as a search");

    assert_eq!(
        target,
        NavigationTarget::SearchQuery("ownership in rust".to_owned())
    );
}

#[test]
fn search_query_is_encoded_in_duckduckgo_url() {
    let url =
        resolve_address("rust ownership & borrowing").expect("a search query should produce a URL");

    assert_eq!(
        url.as_str(),
        "https://duckduckgo.com/?q=rust+ownership+%26+borrowing"
    );
}

#[test]
fn empty_input_is_rejected() {
    let error = resolve_address("   ").expect_err("blank input should be rejected");

    assert!(matches!(error, AddressResolutionError::EmptyInput));
    assert_eq!(error.to_string(), "address input cannot be empty");
    assert!(error.source().is_none());
}

#[test]
fn unsupported_explicit_scheme_is_rejected() {
    let error = resolve_address("ftp://example.com/file")
        .expect_err("an unsupported scheme should be rejected");

    assert!(matches!(
        error,
        AddressResolutionError::UnsupportedScheme(ref scheme) if scheme == "ftp"
    ));
    assert_eq!(error.to_string(), "unsupported URL scheme: ftp");
}

#[test]
fn malformed_explicit_url_preserves_parse_error_as_source() {
    let error = resolve_address("https://").expect_err("a malformed explicit URL should fail");

    assert!(matches!(error, AddressResolutionError::InvalidUrl(_)));
    assert!(error.to_string().starts_with("invalid URL: "));
    assert!(error.source().is_some());
}
