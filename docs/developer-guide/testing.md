---
title: Testing and Quality
description: Run Hermes unit, adapter, vendored dependency, formatting, and Clippy checks.
---

# Testing and quality

Hermes uses small deterministic tests for browser policy and adapter translation, then reserves manual runs for native integration that needs a real window, GPU, media stack, or website.

## The standard checks

```bash
make qa
```

This runs formatting checks, Clippy with warnings denied, workspace tests, the vendored `gaol` shared-memory regression test, and the `servo-media-gstreamer` library tests.

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

Each Hermes-specific upstream regression needs a focused test close to the patch. The `gaol` shared-memory test protects the `memfd` and `ftruncate` behavior that originally killed isolated Servo children with `SIGXFSZ`.

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

