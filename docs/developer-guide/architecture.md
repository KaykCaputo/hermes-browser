---
title: Architecture
description: Follow state, commands, input, and rendered frames through the Hermes Browser layers.
---

# Architecture

Hermes separates product state from engine integration. That boundary lets most browser behavior run in fast Rust tests without initializing Servo, Slint, WGPU, or a desktop event loop.

## Layers

```text
Hermes Desktop
  Slint window, winit events, WGPU composition, runtime integration
          │
          │ UI callbacks and presentation state
          ▼
Hermes UI
  Slint components and UI-facing models
          │
          │ browser commands and events
          ▼
Talaria Core
  tabs, navigation policy, state transitions, WebView contract
          │
          │ engine-neutral WebView calls
          ▼
Talaria Servo
  Servo runtime, WebViews, delegates, rendering, media, blocking
          │
          ▼
Servo, WebRender, Surfman, and GStreamer
```

## Talaria Core

`talaria-core` owns the browser concepts that should survive an engine or UI replacement:

- `BrowserController` owns tabs, the active tab, and the command queue.
- `BrowserState` applies engine events such as URL, title, icon, history, and load-state changes.
- `BrowserCommand` describes navigation and input without Servo types.
- `WebView` is the trait implemented by an engine adapter.
- Address resolution distinguishes supported URLs from DuckDuckGo search queries.

The controller coalesces adjacent pointer movement, scrolling, and focus commands for the same tab. This keeps high-frequency desktop input from growing an unbounded queue between frames.

## Desktop composition

`hermes-desktop` creates the Slint window, selects the graphics backend, creates a `ServoRuntime`, and owns one `ServoWebView` per browser tab.

During each event-loop turn it:

1. Reads pending Servo events and applies them to the matching tab.
2. Ensures every model tab has a WebView and removes closed WebViews.
3. Shows and resumes the active WebView while hiding and throttling the previous one.
4. Updates viewport size and scale factor.
5. Dispatches a bounded number of queued browser commands.
6. Paints only when Servo has announced a new frame.
7. Synchronizes the native UI from the active `BrowserState`.

The bounded command drain prevents a burst of input from monopolizing a frame.

## Command flow

```text
Slint callback or winit input
  → BrowserController queue
  → TabCommand with a stable TabId
  → ServoWebView adapter
  → Servo WebView API
```

Commands carry a `TabId`, so switching tabs does not accidentally redirect already queued work to the newly active page.

## Event flow

```text
Servo WebViewDelegate
  → TabEvent over the runtime channel
  → BrowserController::handle_event
  → BrowserState::apply
  → Slint presentation model
```

The same stable `TabId` routes asynchronous title, favicon, load, and history updates back to the WebView that produced them.

## Rendering ownership

Slint owns the native window and event loop. Servo renders into a separate offscreen Surfman context; on Linux, Hermes imports each completed frame through Vulkan external memory and gives the resulting WGPU texture to Slint. The two renderers never share OpenGL state or the window framebuffer. A consumable frame-ready signal also prevents repainting a page when Servo has not produced a new frame.

On Linux, Mesa and the desktop session choose the default graphics adapter. Hermes removes inherited `DRI_PRIME` overrides before graphics or media threads start, and the vendored Surfman patch avoids applying a process-global GPU preference during context creation.
