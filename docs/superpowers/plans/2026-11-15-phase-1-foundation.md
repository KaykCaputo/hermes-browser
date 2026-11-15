# Mercury Browser Phase 1 — Foundation and Engine Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the Hermes Browser fork into Mercury Browser on current dependencies, with launch/test hooks, reliable engine settings, corrected session behaviour, proper input handling, page feedback and page-initiated tabs.

**Architecture:** Unchanged layering: `talaria-core` (pure session model) ← `talaria-servo` (Servo adapter) ← `mercury-desktop` (Slint event loop and glue), with `mercury-ui` holding the Slint markup. This phase adds no storage and no new crates.

**Tech Stack:** Rust 2024 (stable 1.100), Servo 0.8.0 (crates.io), Slint 1.18.1 (winit 0.30, FemtoVG on wgpu 30), surfman 0.14 (vendored patch).

Design: `docs/superpowers/specs/2026-11-15-mercury-browser-design.md`.
Research (read the sections a task names; they contain exact signatures):
`.superpowers/research/R3-servo-0.8-api.md`, `.superpowers/research/R4-macos-gpu-rendering.md`.
Reference sources on disk: Servo 0.8.0 crates in
`/private/var/folders/bz/mcd91b7943dgjb_pkmry9wtc0000gn/T/opencode/mercury-research/servo-api/crates-0.8.0/`,
servoshell at tag `v0.8.0` in `…/mercury-research/servo-api/servo-v0.8.0/ports/servoshell/`,
Slint 1.18.1 crates in `…/mercury-research/gpu/crates/`.

## Global Constraints

- Work in `/Users/mj/mercury-browser` on branch `mercury`. Never push; never switch branches; never amend or rewrite commits that existed before your task started.
- Run `source ~/.cargo/env` before any cargo command.
- Always build, lint and test in release mode so one set of Servo artifacts is shared:
  `cargo build --release -p mercury-desktop`,
  `cargo clippy --release --workspace --all-targets -- -D warnings`,
  `cargo test --release --workspace`, and `cargo fmt --all -- --check`.
  Before Task 1 is complete the package is still called `hermes-desktop`.
  All four must pass with no warnings before a task's final commit.
