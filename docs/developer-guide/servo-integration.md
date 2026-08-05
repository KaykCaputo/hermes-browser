---
title: Servo Integration
description: Understand how Hermes configures Servo rendering, WebViews, media, content blocking, and process isolation.
---

# Servo integration

`talaria-servo` is the boundary between Hermes' engine-neutral browser model and Servo's embedding API.

## Runtime construction

`ServoRuntime::new` installs the Rustls crypto provider, prepares explicit Servo options and preferences, connects the desktop event-loop waker, creates the Servo instance, and installs the browser-level delegate.

The runtime owns shared engine services. Individual tabs receive their own Servo WebView, frame-ready signal, load metrics, and delegate.

## WebView lifecycle

A new model tab causes the desktop layer to create a Servo WebView at `about:blank`. Navigation begins only when the controller dispatches a `Navigate` command. Closing a tab drops its adapter and closes the corresponding Servo WebView.

Only the active WebView is visible and unthrottled. Hidden tabs remain alive but receive Servo's throttled state.

## Frame delivery

Servo calls `notify_new_frame_ready` on the WebView delegate. Hermes records the first-frame milestone, marks the tab's frame-ready signal, and wakes the native event loop. The desktop then calls `paint()` for the active WebView.

This callback is the important distinction between a page that loaded and one that became visible. URL or title events alone do not prove that rendering succeeded.

## Input translation

The adapter translates Talaria input into Servo types:

- Logical coordinates become Servo page coordinates.
- Mouse buttons and wheel deltas become Servo input events.
- Keyboard events preserve logical key, physical code, location, modifiers, repeats, and composition state.
- Winit IME pre-edit and commit events become Servo composition sequences.

Keep platform-specific event decoding in the desktop crate and engine-specific mapping in `talaria-servo`.

## Content blocking

The build script compiles the bundled Brave filter snapshot into a serialized `adblock` engine. For each subresource request, the WebView delegate maps Servo's content destination to an ad-block resource type and checks the request before it loads.

Matched subresources are intercepted and cancelled. Main-frame navigations remain allowed until Hermes has an interstitial that can explain a blocked page.

## Linux media

The Linux target enables Servo's `media-gstreamer` feature. Hermes vendors a narrowly patched `servo-media-gstreamer` 0.4.0 for codec identifiers, error diagnostics, portable BGRA video frames, Web Audio negotiation, and lifecycle correctness.

GStreamer remains a native system dependency. Cargo selects the Rust backend, but the machine must still provide the runtime plugins and codecs.

## Process isolation

The executable recognizes Servo's `--content-process <token>` bootstrap before it initializes UI, graphics, media, or worker threads. The vendored `gaol` crate also supports the system calls and shared-memory sizing needed by current Linux runtimes.

Despite that preparation, normal options explicitly set:

```rust
multiprocess: false,
sandbox: false,
```

Servo 0.4.0 loads and scripts a page in the child process, but the embedded cross-process Paint path does not deliver visible output to Hermes' rendering context. Keep both flags disabled until a test demonstrates navigation, title updates, first-frame delivery, visible pixels, input, and clean shutdown together.

