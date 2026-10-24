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

Servo calls `notify_new_frame_ready` on the WebView delegate. Hermes records the first-frame milestone and marks the tab's frame-ready signal. Servo's runtime waker schedules the native redraw, and the desktop then calls `paint()` once for that frame without issuing a duplicate wake.

Servo owns a dedicated offscreen Surfman context. On Linux, Hermes copies the completed framebuffer into Vulkan external memory and imports it into the WGPU device already owned by Slint. Slint then composes that texture as a normal image. Servo never draws into Slint's window framebuffer and cannot leak OpenGL state into the UI renderer. This separation is essential during window moves and resizes, when both renderers are active in quick succession.

The Linux bridge requires Slint's Vulkan WGPU backend. The transfer stays on the GPU and explicitly waits for the OpenGL copy to finish before Slint samples the texture, preventing partially updated frames.

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

The Linux target enables Servo's upstream `media-gstreamer` feature. Hermes uses the `servo-media-gstreamer` 0.7.0 release unchanged; this is important because its GL frame-transfer path remains compatible with Servo's isolated content processes.

GStreamer remains a native system dependency. Cargo selects the Rust backend, but the machine must still provide the runtime plugins and codecs.

## Process isolation

The executable recognizes Servo's `--content-process <token>` bootstrap before it initializes UI, graphics, media, or worker threads. The vendored `gaol` crate also supports the system calls and shared-memory sizing needed by current Linux runtimes.

Despite that preparation, normal options explicitly set:

```rust
multiprocess: true,
sandbox: false,
```

Servo 0.7.0 content processes complete navigation and deliver visible frames, input, audio, and video in Hermes. The operating-system sandbox is a separate option: keep it disabled until Linux `gaol` can enforce its profile without destabilizing child processes. A sandbox acceptance test must cover navigation, visible pixels, audio and video, input, and clean shutdown together.
