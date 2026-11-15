//! Login CSV files: reading the exports of other browsers and writing Chrome's format.
//!
//! **An exported file holds every password in clear text.** Write one only when the user has
//! explicitly asked for it, to the location they chose, as a file only they can read (mode
//! `0600`, created fresh rather than overwriting a file others can read), and never to a
//! temporary directory, a log or a crash report. Tell the user to delete it once it has been
//! imported elsewhere. [`write_logins_csv`] returns the text in a buffer that is wiped when
//! dropped; the copy on disk is the caller's responsibility. When reading, the caller should
//! likewise wipe the text it read from the file once [`parse_logins_csv`] has returned.
//!
//! Formats, told apart by their header row (names compared case-insensitively, in any order):
//! - Chrome, Edge and Brave: `name,url,username,password,note`; exports before 2023 have no
//!   `note`. (Chromium `components/password_manager/core/browser/export/password_csv_writer.cc`.)
//! - Firefox: `url,username,password,httpRealm,formActionOrigin,guid,timeCreated,timeLastUsed,
//!   timePasswordChanged`, every field quoted. (`toolkit/components/passwordmgr/LoginExport.sys.mjs`.)
//! - Safari and the Passwords app: `Title,URL,Username,Password,Notes,OTPAuth`; older Safari
//!   versions have no `Notes`. One-time-code secrets in `OTPAuth` are not imported.
//!
//! The reader follows RFC 4180: fields may be quoted, a quote inside a quoted field is doubled,
//! quoted fields may contain commas and line breaks, and records end with CRLF or LF. A leading
//! byte-order mark, blank lines and a missing final line break are accepted.

use url::Url;
use zeroize::Zeroizing;

use crate::{LoginKey, Origin, Password, PasswordError};

/// One login read from a CSV file.
#[derive(Clone, Debug)]
pub struct CsvLogin {
    /// The site's URL as written in the file; [`Origin::parse`] accepts it.
    pub url: String,
    /// The user name; may be empty.
    pub username: String,
    /// The password; never empty.
    pub password: Password,
    /// The entry's title (Chrome `name`, Safari `Title`), if present and not empty.
    pub name: Option<String>,
    /// The entry's note (Chrome `note`, Safari `Notes`), if present and not empty.
    pub note: Option<String>,
    /// The HTTP-authentication realm, for Firefox entries that have one.
    pub http_realm: Option<String>,
}

/// A row that was not imported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvSkipped {
    /// The line the row starts on, counting from 1 (the header is usually line 1).
    pub line: usize,
    /// Why; never contains a field value.
    pub reason: String,
}

/// The result of reading a login CSV file.
#[derive(Clone, Debug)]
pub struct CsvImport {
    /// Rows that became logins, in file order.
    pub logins: Vec<CsvLogin>,
    /// Rows that did not, in file order.
    pub skipped: Vec<CsvSkipped>,
}

const CHROME: &[&str] = &["name", "password", "url", "username"];
const CHROME_WITH_NOTE: &[&str] = &["name", "note", "password", "url", "username"];
const FIREFOX: &[&str] = &[
    "formactionorigin",
    "guid",
    "httprealm",
    "password",
    "timecreated",
    "timelastused",
    "timepasswordchanged",
    "url",
    "username",
];
const SAFARI: &[&str] = &["otpauth", "password", "title", "url", "username"];
const SAFARI_WITH_NOTES: &[&str] = &["notes", "otpauth", "password", "title", "url", "username"];

/// Where each wanted value sits in a row.
struct Columns {
    count: usize,
    url: usize,
    username: usize,
    password: usize,
    name: Option<usize>,
    note: Option<usize>,
    realm: Option<usize>,
}

impl Columns {
    fn from_header(header: &[Zeroizing<String>]) -> Result<Self, PasswordError> {
        let names: Vec<String> = header
            .iter()
            .map(|name| name.trim().to_ascii_lowercase())
            .collect();
        let mut sorted: Vec<&str> = names.iter().map(String::as_str).collect();
        sorted.sort_unstable();
        let known = [CHROME, CHROME_WITH_NOTE, FIREFOX, SAFARI, SAFARI_WITH_NOTES];
        if !known.contains(&sorted.as_slice()) {
            return Err(PasswordError::InvalidInput(
                "the header is not that of a Chrome, Edge, Brave, Firefox or Safari login export",
            ));
        }
        let find = |wanted: &str| names.iter().position(|name| name == wanted);
        let required = |wanted: &str| {
            find(wanted).ok_or(PasswordError::InvalidInput(
                "the header lacks a required column",
            ))
        };
        Ok(Self {
            count: names.len(),
            url: required("url")?,
            username: required("username")?,
            password: required("password")?,
            name: find("name").or_else(|| find("title")),
            note: find("note").or_else(|| find("notes")),
            realm: find("httprealm"),
        })
    }

