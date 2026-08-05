---
title: Hermes Browser Documentation
description: Learn how to use, build, test, and contribute to Hermes Browser.
---

# Hermes Browser documentation

<div class="hermes-hero" markdown>

## A lighter path to the web

Hermes is an independent desktop browser built in Rust, powered by Servo, and wrapped in a small native interface. This documentation is one home for two different journeys: using the browser and helping build it.

[Start using Hermes](user-guide/index.md){ .md-button .md-button--primary }
[Build the browser](developer-guide/index.md){ .md-button }

</div>

!!! warning "Technical preview"
    Hermes is ready for experiments and development, not daily or sensitive browsing. Web content currently runs in the main browser process without operating-system sandboxing. Read [Privacy and security](user-guide/privacy-and-security.md) before visiting untrusted sites.

<div class="hermes-grid" markdown>

<div class="hermes-card" markdown>

### I want to use Hermes

Install the Linux dependencies, launch the browser, learn its shortcuts, and understand which sites work today.

[Open the User Guide →](user-guide/index.md)

</div>

<div class="hermes-card" markdown>

### I want to develop Hermes

Explore the crate boundaries, Servo event flow, native media integration, test strategy, and release process.

[Open the Developer Guide →](developer-guide/index.md)

</div>

</div>

## What Hermes already does

- Renders web content with Servo and WebRender
- Provides native tabs, navigation, keyboard input, and IME composition
- Resolves plain searches through DuckDuckGo
- Blocks known advertising and tracking resources before they load
- Plays supported media through GStreamer on Linux
- Runs on Linux through X11 or Wayland, with Windows build foundations in place

![Hermes rendering the Servo website](screenshots/hermes-servo.png)

## What Hermes is not yet

Hermes does not currently offer the security boundary, persistence, permissions, downloads, extensions, installers, or broad compatibility expected from a daily browser. Those limitations are part of the project, not fine print. The documentation calls them out wherever they affect a workflow.

## Choose your next step

- New to the browser? [Install Hermes](user-guide/installation.md).
- Already running it? [Learn the browsing basics](user-guide/getting-started.md).
- Investigating a broken site? [Use the troubleshooting guide](user-guide/troubleshooting.md).
- Ready to contribute? [Prepare a development environment](developer-guide/setup.md).
- Curious about the internals? [Read the architecture tour](developer-guide/architecture.md).
