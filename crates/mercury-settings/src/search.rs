//! Search engines: the built-in table, user-defined engines, and address-bar keywords.

use serde::{Deserialize, Serialize};

use crate::error::SettingsError;
use crate::settings::{Repair, SearchSettings, quoted};

/// The id of the engine used when none is chosen: DuckDuckGo, as the browser uses today.
pub const DEFAULT_ENGINE_ID: &str = "duckduckgo";
/// The placeholder a URL template carries where the query goes (OpenSearch 1.1).
pub const SEARCH_TERMS_PLACEHOLDER: &str = "{searchTerms}";
/// The longest URL template accepted, in bytes.
pub const MAX_TEMPLATE_BYTES: usize = 2048;
/// The longest engine id accepted, in bytes.
pub const MAX_ENGINE_ID_BYTES: usize = 64;
/// The longest engine name accepted, in bytes.
pub const MAX_ENGINE_NAME_BYTES: usize = 100;
/// The longest keyword accepted, in bytes.
pub const MAX_KEYWORD_BYTES: usize = 32;
/// How many custom engines are kept.
pub const MAX_CUSTOM_ENGINES: usize = 50;

/// A search engine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchEngine {
    /// A stable identifier: 1 to [`MAX_ENGINE_ID_BYTES`] of `a-z`, `0-9`, `-`, `_`, `.`.
    pub id: String,
    /// The name shown to the user: 1 to [`MAX_ENGINE_NAME_BYTES`], without control characters
    /// or invisible formatting characters (bidirectional controls, zero-width characters…).
    pub name: String,
    /// The address-bar keyword (`"@wikipedia"` in `"@wikipedia rust book"`); empty for none. At
    /// most [`MAX_KEYWORD_BYTES`], without whitespace, control or invisible characters.
    pub keyword: String,
    /// The results page, with [`SEARCH_TERMS_PLACEHOLDER`] where the query goes.
    pub url_template: String,
    /// Where suggestions would be fetched from, in the same form. Carried as data only:
    /// nothing in this crate fetches anything.
    pub suggest_template: Option<String>,
}

/// The built-in engines: the default first, then the others in alphabetical order of name, so
/// the order implies no ranking.
///
/// Each template is the engine's own: from its OpenSearch description where it publishes one,
/// otherwise from the search form on its home page, fetched on 2026-11-15. Attribution and language parameters in the published
/// templates are left out; see the crate's report for the full sources. Keywords start with `@`,
/// as Firefox's do, so an ordinary first word of a search is never mistaken for one and sent to
/// a different engine.
pub fn builtin_engines() -> Vec<SearchEngine> {
    vec![
        duckduckgo(),
        // Bing publishes no OpenSearch description (checked 2026-11-15: /osd.xml,
        // /opensearch.xml and /sa/osd/bing.xml give 404 and the home page links none). This is
        // Bing's own form endpoint: the search form on https://www.bing.com/ submits
        // GET /search with the query in `q`.
        engine(
            "bing",
            "Bing",
            "@bing",
            "https://www.bing.com/search?q={searchTerms}",
            None,
        ),
        // https://search.brave.com/opensearch.xml
        engine(
            "brave",
            "Brave Search",
            "@brave",
            "https://search.brave.com/search?q={searchTerms}",
            Some("https://search.brave.com/api/suggest?q={searchTerms}"),
        ),
        // https://www.ecosia.org/opensearch.xml (without `&addon=opensearch`)
        engine(
            "ecosia",
            "Ecosia",
            "@ecosia",
            "https://www.ecosia.org/search?q={searchTerms}",
            Some("https://ac.ecosia.org/autocomplete?q={searchTerms}&type=list"),
        ),
        // Google publishes no OpenSearch description (checked 2026-11-15: /opensearch.xml
        // gives 404 and the home page links none). This is Google's own form endpoint: the
        // search form on https://www.google.com/ is `<form action="/search" method="GET">`
        // with the query in `q`.
        engine(
            "google",
            "Google",
            "@google",
            "https://www.google.com/search?q={searchTerms}",
            None,
        ),
        // https://cdn.startpage.com/sp/cdn/opensearch/en/opensearch.xml, linked from
        // https://www.startpage.com/ (without `&cat=web&pl=opensearch&language=english`)
        engine(
            "startpage",
            "Startpage",
            "@startpage",
            "https://www.startpage.com/sp/search?query={searchTerms}",
            Some("https://www.startpage.com/osuggestions?q={searchTerms}"),
        ),
        // https://en.wikipedia.org/w/opensearch_desc.php
        engine(
            "wikipedia",
            "Wikipedia (en)",
            "@wikipedia",
            "https://en.wikipedia.org/w/index.php?title=Special:Search&search={searchTerms}",
            Some(
                "https://en.wikipedia.org/w/api.php?action=opensearch&search={searchTerms}&namespace=0",
            ),
        ),
    ]
}

