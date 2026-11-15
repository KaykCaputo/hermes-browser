// The writer (`escape`, `write_netscape_bookmarks`, `write_list`) is adapted from Zephium,
// https://github.com/zephium-browser/Zephium, commit 9026a1fffad74d6e201ef97fe8c02c1b01403cd2,
// crates/zephium-core/src/export.rs lines 28-92, licensed under MPL-2.0. The parser is new.

//! The Netscape bookmark file (`bookmarks.html`) that every browser imports and exports.
//!
//! The format, as Firefox's `BookmarkHTMLUtils.sys.mjs` describes it: a heading other than `<H1>`
//! names a folder, the `<DL>` (or `<UL>`, `<MENU>`) that follows holds its contents, `<A HREF>`
//! is a bookmark whose text is its title, `<HR>` is a separator and `<DD>` a description. Real
//! files leave tags unclosed, so the parser keeps a stack of open folders and closes what it has
//! to, as Firefox's importer does, reading the tokens of `html5ever`'s tokenizer.

use std::cell::RefCell;
use std::fmt::Write as _;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};

use crate::model::{ImportError, ImportedNode, MAX_TREE_DEPTH, keep_importable};

/// Parses a Netscape bookmark file. Tolerant: real exports are not well-formed HTML.
///
/// Folders marked `PERSONAL_TOOLBAR_FOLDER="true"` become toolbar folders; `ADD_DATE` (seconds)
/// becomes milliseconds; bookmarks without `HREF` or with an address that cannot be imported are
/// left out, and the addresses kept are normalised. Text that has neither the Netscape doctype
/// nor a list is refused as [`ImportError::Malformed`].
pub fn parse_netscape_bookmarks(html: &str) -> Result<Vec<ImportedNode>, ImportError> {
    let events = tokenize(html);
    let looks_like_bookmarks = events.iter().any(|event| match event {
        Event::Doctype(name) => name.eq_ignore_ascii_case("netscape-bookmark-file-1"),
        Event::Start(start) => is_list(&start.name),
        _ => false,
    });
    if !looks_like_bookmarks {
        return Err(ImportError::Malformed(
            "not a Netscape bookmark file: no bookmark doctype and no list".into(),
        ));
    }
    let mut builder = Builder::default();
    for event in events {
        builder.event(event);
    }
    Ok(keep_importable(builder.finish(), "bookmarks.html", 0))
}

/// What the parser needs of the tokenizer's output.
enum Event {
    Doctype(String),
    Start(StartTag),
    End(String),
    Text(String),
}

struct StartTag {
    name: String,
    /// Names are lower case, values have their character references decoded.
    attrs: Vec<(String, String)>,
}

impl StartTag {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Default)]
struct Sink {
    events: RefCell<Vec<Event>>,
}

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        let event = match token {
            Token::DoctypeToken(doctype) => Event::Doctype(
                doctype
                    .name
                    .map(|name| name.to_string())
                    .unwrap_or_default(),
            ),
            Token::TagToken(Tag {
                kind: TagKind::StartTag,
                name,
                attrs,
                ..
            }) => Event::Start(StartTag {
                name: name.to_string(),
                attrs: attrs
                    .into_iter()
                    .map(|attr| (attr.name.local.to_string(), attr.value.to_string()))
                    .collect(),
            }),
            Token::TagToken(Tag {
                kind: TagKind::EndTag,
                name,
                ..
            }) => Event::End(name.to_string()),
            Token::CharacterTokens(text) => Event::Text(text.to_string()),
            _ => return TokenSinkResult::Continue,
        };
        self.events.borrow_mut().push(event);
        TokenSinkResult::Continue
    }
}

