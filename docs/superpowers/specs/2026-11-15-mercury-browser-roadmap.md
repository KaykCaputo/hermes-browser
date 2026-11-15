# Mercury Browser — roadmap to a full-featured browser

Date: 2026-11-15
Status: supersedes §5 (requirements), §6 (limits) and §10 (order of work) of
`2026-11-15-mercury-browser-design.md`. Sections 1–4 and 7–9 of that document still apply,
with the amendments listed in "Changes to the design" below.

## 1. Goal

The owner's goal changed after the first design: Mercury is to be a complete browser — its own
passwords, cookie and site-data management, history, bookmarks, downloads, settings, session
restore, permissions and everything else a user of Chrome or Brave expects — and to do those
things as well as they do, or better.

That goal has two parts with different answers.

- **The browser itself** (everything around the web page). This is Mercury's own code. The
  research found 87 capabilities users expect; 63 can be built on Servo 0.8.0 as it is (23 of
  them only partly), and 11 more by carrying small patches to Servo.
- **The engine** (what makes sites work). This is Servo. Thirteen capabilities are missing from
  the engine itself and cannot be added from Mercury's side: DRM video, adaptive streaming
  (Media Source Extensions — YouTube, Netflix and Twitch rely on it), passkeys, extensions,
  printing, spell-check, picture-in-picture, screen capture, drag-and-drop into pages,
  fingerprinting protection, installable web apps, and an operating-system sandbox and full
  site isolation on Apple Silicon. General site compatibility is also behind Chromium's.

Mercury aims for parity on the first part. On the second it follows Servo, and says so.

## 2. Decisions

| Topic | Decision | Made by |
|---|---|---|
| GStreamer | Ship the latest stable (1.28.9, official framework packages, checksum-pinned), not Servo's 1.22.3 pin | owner |
| Scope | Every module of a modern browser is in scope; nothing is parked as "optional" | owner |
| Engine loop | Servo is driven from the event loop, independent of drawn frames (`pump`/`present`) | lead |
| `file://` | Accepted in the address bar and at launch | lead |
| Web-platform features | Servo's "experimental" feature set (servoshell's list of 18 preferences, by their 0.8.0 names) is on by default, with a setting to turn it off. `dom_webrtc_enabled` stays **off**: in 0.8.0 `getUserMedia` opens the camera and microphone with no permission check | lead (owner may overrule) |
| Password storage | macOS Keychain through the SecItem API (`security-framework`), one item per login, exact-origin matching. Never through the `security` command-line tool. Linux: Secret Service | lead |
| Zervo code | Adopt scripts, configuration and small pure modules. Do **not** adopt its password storage, download manager, JSON history, keyboard mapping, URL normaliser or HTTP client (defects listed in research report R5 §8.4) | lead |
| Servo patches | Recommended: yes, staged — see §4 Phase 7. Awaiting the owner | owner |
| Apple Developer ID | Needed for notarisation, stable Keychain access across updates, and automatic updates. Awaiting the owner | owner |
| External services | Safe-browsing lists, translation and password-breach checks send data to third parties. Awaiting the owner | owner |

## 3. What cannot reach Chrome's level today

Blocked by the engine (no Servo support; not realistic to add from Mercury):
DRM (Widevine/FairPlay); adaptive streaming (no `MediaSource`); passkeys/WebAuthn; extensions;
print and save-as-PDF; spell-check; picture-in-picture; screen capture; drag-and-drop into
pages; fingerprinting protections; installable web apps; process sandbox on Apple Silicon;
site isolation beyond what Servo does.

Only partly achievable: duplicate and reopen-closed-tab restore the address but not the
back/forward list (Servo has no API to inject history); session restore likewise; password
capture relies on a page script and can only ever offer to save for the page's own origin;
password fill does not reach cross-origin frames; the cookie viewer can list and clear but
per-site or third-party cookie *blocking* needs an engine patch; PDF files open in Preview;
developer tools are Servo's remote devtools server, used from Firefox.

## 4. Phases

Each phase ends with a working browser on `master`. Plans are written per phase, just before
it starts, against the code as it then is.

### Phase 1 — Foundation and engine integration (in progress)
Rename; module split; launch arguments, screenshot mode and `--background`; frame-independent
engine loop; Servo 0.8.0, Slint 1.18.1, adblock 0.13.5; refreshed filter lists; profile
directory, persistent cookies, crash containment; session-state fixes; keyboard mapping,
shortcuts, zoom; cursor, status text, colour scheme, fullscreen; tabs opened by pages.
Added from the Zervo research: **web-compatibility defaults** (experimental feature set,
compatibility user agent, `screen_geometry`), `deny.toml` and the workspace lint table.