fn engine(
    id: &str,
    name: &str,
    keyword: &str,
    url_template: &str,
    suggest_template: Option<&str>,
) -> SearchEngine {
    SearchEngine {
        id: id.to_owned(),
        name: name.to_owned(),
        keyword: keyword.to_owned(),
        url_template: url_template.to_owned(),
        suggest_template: suggest_template.map(str::to_owned),
    }
}

// https://duckduckgo.com/opensearch.xml
fn duckduckgo() -> SearchEngine {
    engine(
        DEFAULT_ENGINE_ID,
        "DuckDuckGo",
        "@duckduckgo",
        "https://duckduckgo.com/?q={searchTerms}",
        Some("https://duckduckgo.com/ac/?q={searchTerms}&type=list"),
    )
}

/// Where in a URL the placeholder sits, which decides how the query is encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Component {
    Path,
    Query,
    Fragment,
}

impl SearchEngine {
    /// Checks the template: http or https, exactly one `{searchTerms}`, a valid URL once filled.
    ///
    /// Also refuses other `{…}` placeholders, a placeholder in the scheme or host, a user name
    /// or password in the URL, a template
    /// over [`MAX_TEMPLATE_BYTES`], the same for `suggest_template` when present, and an id,
    /// name or keyword outside the limits documented on the fields.
    pub fn validate(&self) -> Result<(), SettingsError> {
        match self.problem() {
            None => Ok(()),
            Some(reason) => Err(SettingsError::InvalidSearchEngine {
                id: self.id.clone(),
                reason,
            }),
        }
    }

    /// What [`validate`](Self::validate) objects to, in words that quote nothing from the
    /// engine itself.
    fn problem(&self) -> Option<String> {
        self.check_identity()
            .and_then(|()| check_template(&self.url_template).map(|_| ()))
            .and_then(|()| match &self.suggest_template {
                Some(template) => check_template(template)
                    .map(|_| ())
                    .map_err(|reason| format!("suggestion template: {reason}")),
                None => Ok(()),
            })
            .err()
    }

    /// The results page for `query`, encoded correctly for where the placeholder sits.
    ///
    /// In the query string the query is form-encoded (a space becomes `+`), exactly as the
    /// address bar encodes a DuckDuckGo search today; in the path or the fragment every byte
    /// other than `A-Z a-z 0-9 - . _ ~` is percent-encoded. A query of `.` or `..` filling a
    /// whole path segment cannot be placed (a URL parser resolves it away) and is an error.
    pub fn search_url(&self, query: &str) -> Result<url::Url, SettingsError> {
        let component = check_template(&self.url_template).map_err(|reason| {
            SettingsError::InvalidSearchEngine {
                id: self.id.clone(),
                reason,
            }
        })?;
        let encoded = match component {
            Component::Query => url::form_urlencoded::byte_serialize(query.as_bytes()).collect(),
            Component::Path | Component::Fragment => percent_encode_strict(query),
        };
        if component == Component::Path
            && matches!(query, "." | "..")
            && fills_a_path_segment(&self.url_template)
        {
            return Err(SettingsError::UnplaceableQuery {
                id: self.id.clone(),
            });
        }
        let filled = self
            .url_template
            .replacen(SEARCH_TERMS_PLACEHOLDER, &encoded, 1);
        url::Url::parse(&filled).map_err(|error| SettingsError::InvalidSearchEngine {
            id: self.id.clone(),
            reason: format!("not a valid URL once filled: {error}"),
        })
    }