fn tokenize(html: &str) -> Vec<Event> {
    let tokenizer = Tokenizer::new(Sink::default(), TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    // Without a tree builder no token asks the tokenizer to pause, so one feed reads it all.
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.events.take()
}

fn is_list(name: &str) -> bool {
    matches!(name, "dl" | "ul" | "menu")
}

fn is_folder_heading(name: &str) -> bool {
    matches!(name, "h2" | "h3" | "h4" | "h5" | "h6")
}

/// `ADD_DATE` is whole seconds since the Unix epoch.
fn add_date(start: &StartTag) -> Option<i64> {
    start
        .attr("add_date")?
        .trim()
        .parse::<i64>()
        .ok()?
        .checked_mul(1000)
}

struct FolderHead {
    title: String,
    toolbar: bool,
    added_at_ms: Option<i64>,
}

impl FolderHead {
    fn into_node(self, children: Vec<ImportedNode>) -> ImportedNode {
        ImportedNode::Folder {
            title: self.title,
            toolbar: self.toolbar,
            added_at_ms: self.added_at_ms,
            children,
        }
    }
}

/// An element whose text is being read as a title.
enum Titled {
    Folder(FolderHead),
    Bookmark {
        url: Option<String>,
        added_at_ms: Option<i64>,
        title: String,
    },
}

/// A folder being filled. The outermost frame is the file itself.
struct Frame {
    head: Option<FolderHead>,
    children: Vec<ImportedNode>,
    /// Lists opened in this frame and not yet closed; the frame ends when the last one closes.
    open_lists: usize,
}

struct Builder {
    stack: Vec<Frame>,
    titled: Option<Titled>,
    /// A folder heading that is complete and waits for the list holding its contents.
    awaiting_list: Option<FolderHead>,
}

impl Default for Builder {
    fn default() -> Self {
        Builder {
            stack: vec![Frame {
                head: None,
                children: Vec::new(),
                open_lists: 0,
            }],
            titled: None,
            awaiting_list: None,
        }
    }
}

impl Builder {
    fn event(&mut self, event: Event) {
        match event {
            Event::Text(text) => match &mut self.titled {
                Some(Titled::Folder(head)) => head.title.push_str(&text),
                Some(Titled::Bookmark { title, .. }) => title.push_str(&text),
                None => {}
            },
            Event::Doctype(_) => {}
            // Any tag ends a title: titles hold text only, and files leave `</A>` and `</H3>`
            // out.
            Event::Start(start) => {
                self.end_title();
                self.start(start);
            }
            Event::End(name) => {
                self.end_title();
                if is_list(&name) {
                    self.close_list();
                }
            }
        }
    }

    fn start(&mut self, start: StartTag) {
        match start.name.as_str() {
            "a" => {
                self.flush_awaiting();
                self.titled = Some(Titled::Bookmark {
                    url: start.attr("href").map(|href| href.trim().to_owned()),
                    added_at_ms: add_date(&start),
                    title: String::new(),
                });
            }
            name if is_folder_heading(name) => {
                self.flush_awaiting();
                self.titled = Some(Titled::Folder(FolderHead {
                    title: String::new(),
                    toolbar: start
                        .attr("personal_toolbar_folder")
                        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true")),
                    added_at_ms: add_date(&start),
                }));
            }
            "hr" => {
                self.flush_awaiting();
                self.current().children.push(ImportedNode::Separator);
            }
            name if is_list(name) => match self.awaiting_list.take() {
                // The outermost frame is not a folder, so the stack holds at most
                // MAX_TREE_DEPTH folders; deeper ones are opened into their parent.
                Some(head) if self.stack.len() <= MAX_TREE_DEPTH => self.stack.push(Frame {
                    head: Some(head),
                    children: Vec::new(),
                    open_lists: 1,
                }),
                _ => self.current().open_lists += 1,
            },
            _ => {}
        }
    }

    fn current(&mut self) -> &mut Frame {
        self.stack
            .last_mut()
            .expect("the outermost frame is never removed")
    }

    fn end_title(&mut self) {
        match self.titled.take() {
            Some(Titled::Folder(mut head)) => {
                head.title = head.title.trim().to_owned();
                self.awaiting_list = Some(head);
            }
            Some(Titled::Bookmark {
                url: Some(url),
                added_at_ms,
                title,
            }) => self.current().children.push(ImportedNode::Bookmark {
                title: title.trim().to_owned(),
                url,
                added_at_ms,
            }),
            // An anchor without an address is not a bookmark.
            Some(Titled::Bookmark { url: None, .. }) | None => {}
        }
    }

    /// A heading followed by something other than a list is an empty folder.
    fn flush_awaiting(&mut self) {
        if let Some(head) = self.awaiting_list.take() {
            self.current().children.push(head.into_node(Vec::new()));
        }
    }

    fn close_list(&mut self) {
        self.flush_awaiting();
        let frame = self.current();
        frame.open_lists = frame.open_lists.saturating_sub(1);
        if frame.open_lists == 0 && self.stack.len() > 1 {
            self.pop_frame();
        }
    }

    fn pop_frame(&mut self) {
        let frame = self
            .stack
            .pop()
            .expect("only called above the outermost frame");
        if let Some(head) = frame.head {
            self.current().children.push(head.into_node(frame.children));
        }
    }

    /// Closes whatever the file left open.
    fn finish(mut self) -> Vec<ImportedNode> {
        self.end_title();
        self.flush_awaiting();
        while self.stack.len() > 1 {
            self.pop_frame();
        }
        self.stack
            .pop()
            .map(|frame| frame.children)
            .unwrap_or_default()
    }
}

/// Writes a Netscape bookmark file that Chrome, Firefox and Safari can import.
///
/// Folders carry `ADD_DATE` when known and `PERSONAL_TOOLBAR_FOLDER="true"` for the toolbar;
/// dates are written in whole seconds, so milliseconds below a second do not survive a round
/// trip. Folders nested deeper than the parser keeps (48 levels) are opened into the deepest
/// one kept, as the parser would; their own titles are not written. The same tree always gives
/// the same text.
///
/// `parse_netscape_bookmarks(&write_netscape_bookmarks(tree)) == tree` holds exactly for trees
/// whose URLs are already normalised (as every URL this crate reads is), whose dates are whole
/// seconds, whose titles have no surrounding whitespace or control characters other than line
/// breaks and tabs, and which nest at most 48 folders deep.
pub fn write_netscape_bookmarks(nodes: &[ImportedNode]) -> String {
    let mut out = String::from(
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n\
         <!-- This is an automatically generated file.\n     \
         It will be read and overwritten.\n     \
         DO NOT EDIT! -->\n\
         <META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n\
         <TITLE>Bookmarks</TITLE>\n\
         <H1>Bookmarks</H1>\n\
         <DL><p>\n",
    );
    // An explicit stack rather than recursion, so no tree can exhaust the call stack.
    let mut lists = vec![List {
        nodes: nodes.iter(),
        depth: 0,
        closes: true,
    }];
    while let Some(list) = lists.last_mut() {
        let depth = list.depth;
        let Some(node) = list.nodes.next() else {
            if lists.pop().is_some_and(|list| list.closes) {
                let _ = writeln!(out, "{}</DL><p>", indent(depth));
            }
            continue;
        };
        if let ImportedNode::Folder { children, .. } = node
            && depth >= MAX_TREE_DEPTH
        {
            // Opened into the list around it: nothing of the folder itself is written.
            lists.push(List {
                nodes: children.iter(),
                depth,
                closes: false,
            });
            continue;
        }
        out.push_str(&indent(depth));
        match node {
            ImportedNode::Folder {
                title,
                toolbar,
                added_at_ms,
                children,
            } => {
                out.push_str("    <DT><H3");
                write_add_date(*added_at_ms, &mut out);
                if *toolbar {
                    out.push_str(" PERSONAL_TOOLBAR_FOLDER=\"true\"");
                }
                out.push('>');
                escape(title, &mut out);
                out.push_str("</H3>\n");
                let _ = writeln!(out, "{}<DL><p>", indent(depth + 1));
                lists.push(List {
                    nodes: children.iter(),
                    depth: depth + 1,
                    closes: true,
                });
            }
            ImportedNode::Bookmark {
                title,
                url,
                added_at_ms,
            } => {
                out.push_str("    <DT><A HREF=\"");
                escape(url, &mut out);
                out.push('"');
                write_add_date(*added_at_ms, &mut out);
                out.push('>');
                escape(title, &mut out);
                out.push_str("</A>\n");
            }
            ImportedNode::Separator => out.push_str("    <HR>\n"),
        }
    }
    out
}

/// One list being written: its remaining entries, how deep it is, and whether it was opened
/// with `<DL>` (an opened-up folder's entries are written into the list around it).
struct List<'a> {
    nodes: std::slice::Iter<'a, ImportedNode>,
    depth: usize,
    closes: bool,
}

/// Four spaces a level.
fn indent(depth: usize) -> String {
    "    ".repeat(depth)
}

fn write_add_date(added_at_ms: Option<i64>, out: &mut String) {
    if let Some(ms) = added_at_ms {
        let _ = write!(out, " ADD_DATE=\"{}\"", ms.div_euclid(1000));
    }
}

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // As character references, line breaks and tabs survive a round trip; the parser only
            // trims them from the ends of a title.
            '\n' | '\r' | '\t' => {
                let _ = write!(out, "&#{};", u32::from(c));
            }
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
}