    fn login(&self, fields: &mut [Zeroizing<String>]) -> Result<CsvLogin, &'static str> {
        if fields.len() != self.count {
            return Err("the row has the wrong number of fields");
        }
        let url = fields[self.url].as_str();
        if url
            .get(..10)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("android://"))
        {
            return Err("the entry is for an Android app, not a website");
        }
        let parsed = Url::parse(url).map_err(|_| "the URL could not be read")?;
        Origin::from_url(&parsed)
            .map_err(|_| "the URL is not an https site or a loopback http site")?;
        if fields[self.password].is_empty() {
            return Err("the password is empty");
        }
        let optional = |index: Option<usize>| {
            index
                .map(|index| fields[index].as_str())
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        let (name, note, http_realm) = (
            optional(self.name),
            optional(self.note),
            optional(self.realm),
        );
        Ok(CsvLogin {
            url: fields[self.url].as_str().to_owned(),
            username: fields[self.username].as_str().to_owned(),
            password: Password::new(std::mem::take(&mut *fields[self.password])),
            name,
            note,
            http_realm,
        })
    }
}

/// Reads the login exports of Chrome/Edge/Brave, Firefox and Safari (told apart by header).
///
/// Headers: Chrome, Edge and Brave `name,url,username,password[,note]`; Firefox
/// `url,username,password,httpRealm,formActionOrigin,guid,timeCreated,timeLastUsed,timePasswordChanged`;
/// Safari `Title,URL,Username,Password[,Notes],OTPAuth`. Names are compared case-insensitively
/// and may come in any order. The file is clear text: the caller should wipe its copy of `text`
/// once this returns.
///
/// Rows with the wrong number of fields, broken quoting, an Android-app entry, a URL that is not
/// an https site (or a loopback http site), or an empty password are skipped and listed in
/// [`CsvImport::skipped`]. A missing or unrecognised header is an error.
pub fn parse_logins_csv(text: &str) -> Result<CsvImport, PasswordError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut records = Reader::new(text);
    let columns = loop {
        match records.next() {
            None => return Err(PasswordError::InvalidInput("the file has no header row")),
            Some(Record {
                row: Row::Blank, ..
            }) => {}
            Some(Record {
                row: Row::Malformed(_),
                ..
            }) => {
                return Err(PasswordError::InvalidInput(
                    "the header row has broken quoting",
                ));
            }
            Some(Record {
                row: Row::Fields(header),
                ..
            }) => break Columns::from_header(&header)?,
        }
    };

    let mut import = CsvImport {
        logins: Vec::new(),
        skipped: Vec::new(),
    };
    for Record { line, row } in records {
        let outcome = match row {
            Row::Blank => continue,
            Row::Malformed(reason) => Err(reason),
            Row::Fields(mut fields) => columns.login(&mut fields),
        };
        match outcome {
            Ok(login) => import.logins.push(login),
            Err(reason) => import.skipped.push(CsvSkipped {
                line,
                reason: reason.to_owned(),
            }),
        }
    }
    Ok(import)
}

/// Writes Chrome's format. The result holds every password in clear text; it is wiped on drop.
///
/// Write the result only when the user has explicitly asked for an export, to the location they
/// chose, as a new file only they can read (mode `0600`); never to a temporary directory, a log
/// or a crash report. Tell the user to delete the file once it has been imported elsewhere.
///
/// Columns are `name,url,username,password,note`: the host, the origin, the user name, the
/// password and an empty note. Fields are quoted when they contain a comma, a quote or a line
/// break, and lines end with LF, as Chrome writes them on macOS and Linux. Chrome's format has
/// no realm column, so an HTTP-authentication login is written as a plain entry for its origin
/// and its realm is lost.
pub fn write_logins_csv<'a>(
    logins: impl IntoIterator<Item = (&'a LoginKey, &'a Password)>,
) -> Zeroizing<String> {
    const HEADER: &str = "name,url,username,password,note\n";
    let rows: Vec<[&str; 5]> = logins
        .into_iter()
        .map(|(key, password)| {
            [
                key.origin.host(),
                key.origin.as_str(),
                key.username.as_str(),
                password.expose(),
                "",
            ]
        })
        .collect();
    // Size the buffer exactly, so it never reallocates and leaves an unwiped copy behind.
    let capacity = HEADER.len()
        + rows
            .iter()
            .map(|row| row.iter().map(|field| written_len(field)).sum::<usize>() + row.len())
            .sum::<usize>();
    let mut out = Zeroizing::new(String::with_capacity(capacity));
    out.push_str(HEADER);
    for row in &rows {
        for (index, field) in row.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            write_field(&mut out, field);
        }
        out.push('\n');
    }
    debug_assert_eq!(out.len(), capacity);
    out
}

