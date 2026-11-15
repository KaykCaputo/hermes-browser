// Adapted from Verso (https://github.com/versotile-org/verso), commit
// eb719bdd6c7b4a22b640d33ea55c6d440a5a850d, `src/utils/content_disposition_parser.rs`,
// licensed Apache-2.0 OR MIT (used here under MIT).
// Copyright (c) 2020-2023 Ngo Iok Ui & Tauri Programme within The Commons Conservancy
// The MIT licence text and permission notice for this code are in `LICENSES/MIT-verso.txt`.
//
// Changes from Verso: a character-level tokenizer replaces splitting on `;`, so quoted values may
// contain `;`, `=` and backslash escapes; the `charset'language'` prefix of `filename*` is parsed
// properly (any language tag, UTF-8 and ISO-8859-1), and an undecodable `filename*` falls back to
// `filename`; unknown or missing disposition types still yield the name (RFC 6266 §4.2); the
// value is returned unsanitised, because sanitising is `naming`'s job.

//! Reading the file name out of a `Content-Disposition` header (RFC 6266, RFC 5987).

use percent_encoding::percent_decode_str;

/// The file name a `Content-Disposition` header carries, decoded but not sanitised.
///
/// `filename*` is preferred over `filename` wherever each appears. Empty names count as absent.
pub(crate) fn filename(header: &str) -> Option<String> {
    let params = parameters(header);
    let extended = params
        .iter()
        .filter(|(key, _)| key == "filename*")
        .find_map(|(_, value)| decode_extended(value))
        .filter(|name| !name.is_empty());
    extended.or_else(|| {
        params
            .into_iter()
            .find(|(key, value)| key == "filename" && !value.is_empty())
            .map(|(_, value)| value)
    })
}

/// The `key=value` parameters of the header, keys lower-cased, quoted values unquoted.
fn parameters(header: &str) -> Vec<(String, String)> {
    let mut chars = header.chars().peekable();
    let mut params = Vec::new();

    // The disposition type comes first, unless the header starts straight with a parameter.
    let first_segment = header.split(';').next().unwrap_or("");
    if !first_segment.contains('=') {
        for character in chars.by_ref() {
            if character == ';' {
                break;
            }
        }
    }

    loop {
        while chars.next_if(|c| *c == ';' || c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            break;
        }

        let mut key = String::new();
        let mut has_value = false;
        for character in chars.by_ref() {
            match character {
                '=' => {
                    has_value = true;
                    break;
                }
                ';' => break,
                other => key.push(other),
            }
        }
        if !has_value {
            continue;
        }

        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let mut value = String::new();
        if chars.next_if_eq(&'"').is_some() {
            while let Some(character) = chars.next() {
                match character {
                    '"' => break,
                    '\\' => {
                        if let Some(escaped) = chars.next() {
                            value.push(escaped);
                        }
                    }
                    other => value.push(other),
                }
            }
            // Anything between the closing quote and the next `;` is not part of the value.
            for character in chars.by_ref() {
                if character == ';' {
                    break;
                }
            }
        } else {
            for character in chars.by_ref() {
                if character == ';' {
                    break;
                }
                value.push(character);
            }
            value.truncate(value.trim_end().len());
        }

        let key = key.trim().to_ascii_lowercase();
        if !key.is_empty() {
            params.push((key, value));
        }
    }
    params
}

