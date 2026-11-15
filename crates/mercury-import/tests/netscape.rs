use mercury_import::{
    ImportError, ImportedNode, parse_netscape_bookmarks, write_netscape_bookmarks,
};

const CHROME_EXPORT: &str = include_str!("fixtures/chrome_export.html");
const FIREFOX_EXPORT: &str = include_str!("fixtures/firefox_export.html");
const SAFARI_EXPORT: &str = include_str!("fixtures/safari_export.html");

fn folder(title: &str, added_at_ms: Option<i64>, children: Vec<ImportedNode>) -> ImportedNode {
    ImportedNode::Folder {
        title: title.into(),
        toolbar: false,
        added_at_ms,
        children,
    }
}

fn toolbar(title: &str, added_at_ms: Option<i64>, children: Vec<ImportedNode>) -> ImportedNode {
    ImportedNode::Folder {
        title: title.into(),
        toolbar: true,
        added_at_ms,
        children,
    }
}

fn bookmark(title: &str, url: &str, added_at_ms: Option<i64>) -> ImportedNode {
    ImportedNode::Bookmark {
        title: title.into(),
        url: url.into(),
        added_at_ms,
    }
}

#[test]
fn a_chrome_export_keeps_the_bar_nesting_entities_and_dates() {
    let nodes = parse_netscape_bookmarks(CHROME_EXPORT).expect("the Chrome export should parse");

    assert_eq!(
        nodes,
        vec![
            toolbar(
                "Bookmarks bar",
                Some(1_700_000_000_000),
                vec![
                    bookmark(
                        "News & Views",
                        "https://news.example/",
                        Some(1_700_000_100_000)
                    ),
                    folder(
                        "Work",
                        Some(1_700_000_200_000),
                        vec![bookmark(
                            "Mail",
                            "https://mail.example/inbox?a=1&b=2",
                            Some(1_700_000_300_000)
                        )]
                    ),
                ]
            ),
            bookmark(
                "Read later",
                "https://later.example/",
                Some(1_700_000_450_000)
            ),
        ]
    );
}

#[test]
fn a_firefox_export_keeps_separators_and_its_root_folders() {
    let nodes = parse_netscape_bookmarks(FIREFOX_EXPORT).expect("the Firefox export should parse");

    assert_eq!(
        nodes,
        vec![
            bookmark(
                "Firefox \u{2014} Help",
                "https://www.mozilla.example/en-US/firefox/",
                Some(1_690_000_100_000)
            ),
            ImportedNode::Separator,
            folder(
                "Recettes à essayer",
                Some(1_690_000_200_000),
                vec![bookmark(
                    "Crêpes",
                    "https://cuisine.example/cr%C3%AApes",
                    Some(1_690_000_250_000)
                )]
            ),
            toolbar(
                "Bookmarks Toolbar",
                Some(1_690_000_000_000),
                vec![
                    bookmark("Docs", "https://docs.example/", Some(1_690_000_500_000)),
                    ImportedNode::Separator,
                    bookmark("Files", "ftp://files.example/pub/", Some(1_690_000_600_000)),
                ]
            ),
            folder(
                "Other Bookmarks",
                Some(1_690_000_000_000),
                vec![bookmark("Config", "about:config", Some(1_690_000_800_000))]
            ),
        ]
    );
}

#[test]
fn a_safari_export_without_an_outer_list_or_dates_is_read() {
    let nodes = parse_netscape_bookmarks(SAFARI_EXPORT).expect("the Safari export should parse");

    assert_eq!(
        nodes,
        vec![
            folder(
                "Favorites",
                None,
                vec![
                    bookmark("Apple", "https://www.apple.example/", None),
                    bookmark("Café map", "https://maps.example/?q=caf%C3%A9&z=3", None),
                ]
            ),
            folder("Bookmarks Menu", None, vec![]),
            folder(
                "Travel",
                None,
                vec![bookmark("Trains", "https://trains.example/", None)]
            ),
            folder(
                "Reading List",
                None,
                vec![bookmark(
                    "A long read",
                    "https://article.example/long-read",
                    None
                )]
            ),
        ]
    );
}

