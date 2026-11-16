---
title: Testing and Quality
description: Run Mercury unit, adapter, vendored dependency, formatting, and Clippy checks.
---

# Testing and quality

Mercury uses small deterministic tests for browser policy and adapter translation, then reserves manual runs for native integration that needs a real window, GPU, media stack, or website.

## The standard checks

```bash
make qa
```

This runs formatting checks, Clippy with warnings denied, workspace tests, and the vendored `gaol` shared-memory regression test.

Run stages separately while iterating:

```bash
make fmt-check
make lint
make test
```

## Type-check all targets and features

```bash
make check
```

This checks the complete workspace with all targets and features. It is useful after changing feature flags, target-specific dependencies, or public APIs.

## Test layers

### Talaria Core

Core tests use fake WebViews to cover:

- URL and search classification
- Browser state transitions
- Tab creation, selection, cycling, and closure
- Command routing by `TabId`
- High-frequency input coalescing
- Engine-neutral WebView dispatch

Prefer adding a core test when the behavior can be expressed without Servo.

### Talaria Servo

Adapter tests cover mappings between Talaria and Servo types, favicon conversion, load metrics, history state, content-filter decisions, runtime options, and content-process argument parsing.

### Desktop composition

Desktop tests exercise event routing, bounded command dispatch, shortcuts, key conversion, and IME composition ownership without opening the full application where possible.

### Menus and the About window

`apps/mercury-desktop/src/menus/model.rs` holds the menu bar as a table. Its tests check that every item shows the key that runs its action in `shortcuts.rs`, and that each key has exactly one owner: either the window's key path or the native menu. `decide_key_equivalent` holds the main menu's whole decision for a key equivalent; it is tested as a table that covers the browser window, other Mercury windows and native panels, with and without auto-repeat. `crates/mercury-ui/tests/render_menus.rs` renders the browser menu and the About window with Slint's software renderer, with the chrome theme set to Light, Dark and System. It also opens the browser menu in the browser window and drives it with the arrow keys and Return. Set `MERCURY_UI_RENDER_DIR` to keep the PNGs.

The macOS menu bar cannot be opened without input. To read it back, use the diagnostic `MERCURY_MENU_DIAGNOSTICS`. It is compiled only with the `menu-diagnostics` feature of `mercury-desktop`, which `make build-release` and the app bundle leave off; build a binary for it with `cargo build --release -p mercury-desktop --features menu-diagnostics` in the GStreamer environment, then run `make build-release` again afterwards. Run a background launch with `MERCURY_MENU_DIAGNOSTICS=<directory>`. The log then says whether Mercury's menu is the main menu after start-up, while the About window is open and after it closes, and while the settings window is open and after it closes. The About window, drawn by the app's renderer, is saved as `about-window.png` in that directory. Use `RUST_LOG=mercury_browser=info` or more to see the log lines.

### Vendored dependencies

Each Mercury-specific upstream regression needs a focused test close to the patch. The `gaol` shared-memory test protects the `memfd` and `ftruncate` behavior that originally killed isolated Servo children with `SIGXFSZ`.

## Manual browser checks

Before merging a rendering or media change, verify at least:

1. The new-tab page paints.
2. A simple HTTPS page loads, paints, and updates its title.
3. Back, forward, reload, tabs, keyboard input, and scrolling work.
4. A direct MP4 or WebM resource plays on Linux when media code changed.
5. Closing the browser does not leave a repeated broken-pipe error loop.

For an engine-level website failure, reproduce it in the pinned Servo `servoshell`. Record whether the same revision succeeds or fails there.

## Documentation checks

```bash
make docs
```

MkDocs runs in strict mode, so missing navigation entries, broken internal references, and configuration warnings fail the build.

