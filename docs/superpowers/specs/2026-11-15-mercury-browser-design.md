# Mercury Browser — design

Date: 2026-11-15
Status: approved scope (owner), design under review
Branch: `mercury` (merged to `master` when complete)

## 1. Purpose

Mercury Browser is a fork of [Hermes Browser](https://github.com/KaykCaputo/hermes-browser)
(MPL-2.0, © Kayk Caputo): a desktop browser in Rust built on the Servo engine with a Slint
interface. Upstream supports Linux only.

This work delivers, in the GitHub repository (not just on one machine):

1. The rename to Mercury Browser.
2. A real macOS port for Apple Silicon (`aarch64-apple-darwin`): GPU frame hand-off, audio and
   video, a signed `.app`, Mac conventions, CI and release artifacts.
3. Current dependencies (Servo 0.8.0, Slint 1.18.1, adblock 0.13.5).
4. Fixes for known defects.
5. New functionality, including complete bookmarks, history and downloads.

Owner's standing instruction: reuse proven open-source code wherever the licence allows; write
new code only where nothing suitable exists.

## 2. Baseline (measured on macOS 27.2 / Apple M2, upstream `a41f18e`, unmodified)

- `cargo build --release -p hermes-desktop` succeeds in about 6 minutes.
- The app starts; tabs, the new-tab page and Servo's separate content process work.
- `example.com` and a Wikipedia article render correctly at Retina resolution.
- Rendering uses hardware OpenGL, then copies every frame through the CPU
  (`glReadPixels` → flip → upload). This is the main performance problem.
- No audio or video (Servo's dummy media backend).
- Defects observed:
  - A navigation issued before Servo has created the browsing context is dropped
    (`LoadUrl for unknown browsing context`). There is no way to open a URL at launch.
  - Killing the app leaves the content process running.
  - Some icons on Wikipedia draw as black squares (cause not yet known).
  - `ash` (Vulkan) is compiled on macOS but unused.

Findings from source review (see §9 for the research reports):

- `Opts::config_dir` is never set, so cookies and site storage are lost on every exit.
- `Opts::hard_fail` defaults to `true`, so a reported panic in one page exits the whole browser.
- Pointer positions are sent as page (CSS) points, which become wrong as soon as zoom is added.
- `window.open` / `target=_blank`, JavaScript dialogs, `<select>` menus, context menus, file
  pickers, cursor changes and link-hover status text are not implemented, so they do nothing.

## 3. Decisions

| Topic | Decision |
|---|---|
| Product name | Mercury Browser |
| Repository | `thepigdestroyer/mercury-browser` (fork; `upstream` remote kept) |
| Binary | `mercury-browser` |
| Crates | `apps/mercury-desktop`, `crates/mercury-ui`; `crates/talaria-core` and `crates/talaria-servo` keep their names ("talaria" is the Latin word for Mercury's winged sandals) |
| New crates | `crates/mercury-store`, `crates/mercury-downloads` |
| macOS bundle | `Mercury Browser.app`, identifier `io.github.thepigdestroyer.mercury-browser` |
| Version | `0.2.0` (first Mercury release) |
| Licence | MPL-2.0, unchanged. Upstream copyright and attribution are kept. |
| Servo | `=0.8.0`, features `vello`, `media-gstreamer` (Linux and macOS), `webgl`, `subtlecrypto`, `brotli-compression-stream` |
| Slint | `~1.18.1` with `unstable-wgpu-30` (upgraded together with Servo so the build has one wgpu) |
| Shared-type pins | `surfman 0.14`, `gleam 0.15`, `glow 0.17`, `euclid 0.22`, `dpi 0.1`, `url 2.5`, `rusqlite 0.38` stay where Servo 0.8.0 needs them |
| `dom_script_asynch` | Follow Servo's default (`false`); upstream Hermes set `true`, which was a no-op in 0.7 and is experimental in 0.8 |
| Content sandbox | Stays off. Servo 0.8.0 compiles the sandbox out on `aarch64`; enabling it panics the content process |
| Profile directory | macOS `~/Library/Application Support/Mercury Browser/Default/`; Linux `${XDG_DATA_HOME:-~/.local/share}/mercury-browser/Default/` |
| UI for bookmarks, history, downloads | Native Slint panels, not internal web pages |
| Storage access | Synchronous SQLite calls on the UI thread (WAL mode). Queries are small; revisit if profiling shows jank |

## 4. Architecture

```text
apps/mercury-desktop     binary: event loop, shortcuts, profile, glue between everything below
crates/mercury-ui        Slint components and models (no logic beyond presentation)
crates/talaria-core      engine-independent session model: tabs, commands, events,
                         address resolution, history-recording policy, download detection.
                         No I/O. Depends only on `url`.
crates/talaria-servo     Servo adapter: runtime, web views, delegates, request stores,
                         content blocking, media environment
crates/mercury-store     SQLite profile store: history, bookmarks, downloads records
crates/mercury-downloads HTTP transfer engine (reqwest + tokio), no database access
third_party/{surfman,gaol}  vendored patches (unchanged)
```

Rules that keep the pieces testable:

- `talaria-core` never touches the network, disk, clock or Servo types. Time and identifiers
  are passed in.
- `talaria-servo` is the only crate that names Servo types. Objects Servo expects an answer to
  (dialogs, menus, permission and authentication requests, select controls) are kept in a
  per-tab request store inside `talaria-servo`; only an id and plain display data cross into
  `talaria-core` and the UI.
- `mercury-store` and `mercury-downloads` do not depend on each other. The desktop app records
  download state changes in the store.

### 4.1 Event and command flow

Unchanged pattern: Servo delegate → `TabEvent` over a channel → `BrowserController` → UI sync;
UI callback → `BrowserCommand` queue → `ServoWebView`.

Additions:

- `BrowserController` gains an **effects outbox** (`BrowserEffect`), drained by the desktop app
  each frame: `RecordVisit`, `UpdateVisitTitle`, `StartDownload`. This is how pure policy in
  `talaria-core` causes I/O without performing it.
- Tabs are created **with their initial URL** (`WebViewBuilder::url`). No navigation is issued
  to a web view that Servo has not finished creating.
- Page-initiated tabs (`request_create_new`) are built synchronously inside the delegate
  callback, because Servo's constellation blocks on the answer. The delegate obtains a `TabId`
  from a callback supplied by the app, stores the new web view in a hand-off queue, and emits
  `TabEvent::Opened`.

### 4.2 Rendering

`apps/mercury-desktop/src/rendering/`:

| File | Platforms | Content |
|---|---|---|
| `mod.rs` | all | cfg routing, `BrowserRenderingContext` re-export |
| `surfman.rs` | Linux, macOS | shared surfman context, `impl servo::RenderingContext`, error type |
| `vulkan.rs` | Linux | existing Vulkan external-memory import (ported to wgpu 30) |
| `metal.rs` | macOS | IOSurface → `MTLTexture` → wgpu import and the flip/convert pass, adapted from Slint's Servo example (MIT) |
| `software.rs` | others | existing `SoftwareRenderingContext` wrapper |

macOS frame path: Servo paints with OpenGL 4.1 into an IOSurface; `glFinish`; the surface is
unbound; Metal wraps the IOSurface; one cached wgpu pass flips Y and converts BGRA to RGBA into
the texture Slint draws; the surface is re-bound on every exit path. No pixel crosses the CPU.
If the import fails once, the context latches to a `glReadPixels` fallback on the same GL
context (what upstream does today) and logs the reason. `MERCURY_FORCE_FRAME_READBACK=1` forces
the fallback for testing.

Resize and present errors are no longer discarded: a failed swap-chain resize does not commit
the new size, and failures are logged once per kind (upstream issue #11).

### 4.3 Media

*(Completed from the audio/video research report; see §5.C2.)*

Servo's only production media backend on desktop is GStreamer. Mercury enables Servo's
`media-gstreamer` feature on macOS as well as Linux. For development the system GStreamer is
used. For distribution the GStreamer libraries and plugins are copied into the `.app` and their
install names rewritten, following Servo's own packaging scripts (MPL-2.0), so the app plays
media on a Mac with no GStreamer installed.

### 4.4 Profile store

One SQLite file, `places.sqlite` (WAL, foreign keys on, `PRAGMA user_version` migrations),
using the `rusqlite 0.38` already in the build through Servo. Schema: `places` (one row per
URL, with visit counts, last visit and frecency), `visits` (one row per visit, with Firefox's
transition numbering), `bookmarks` (a tree by `parent_id` + `position`, with fixed roots
"Bookmarks Bar" and "Other Bookmarks"), `downloads`, and an FTS5 index over URL and title.
Times are milliseconds since the Unix epoch. The full DDL is in research report R2 §7.3.

Servo's own persistent state (cookies, HSTS, auth cache, site storage) goes in
`<profile>/servo/` via `Opts::config_dir`.

### 4.5 Downloads

Servo 0.8.0 has no download support of any kind (upstream issue servo/servo#40210 is open).
Mercury therefore performs downloads itself with `reqwest`, which reuses the hyper and rustls
already in the build, sending Servo's User-Agent, the referrer and the cookies Servo holds for
that URL (`SiteDataManager::cookies_for_url`).

A download starts when:

1. The user chooses "Download Linked File" or "Save Image As…" in the context menu, or
   Option-clicks a link.
2. A page-initiated navigation, a typed address or a bookmark points at a URL whose path ends
   in a known download extension (archive, installer, document, media container). The
   navigation is denied and the file is downloaded instead.
3. The user presses Cmd/Ctrl+S ("Save Page As…"), which saves the current URL.
4. *(after a feasibility spike)* a committed page turns out to be a type Servo cannot render
   (`document.contentType`), in which case Mercury goes back and offers the download.

Files are written to `<name>.part` in the destination folder and renamed on completion; names
come from `Content-Disposition`, else the final URL, else the MIME type, and are sanitised and
made unique. Transfers can be cancelled, retried, and resumed with `Range`/`If-Range` when the
server supplied a validator. State is persisted; transfers in progress at exit are marked
interrupted at the next start. On macOS completed files get the quarantine attribute so
Gatekeeper checks them.

## 5. Requirements by workstream

Each item is a requirement the finished repository must meet. "Verify" lines say how.

### A. Foundation

- **A1 Rename.** No user-visible "Hermes" remains in the UI, binary, package, bundle, Makefile,
  CI, README or docs, except attribution to the upstream project and the history. Vendored
  patch notes are renamed `MERCURY.md`. *Verify:* build, tests, and a case-insensitive search.
- **A2 Dependencies.** Versions as in §3; `Cargo.lock` refreshed; the two Servo 0.8 source
  breaks fixed (`ServoUrl` in the content blocker; `LoadStatus::Stopped`); Slint moved to the
  wgpu 30 API on every platform; bundled ad-block lists refreshed from the Brave mirror with
  updated checksums. *Verify:* build and tests on macOS; Linux through CI.
- **A3 Platform hygiene.** `ash` and the GL extension bindings are Linux-only; the Makefile's
  packaging helpers work on macOS and Linux; `MACOSX_DEPLOYMENT_TARGET` is set.

### B. Engine integration and core behaviour

- **B1 Launch and test hooks.** `mercury-browser [URL…]` opens each URL in a tab.
  `--screenshot <file.png>` loads, waits for load completion plus a settle delay, writes a PNG
  and exits, without stealing keyboard focus; `--window-size WxH` sets the size. These hooks
  are what automated checks and CI smoke tests use.
- **B2 Reliability.** Profile directory created on first run; `config_dir` set (cookies
  persist); `hard_fail: false` with a logging panic hook and crash signal handler adopted from
  servoshell; content processes exit when the browser exits, including abnormal exit.
- **B3 Upstream defects.** #4 stale tab title on navigation; #8 address-bar edits overwritten
  by background updates; #10 previous tab's frame shown after a tab switch; #11 ignored
  resize/present errors; #5 inert menu button (made functional in B8). Closing the last tab
  closes the window.
- **B4 Input.** Key mapping adopted from servoshell's `keyutils.rs`; pointer positions sent as
  device points; platform shortcut conventions (Cmd on macOS, Ctrl elsewhere): new/close tab,
  tab 1–9, next/previous tab, back/forward (Cmd+[ / Cmd+] and Cmd+←/→ on macOS, Alt+←/→
  elsewhere), reload, focus address bar, zoom in/out/reset, pinch zoom.
- **B5 Page feedback.** Cursor shape follows the page; link-hover status text; the page is told
  the system light/dark theme; fullscreen requests enter and leave window fullscreen and hide
  the browser chrome; Escape leaves fullscreen.
- **B6 New tabs from pages.** `window.open`, `target=_blank` and "Open Link in New Tab" open a
  tab; `window.close()` closes it where Servo allows.
- **B7 Embedder controls.** JavaScript `alert`/`confirm`/`prompt`; HTTP authentication prompt;
  context menu showing Servo's items plus Mercury's download items; `<select>` popup; file
  picker via `rfd`; colour picker; permission prompt (allow/deny, not remembered). Dialogs show
  the requesting origin and are visually distinct from browser chrome.
- **B8 Menu.** The "⋮" button opens a menu: New Tab, zoom controls, Bookmarks, History,
  Downloads, Bookmark This Page, Show Bookmarks Bar, Import/Export Bookmarks, Clear Browsing
  Data, About. The About dialog names Mercury, credits Hermes Browser, Servo and Slint, and
  includes Slint's `AboutSlint` attribution.

### C. macOS

- **C1 GPU frame path** as in §4.2. *Verify:* correct colours and orientation, live resize,
  tab switching, forced-fallback parity, a WebGL page.
- **C2 Audio and video.** `<audio>`, `<video>` and Web Audio play on macOS in a development
  build and in the packaged app on a machine without GStreamer. *Verify:* a local test page
  with an MP4/H.264+AAC and a WebM/VP9+Opus file plays with sound; the packaged app passes the
  same check with the system GStreamer hidden.
- **C3 App bundle.** `make app-macos` produces `dist/Mercury Browser.app` (Info.plist, icon,
  bundled media runtime, hardened-runtime entitlements for the JIT, ad-hoc signature that
  verifies with `codesign --verify --deep --strict`); `make package-macos` produces a `.dmg`
  and checksum. The bare binary embeds the same Info.plist so it behaves like an app when run
  from `cargo`.
- **C4 CI and releases.** A macOS arm64 job builds, tests and smoke-tests on every push; the
  release workflow publishes the macOS `.dmg` next to the Linux archive.
- **C5 Mac conventions** not covered by B4: standard application menu (About, Hide, Quit), and
  the window title/traffic-light layout left native.

### D. Bookmarks, history, downloads

- **D1 Store.** `mercury-store` with schema, migrations, and typed APIs for history,
  bookmarks and downloads; frecency ported from Mozilla application-services.
- **D2 History recording.** Every committed top-level `http`/`https` page is recorded once,
  with its title when known and a transition (typed, link, bookmark, reload). Back/forward
  traversals, iframe navigations and `about:` pages are not recorded.
- **D3 History UI.** Panel grouped by day, newest first; search; delete one entry; "Clear
  Browsing Data" with a time range (history, and optionally cookies and cache through Servo's
  APIs). Shortcut Cmd+Y (macOS) / Ctrl+H.
- **D4 Address-bar suggestions** from history and bookmarks, ranked by frecency, navigable with
  the arrow keys.
- **D5 Bookmarks.** Star toggle and Cmd/Ctrl+D; bookmarks bar (Cmd/Ctrl+Shift+B); panel with
  folders, rename, delete, move and reorder; opening a bookmark is recorded as a bookmark
  visit.
- **D6 Bookmark import and export.** Netscape `bookmarks.html` import (written on the
  `html5ever` tokenizer already in the build) and export (adapted from Zephium). Direct import
  from Chrome, Firefox and Safari profiles (adapted from Zephium's importer crate) follows if
  the port to `rusqlite 0.38` is straightforward.
- **D7 Download engine** as in §4.5.
- **D8 Download triggers** as in §4.5 items 1–3, and item 4 if the spike succeeds.
- **D9 Downloads UI.** Toolbar button with overall progress; panel listing downloads with
  progress, speed and state; actions Open, Show in Finder/Folder, Cancel, Retry, Remove, Copy
  Link. Shortcut Cmd+Shift+J (macOS) / Ctrl+J.

### E. Finish

- **E1** User and developer documentation, README, screenshots, third-party notices and the
  docs site configuration updated for Mercury and for macOS.
- **E2** Whole-branch review, merge to `master`, push.

## 6. What will not work, stated plainly

- **Downloads triggered by submitting a form**, and files a server marks
  `Content-Disposition: attachment` but which Servo can display (for example a CSV), cannot be
  intercepted without patching Servo. The user can still save the latter with Save Page As.
- **Stop-loading, find-in-page and tab muting**: Servo 0.8.0 has no API for them.
- **Date and time pickers**: Servo has no control for them; the fields accept typed input.
- **OS sandbox for web content on Apple Silicon**: unavailable in Servo 0.8.0.
- **Patent-encumbered codecs**: playback depends on which GStreamer plugins are bundled; see
  the media section once research is complete.
- **Linux changes are verified in CI only.** There is no Linux machine in this development
  environment.
- **Notarisation**: the `.app` is ad-hoc signed. Notarising needs the owner's Apple Developer
  account; the packaging script accepts a signing identity so this can be added later.

## 7. Reuse and provenance

Copied or adapted code keeps its original licence and is recorded in `THIRD-PARTY-NOTICES.md`
with source repository, commit or tag, and path.

| Used for | Source | Licence |
|---|---|---|
| macOS frame import and flip pass | slint-ui/slint `examples/servo/.../metal.rs` (tags v1.17.1, v1.18.1) | MIT, © SixtyFPS GmbH |
| Existing Linux frame bridge (attribution added) | slint-ui/slint `examples/servo/.../vulkan.rs` | MIT, © SixtyFPS GmbH |
| Key mapping, panic hook, crash handler, embedded Info.plist, cursor table, shortcut conventions, dialog/menu/select logic | servo/servo `ports/servoshell` at `v0.8.0` | MPL-2.0 |
| GStreamer bundling for the `.app`, signing script | servo/servo `python/servo/{gstreamer,package_commands}.py`, `etc/macos_sign.py` | MPL-2.0 |
| Frecency | mozilla/application-services `components/places/src/frecency.rs` | MPL-2.0 |
| Bookmark export, FTS query sanitiser, browser-profile importers | zephium-browser/Zephium at `9026a1ff` | MPL-2.0 |
| Download write loop, `Content-Disposition` parsing | versotile-org/verso at `eb719bdd` | Apache-2.0 OR MIT |
| Libraries added | `reqwest`, `dirs`, `rfd`, `opener` | MIT / Apache-2.0 |

Not to be copied: retsurf (GPL), and any GPL/LGPL/AGPL crate.

## 8. Verification strategy

- **Logic** (`talaria-core`, `mercury-store`, `mercury-downloads`, pure helpers elsewhere):
  test-first unit tests. The store is tested against in-memory SQLite; the download engine
  against a local HTTP server started by the test.
- **Engine adapter**: unit tests for pure mappings; behaviour checked by running the app.
- **UI and rendering**: `--screenshot` runs against local test pages committed under
  `tests/pages/`, compared by inspection during development and asserted non-blank in CI.
- **Every task** ends with `cargo fmt --check`, `cargo clippy --release --workspace
  --all-targets -- -D warnings` and `cargo test --release --workspace` passing on macOS.
  `--release` is used throughout so one set of Servo build artifacts is shared.
- **Linux**: the existing CI job on the fork, extended as the work proceeds.
- No automated or manual check may synthesise keyboard or mouse input on the developer's
  desktop, and test launches must not take focus.

## 9. Research reports

Kept in `.superpowers/research/` (not committed): R1 macOS media, R2 bookmarks/history/
downloads, R3 Servo 0.8 embedding API and servoshell harvest map, R4 macOS GPU rendering.
Reference checkouts of the cited projects are under the session's scratch directory.

## 10. Order of work

Plans are written per phase, immediately before the phase starts, so each plan describes the
code as it then exists.

1. **Phase 1 — Foundation and engine integration:** A1, A2, A3, B1, B2, B3, B4, B5, B6.
2. **Phase 2 — macOS platform:** C1, C2, C3, C4, C5.
3. **Phase 3 — Browser controls:** B7, B8.
4. **Phase 4 — Profile features:** D1–D9.
5. **Phase 5 — Finish:** E1, E2.

B7 and B8 follow the macOS work because the context menu and the application menu are entry
points for Phase 4 and are best built once, against the final platform behaviour.
