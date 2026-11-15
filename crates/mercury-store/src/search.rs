// Adapted from Zephium, https://github.com/zephium-browser/Zephium, commit
// `9026a1fffad74d6e201ef97fe8c02c1b01403cd2`, `crates/zephium-store/src/hub/history.rs:617-640`
// (`fts_query`), MPL-2.0.

//! Turning user text into a safe FTS5 query.

/// Longer input is not searched at all.
const MAX_QUERY_BYTES: usize = 4 * 1024;
/// Only this many words are used.
const MAX_TOKENS: usize = 8;
/// Each word is cut to this many characters.
const MAX_TOKEN_CHARS: usize = 256;

/// Builds an FTS5 `MATCH` argument that requires every word of `query`, the last one as a
/// prefix. Every word is quoted, so nothing the user types is read as FTS5 syntax. Returns
/// `None` if no searchable word is left.
///
/// The result is bound as a parameter, never formatted into SQL.
///
/// Differences from the donor: only the last word is a prefix (the donor makes every word
/// one), and a word with no character the tokenizer keeps is dropped (see [`is_token_char`]).
pub(crate) fn fts_query(query: &str) -> Option<String> {
    if query.len() > MAX_QUERY_BYTES {
        return None;
    }
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|token| {
            token
                .chars()
                .filter(|c| !c.is_control() && *c != '"')
                .take(MAX_TOKEN_CHARS)
                .collect::<String>()
        })
        .filter(|token| token.chars().any(is_token_char))
        .take(MAX_TOKENS)
        .map(|token| format!("\"{token}\""))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" ") + "*")
    }
}

/// Whether FTS5's `unicode61` tokenizer, as the tables use it (default options), keeps `c` as
/// part of a token rather than treating it as a separator.
///
/// The tokenizer keeps the Unicode categories `L*`, `N*` and `Co` (`sqlite3.c` 3.51.1, line
/// 262484, `zCat = "L* N* Co"`). Rust's standard library has no general-category lookup, so this
/// approximates it: letters and numbers, without the combining-mark blocks (which Rust counts
/// as alphabetic when they are `Other_Alphabetic`, such as U+0345), plus the private-use areas.
/// A word the approximation keeps but the tokenizer drops becomes an empty phrase, which FTS5
/// ignores next to other words and which matches nothing on its own — the same results as
/// dropping it, so the approximation cannot change what a search finds.
pub(crate) fn is_token_char(c: char) -> bool {
    let combining = matches!(
        c as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
    );
    let private_use = matches!(
        c as u32,
        0xE000..=0xF8FF | 0xF_0000..=0xF_FFFD | 0x10_0000..=0x10_FFFD
    );
    (c.is_alphanumeric() && !combining) || private_use
}

/// The form of a URL, or of what the user typed, that host-prefix matching and the page
/// full-text index use: without a leading `scheme://` and user info (`user:pass@`), the host in
/// lower-case Unicode (`xn--bcher-kva.example` becomes `bücher.example`, using the `url`
/// crate's IDNA conversion), the rest in ASCII lower case. With `strip_www`, a leading `www.`
/// is dropped too.
pub(crate) fn url_key(url: &str, strip_www: bool) -> String {
    let url = url.trim();
    let rest = match url.find("://") {
        Some(end)
            if end > 0
                && url[..end]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) =>
        {
            &url[end + 3..]
        }
        _ => url,
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host_port = host_port.to_lowercase();
    let host_port = if host_port.contains("xn--") && !host_port.starts_with('[') {
        let (host, port) = match host_port.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => (host, Some(port)),
            _ => (host_port.as_str(), None),
        };
        match url::quirks::domain_to_unicode(host) {
            unicode if unicode.is_empty() => host_port.clone(),
            unicode => match port {
                Some(port) => format!("{unicode}:{port}"),
                None => unicode,
            },
        }
    } else {
        host_port
    };
    let host_port = if strip_www {
        host_port
            .strip_prefix("www.")
            .map_or(host_port.clone(), str::to_owned)
    } else {
        host_port
    };
    host_port + &tail.to_ascii_lowercase()
}

/// The words of `text` as the tokenizer sees them, in lower case.
pub(crate) fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !is_token_char(c))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

#[cfg(test)]
mod tests {
    use super::{fts_query, url_key};

    #[test]
    fn url_keys_drop_scheme_and_www() {
        assert_eq!(url_key("https://www.Example.com/A", true), "example.com/a");
        assert_eq!(url_key("http://example.org", true), "example.org");
        assert_eq!(url_key("about:blank", true), "about:blank");
        assert_eq!(url_key("HTTPS://www.exa", false), "www.exa");
        assert_eq!(url_key("exam", false), "exam");
        assert_eq!(url_key("://x", true), "://x");
        assert_eq!(
            url_key("https://u:p@www.Example.com:8080/P?q#f", true),
            "example.com:8080/p?q#f"
        );
        assert_eq!(
            url_key("https://www.xn--bcher-kva.example/X", true),
            "bücher.example/x"
        );
        assert_eq!(
            url_key("http://xn--bcher-kva.example:81", true),
            "bücher.example:81"
        );
        assert_eq!(url_key("BÜCH", false), "büch");
        assert_eq!(url_key("https://[::1]:8080/", true), "[::1]:8080/");
    }

    #[test]
    fn words_are_quoted_and_only_the_last_is_a_prefix() {
        assert_eq!(fts_query("learn ru").as_deref(), Some("\"learn\" \"ru\"*"));
        assert_eq!(fts_query("  one  ").as_deref(), Some("\"one\"*"));
    }

    #[test]
    fn syntax_is_neutralised() {
        assert_eq!(
            fts_query("\"a\" OR b*").as_deref(),
            Some("\"a\" \"OR\" \"b*\"*")
        );
        assert_eq!(fts_query("NEAR(x").as_deref(), Some("\"NEAR(x\"*"));
        assert_eq!(fts_query("servo -").as_deref(), Some("\"servo\"*"));
    }

    #[test]
    fn words_the_tokenizer_drops_are_ignored() {
        assert_eq!(fts_query("servo \u{345}").as_deref(), Some("\"servo\"*"));
        assert_eq!(fts_query("\u{301}\u{345}"), None);
        assert_eq!(fts_query("caf\u{E9}").as_deref(), Some("\"caf\u{E9}\"*"));
        assert_eq!(fts_query("\u{E000}").as_deref(), Some("\"\u{E000}\"*"));
    }

    #[test]
    fn nothing_searchable_gives_none() {
        for query in ["", "   ", "\"", "*", "(", "-", "\"\" - *", "\u{0}"] {
            assert_eq!(fts_query(query), None, "{query:?}");
        }
    }

    #[test]
    fn input_is_bounded() {
        assert_eq!(fts_query(&"a ".repeat(3000)), None);
        let many = fts_query(&"word ".repeat(400)).unwrap();
        assert_eq!(many.matches("\"word\"").count(), 8);
        let long = fts_query(&"x".repeat(2000)).unwrap();
        assert_eq!(long.len(), 256 + 3);
    }
}
