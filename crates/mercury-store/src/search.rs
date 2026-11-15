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
/// one), and words with no letter or digit are dropped, since the FTS5 tokenizer would reduce
/// them to an empty phrase.
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
        .filter(|token| token.chars().any(char::is_alphanumeric))
        .take(MAX_TOKENS)
        .map(|token| format!("\"{token}\""))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" ") + "*")
    }
}

/// The form of a URL, or of what the user typed, that host-prefix matching compares: ASCII
/// lower case, without a leading `scheme://`. For a stored URL a leading `www.` is dropped as
/// well, so `exam` finds `https://www.example.com/`; typed input keeps its `www.`, which the
/// full-text match handles.
pub(crate) fn url_key(url: &str, strip_www: bool) -> String {
    let lower = url.trim().to_ascii_lowercase();
    let rest = match lower.find("://") {
        Some(end)
            if end > 0
                && lower[..end]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) =>
        {
            &lower[end + 3..]
        }
        _ => lower.as_str(),
    };
    let rest = if strip_www {
        rest.strip_prefix("www.").unwrap_or(rest)
    } else {
        rest
    };
    rest.to_owned()
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