fn needs_quotes(field: &str) -> bool {
    field.contains([',', '"', '\r', '\n'])
}

fn written_len(field: &str) -> usize {
    if needs_quotes(field) {
        field.len() + 2 + field.matches('"').count()
    } else {
        field.len()
    }
}

fn write_field(out: &mut String, field: &str) {
    if needs_quotes(field) {
        out.push('"');
        for piece in field.split_inclusive('"') {
            out.push_str(piece);
            if piece.ends_with('"') {
                out.push('"');
            }
        }
        out.push('"');
    } else {
        out.push_str(field);
    }
}

enum Row {
    /// An empty line.
    Blank,
    Fields(Vec<Zeroizing<String>>),
    /// Broken quoting; the rest of the line was skipped.
    Malformed(&'static str),
}

struct Record {
    line: usize,
    row: Row,
}

/// Splits CSV text into records. Every field is built in a buffer of its final size and wiped
/// when dropped, because any field may be a password.
struct Reader<'a> {
    text: &'a str,
    position: usize,
    line: usize,
}

impl<'a> Reader<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            position: 0,
            line: 1,
        }
    }

    fn bytes(&self) -> &'a [u8] {
        self.text.as_bytes()
    }

    /// The length of the line break at `index` (LF or CRLF), if there is one.
    fn line_break_at(&self, index: usize) -> Option<usize> {
        match self.bytes().get(index..) {
            Some([b'\n', ..]) => Some(1),
            Some([b'\r', b'\n', ..]) => Some(2),
            _ => None,
        }
    }

    fn unquoted(&mut self) -> Zeroizing<String> {
        let start = self.position;
        while self.position < self.bytes().len()
            && self.bytes()[self.position] != b','
            && self.line_break_at(self.position).is_none()
        {
            self.position += 1;
        }
        Zeroizing::new(self.text[start..self.position].to_owned())
    }

    fn quoted(&mut self) -> Result<Zeroizing<String>, &'static str> {
        let start = self.position + 1;
        let mut index = start;
        let mut line_breaks = 0;
        let end = loop {
            match self.bytes().get(index) {
                None => {
                    self.position = self.bytes().len();
                    self.line += line_breaks;
                    return Err("a quoted field is not closed");
                }
                Some(b'"') if self.bytes().get(index + 1) == Some(&b'"') => index += 2,
                Some(b'"') => break index,
                Some(b'\n') => {
                    line_breaks += 1;
                    index += 1;
                }
                Some(_) => index += 1,
            }
        };
        let raw = &self.text[start..end];
        let mut value = Zeroizing::new(String::with_capacity(raw.len()));
        let mut rest = raw;
        while let Some(found) = rest.find("\"\"") {
            value.push_str(&rest[..found]);
            value.push('"');
            rest = &rest[found + 2..];
        }
        value.push_str(rest);
        self.position = end + 1;
        self.line += line_breaks;
        Ok(value)
    }

    fn skip_rest_of_line(&mut self) {
        while self.position < self.bytes().len() {
            if let Some(length) = self.line_break_at(self.position) {
                self.position += length;
                self.line += 1;
                return;
            }
            self.position += 1;
        }
    }
}

impl Iterator for Reader<'_> {
    type Item = Record;

    fn next(&mut self) -> Option<Record> {
        if self.position >= self.bytes().len() {
            return None;
        }
        let line = self.line;
        if let Some(length) = self.line_break_at(self.position) {
            self.position += length;
            self.line += 1;
            return Some(Record {
                line,
                row: Row::Blank,
            });
        }
        let mut fields = Vec::new();
        loop {
            let field = if self.bytes().get(self.position) == Some(&b'"') {
                self.quoted()
            } else {
                Ok(self.unquoted())
            };
            match field {
                Ok(value) => fields.push(value),
                Err(reason) => {
                    self.skip_rest_of_line();
                    return Some(Record {
                        line,
                        row: Row::Malformed(reason),
                    });
                }
            }
            match self.bytes().get(self.position) {
                None => break,
                Some(b',') => self.position += 1,
                Some(_) => {
                    if let Some(length) = self.line_break_at(self.position) {
                        self.position += length;
                        self.line += 1;
                        break;
                    }
                    self.skip_rest_of_line();
                    return Some(Record {
                        line,
                        row: Row::Malformed("text follows a closing quote"),
                    });
                }
            }
        }
        Some(Record {
            line,
            row: Row::Fields(fields),
        })
    }
}