#[test]
fn missing_closing_tags_lower_case_and_any_attribute_order_are_tolerated() {
    let html = r#"<!doctype netscape-bookmark-file-1>
<dl><p>
  <dt><h3 personal_toolbar_folder="TRUE" add_date="10">Bar
  <dl><p>
    <dt><a add_date="20" href="https://a.example/?x=1&amp;y=&lt;2&gt;">A &amp; B
    <dt><A HREF='https://b.example/'>B</a>
    <dt><a>No address</a>
    <dt><a href="https://c.example/">C
  </dl>
  <dt><h3>Empty folder</h3>
  <dt><a href="https://d.example/">D</a>
"#;

    let nodes = parse_netscape_bookmarks(html).expect("a sloppy file should still parse");

    assert_eq!(
        nodes,
        vec![
            toolbar(
                "Bar",
                Some(10_000),
                vec![
                    bookmark("A & B", "https://a.example/?x=1&y=<2>", Some(20_000)),
                    bookmark("B", "https://b.example/", None),
                    bookmark("C", "https://c.example/", None),
                ]
            ),
            folder("Empty folder", None, vec![]),
            bookmark("D", "https://d.example/", None),
        ]
    );
}

#[test]
fn add_date_is_read_as_seconds_and_a_bad_one_is_left_out() {
    let html = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
    <DT><A HREF="https://a.example/" ADD_DATE="1">A</A>
    <DT><A HREF="https://b.example/" ADD_DATE="yesterday">B</A>
    <DT><A HREF="https://c.example/" ADD_DATE="99999999999999999999">C</A>
    <DT><A HREF="https://d.example/" ADD_DATE="9223372036854775">D</A>
</DL><p>
"#;

    let nodes = parse_netscape_bookmarks(html).expect("the file should parse");

    assert_eq!(
        nodes,
        vec![
            bookmark("A", "https://a.example/", Some(1_000)),
            bookmark("B", "https://b.example/", None),
            bookmark("C", "https://c.example/", None),
            bookmark("D", "https://d.example/", Some(9_223_372_036_854_775_000)),
        ]
    );
}

#[test]
fn only_web_file_and_about_addresses_are_imported() {
    let html = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
    <DT><A HREF="http://plain.example/">http</A>
    <DT><A HREF="https://secure.example/">https</A>
    <DT><A HREF="ftp://files.example/">ftp</A>
    <DT><A HREF="file:///Users/someone/notes.html">file</A>
    <DT><A HREF="about:blank">about</A>
    <DT><A HREF="javascript:void(0)">javascript</A>
    <DT><A HREF="JavaScript:alert(1)">mixed-case javascript</A>
    <DT><A HREF="data:text/html,<p>hi</p>">data</A>
    <DT><A HREF="chrome://settings/">chrome</A>
    <DT><A HREF="place:sort=8&maxResults=10">place</A>
    <DT><A HREF="not a url at all">unparsable</A>
    <DT><A HREF="">empty</A>
    <DT><H3>Nested</H3>
    <DL><p>
        <DT><A HREF="javascript:alert(2)">nested javascript</A>
        <DT><A HREF="https://kept.example/">kept</A>
    </DL><p>
</DL><p>
"#;

    let nodes = parse_netscape_bookmarks(html).expect("the file should parse");

    assert_eq!(
        nodes,
        vec![
            bookmark("http", "http://plain.example/", None),
            bookmark("https", "https://secure.example/", None),
            bookmark("ftp", "ftp://files.example/", None),
            bookmark("file", "file:///Users/someone/notes.html", None),
            bookmark("about", "about:blank", None),
            folder(
                "Nested",
                None,
                vec![bookmark("kept", "https://kept.example/", None)]
            ),
        ]
    );
}

#[test]
fn text_that_is_not_a_bookmark_file_is_refused() {
    for text in [
        "",
        "just some text",
        "<html><body><p>Hello</p></body></html>",
    ] {
        assert!(
            matches!(
                parse_netscape_bookmarks(text),
                Err(ImportError::Malformed(_))
            ),
            "{text:?} should be refused"
        );
    }
}