### Phase 2 — macOS platform
GPU frame path (IOSurface → Metal). Audio and video with GStreamer 1.28.9, bundled in the app.
`Mercury Browser.app`: Info.plist, icon (`make-icns.sh`), entitlements, inside-out signing,
dmg, notarisation flags ready for a Developer ID. Default-browser registration and opening
links from other apps. Native menu bar, Dock menu, Services. Swipe-to-navigate and pinch.
CI on macOS arm64 with bundle checks; release artifacts.

### Phase 3 — Browser controls
JavaScript dialogs and `beforeunload`; HTTP authentication; context menu; `<select>`; file and
colour pickers. Site permissions with a per-origin store and a per-site settings page.
Notifications (in-window; native once the engine patch or workaround is in). Branded error,
certificate and crash pages through Servo's resource reader. Per-site zoom. Stop (script-level).
Settings: search engines and keywords, start-up behaviour, home/new-tab page, download
location, appearance, language. The "⋮" menu and About.

### Phase 4 — Profile data
`mercury-store` (SQLite). History with search; address-bar suggestions and inline
autocomplete ranked by frecency. Bookmarks: star, bar, manager, import/export. Downloads
(embedder-side): context menu, direct file links, Save Page As; consent prompt before writing;
quarantine attribute. Session restore with crash recovery; reopen closed tab; duplicate tab.
Clear browsing data; cookie and site-data viewer. Password manager: Keychain storage, fill,
generation, capture, optional breach check.

### Phase 5 — Privacy and power features
Ad and tracker blocking extended with cosmetic rules and a per-site toggle; HTTPS-only mode;
pop-up policy. Media keys and Now Playing; playing-audio indicator. Tab pinning, groups, drag
reorder, tab search, hover preview. Developer-tools enablement; task manager (memory report).
View source; reader mode. User scripts and styles.

### Phase 6 — Windows and profiles
Multiple windows and tab tear-off. Private windows and separate profiles, each as its own
process (Servo has no per-web-view isolation).

### Phase 7 — Engine patches (if the owner approves)
A vendored overlay of the six affected Servo crates under `[patch.crates-io]`, behind a cargo
feature, re-ported at each Servo upgrade. In order: notifications without an icon (15 lines,
applies cleanly to 0.8.0); real downloads (`Content-Disposition`, form-triggered, blob/data),
after fixing the four defects found in Zervo's version and adding cancel and back-pressure;
then tab mute, stop, find in page, cookie policy, certificate details, and a permission check
for `getUserMedia` (which unlocks WebRTC).

### Phase 8 — Distribution and finish
Automatic updates; crash reporting (local log, opt-in upload); Linux desktop integration
(`.desktop`, AppStream, man page); release provenance attestation; documentation, README,
screenshots, third-party notices.

### Owner policy, unscheduled
Safe-browsing warnings; translation; in-browser PDF viewing (pdf.js, unverified on Servo);
address and payment autofill; sync.

## 5. Changes to the design

- §3 Decisions: GStreamer 1.28.9; experimental web features on; WebRTC off.
- §4.1: the engine is `pump` (frame-independent) plus `present`; a user navigation on a tab
  whose first page has not committed replaces that tab's web view.
- §4.3 Media: official GStreamer 1.28.9 framework; all 38 plugins Servo loads exist in it.
- §4.5 Downloads: embedder-side first; engine patch later (Phase 7). A consent prompt precedes
  any write.
- §7 Reuse table gains: Zervo (github.com/Goddv/Zervo at `86cb7126`, MPL-2.0; attribution by
  link, per its owner) — bundling, signing and dmg script blocks, `make-icns.sh`, Info.plist
  keys, entitlements, CI steps, `deny.toml`, lint table, atomic JSON writer, filename
  sanitiser, swipe recogniser, notification queue model. Its name, logo and screenshots are
  not copied.
- §8 Verification: automated launches use `--screenshot` or `--background`; Servo's WebDriver
  support is to be evaluated as an end-to-end test driver.

## 6. Security notes carried into the plans

- `getUserMedia` has no permission check in Servo 0.8.0: keep `dom_webrtc_enabled` off.
- Servo's "allow this certificate temporarily" override probably fails in multiprocess mode
  (per-process secret; inferred from source, not run). Verify before relying on it.
- Check whether an http(s) page can navigate to a `file://` URL; if so, deny it in
  `request_navigation`.
- Download filenames come from servers: sanitise, cap at 255 bytes, never write outside the
  chosen folder, never open automatically.
- Passwords never appear on a command line or in logs.