- Product name is exactly `Mercury Browser`; binary `mercury-browser`; crates `mercury-desktop`, `mercury-ui`; `talaria-core` and `talaria-servo` keep their names.
- Licence is MPL-2.0. New Rust files need no header. A file that contains code copied or adapted from another project starts with a comment naming the source repository, tag or commit, path and licence, and the same entry is added to `THIRD-PARTY-NOTICES.md` (create it if absent).
- Only copy code from MPL-2.0, MIT, Apache-2.0, BSD or ISC sources.
- `talaria-core` must not gain dependencies beyond `url` and must not perform I/O or read the clock.
- `talaria-servo` is the only crate allowed to name Servo types, except the rendering and keyboard glue in `mercury-desktop` that already does.
- Dependency pins that must not move: `surfman 0.14`, `gleam 0.15`, `glow 0.17`, `euclid 0.22`, `dpi 0.1`, `url 2.5`, `rustls 0.23`, `content-security-policy 0.9`.
- This machine is macOS arm64. Linux-only code (`#[cfg(target_os = "linux")]`) cannot be compiled here; it is checked later by CI. Keep Linux-only edits minimal and list every one in your report.
- GUI checks: never synthesise keyboard or mouse input and never use AppleScript/`cliclick`. Never launch the app in a way that takes focus from the user: use `--screenshot`, or pass `--background` (for example `.superpowers/research/shot.sh <binary> mercury <out.png> <seconds> --background [URL…]`, which captures the app's own window). Close what you launch and confirm with `pgrep -fl mercury-browser`.
- Do not edit `third_party/`, `crates/talaria-servo/resources/adblock/*.txt` or `docs/screenshots/` unless your task says so.
- Commit messages follow the repository's style: `type(scope): summary` (for example `feat(core): …`, `fix(desktop): …`, `build: …`, `docs: …`, `refactor(desktop): …`).
- Follow test-driven development for logic: write the failing test, watch it fail, implement, watch it pass.

---

### Task 1: Rename the project to Mercury Browser

**Files:**
- Move: `apps/hermes-desktop/` → `apps/mercury-desktop/`; `crates/hermes-ui/` → `crates/mercury-ui/`
- Move: `apps/mercury-desktop/resources/hermes-browser.svg` → `…/mercury-browser.svg`; `docs/assets/hermes-browser.svg` → `docs/assets/mercury-browser.svg`
- Move: `third_party/gaol/HERMES.md` → `third_party/gaol/MERCURY.md`; `third_party/surfman/HERMES.md` → `third_party/surfman/MERCURY.md`
- Modify: `Cargo.toml`, both moved crates' `Cargo.toml`, every `.rs` and `.slint` file that names Hermes, `Makefile`, `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `mkdocs.yml`, `README.md`, every file under `docs/` except `docs/screenshots/` and `docs/superpowers/`
- Create: `THIRD-PARTY-NOTICES.md`

**Interfaces:**
- Produces: package `mercury-desktop` with `[[bin]] name = "mercury-browser"`, `path = "src/main.rs"`; library crate `mercury_ui`; workspace version `0.2.0`; functions `mercury_options()` and `mercury_preferences()` in `crates/talaria-servo/src/runtime.rs` (renamed from `hermes_*`).

**Requirements:**
1. Use `git mv` so history follows the files.
2. Workspace `Cargo.toml`: members, profile override `[profile.dev.package.hermes-desktop]` → `mercury-desktop`, version `0.2.0`. `Makefile`: `APP_PACKAGE ?= mercury-desktop`, `PROJECT_VERSION ?= 0.2.0`, archive names `mercury-browser-…`, the packaged binary copied from `target/release/mercury-browser`.
3. All UI strings, window title, log messages, wgpu labels, identifiers and test fixtures say Mercury.
4. Workflows: artifact, package and directory names use `mercury-browser`; release job builds `--package mercury-desktop` and reads `target/release/mercury-browser`.
5. `mkdocs.yml`: `site_name: Mercury Browser`, `site_url: https://thepigdestroyer.github.io/mercury-browser/`, `repo_name`/`repo_url` for `thepigdestroyer/mercury-browser`, `copyright: Copyright (c) 2026 Kayk Caputo and Mercury Browser contributors`, logo and favicon paths, and add at top level:
   ```yaml
   exclude_docs: |
     superpowers/
   ```
6. `README.md`: rename throughout; directly under the title block add a short "Origins" paragraph: Mercury Browser is a fork of Hermes Browser by Kayk Caputo (link to `https://github.com/KaykCaputo/hermes-browser`), continued under the same MPL-2.0 licence. Point documentation links at the new `site_url`. Leave feature descriptions otherwise unchanged (they are rewritten in a later phase).
7. `docs/`: rename in prose, CSS class names and links. Where a sentence refers to something only upstream did (release history), keep "Hermes" and make the attribution explicit.
8. `third_party/*/MERCURY.md`: keep the technical explanation; say the patch was introduced by Hermes Browser and is carried by Mercury.
9. `THIRD-PARTY-NOTICES.md`: a heading, one paragraph explaining its purpose, and a first entry: "Hermes Browser — https://github.com/KaykCaputo/hermes-browser — MPL-2.0 — Mercury Browser is derived from this project at commit `a41f18e`."
10. Do not touch `LICENSE`, the adblock `.txt` lists, `docs/screenshots/`, `docs/superpowers/` or `Cargo.lock` by hand (cargo rewrites the two package names).

- [ ] **Step 1:** Record the baseline: `git rev-parse HEAD`; `cargo test --release --workspace 2>&1 | tail -5` (expect all passing).
- [ ] **Step 2:** Perform the moves with `git mv`, then the manifest edits, then source/UI edits, then build files, then docs.
- [ ] **Step 3:** `cargo build --release -p mercury-desktop` — expect `Finished`; `ls target/release/mercury-browser` exists.
- [ ] **Step 4:** Run the four quality commands from Global Constraints; all pass.
- [ ] **Step 5:** Run
  `grep -rIni hermes --exclude-dir=target --exclude-dir=.git --exclude-dir=.superpowers --exclude-dir=superpowers --exclude-dir=screenshots --exclude=Cargo.lock --exclude='*.txt' . `
  Every remaining line must be attribution, an upstream URL, or upstream history. Paste the remaining lines into your report.
- [ ] **Step 6:** Launch check: `.superpowers/research/shot.sh ./target/release/mercury-browser mercury-browser /tmp/mercury-task1.png 8` — expect a window titled `New Tab — Mercury Browser`; view the PNG and confirm the new-tab page says "Mercury Browser".
- [ ] **Step 7:** Commit: `refactor: rename Hermes Browser to Mercury Browser` (one commit; add a second `docs:` commit if you prefer to separate documentation).

---

### Task 2: Split the desktop crate into modules (no behaviour change)

**Files:**
- Modify: `apps/mercury-desktop/src/main.rs`
- Create: `apps/mercury-desktop/src/engine.rs`, `ui_sync.rs`, `callbacks.rs`, `keyboard.rs`, `shortcuts.rs`

**Interfaces:**
- Produces (all `pub(crate)`, signatures unchanged from today's private functions):
  - `engine.rs`: `MAX_COMMANDS_PER_FRAME`, `connect_servo_runtime`, `report_web_view_error`, `apply_pending_browser_events`, `dispatch_pending_commands`
  - `ui_sync.rs`: `sync_ui`, `slint_image_from_page_icon`, `pointer_button_from_ui`
  - `callbacks.rs`: `connect_ui_callbacks`
  - `keyboard.rs`: `connect_native_keyboard_input`, `keyboard_input_from_winit`, `physical_key_string`, `logical_key_string`, `queue_ime_input`, `dismiss_composition`
  - `shortcuts.rs`: `handle_browser_shortcut`
  - `main.rs` keeps `main`, `use_automatic_gpu_selection`, `select_hardware_accelerated_backend` and the `mod` declarations.

**Requirements:** Move code verbatim. Each existing test moves into a `#[cfg(test)] mod tests` in the file that now holds the function it tests (the `CountingWebView` fake goes with the dispatch tests in `engine.rs`). No logic, naming or formatting changes beyond imports and visibility.

- [ ] **Step 1:** Note the test count: `cargo test --release -p mercury-desktop 2>&1 | grep "test result"`.
- [ ] **Step 2:** Move the code.
- [ ] **Step 3:** Quality commands pass; the test count for `mercury-desktop` is unchanged.
- [ ] **Step 4:** `git diff --stat` shows only files under `apps/mercury-desktop/src/`.
- [ ] **Step 5:** Commit: `refactor(desktop): split main.rs into focused modules`.

---

### Task 3: Launch arguments, initial-URL tabs and the screenshot hook

**Files:**
- Modify: `crates/talaria-core/src/{controller.rs,tab.rs,browser.rs,lib.rs}`; tests in `crates/talaria-core/tests/browser_controller.rs`
- Create: `apps/mercury-desktop/src/cli.rs`, `apps/mercury-desktop/src/screenshot.rs`, `tests/pages/colors.html`
- Modify: `apps/mercury-desktop/src/{main.rs,engine.rs}`, `apps/mercury-desktop/Cargo.toml`

**Interfaces:**
- Produces in `talaria-core`:
  ```rust
  pub enum TabActivation { Foreground, Background }
  impl BrowserController {
      /// Opens a tab. With `Some(address)` the address is resolved like `queue_navigation`
      /// and becomes the tab's initial URL; the tab starts in `LoadState::Loading`.
      pub fn open_tab_with(&mut self, initial_address: Option<&str>, activation: TabActivation)
          -> Result<TabId, AddressResolutionError>;
      /// Sets the initial URL of a tab whose web view has not been created yet.
      /// Returns Ok(false) if the tab does not exist or its initial URL was already taken.
      pub fn set_initial_address(&mut self, tab_id: TabId, address: &str)
          -> Result<bool, AddressResolutionError>;
      /// Returns the tab's initial URL exactly once (None afterwards, and None for a blank tab).
      pub fn take_initial_url(&mut self, tab_id: TabId) -> Option<String>;
  }
  ```
  `open_tab()` keeps its signature and behaves as `open_tab_with(None, Foreground)`.
- Produces in `talaria-core` (command gating): a tab is **engine-ready** once the controller has handled its first `AddressChanged` or `HistoryChanged` event. `next_command()` returns the oldest queued command whose tab is engine-ready, leaving commands for other tabs queued in order; `has_pending_commands()` reports only dispatchable commands. `FocusChanged`, pointer and scroll commands for a not-yet-ready tab are discarded instead of held.
- Produces in `mercury-desktop/src/cli.rs`:
  ```rust
  pub(crate) struct LaunchOptions {
      pub urls: Vec<String>,
      pub screenshot: Option<ScreenshotOptions>,
      pub window_size: Option<(u32, u32)>,
  }
  pub(crate) struct ScreenshotOptions { pub path: PathBuf, pub delay: Duration }
  pub(crate) enum CliOutcome { Run(LaunchOptions), PrintAndExit { text: String, code: i32 } }
  pub(crate) fn parse_arguments(arguments: impl IntoIterator<Item = String>) -> CliOutcome;
  ```
- Consumes: `talaria_servo::run_content_process_if_requested` (runs first, unchanged).

**Requirements:**
1. Command line: `mercury-browser [OPTIONS] [URL…]`. Options: `--screenshot <file.png>`, `--screenshot-delay <milliseconds>` (default 1000), `--window-size <W>x<H>`, `--help`, `--version`. Arguments beginning `-psn_` are ignored (macOS launch services). An unknown option or a missing/invalid value yields `PrintAndExit` with code 2 and a one-line message plus the usage text. `--help`/`--version` yield code 0. Hand-written parser; no new dependency for parsing.
2. Start-up tabs: the first URL becomes the initial URL of the first tab (`set_initial_address`); each further URL opens a background tab. An address that fails to resolve is reported on stderr and skipped.
3. `engine.rs` creates each web view with `take_initial_url(tab_id)` or `NEW_TAB_URL`. No `Navigate` command is issued for start-up URLs.
4. Screenshot mode: once the active tab is `LoadState::Loaded`, wait `delay`, then capture the whole window with `slint::Window::take_snapshot()`, encode PNG (add `image = { version = "0.25", default-features = false, features = ["png"] }` to the workspace dependencies; that version is already in `Cargo.lock`), write the file, and quit the event loop with exit code 0. If the page has not loaded after 30 seconds, print an error and exit with code 3. If `take_snapshot()` is not supported by the wgpu FemtoVG renderer, stop and report BLOCKED with the exact error rather than inventing another mechanism.
5. Screenshot mode must not take keyboard focus or come to the front. Use Slint's winit hooks (`BackendSelector::with_winit_window_attributes_hook`, `with_winit_event_loop_builder` under `unstable-winit-030`; verify names in the Slint 1.17.1 sources in `~/.cargo/registry/src/index.crates.io-*/slint-1.17.1/` and `i-slint-backend-winit-1.17.1/`) to create the window inactive and, on macOS, to keep the app from activating.
6. `--window-size` sets the window's logical size; without it the size is unchanged from today.
7. `tests/pages/colors.html`: a self-contained page (no network) with an asymmetric layout: a red (#ff0000) square top-left, a blue (#0000ff) square top-right, a green (#00ff00) bar along the bottom, and the text "Mercury test page" — used to detect swapped channels or a flipped image.

**Tests (write first):**
- `talaria-core/tests/browser_controller.rs`:
  - `open_tab_with_address_sets_initial_url_and_loading_state` — URL `https://example.com/` stored, `LoadState::Loading`, `take_initial_url` returns it once then `None`.
  - `open_tab_with_search_terms_uses_the_search_url` — initial URL starts with `https://duckduckgo.com/?q=`.
  - `background_tab_does_not_change_the_active_tab`.
  - `open_tab_with_invalid_address_creates_no_tab` — `"ftp://x"` → `Err`, tab count unchanged.
  - `set_initial_address_is_rejected_after_the_url_was_taken` → `Ok(false)`.
  - `navigation_for_a_tab_that_is_not_engine_ready_is_held` — queue `Navigate` on a new tab: `next_command()` is `None`, `has_pending_commands()` is `false`; after `handle_event(AddressChanged)` the command is returned.
  - `held_commands_do_not_block_other_tabs` — two tabs, only the second ready: its command is returned first; the first tab's command is still returned after it becomes ready, in original order.
  - `pointer_commands_for_a_not_ready_tab_are_discarded`.
  Existing tests that queue commands on fresh controllers must be updated to mark the tab ready first (send an `AddressChanged` event); do not weaken their assertions.
- `mercury-desktop/src/cli.rs` unit tests: no arguments → `Run` with empty `urls`; two URLs kept in order; `--screenshot out.png --screenshot-delay 250 --window-size 800x600 https://a.test` parsed exactly; `--window-size 800` → code 2; `--screenshot` without a value → code 2; `--bogus` → code 2; `--version` → code 0 and text containing `Mercury Browser 0.2.0` (use `env!("CARGO_PKG_VERSION")`); `-psn_0_123` ignored.

- [ ] **Step 1:** Write the `talaria-core` tests; run `cargo test --release -p talaria-core` — expect compile failure on the missing API.
- [ ] **Step 2:** Implement the controller changes; tests pass.
- [ ] **Step 3:** Write the `cli.rs` tests; watch them fail; implement `parse_arguments`; tests pass.
- [ ] **Step 4:** Wire `main.rs`/`engine.rs` and add `screenshot.rs`.
- [ ] **Step 5:** Verify end to end:
  `./target/release/mercury-browser --screenshot /tmp/mercury-colors.png --window-size 900x700 "file://$PWD/tests/pages/colors.html"; echo "exit=$?"`
  Expect `exit=0`, a PNG whose page area shows red top-left, blue top-right, green at the bottom. View the image. Also run it with `https://example.com` and confirm the page text is visible. Confirm with `pgrep -fl mercury-browser` that no process is left behind.
- [ ] **Step 6:** Quality commands pass. Commit: `feat(desktop): open URLs at launch and add a screenshot mode` (core changes may be a separate `feat(core): …` commit).

---

### Task 4: Upgrade Servo, Slint and adblock; make Vulkan dependencies Linux-only

**Required reading:** R3 §1 (all of it; §1.11 is the checklist) and R4 §2.1, §2.2, §4.5, §4.10.

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `.cargo/config.toml`, `apps/mercury-desktop/Cargo.toml`, `apps/mercury-desktop/build.rs`, `apps/mercury-desktop/src/{main.rs,engine.rs,rendering.rs,screenshot.rs}`, `crates/talaria-servo/Cargo.toml`, `crates/talaria-servo/src/{adblocker.rs,runtime.rs}`, `third_party/gaol/{sandbox.rs,platform/macos/mod.rs,platform/unix/process.rs,MERCURY.md}`

**Interfaces:**
- Produces: workspace dependencies `servo = { version = "=0.8.0", features = ["vello", "webgl", "subtlecrypto", "brotli-compression-stream"] }`, `slint = { version = "~1.18.1", default-features = false, features = ["backend-winit", "compat-1-2", "renderer-femtovg-wgpu", "std", "unstable-wgpu-30", "unstable-winit-030"] }`, `slint-build = "~1.18.1"`, `adblock = "=0.13.5"`.
- Produces: `load_state_from_servo(LoadStatus::Stopped) == LoadState::Loaded`.

**Requirements:**
1. Manifest changes above. `ash` moves to `[target.'cfg(target_os = "linux")'.dependencies]` in `apps/mercury-desktop/Cargo.toml`. `build.rs` generates the GL bindings only when `CARGO_CFG_TARGET_OS` is `linux` (R4 §4.5 shows the form).
2. `.cargo/config.toml` `[env]`: `SERVO_STYLE_THREAD_STACK_SIZE_KB = "8192"` (Servo 0.8's own value) and `MACOSX_DEPLOYMENT_TARGET = "13.0"`.
3. Lockfile, in two commits so a problem can be bisected: first `cargo update -p servo --precise 0.8.0` and whatever Cargo requires for the new Slint and adblock versions; build and test. Then a full `cargo update`; build, test and smoke-test again. If the full update breaks the build or the smoke test and the cause is not a one-line fix, drop that second commit and say so in the report.
4. Source fixes: `ServoUrl` conversions in `adblocker.rs` (R3 §1.3a); `LoadStatus::Stopped` arms in `runtime.rs` (R3 §1.3b) with a unit test `stopped_is_a_loaded_state`; remove the explicit `dom_script_asynch: true` and rename/adjust the test that asserted it so it asserts only what Mercury sets (`dom_canvas_backend == "vello"`) and that `dom_script_asynch` equals Servo's default; update comments that name Servo 0.7.
5. Slint API moves (R4 §2.1 table): `slint::wgpu_29` → `slint::wgpu_30`, `require_wgpu_29` → `require_wgpu_30`, `GraphicsAPI::WGPU29` → `GraphicsAPI::WGPU30`. In the Linux-only frame import, `create_texture_from_hal` takes a third argument `wgpu::TextureUses::UNINITIALIZED` (R4 §4.10). That is the only Linux-only code change allowed in this task.
6. If `webgl`, `subtlecrypto` or `brotli-compression-stream` fails to build on macOS, remove just that feature and report it.
7. There must be exactly one `wgpu` major in the result: `cargo tree --release -p mercury-desktop -i wgpu` lists a single 30.x.
8. Remove the three compiler warnings in the vendored `third_party/gaol` crate with the smallest possible edits (`sandbox.rs:79` `try!(…)` → `…?`; `platform/macos/mod.rs:176` `extern {` → `extern "C" {`; `platform/unix/process.rs:79` drop the unnecessary `unsafe` block, keeping its body), and add a line to `third_party/gaol/MERCURY.md` recording that Mercury made these three warning-only edits. With this and requirement 1 (`ash` Linux-only), the four quality commands must print **no warnings at all** from this task onward.
9. The engine bootstraps graphics by taking one throw-away `Window::take_snapshot()` when no frame has been drawn (see `engine.rs`). This relies on the FemtoVG renderer running the rendering notifier for snapshot renders. It was checked in the Slint 1.18.1 sources; confirm it still works after the upgrade with a screenshot run while the window is fully covered, if the desktop happens to allow it, and say whether you could.

- [ ] **Step 1:** Add the `stopped_is_a_loaded_state` test; it fails to compile until the upgrade (expected).
- [ ] **Step 2:** Apply manifest changes and the targeted lock update; fix compile errors; quality commands pass. Commit: `build: upgrade to Servo 0.8.0, Slint 1.18.1 and adblock 0.13.5`.
- [ ] **Step 3:** Smoke test with the Task 3 hook: `colors.html` (channels and orientation correct), `https://example.com`, and `https://en.wikipedia.org/wiki/Mercury_(mythology)`. View all three PNGs. Then a WebGL check: `--screenshot` of `https://get.webgl.org/` shows the spinning-cube canvas, not the "does not support WebGL" text. Report what you see for each.
- [ ] **Step 4:** Full `cargo update`; repeat quality commands and the smoke test. Commit: `build: refresh Cargo.lock` (or drop it per requirement 3).
- [ ] **Step 5:** Record in the report: `cargo tree -i wgpu` output, binary size, and the clean release build time if you measured it.

---

### Task 5: Refresh the bundled ad-block filter snapshots

**Files:**
- Modify: `crates/talaria-servo/resources/adblock/{ublock-filters,easylist,easyprivacy,brave-firstparty,brave-specific,brave-unbreak}.txt`, `crates/talaria-servo/resources/adblock/README.md`

**Requirements:**
1. Read `resources/adblock/README.md`: the six files are snapshots from the `lists` branch of `https://github.com/brave/adblock-lists-mirror`, each stored there under an MD5-named artifact listed in the README table.
2. Find the current head commit of that branch (`gh api repos/brave/adblock-lists-mirror/branches/lists --jq .commit.sha`). Confirm the artifact names still exist at that commit; if the mirror's layout changed, stop and report NEEDS_CONTEXT with what you found.
3. Download each artifact at that exact commit (`https://raw.githubusercontent.com/brave/adblock-lists-mirror/<sha>/lists/<artifact>` or the path the repository actually uses), replace the local files, and update the README's commit hash and SHA-256 column (`shasum -a 256`).
4. Do not edit the lists' contents.

- [ ] **Step 1:** Download and replace; update the README.
- [ ] **Step 2:** `cargo test --release -p talaria-servo` — the existing tests `bundled_brave_filters_block_ads_and_trackers` and `top_level_navigation_is_left_to_the_browser` must pass against the new lists.
- [ ] **Step 3:** Quality commands pass. Commit: `chore(adblock): refresh bundled filter snapshots`.

---

### Task 6: Profile directory, persistent site data and crash containment

**Required reading:** R3 §3.16 (persistence) and §3.17 (crashes, `hard_fail`, panic hook).

**Files:**
- Create: `apps/mercury-desktop/src/profile.rs`, `apps/mercury-desktop/src/panic_hook.rs`, `apps/mercury-desktop/src/crash_handler.rs`
- Modify: `apps/mercury-desktop/src/{main.rs,cli.rs,engine.rs}`, `apps/mercury-desktop/Cargo.toml`, `Cargo.toml`, `crates/talaria-servo/src/{runtime.rs,content_process.rs,lib.rs}`, `THIRD-PARTY-NOTICES.md`

**Interfaces:**
- Produces in `talaria-servo`:
  ```rust
  #[derive(Clone, Debug, Default)]
  pub struct RuntimeOptions { pub config_dir: Option<PathBuf> }
  impl ServoRuntime { pub fn new(event_loop_waker: EventLoopWaker, options: RuntimeOptions) -> Self; }
  ```
- Produces in `mercury-desktop/src/profile.rs`:
  ```rust
  pub(crate) struct ProfilePaths { pub root: PathBuf, pub servo_config_dir: PathBuf, pub places_database: PathBuf }
  impl ProfilePaths {
      /// `<data_dir>/<app dir>/Default`, or `override_root` when given. Does not touch the disk.
      pub(crate) fn resolve(data_dir: Option<&Path>, override_root: Option<&Path>) -> Option<Self>;
      /// Creates `root` and `servo_config_dir`.
      pub(crate) fn create(&self) -> std::io::Result<()>;
  }
  pub(crate) const APP_DIR_NAME: &str; // "Mercury Browser" on macOS, "mercury-browser" elsewhere
  ```
  `servo_config_dir = root/servo`, `places_database = root/places.sqlite`.
- Produces in `cli.rs`: `LaunchOptions.profile_dir: Option<PathBuf>` from `--profile-dir <path>`.

**Requirements:**
1. Add `dirs = "7.0"` to the workspace and use `dirs::data_dir()` (macOS `~/Library/Application Support`, Linux `$XDG_DATA_HOME` or `~/.local/share`).
2. Normal runs use the resolved profile. Screenshot mode without `--profile-dir` uses a fresh temporary directory that is removed on exit, so automated runs never touch the user's profile.
3. `mercury_options()` takes the config directory: `config_dir` set, `hard_fail: false`, `multiprocess: true`, `sandbox: false`. Update the two existing options tests and add `hard_fail_is_disabled` and `config_dir_is_passed_to_servo`.
4. If the profile cannot be created, log the error and continue with `config_dir: None`.
5. Port servoshell's `panic_hook.rs` and `crash_handler.rs` (and what they need from `backtrace.rs`) from the `v0.8.0` checkout, MPL-2.0, with source headers and a notices entry. Install both at the very start of `main`, before `run_content_process_if_requested()`, so content processes get them too. `signal-hook-registry` and `backtrace` are already in `Cargo.lock`; add them as direct dependencies at the locked versions (Unix-only for `signal-hook-registry`).
6. `THIRD-PARTY-NOTICES.md`: its opening paragraph currently says every file containing copied code names its source at the top. That is not true of the code inherited from Hermes Browser. Reword it so the statement applies to code taken from projects other than the upstream base, which the whole codebase derives from.
7. Orphan prevention: in `talaria-servo/src/content_process.rs`, before `servo::run_content_process(token)`, record `std::os::unix::process::parent_id()` and start a thread that checks once a second and calls `std::process::exit(0)` when the parent id has changed (Unix only; no-op elsewhere). Keep it in its own small function with a doc comment explaining why.

**Tests (write first):**
- `profile.rs`: `resolves_under_the_data_directory` (`/data` → `/data/<APP_DIR_NAME>/Default`, with the two derived paths); `override_root_wins`; `no_data_directory_and_no_override_is_none`; `create_makes_both_directories` (use a temporary directory from `std::env::temp_dir()` with a unique name; clean up).
- `cli.rs`: `--profile-dir /x` parsed; missing value → code 2.
- `runtime.rs`: the options tests in requirement 3.

- [ ] **Step 1:** Write the tests; watch them fail; implement `profile.rs`, CLI option and `RuntimeOptions`.
- [ ] **Step 2:** Port the panic hook and crash handler; wire `main.rs`.
- [ ] **Step 3:** Add the parent watchdog.
- [ ] **Step 4:** Verify persistence: run `--screenshot /tmp/p1.png --profile-dir /tmp/mercury-profile-test https://example.com`, then `ls /tmp/mercury-profile-test/servo` and report what Servo wrote. Then visit a page that sets a persistent cookie (for example `https://httpbin.org/cookies/set/mercury/1`) with the same profile, exit, and check `cookie_jar.json` contains `mercury`. If the site is unreachable, say so and report the directory listing instead.
- [ ] **Step 5:** Verify orphan prevention: start `./target/release/mercury-browser --background --profile-dir /tmp/mercury-profile-test &`, wait 5 s, `kill -9` the main process only, wait 3 s, and show with `pgrep -fl "mercury-browser --content-process"` that no content process remains.
- [ ] **Step 6:** Quality commands pass. Commit: `feat(desktop): add a profile directory and contain engine crashes`.

---

### Task 7: Session-state fixes (upstream issues #4, #8, #10; closing the last tab)

**Files:**
- Modify: `crates/talaria-core/src/{browser.rs,controller.rs}`; tests in `crates/talaria-core/tests/{browser_state.rs,browser_controller.rs}`
- Modify: `crates/mercury-ui/ui/main-window.slint`, `apps/mercury-desktop/src/{ui_sync.rs,engine.rs,callbacks.rs,shortcuts.rs}`, `crates/talaria-servo/src/web_view.rs`

**Interfaces:**
- Produces in `talaria-core`:
  ```rust
  /// Title shown until a document supplies its own: the URL's host, else the URL, else "New Tab".
  pub fn placeholder_title(url: &str) -> String;
  pub enum CloseTabOutcome { Closed, NotFound, LastTab }
  impl BrowserController {
      pub fn close_tab(&mut self, tab_id: TabId) -> CloseTabOutcome;      // was -> bool
      pub fn close_tab_at(&mut self, index: usize) -> CloseTabOutcome;    // was -> bool
  }
  ```
- Produces in `ui_sync.rs`:
  ```rust
  /// What the address field should show. `None` means leave the user's text alone.
  pub(crate) fn address_bar_update(committed: &str, field_has_focus: bool, tab_changed: bool) -> Option<String>;
  ```
- Produces in `talaria-servo`: `ServoWebView::paint(&self)` (unconditional paint) alongside `paint_if_available`.

**Requirements:**
1. **#4 stale title.** On `AddressChanged` to a URL that differs from the current one in anything other than the fragment, the title becomes `placeholder_title(new_url)` and the icon is cleared. A fragment-only change or an identical URL keeps both. A later `PageTitleChanged` replaces the placeholder; an empty or missing title falls back to `placeholder_title(current_url)` rather than "New Tab" (which remains the title only for the blank tab).
2. **#8 address draft.** The Slint window exposes `out property <bool> address-has-focus` and a `public function reset-address(string)`. `sync_ui` writes the address only when `address_bar_update` returns `Some`: the field does not have focus, or the active tab changed. Pressing Escape in the field restores the committed address and moves focus to the web view. Accepting (Enter) navigates as today. `sync_ui` must receive whether the active tab changed since the previous sync (track the last synced `TabId`).
3. **#10 stale frame.** When the active tab changes, the frame presented next must belong to the new tab or be the neutral background. After `show()` on the newly active web view, paint it unconditionally in the same frame and import that frame; if no frame can be produced, clear the texture (`Image::default()`). Never leave the previous tab's image on screen.
4. **Last tab.** Closing the only tab (close button, Cmd/Ctrl+W) quits the application cleanly (`slint::quit_event_loop()`); the close button is therefore shown on the last tab too.

**Tests (write first):**
- `browser_state.rs`: `navigating_to_another_document_replaces_the_title_with_its_host` (`https://a.test/x` titled "A" → `AddressChanged("https://b.test/y")` → title `b.test`, icon `None`); `fragment_navigation_keeps_title_and_icon`; `same_url_keeps_title`; `later_title_replaces_the_placeholder`; `empty_title_falls_back_to_the_host`; `blank_tab_is_titled_new_tab`.
- `placeholder_title`: `https://example.com/a` → `example.com`; `about:blank` → `about:blank`; `""` → `New Tab`.
- `browser_controller.rs`: `closing_the_last_tab_reports_last_tab` (tab remains, outcome `LastTab`); update existing `close_tab` tests to the new enum without weakening them.
- `ui_sync.rs`: `address_is_written_when_the_field_is_idle`; `address_is_left_alone_while_the_user_is_typing`; `tab_change_overrides_typing`.

- [ ] **Step 1:** Core tests, fail, implement, pass.
- [ ] **Step 2:** `address_bar_update` tests, fail, implement, pass; update the Slint window and `sync_ui`.
- [ ] **Step 3:** Tab-switch painting in `engine.rs`; last-tab quit in `callbacks.rs`/`shortcuts.rs`.
- [ ] **Step 4:** Verify by screenshot: launch with two URLs of visibly different pages (`tests/pages/colors.html` and `https://example.com`); the screenshot shows the first tab's page and two tabs with titles. Report honestly which of requirements 2–4 you could only verify by reasoning (interactive behaviour cannot be scripted here).
- [ ] **Step 5:** Quality commands pass. Commit: `fix: correct tab titles, address-bar drafts and tab-switch frames` (split by crate if you prefer).

---

### Task 8: Keyboard mapping, device-pixel pointer input, shortcuts and zoom

**Required reading:** R3 §3.0 (coordinates), §3.4 (zoom), §3.14 (shortcuts and `keyutils.rs`).

**Files:**
- Create: `apps/mercury-desktop/src/keyutils.rs` (ported), `crates/talaria-core/src/zoom.rs`
- Modify: `apps/mercury-desktop/src/{keyboard.rs,shortcuts.rs,callbacks.rs,engine.rs,ui_sync.rs}`, `crates/talaria-core/src/{browser.rs,controller.rs,web_view.rs,lib.rs}`, `crates/talaria-servo/src/web_view.rs`, `crates/mercury-ui/ui/main-window.slint`, `THIRD-PARTY-NOTICES.md`; tests in `crates/talaria-core/tests/`

**Interfaces:**
- Produces in `talaria-core`:
  ```rust
  // zoom.rs
  pub const DEFAULT_ZOOM_PERCENT: u16 = 100;
  pub const ZOOM_STEPS: [u16; 16] = [30, 50, 67, 80, 90, 100, 110, 120, 133, 150, 170, 200, 240, 300, 400, 500];
  pub fn zoom_in(current: u16) -> u16;   // next larger step, saturating at 500
  pub fn zoom_out(current: u16) -> u16;  // next smaller step, saturating at 30
  // browser.rs
  BrowserCommand::SetPageZoom { percent: u16 }
  BrowserCommand::PinchZoom { delta: f32, x: f32, y: f32 }
  impl BrowserState { pub fn zoom_percent(&self) -> u16; }
  // controller.rs
  impl BrowserController {
      pub fn zoom_in(&mut self); pub fn zoom_out(&mut self); pub fn reset_zoom(&mut self);
      pub fn select_last_tab(&mut self);
  }
  // web_view.rs (trait WebView)
  fn set_page_zoom(&mut self, percent: u16) -> Result<(), Self::Error>;
  fn pinch_zoom(&mut self, delta: f32, x: f32, y: f32) -> Result<(), Self::Error>;
  ```
- Produces in `mercury-desktop/src/shortcuts.rs`:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub(crate) enum Platform { MacOs, Other }
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub(crate) enum ShortcutAction {
      NewTab, CloseTab, NextTab, PreviousTab, SelectTab(usize), SelectLastTab,
      Back, Forward, Reload, FocusAddressBar, ZoomIn, ZoomOut, ZoomReset, Quit,
  }
  pub(crate) fn shortcut_action(key: &Key, modifiers: ModifiersState, platform: Platform) -> Option<ShortcutAction>;
  ```
  (`Key` and `ModifiersState` are winit's.)

**Requirements:**
1. Port servoshell `desktop/keyutils.rs` at `v0.8.0` (MPL-2.0) with a source header and a notices entry. Use it to derive `key`, `code` and `location` for `KeyboardInput` (as their W3C string names via `Display`), replacing the `format!("{:?}")` approach; keep passing the real `repeat` and `is_composing` values (keyutils hard-codes `false` for both).
2. Pointer positions reach Servo as `WebViewPoint::Device`: `ServoWebView` remembers the scale factor given to `set_scale_factor`/creation and multiplies the logical coordinates. Update the existing `logical_coordinates_become_page_coordinates` test to the new behaviour (`(12.5, 24.0)` at scale 2.0 → device `(25.0, 48.0)`).
3. Shortcuts. "Primary" is Cmd on macOS and Ctrl elsewhere; on macOS, Ctrl combinations are not browser shortcuts except Ctrl+Tab.

   | Action | macOS | Elsewhere |
   |---|---|---|
   | New tab / Close tab | Cmd+T / Cmd+W | Ctrl+T / Ctrl+W |
   | Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab; Cmd+Shift+] / Cmd+Shift+[ | Ctrl+Tab / Ctrl+Shift+Tab; Ctrl+PageDown / Ctrl+PageUp |
   | Tab 1–8 / last tab | Cmd+1…8 / Cmd+9 | Ctrl+1…8 / Ctrl+9 |
   | Back / Forward | Cmd+[ / Cmd+]; Cmd+← / Cmd+→ | Alt+← / Alt+→ |
   | Reload | Cmd+R | Ctrl+R, F5 |
   | Focus address bar | Cmd+L | Ctrl+L, Alt+D |
   | Zoom in / out / reset | Cmd+= or Cmd++ / Cmd+- / Cmd+0 | Ctrl+= or Ctrl++ / Ctrl+- / Ctrl+0 |
   | Quit | Cmd+Q | Ctrl+Q |

   On macOS, Cmd+←/→ is Back/Forward only when the web view does **not** have focus in a text field; since that cannot be known, apply it only when the address bar and the web view are both unfocused, and otherwise let the page have it (Servo uses it for line movement). `handle_browser_shortcut` becomes a thin executor over `shortcut_action`.
4. Zoom is per tab, stored as a percentage in `BrowserState`, changed in `ZOOM_STEPS`, applied with `WebView::set_page_zoom(percent as f32 / 100.0)`, and re-applied when that tab's web view is created. The window title area or toolbar shows the percentage when it is not 100 (a small text label left of the menu button is enough).
5. Pinch: winit `WindowEvent::PinchGesture { delta, .. }` while the pointer is over the web view queues `PinchZoom { delta: delta as f32 + 1.0, x, y }` at the last pointer position (device pixels, via the same conversion) → `adjust_pinch_zoom`.

**Tests (write first):**
- `zoom.rs`: `zoom_in_moves_to_the_next_step` (100→110, 133→150); `zoom_in_saturates` (500→500); `zoom_out_moves_to_the_previous_step` (100→90); `zoom_out_saturates` (30→30); `off_step_values_snap_in_the_requested_direction` (105→110 in, 105→100 out).
- controller: `zoom_in_queues_a_zoom_command_for_the_active_tab` (state 110, command `SetPageZoom { percent: 110 }`); `reset_zoom_returns_to_100`; `zoom_is_tracked_per_tab`; `select_last_tab_activates_the_final_tab`.
- `web_view_dispatch.rs`: dispatch of the two new commands reaches the trait methods.
- `shortcuts.rs`: one test per table row and platform, plus `control_w_is_not_a_shortcut_on_macos`, `command_9_selects_the_last_tab`, `plain_letters_are_not_shortcuts`.
- `keyboard.rs`: update the two existing mapping tests to expect the same W3C strings as before (`"Enter"`, `" "`, `"KeyA"`, `"MetaLeft"`), now produced through keyutils; add `arrow_keys_and_function_keys_map_to_w3c_names`.

- [ ] **Step 1:** Core zoom tests → implement → pass.
- [ ] **Step 2:** Shortcut table tests → implement `shortcut_action` → pass; rewire the executor.
- [ ] **Step 3:** Port keyutils; update keyboard mapping and its tests.
- [ ] **Step 4:** Device points and zoom in `talaria-servo`; pinch in `keyboard.rs`/`engine.rs`.
- [ ] **Step 5:** Verify: screenshot of `colors.html` is unchanged from Task 4 (zoom 100%). There is no scriptable way to press keys; state that the shortcut behaviour is covered by unit tests only.
- [ ] **Step 6:** Quality commands pass. Commit: `feat: platform shortcuts, page zoom and standards-based key mapping`.

---

### Task 9: Page feedback — cursor, status text, colour scheme and fullscreen

**Required reading:** R3 §3.2, §3.3, §3.10, §3.13.

**Files:**
- Create: `crates/talaria-core/src/cursor.rs`
- Modify: `crates/talaria-core/src/{browser.rs,web_view.rs,lib.rs}`, `crates/talaria-servo/src/{runtime.rs,web_view.rs}`, `crates/mercury-ui/ui/main-window.slint`, `apps/mercury-desktop/src/{ui_sync.rs,engine.rs,keyboard.rs,shortcuts.rs}`; tests in `crates/talaria-core/tests/browser_state.rs`

**Interfaces:**
- Produces in `talaria-core`:
  ```rust
  // cursor.rs — one variant per CSS cursor keyword that Servo reports
  pub enum PointerCursor { Default, Pointer, Text, Wait, Progress, Crosshair, Move, NotAllowed, Grab, Grabbing,
      Help, ContextMenu, Cell, VerticalText, Alias, Copy, NoDrop, AllScroll, ColResize, RowResize,
      NResize, EResize, SResize, WResize, NeResize, NwResize, SeResize, SwResize, EwResize, NsResize,
      NeswResize, NwseResize, ZoomIn, ZoomOut, None }
  pub enum ColorScheme { Light, Dark }
  // browser.rs
  BrowserEvent::CursorChanged { cursor: PointerCursor }
  BrowserEvent::StatusTextChanged { text: Option<String> }
  BrowserEvent::FullscreenChanged { fullscreen: bool }
  BrowserCommand::SetColorScheme(ColorScheme)
  BrowserCommand::ExitFullscreen
  impl BrowserState { pub fn cursor(&self) -> PointerCursor; pub fn status_text(&self) -> Option<&str>; pub fn is_fullscreen(&self) -> bool; }
  // web_view.rs (trait WebView)
  fn set_color_scheme(&mut self, scheme: ColorScheme) -> Result<(), Self::Error>;
  fn exit_fullscreen(&mut self) -> Result<(), Self::Error>;
  ```

**Requirements:**
1. `BrowserDelegate` forwards `notify_cursor_changed`, `notify_status_text_changed` and `notify_fullscreen_state_changed`. The Servo `Cursor` → `PointerCursor` mapping is a total `match` in `talaria-servo`.
2. Cursor: the Slint web-view `TouchArea` takes its `mouse-cursor` from a window property set in `sync_ui` from the active tab's cursor. Slint's `MouseCursor` lacks a few shapes; map those to the nearest one (documented in a table comment). `PointerCursor::None` hides the cursor if Slint supports it, else uses the default.
3. Status text: when the active tab has status text, show it in a small rounded label at the bottom-left of the web view, single line, elided, above page content; it must not intercept pointer input.
4. Colour scheme: on start and on winit `ThemeChanged`, queue `SetColorScheme` for **every** tab; a tab created later receives the current scheme at creation.
5. Fullscreen: when the active tab reports fullscreen, hide the tab strip and toolbar and make the winit window borderless-fullscreen on its current monitor; when it reports leaving, restore both. Escape while fullscreen queues `ExitFullscreen` and is not forwarded to the page. Switching to another tab while one is fullscreen leaves fullscreen first.
6. State changes clear correctly: a navigation to another document resets cursor to `Default` and status text to `None`.

**Tests (write first):** `cursor_change_is_stored`; `status_text_is_stored_and_cleared`; `fullscreen_flag_follows_events`; `navigation_resets_cursor_and_status_text`; dispatch tests for the two new commands; in `talaria-servo`, `every_servo_cursor_maps_to_a_pointer_cursor` (spot-check `Pointer`, `Text`, `Grabbing`, `None`).

- [ ] **Step 1:** Core tests → implement → pass.
- [ ] **Step 2:** Delegate forwarding and mapping; Slint properties and status label; `sync_ui`.
- [ ] **Step 3:** Theme and fullscreen wiring through the winit hook.
- [ ] **Step 4:** Verify with a local page `tests/pages/dark-mode.html` (create it): background black with white text "dark" under `@media (prefers-color-scheme: dark)`, white with black text "light" otherwise. Screenshot it and report which variant rendered and what the system appearance is (`defaults read -g AppleInterfaceStyle` prints `Dark` in dark mode and errors in light mode). Cursor, status text and fullscreen cannot be scripted; say so.
- [ ] **Step 5:** Quality commands pass. Commit: `feat: follow page cursor, status text, colour scheme and fullscreen`.

---

### Task 10: Tabs opened and closed by pages

**Required reading:** R3 §3.1 (the request must be answered synchronously inside the callback).

**Files:**
- Modify: `crates/talaria-core/src/{controller.rs,tab.rs,lib.rs}`, `crates/talaria-servo/src/{runtime.rs,web_view.rs,lib.rs}`, `apps/mercury-desktop/src/engine.rs`; tests in `crates/talaria-core/tests/browser_controller.rs`
- Create: `tests/pages/opener.html`, `tests/pages/opened.html`

**Interfaces:**
- Produces in `talaria-core`:
  ```rust
  impl BrowserController {
      /// Reserves a tab for a web view the engine is creating itself. The tab is inserted
      /// directly after `opener`, becomes active, and has no initial URL. Like any other tab it
      /// becomes engine-ready on its first address or history event.
      /// Returns None if `opener` does not exist.
      pub fn adopt_tab(&mut self, opener: TabId) -> Option<TabId>;
  }
  TabEvent variant or BrowserEvent::CloseRequested   // page called window.close()
  ```
- Produces in `talaria-servo`:
  ```rust
  /// Called synchronously when a page asks for a new web view. Returns the id the new tab
  /// should use, or None to refuse.
  pub type NewTabAllocator = Rc<dyn Fn(TabId /* opener */) -> Option<TabId>>;
  impl ServoRuntime {
      pub fn set_new_tab_allocator(&self, allocator: NewTabAllocator);
      /// Web views created by pages since the last call, in creation order.
      pub fn take_adopted_web_views(&self) -> Vec<(TabId, ServoWebView)>;
  }
  ```

**Requirements:**
1. `request_create_new`: call the allocator with the opener's `TabId`; if it returns an id, build the web view inside the callback from `request.builder(parent.rendering_context())` with the opener's scale factor and a fresh per-tab `BrowserDelegate`, hide the opener's web view, and push `(id, ServoWebView)` to the adoption queue. If there is no allocator or it returns `None`, drop the request.
2. `engine.rs`: install an allocator that calls `controller.borrow_mut().adopt_tab(opener)`; each frame, move adopted web views into the `web_views` map **before** the creation loop so no second web view is created for those tabs; apply the current colour scheme and zoom defaults to them.
3. `notify_closed` (page called `window.close()`): close that tab through the controller; if it was the last tab, quit as in Task 7.
4. The opener's tab must keep working after the new tab closes.
5. Test pages: `opener.html` has a link `<a href="opened.html" target="_blank">` and a script that calls `window.open('opened.html')` 300 ms after load; `opened.html` shows the text "opened by a page" on a yellow background.

6. Readiness follows the rules Task 3 established: the adopted tab is not engine-ready until its web view reports, and `take_initial_url` for it returns `None`, so the engine does not create a second web view. If the user navigates it before it is ready, the normal replacement path applies and the tab gets a fresh web view (the link to its opener is lost; that is acceptable). Each web view has its own event channel; give adopted web views one too.

**Tests (write first):** `adopted_tab_is_inserted_after_its_opener_and_activated`; `adopted_tab_has_no_initial_url_and_becomes_ready_on_its_first_event`; `navigating_an_adopted_tab_before_it_is_ready_requests_a_fresh_web_view`; `adopting_from_an_unknown_opener_is_refused`; `close_request_closes_the_tab`.

- [ ] **Step 1:** Core tests → implement → pass.
- [ ] **Step 2:** Delegate, allocator and adoption queue; `engine.rs` wiring.
- [ ] **Step 3:** Verify: `--screenshot /tmp/mercury-open.png --screenshot-delay 2500 "file://$PWD/tests/pages/opener.html"`. Because the script opens a tab after load, the capture must show two tabs with the yellow "opened by a page" page active. If Servo blocks the script-initiated `window.open` as a popup (no user activation), report exactly what happened and what the log says; do not disable any Servo protection without asking.
- [ ] **Step 4:** Quality commands pass. Commit: `feat: open and close tabs requested by pages`.

---

### Task 11: Web-compatibility defaults

**Required reading:** `.superpowers/research/R5-zervo-harvest.md` §7.11 and §8.6; `.superpowers/research/R3-servo-0.8-api.md` §1.4, §2.1 (`screen_geometry`), §4.5 (default user agent). Reference: servoshell at `v0.8.0`, `ports/servoshell/prefs.rs:29-53` (`EXPERIMENTAL_PREFS`), and Zervo at `86cb7126` (`…/mercury-research/zervo/Zervo/src/main.rs:152-156,455-507`, `src/app.rs:510-530`; MPL-2.0).

**Files:**
- Modify: `crates/talaria-servo/src/{runtime.rs,lib.rs}`, `apps/mercury-desktop/src/{cli.rs,main.rs,engine.rs}`, `THIRD-PARTY-NOTICES.md`
- Create: `crates/talaria-servo/src/screen.rs`, `tests/pages/features.html`

**Interfaces:**
- Produces in `talaria-servo`:
  ```rust
  pub struct RuntimeOptions { pub config_dir: Option<PathBuf>, pub experimental_web_features: bool /* default true */ }
  /// Screen and window geometry in device pixels, as the page's `window.screen` and
  /// `outerWidth`/`screenX` should report them.
  #[derive(Clone, Copy, Debug, PartialEq)]
  pub struct ScreenMetrics {
      pub screen_size: (u32, u32),
      pub available_origin: (i32, i32),
      pub available_size: (u32, u32),
      pub window_origin: (i32, i32),
      pub window_size: (u32, u32),
  }
  impl ServoRuntime { pub fn set_screen_metrics(&self, metrics: Option<ScreenMetrics>); }
  ```
- Produces in `cli.rs`: `LaunchOptions.experimental_web_features: bool` (default `true`), cleared by `--disable-experimental-web-features`.

**Requirements:**
1. When `experimental_web_features` is true, `mercury_preferences()` turns on exactly the preferences in servoshell's `EXPERIMENTAL_PREFS` list at `v0.8.0` (18 names; copy the list with a source comment, MPL-2.0, and a notices entry). When false, Servo's defaults are left alone.
2. `dom_webrtc_enabled` is always `false`, with a comment giving the reason: in Servo 0.8.0 `getUserMedia` opens the camera and microphone without asking the embedder for permission. A unit test asserts it stays `false` in both modes.
3. User agent: Servo's default for the platform with the `Servo/<version>` token removed and `Gecko/20100101` in its place, so it reads as the Firefox release Servo already claims (derive it from Servo's own string at run time — do not hard-code a Firefox version). It applies in both modes. Unit-test the transformation on the macOS and Linux default strings.
4. `screen_geometry`: `BrowserDelegate` answers from the metrics last given to `set_screen_metrics` (`None` → Servo's default). The engine updates the metrics in `pump` from the winit window and its current monitor (size, position, scale factor), and only calls the setter when they change. On macOS the available area excludes the menu bar and Dock (`NSScreen.visibleFrame`); if that needs a dependency not already in `Cargo.lock`, use the full screen and say so.
5. `tests/pages/features.html`: a self-contained page that writes one line per check into the document, in large text: `navigator.userAgent`; `screen.width × screen.height`; `'IntersectionObserver' in window`; `'indexedDB' in window`; `typeof RTCPeerConnection`; `CSS.supports('container-type: inline-size')`; whether a WebGL2 context can be created.

**Tests (write first):** `experimental_features_enable_servoshells_list`; `experimental_features_can_be_disabled`; `webrtc_stays_disabled`; `user_agent_drops_the_servo_token`; `screen_metrics_convert_to_servo_geometry`; CLI: `--disable-experimental-web-features` parsed, default enabled.

- [ ] **Step 1:** Tests → implement preferences, user agent and CLI flag → pass.
- [ ] **Step 2:** `ScreenMetrics`, the delegate method and the engine update.
- [ ] **Step 3:** Verify with `--screenshot` of `features.html`, once normally and once with `--disable-experimental-web-features`; report every line the page shows in each run. Then screenshot `https://en.wikipedia.org/wiki/Mercury_(mythology)` and `https://www.google.com/` and report whether they look like desktop layouts.
- [ ] **Step 4:** Quality commands pass with no warnings. Commit: `feat(servo): enable web-compatibility defaults`.
