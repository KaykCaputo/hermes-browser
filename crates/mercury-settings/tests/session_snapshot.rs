use mercury_settings::{
    ClosedTab, MAX_CLOSED_TABS, MAX_RESTORED_TABS, MAX_TITLE_BYTES, MAX_URL_BYTES, SessionSnapshot,
    TabSnapshot, WindowBounds, WindowSnapshot,
};

fn tab(url: &str) -> TabSnapshot {
    TabSnapshot {
        url: url.to_owned(),
        title: format!("title of {url}"),
        ..TabSnapshot::default()
    }
}

fn numbered(n: usize) -> TabSnapshot {
    tab(&format!("https://site{n}.example/"))
}

fn window(tabs: Vec<TabSnapshot>, active_tab: usize) -> WindowSnapshot {
    WindowSnapshot {
        tabs,
        active_tab,
        ..WindowSnapshot::default()
    }
}

fn snapshot(windows: Vec<WindowSnapshot>) -> SessionSnapshot {
    SessionSnapshot {
        windows,
        ..SessionSnapshot::default()
    }
}

fn urls(window: &WindowSnapshot) -> Vec<&str> {
    window.tabs.iter().map(|tab| tab.url.as_str()).collect()
}

fn closed(n: usize) -> ClosedTab {
    ClosedTab {
        tab: numbered(n),
        window: n % 3,
        index: n,
        closed_at_ms: 1_000 + i64::try_from(n).expect("small"),
    }
}

