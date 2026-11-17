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

### Vendored dependencies

Each Mercury-specific upstream regression needs a focused test close to the patch. The `gaol` shared-memory test protects the `memfd` and `ftruncate` behavior that originally killed isolated Servo children with `SIGXFSZ`.

## Manual browser checks

Before merging a rendering or media change, verify at least:

1. The new-tab page paints.
2. A simple HTTPS page loads, paints, and updates its title.
3. Back, forward, reload, tabs, keyboard input, and scrolling work.
4. A direct MP4 or WebM resource plays on Linux when media code changed.
5. Closing the browser does not leave a repeated broken-pipe error loop.

### Quit paths

Nobody may synthesise input on the shared test machine, so the quits a user performs are exercised from inside the process with the diagnostic options `--exit-after <milliseconds>` and `--exit-by close-request|quit|terminate|logout`. `close-request` dispatches Slint's close request to the window, the same call the winit backend makes for the title bar's close button; `quit` calls `slint::quit_event_loop()`, as the quit shortcut does. On macOS, `terminate` sends `-[NSApplication terminate:]` from the run loop, as a Quit menu item does, and `logout` sends the browser's own process (never another one) the quit Apple event of a log-out (`keyAEQuitReason` = `kAEReallyLogOut`); an event addressed to the current process is dispatched directly to its handler and never leaves the process. Neither can log the user out. `logout` exists only in builds with the `quit-diagnostics` cargo feature (`cargo build --release -p mercury-desktop --features quit-diagnostics`, with the GStreamer environment); `make lint` and `make check` compile it through `--all-features`, released builds never contain it. They work with `--background` (exit status 0) and with `--screenshot`, where a quit before the capture exits with status 1. Signals are tested by sending `SIGTERM`, `SIGINT` or `SIGHUP` to a `--background` run: it quits in order and ends by that signal.

```bash
target/release/mercury-browser --background --exit-after 3000 --exit-by close-request \
    "file://$PWD/tests/pages/animation.html"
```

After any quit, no `mercury-browser` process of the run may be left, and a temporary profile (`$TMPDIR/mercury-browser-profile-<pid>-*`) must be gone. `SLINT_DESTROY_WINDOW_ON_HIDE=1` makes hiding destroy the window on macOS too, which exercises the Wayland teardown path.

For an engine-level website failure, reproduce it in the pinned Servo `servoshell`. Record whether the same revision succeeds or fails there.

## Documentation checks

```bash
make docs
```

MkDocs runs in strict mode, so missing navigation entries, broken internal references, and configuration warnings fail the build.

