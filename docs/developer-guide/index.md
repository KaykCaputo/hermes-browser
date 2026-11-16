---
title: Developer Guide
description: Understand the Mercury Browser codebase and prepare a focused contribution.
---

# Developer Guide

Mercury is a browser shell with deliberately visible boundaries. The UI should not need to understand Servo internals, and the core tab model should remain testable without creating a window or graphics context.

The codebase is small enough to read, but the engine underneath it is not. This guide shows where Mercury ends, where Servo begins, and how to test the seam between them.

## A useful reading order

1. [Development setup](setup.md) — build the exact locked dependency graph.
2. [Architecture](architecture.md) — follow commands from Slint to a WebView and events back to the UI.
3. [Servo integration](servo-integration.md) — understand rendering, media, content blocking, and process mode.
4. [Testing and quality](testing.md) — run the right checks for the area you changed.
5. [Vendored patches](vendored-patches.md) — preserve the project's narrow patch policy.
6. [Continuous integration](continuous-integration.md) — what CI and the release workflow check, the dependency policy, and how to update the pins.

## Repository map

| Path | Purpose |
| --- | --- |
| `apps/mercury-desktop` | Desktop lifecycle, Slint callbacks, the native event loop, and runtime composition |
| `crates/mercury-ui` | Slint components and UI-facing presentation types |
| `crates/mercury-passwords` | Saved logins in the macOS Keychain, password generation, and login CSV import and export ([details](passwords.md)) |
| `crates/talaria-core` | Engine-independent browser state, commands, events, tabs, and address resolution |
| `crates/talaria-servo` | Servo WebViews, browser events, media selection, metrics, and content blocking |
| `third_party` | Focused compatibility patches for dependencies pinned by Servo |
| `docs` | This MkDocs site and its screenshots |

## Design priorities

- Keep browser policy out of the UI toolkit.
- Keep Servo types behind the Talaria adapter boundary.
- Prefer explicit state transitions that can be unit tested.
- Make platform limitations visible instead of silently claiming support.
- Carry dependency patches only when they are narrow, documented, and removable.

## Current engineering constraint

The normal runtime enables Servo 0.7.0 content-process isolation. Navigation, rendering, input, audio, and video have been validated across that boundary. The independent operating-system sandbox remains disabled because the Linux `gaol` path is not yet reliable enough for a release.