    fn check_identity(&self) -> Result<(), String> {
        let id_ok = !self.id.is_empty()
            && self.id.len() <= MAX_ENGINE_ID_BYTES
            && self.id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_.".contains(&byte)
            });
        if !id_ok {
            return Err(format!(
                "the id must be 1 to {MAX_ENGINE_ID_BYTES} bytes of a-z, 0-9, '-', '_' or '.'"
            ));
        }
        if self.name.trim().is_empty()
            || self.name.len() > MAX_ENGINE_NAME_BYTES
            || self.name.chars().any(is_invisible_or_control)
        {
            return Err(format!(
                "the name must be 1 to {MAX_ENGINE_NAME_BYTES} bytes without control or \
                 invisible characters"
            ));
        }
        if self.keyword.len() > MAX_KEYWORD_BYTES
            || self
                .keyword
                .chars()
                .any(|c| c.is_whitespace() || is_invisible_or_control(c))
        {
            return Err(format!(
                "the keyword must be at most {MAX_KEYWORD_BYTES} bytes without spaces, control \
                 or invisible characters"
            ));
        }
        Ok(())
    }
}

/// Checks a template and says where its placeholder sits.
fn check_template(template: &str) -> Result<Component, String> {
    if template.len() > MAX_TEMPLATE_BYTES {
        return Err(format!(
            "the template is longer than {MAX_TEMPLATE_BYTES} bytes"
        ));
    }
    let mut places = template.match_indices(SEARCH_TERMS_PLACEHOLDER);
    let Some((position, _)) = places.next() else {
        return Err(format!("the template has no {SEARCH_TERMS_PLACEHOLDER}"));
    };
    if places.next().is_some() {
        return Err(format!(
            "the template has more than one {SEARCH_TERMS_PLACEHOLDER}"
        ));
    }
    let rest = template.replacen(SEARCH_TERMS_PLACEHOLDER, "", 1);
    if rest.contains(['{', '}']) {
        return Err("the template has a placeholder other than {searchTerms}".to_owned());
    }

    let before = &template[..position];
    let Some((scheme, after_scheme)) = before.split_once("://") else {
        return Err("the placeholder must come after the host".to_owned());
    };
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return Err("the template must be an http or https URL".to_owned());
    }
    let component = if after_scheme.contains('#') {
        Component::Fragment
    } else if after_scheme.contains('?') {
        Component::Query
    } else if after_scheme.contains('/') {
        Component::Path
    } else {
        return Err("the placeholder must come after the host".to_owned());
    };

    let filled = template.replacen(SEARCH_TERMS_PLACEHOLDER, "test", 1);
    let url = url::Url::parse(&filled).map_err(|error| format!("not a valid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("the template must be an http or https URL".to_owned());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("the template has no host".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the template must not carry a user name or password".to_owned());
    }
    Ok(component)
}