#[test]
fn a_tab_defaults_to_100_percent_zoom() {
    assert_eq!(TabSnapshot::default().zoom_percent, 100);
    let parsed: TabSnapshot =
        serde_json::from_str(r#"{"url": "https://example.org/"}"#).expect("parse");
    assert_eq!(parsed.zoom_percent, 100);
    assert!(!parsed.pinned);
}

#[test]
fn a_snapshot_survives_a_serde_round_trip() {
    let original = SessionSnapshot {
        windows: vec![WindowSnapshot {
            tabs: vec![
                TabSnapshot {
                    url: "https://example.org/".to_owned(),
                    title: "Example — “quoted” 🦀".to_owned(),
                    zoom_percent: 150,
                    pinned: true,
                },
                tab("about:blank"),
            ],
            active_tab: 1,
            bounds: Some(WindowBounds {
                x: -1200,
                y: 40,
                width: 1280,
                height: 800,
            }),
            maximized: true,
            fullscreen: false,
        }],
        closed_tabs: vec![closed(1)],
        saved_at_ms: 1_763_200_000_000,
    };

    let json = serde_json::to_string(&original).expect("serialise");
    let back: SessionSnapshot = serde_json::from_str(&json).expect("deserialise");

    assert_eq!(back, original);
    assert_eq!(
        back.clone().sanitized(),
        original,
        "a clean snapshot is unchanged"
    );
}

#[test]
fn only_http_https_file_and_about_tabs_are_kept() {
    let long = format!("https://example.org/{}", "a".repeat(MAX_URL_BYTES));
    let session = snapshot(vec![window(
        vec![
            tab("https://example.org/"),
            tab("javascript:alert(1)"),
            tab("http://example.org/"),
            tab("data:text/html,<p>hi</p>"),
            tab("blob:https://example.org/0b7a"),
            tab("file:///Users/someone/page.html"),
            tab("not a url"),
            tab(""),
            tab(&long),
            tab("about:blank"),
            tab("mercury://settings"),
        ],
        0,
    )])
    .sanitized();

    assert_eq!(
        urls(&session.windows[0]),
        vec![
            "https://example.org/",
            "http://example.org/",
            "file:///Users/someone/page.html",
            "about:blank",
        ]
    );
}

#[test]
fn titles_are_truncated_at_a_character_boundary() {
    let mut long = tab("https://example.org/");
    long.title = format!("{}é{}", "a".repeat(MAX_TITLE_BYTES - 1), "b".repeat(50));
    let session = snapshot(vec![window(vec![long], 0)]).sanitized();

    let title = &session.windows[0].tabs[0].title;
    assert!(title.len() <= MAX_TITLE_BYTES);
    assert_eq!(title, &"a".repeat(MAX_TITLE_BYTES - 1));
}

#[test]
fn zoom_is_clamped_into_30_to_500() {
    let zoomed = |zoom_percent| TabSnapshot {
        zoom_percent,
        ..tab("https://example.org/")
    };
    let session = snapshot(vec![window(
        vec![
            zoomed(0),
            zoomed(29),
            zoomed(30),
            zoomed(250),
            zoomed(500),
            zoomed(9000),
        ],
        0,
    )])
    .sanitized();

    let zooms: Vec<u16> = session.windows[0]
        .tabs
        .iter()
        .map(|tab| tab.zoom_percent)
        .collect();
    assert_eq!(zooms, vec![30, 30, 30, 250, 500, 500]);
}

#[test]
fn windows_left_without_tabs_are_dropped() {
    let session = snapshot(vec![
        window(Vec::new(), 0),
        window(vec![tab("javascript:void(0)")], 0),
        window(vec![tab("https://kept.example/")], 0),
    ])
    .sanitized();

    assert_eq!(session.windows.len(), 1);
    assert_eq!(urls(&session.windows[0]), vec!["https://kept.example/"]);
}

#[test]
fn an_active_tab_out_of_range_becomes_the_last() {
    let session = snapshot(vec![window(
        vec![numbered(0), numbered(1), numbered(2)],
        99,
    )])
    .sanitized();
    assert_eq!(session.windows[0].active_tab, 2);
}

#[test]
fn when_the_active_tab_is_dropped_its_right_neighbour_becomes_active() {
    let session = snapshot(vec![window(
        vec![
            numbered(0),
            numbered(1),
            tab("javascript:void(0)"),
            tab("data:,x"),
            numbered(4),
        ],
        2,
    )])
    .sanitized();

    let window = &session.windows[0];
    assert_eq!(window.tabs[window.active_tab].url, "https://site4.example/");
}

#[test]
fn when_the_active_tab_and_everything_right_of_it_are_dropped_the_left_neighbour_is_active() {
    let session = snapshot(vec![window(
        vec![
            numbered(0),
            numbered(1),
            tab("javascript:void(0)"),
            tab("data:,x"),
        ],
        2,
    )])
    .sanitized();

    let window = &session.windows[0];
    assert_eq!(window.tabs[window.active_tab].url, "https://site1.example/");
}

#[test]
fn the_active_tab_keeps_pointing_at_the_same_tab_after_earlier_tabs_are_dropped() {
    let session = snapshot(vec![window(
        vec![
            tab("javascript:void(0)"),
            numbered(1),
            tab("data:,x"),
            numbered(3),
        ],
        3,
    )])
    .sanitized();

    let window = &session.windows[0];
    assert_eq!(window.active_tab, 1);
    assert_eq!(window.tabs[1].url, "https://site3.example/");
}

#[test]
fn too_many_tabs_keep_the_pinned_the_active_and_the_newest_positions() {
    let total = MAX_RESTORED_TABS + 100;
    let mut tabs: Vec<TabSnapshot> = (0..total).map(numbered).collect();
    for pinned in &mut tabs[..3] {
        pinned.pinned = true;
    }
    let active = 50;
    let session = snapshot(vec![window(tabs, active)]).sanitized();

    let window = &session.windows[0];
    assert_eq!(window.tabs.len(), MAX_RESTORED_TABS);
    let kept = urls(window);
    for n in [0, 1, 2, active, total - 1] {
        assert!(
            kept.contains(&format!("https://site{n}.example/").as_str()),
            "tab {n} should survive"
        );
    }
    // After the pinned and the active tab, the rest come from the end of the strip, where new
    // tabs open.
    let first_of_tail = total - (MAX_RESTORED_TABS - 4);
    assert_eq!(kept[4], format!("https://site{first_of_tail}.example/"));
    assert!(!kept.contains(&"https://site3.example/"));
    assert_eq!(
        window.tabs[window.active_tab].url,
        "https://site50.example/"
    );
    let mut numbers: Vec<usize> = kept
        .iter()
        .map(|url| {
            url.trim_start_matches("https://site")
                .trim_end_matches(".example/")
                .parse()
                .expect("a number")
        })
        .collect();
    let in_order = numbers.clone();
    numbers.sort_unstable();
    assert_eq!(in_order, numbers, "the original order is kept");
}

#[test]
fn the_tab_cap_is_shared_fairly_between_windows() {
    let half = MAX_RESTORED_TABS;
    let session = snapshot(vec![
        window((0..half).map(numbered).collect(), 0),
        window((half..2 * half).map(numbered).collect(), 0),
    ])
    .sanitized();

    let counts: Vec<usize> = session
        .windows
        .iter()
        .map(|window| window.tabs.len())
        .collect();
    assert_eq!(counts.iter().sum::<usize>(), MAX_RESTORED_TABS);
    assert_eq!(counts, vec![MAX_RESTORED_TABS / 2, MAX_RESTORED_TABS / 2]);
}

#[test]
fn closed_tabs_are_capped_and_invalid_ones_dropped() {
    let mut session = snapshot(vec![window(vec![numbered(0)], 0)]);
    session.closed_tabs = (0..MAX_CLOSED_TABS + 10).map(closed).collect();
    session.closed_tabs[0].tab.url = "javascript:alert(1)".to_owned();

    let session = session.sanitized();

    assert_eq!(session.closed_tabs.len(), MAX_CLOSED_TABS);
    assert_eq!(session.closed_tabs[0], closed(1), "newest valid first");
    assert_eq!(
        session.closed_tabs.last(),
        Some(&closed(MAX_CLOSED_TABS)),
        "the oldest beyond the cap are dropped"
    );
}

#[test]
fn zero_sized_or_absurd_window_bounds_are_forgotten() {
    let bounded = |x, y, width, height| WindowSnapshot {
        bounds: Some(WindowBounds {
            x,
            y,
            width,
            height,
        }),
        ..window(vec![numbered(0)], 0)
    };
    let session = snapshot(vec![
        bounded(0, 0, 0, 600),
        bounded(0, 0, 800, 0),
        bounded(0, 0, 10, 10),
        bounded(0, 0, 100_000, 800),
        bounded(i32::MIN, 0, 800, 600),
        bounded(0, 1_000_000, 800, 600),
        bounded(-1440, -200, 1280, 800),
        bounded(0, 25, 100, 100),
    ])
    .sanitized();

    let bounds: Vec<Option<WindowBounds>> =
        session.windows.iter().map(|window| window.bounds).collect();
    assert_eq!(
        bounds,
        vec![
            None,
            None,
            None,
            None,
            None,
            None,
            Some(WindowBounds {
                x: -1440,
                y: -200,
                width: 1280,
                height: 800
            }),
            Some(WindowBounds {
                x: 0,
                y: 25,
                width: 100,
                height: 100
            }),
        ]
    );
}

#[test]
fn sanitizing_twice_changes_nothing_more() {
    let mut tabs: Vec<TabSnapshot> = (0..MAX_RESTORED_TABS + 20).map(numbered).collect();
    tabs[7].url = "data:,x".to_owned();
    tabs[8].zoom_percent = 1;
    let once = snapshot(vec![window(tabs, 7)]).sanitized();
    assert_eq!(once.clone().sanitized(), once);
}

#[test]
fn closed_tabs_pop_newest_first_with_their_old_place() {
    let mut session = SessionSnapshot::default();
    assert_eq!(session.pop_closed(), None);

    session.push_closed(closed(1));
    session.push_closed(closed(2));
    session.push_closed(closed(3));

    let reopened = session.pop_closed().expect("a closed tab");
    assert_eq!(reopened.tab.url, "https://site3.example/");
    assert_eq!((reopened.window, reopened.index), (0, 3));
    assert_eq!(session.pop_closed(), Some(closed(2)));
    assert_eq!(session.pop_closed(), Some(closed(1)));
    assert_eq!(session.pop_closed(), None);
}

#[test]
fn the_closed_tab_list_is_bounded() {
    let mut session = SessionSnapshot::default();
    for n in 0..MAX_CLOSED_TABS + 5 {
        session.push_closed(closed(n));
    }

    assert_eq!(session.closed_tabs.len(), MAX_CLOSED_TABS);
    assert_eq!(session.closed_tabs[0], closed(MAX_CLOSED_TABS + 4));
    assert_eq!(session.closed_tabs.last(), Some(&closed(5)));
}

#[test]
fn credentials_are_removed_and_urls_stored_as_parsed() {
    let mut session = snapshot(vec![window(
        vec![
            tab("https://alice:hunter2@example.org/inbox"),
            tab("https://bob@example.org/"),
            tab(" https://exa\tmple.org/a b\n"),
        ],
        0,
    )]);
    session.closed_tabs = vec![ClosedTab {
        tab: tab("http://carol:pw@example.org:8080/x"),
        ..closed(0)
    }];

    let session = session.sanitized();

    assert_eq!(
        urls(&session.windows[0]),
        vec![
            "https://example.org/inbox",
            "https://example.org/",
            "https://example.org/a%20b",
        ]
    );
    assert_eq!(session.closed_tabs[0].tab.url, "http://example.org:8080/x");
    assert_eq!(session.clone().sanitized(), session);
}