/// Decodes an RFC 5987 `ext-value`: `charset'[language]'percent-encoded`.
fn decode_extended(value: &str) -> Option<String> {
    let (charset, rest) = value.split_once('\'')?;
    let (_language, encoded) = rest.split_once('\'')?;
    let bytes: Vec<u8> = percent_decode_str(encoded).collect();
    if charset.eq_ignore_ascii_case("utf-8") {
        String::from_utf8(bytes).ok()
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        Some(bytes.into_iter().map(char::from).collect())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::filename;

    #[test]
    fn headers_without_a_file_name_give_none() {
        assert_eq!(filename(""), None);
        assert_eq!(filename("inline"), None);
        assert_eq!(filename("attachment"), None);
        assert_eq!(filename(" attachment; name=\"field_name\""), None);
        assert_eq!(filename("attachment; filename="), None);
        assert_eq!(filename("attachment; filename=\"\""), None);
        assert_eq!(filename("attachment; filename*=UTF-8''"), None);
    }

    #[test]
    fn quoted_and_bare_file_names_are_read() {
        assert_eq!(
            filename("attachment; filename=\"report.pdf\"").as_deref(),
            Some("report.pdf")
        );
        assert_eq!(
            filename("attachment; filename=report.pdf").as_deref(),
            Some("report.pdf")
        );
        assert_eq!(
            filename("ATTACHMENT; FileName = \"report.pdf\" ").as_deref(),
            Some("report.pdf")
        );
        // A plain `filename` is not percent-decoded: RFC 6266 gives it no encoding.
        assert_eq!(
            filename("form-data; name=field_name; filename=\"te%20st.jpg\"").as_deref(),
            Some("te%20st.jpg")
        );
    }

    #[test]
    fn a_quoted_value_may_contain_separators_and_escapes() {
        assert_eq!(
            filename("attachment; filename=\"a;b=c.txt\"; size=3").as_deref(),
            Some("a;b=c.txt")
        );
        assert_eq!(
            filename(r#"attachment; filename="say \"hi\".txt""#).as_deref(),
            Some("say \"hi\".txt")
        );
    }

    #[test]
    fn extended_file_names_are_percent_decoded() {
        assert_eq!(
            filename("attachment; filename*=UTF-8''%E6%B8%AC%E8%A9%A6.html").as_deref(),
            Some("測試.html")
        );
        assert_eq!(
            filename("attachment; filename*=utf-8''file%20name.jpg").as_deref(),
            Some("file name.jpg")
        );
        assert_eq!(
            filename("attachment; filename*=UTF-8'en'file%20name.jpg").as_deref(),
            Some("file name.jpg")
        );
        assert_eq!(
            filename("attachment; filename*=ISO-8859-1''%E9t%E9.txt").as_deref(),
            Some("été.txt")
        );
        // Verso's test: some servers quote the extended value and send raw UTF-8 in it.
        assert_eq!(
            filename("attachment; name= \"field_name\"; filename*=\"UTF-8''測試.html\"").as_deref(),
            Some("測試.html")
        );
    }

    #[test]
    fn the_extended_file_name_wins_wherever_it_appears() {
        assert_eq!(
            filename("attachment; filename=\"plain.txt\"; filename*=UTF-8''fancy%E2%82%AC.txt")
                .as_deref(),
            Some("fancy€.txt")
        );
        assert_eq!(
            filename("attachment; filename*=UTF-8''fancy%E2%82%AC.txt; filename=\"plain.txt\"")
                .as_deref(),
            Some("fancy€.txt")
        );
    }

    #[test]
    fn an_unusable_extended_file_name_falls_back_to_the_plain_one() {
        assert_eq!(
            filename("attachment; filename*=KOI8-R''%E1.txt; filename=\"plain.txt\"").as_deref(),
            Some("plain.txt")
        );
        assert_eq!(
            filename("attachment; filename*=no-quotes.txt; filename=\"plain.txt\"").as_deref(),
            Some("plain.txt")
        );
    }

    #[test]
    fn unknown_or_missing_disposition_types_still_yield_the_name() {
        // RFC 6266 §4.2: unknown types are handled as `attachment`.
        assert_eq!(
            filename("x-custom; filename=\"a.bin\"").as_deref(),
            Some("a.bin")
        );
        assert_eq!(filename("filename=\"a.bin\"").as_deref(), Some("a.bin"));
    }

    #[test]
    fn raw_utf8_and_path_characters_are_returned_unsanitised() {
        assert_eq!(
            filename("attachment; filename=\"résumé.pdf\"").as_deref(),
            Some("résumé.pdf")
        );
        // Sanitising is `naming`'s job; the parser returns what the server said.
        assert_eq!(
            filename("attachment; filename=../path/to/file.html").as_deref(),
            Some("../path/to/file.html")
        );
    }

    #[test]
    fn malformed_headers_do_not_panic() {
        for header in [
            ";",
            ";;;=;",
            "attachment; filename=\"unterminated",
            "attachment; filename*=UTF-8''%",
            "attachment; filename*=UTF-8''%E6%B8",
            "attachment; filename*='''",
            "attachment; =x",
            "attachment; filename=\"\\",
            "\u{feff}attachment; filename=é",
        ] {
            let _ = filename(header);
        }
        assert_eq!(
            filename("attachment; filename=\"unterminated").as_deref(),
            Some("unterminated")
        );
    }
}