#[test]
fn an_empty_bookmark_file_is_an_empty_import() {
    let html = "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<H1>Bookmarks</H1>\n<DL><p>\n</DL><p>\n";

    assert_eq!(parse_netscape_bookmarks(html).unwrap(), vec![]);
}

#[test]
fn deeply_nested_lists_neither_overflow_nor_lose_bookmarks() {
    let depth = 20_000;
    let mut html = String::from("<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<DL><p>\n");
    for level in 0..depth {
        html.push_str(&format!("<DT><H3>Level {level}</H3>\n<DL><p>\n"));
    }
    html.push_str("<DT><A HREF=\"https://deep.example/\">Deep</A>\n");

    let nodes = parse_netscape_bookmarks(&html).expect("a deep file should parse");

    let mut levels = 0;
    let mut node = &nodes[0];
    while let ImportedNode::Folder { children, .. } = node {
        assert_eq!(children.len(), 1, "each level holds one entry");
        levels += 1;
        node = &children[0];
    }
    assert!(levels < 100, "nesting is capped, got {levels} levels");
    assert_eq!(node, &bookmark("Deep", "https://deep.example/", None));
}

#[test]
fn the_writer_produces_the_standard_file() {
    let nodes = vec![
        toolbar(
            "Bar",
            Some(1_700_000_000_000),
            vec![
                bookmark(
                    "A <b>&\"'",
                    "https://a.example/?x=1&y=\"2\"",
                    Some(1_700_000_001_999),
                ),
                ImportedNode::Separator,
            ],
        ),
        folder("Empty", None, vec![]),
        bookmark("Plain", "https://plain.example/", None),
    ];

    let html = write_netscape_bookmarks(&nodes);

    assert_eq!(
        html,
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n\
         <!-- This is an automatically generated file.\n     \
         It will be read and overwritten.\n     \
         DO NOT EDIT! -->\n\
         <META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n\
         <TITLE>Bookmarks</TITLE>\n\
         <H1>Bookmarks</H1>\n\
         <DL><p>\n    \
         <DT><H3 ADD_DATE=\"1700000000\" PERSONAL_TOOLBAR_FOLDER=\"true\">Bar</H3>\n    \
         <DL><p>\n        \
         <DT><A HREF=\"https://a.example/?x=1&amp;y=&quot;2&quot;\" ADD_DATE=\"1700000001\">A &lt;b&gt;&amp;&quot;&#39;</A>\n        \
         <HR>\n    \
         </DL><p>\n    \
         <DT><H3>Empty</H3>\n    \
         <DL><p>\n    \
         </DL><p>\n    \
         <DT><A HREF=\"https://plain.example/\">Plain</A>\n\
         </DL><p>\n"
    );
    assert_eq!(
        write_netscape_bookmarks(&nodes),
        html,
        "output is deterministic"
    );
}

#[test]
fn writing_then_parsing_gives_the_same_tree() {
    let long_title = "Ω long title ".repeat(400);
    let tree = vec![
        toolbar(
            "Bookmarks bar",
            Some(1_600_000_000_000),
            vec![
                bookmark(
                    "<b>&\"'",
                    "https://example.com/?q=<b>&x=\"y\"&z='w'",
                    Some(1_600_000_001_000),
                ),
                ImportedNode::Separator,
                folder(
                    "Ünïcödé 文件夹 📚",
                    Some(1_600_000_002_000),
                    vec![
                        bookmark("Ελληνικά", "https://el.wikipedia.org/wiki/Ελλάδα", None),
                        folder(
                            "Deeper",
                            None,
                            vec![
                                bookmark(long_title.trim(), "https://long.example/", Some(0)),
                                ImportedNode::Separator,
                            ],
                        ),
                        folder("Empty", None, vec![]),
                    ],
                ),
            ],
        ),
        ImportedNode::Separator,
        bookmark("File", "file:///tmp/a%20b.html", Some(-86_400_000)),
        bookmark("About", "about:blank", None),
    ];

    let html = write_netscape_bookmarks(&tree);

    assert_eq!(parse_netscape_bookmarks(&html).unwrap(), tree);
}
