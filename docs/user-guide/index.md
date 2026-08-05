---
title: User Guide
description: Install Hermes, browse the web, play media, and understand the current preview limitations.
---

# User Guide

Hermes is deliberately small: it gives you tabs, an address bar, native input, content blocking, and Servo's view of the web without pretending to be a finished everyday browser.

This guide helps you get useful results from the technical preview and recognize the difference between a site problem, a missing media component, and a known engine limitation.

## Start here

1. [Install Hermes](installation.md) and its native Linux dependencies.
2. [Start browsing](getting-started.md) with a URL or DuckDuckGo search.
3. Keep the [privacy and security model](privacy-and-security.md) in mind.
4. If a page is blank or incomplete, work through [Troubleshooting](troubleshooting.md).

## Current platform support

| Platform | Status | Notes |
| --- | --- | --- |
| Linux x86-64 | Primary development platform | X11 and Wayland are supported; multimedia uses system GStreamer plugins. |
| Windows x86-64 | Build foundation | Native GStreamer is not packaged yet, so media support is limited. |
| macOS | Not currently packaged | No supported release workflow is available. |

!!! note
    Source builds follow Servo's own platform requirements. A successful build does not imply that every web platform feature is implemented.

## The short version

Hermes is a good fit for:

- Exploring Servo on real websites
- Testing browser-shell ideas in a native Rust application
- Following the project's progress
- Reproducing and reporting focused compatibility issues

It is not yet a good fit for:

- Passwords, payments, private accounts, or sensitive browsing
- A primary browser profile
- Sites that depend on extensions or DRM
- Adaptive streaming players that require Media Source Extensions