/// Percent-encodes every byte except the unreserved characters of RFC 3986.
fn percent_encode_strict(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Whether the placeholder is a whole path segment: `/{searchTerms}` followed by `/`, `?`, `#`
/// or the end.
fn fills_a_path_segment(template: &str) -> bool {
    template
        .split_once(SEARCH_TERMS_PLACEHOLDER)
        .is_some_and(|(before, after)| {
            before.ends_with('/') && (after.is_empty() || after.starts_with(['/', '?', '#']))
        })
}

/// The custom engines that can be used alongside the built-in ones, and at most one repair
/// counting those that cannot: invalid, or reusing a built-in id, an earlier id, or a keyword
/// already taken (compared case-insensitively), or beyond [`MAX_CUSTOM_ENGINES`] (including
/// `unread` that a bounded decode skipped). The repair names the first engine dropped.
pub(crate) fn usable_custom_engines(
    custom: &[SearchEngine],
    unread: u64,
) -> (Vec<SearchEngine>, Option<Repair>) {
    let builtins = builtin_engines();
    let mut ids: Vec<String> = builtins.iter().map(|engine| engine.id.clone()).collect();
    let mut keywords: Vec<String> = builtins
        .iter()
        .filter(|engine| !engine.keyword.is_empty())
        .map(|engine| engine.keyword.to_lowercase())
        .collect();
    let mut usable = Vec::new();
    let mut dropped = 0_u64;
    let mut first = None;
    let mut over_limit = unread;

    for engine in custom {
        if usable.len() == MAX_CUSTOM_ENGINES {
            over_limit += 1;
            continue;
        }
        let problem = engine
            .problem()
            .or_else(|| {
                builtins
                    .iter()
                    .any(|builtin| builtin.id == engine.id)
                    .then(|| "the id is a built-in engine's".to_owned())
            })
            .or_else(|| {
                ids.contains(&engine.id)
                    .then(|| "the id is used by an earlier engine".to_owned())
            });
        let keyword = engine.keyword.to_lowercase();
        let problem = problem.or_else(|| {
            (!keyword.is_empty() && keywords.contains(&keyword))
                .then(|| "the keyword is already taken".to_owned())
        });
        if let Some(problem) = problem {
            dropped += 1;
            first.get_or_insert_with(|| format!("{}: {problem}", quoted(&engine.id)));
            continue;
        }
        ids.push(engine.id.clone());
        if !keyword.is_empty() {
            keywords.push(keyword);
        }
        usable.push(engine.clone());
    }

    let mut parts = Vec::new();
    if dropped > 0 {
        parts.push(format!(
            "dropped {dropped} unusable (the first: {})",
            first.unwrap_or_default()
        ));
    }
    if over_limit > 0 {
        parts.push(format!(
            "dropped {over_limit} beyond the first {MAX_CUSTOM_ENGINES}"
        ));
    }
    let repair = (!parts.is_empty()).then(|| Repair {
        field: crate::bounded::CUSTOM_ENGINES,
        reason: parts.join("; "),
    });
    (usable, repair)
}

/// Whether `c` is a control character or one that changes or hides how text around it is
/// shown: Unicode's format characters (category Cf: bidirectional controls, zero-width
/// characters, the soft hyphen, tags…), line and paragraph separators, and the invisible
/// Hangul fillers.
fn is_invisible_or_control(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061C}'
                | '\u{06DD}'
                | '\u{070F}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08E2}'
                | '\u{115F}'..='\u{1160}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{110BD}'
                | '\u{110CD}'
                | '\u{13430}'..='\u{1343F}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0001}'
                | '\u{E0020}'..='\u{E007F}'
        )
}

impl SearchSettings {
    /// `"w rust book"` → (Wikipedia, "rust book") when `w` is an engine's keyword.
    ///
    /// `w` would be a custom engine's keyword; the built-in Wikipedia engine's is `@wikipedia`,
    /// so `"@wikipedia rust book"` gives the same.
    ///
    /// The keyword must be the first word, compared case-insensitively, and be followed by
    /// whitespace and a non-empty query; the query is returned trimmed. A keyword alone, or a
    /// word that merely starts with a keyword (`w.org`), is not a match.
    pub fn match_keyword<'a>(&self, input: &'a str) -> Option<(SearchEngine, &'a str)> {
        let (word, rest) = input.trim_start().split_once(char::is_whitespace)?;
        let query = rest.trim();
        if word.is_empty() || query.is_empty() {
            return None;
        }
        let word = word.to_lowercase();
        self.engines()
            .into_iter()
            .find(|engine| !engine.keyword.is_empty() && engine.keyword.to_lowercase() == word)
            .map(|engine| (engine, query))
    }

    /// Built-in engines followed by valid custom ones; ids and keywords unique.
    pub fn engines(&self) -> Vec<SearchEngine> {
        let mut engines = builtin_engines();
        engines.extend(usable_custom_engines(&self.custom_engines, 0).0);
        engines
    }

    /// The engine named by `default_engine`, or DuckDuckGo when that is not a usable engine.
    pub fn default_engine(&self) -> SearchEngine {
        self.engines()
            .into_iter()
            .find(|engine| engine.id == self.default_engine)
            .unwrap_or_else(duckduckgo)
    }
}
