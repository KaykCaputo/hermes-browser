---
title: Servo Integration
description: Understand how Mercury configures Servo rendering, WebViews, media, content blocking, and process isolation.
---

# Servo integration

`talaria-servo` is the boundary between Mercury's engine-neutral browser model and Servo's embedding API.

## Runtime construction

`ServoRuntime::new` installs the Rustls crypto provider, prepares explicit Servo options and preferences, connects the desktop event-loop waker, creates the Servo instance, and installs the browser-level delegate.

The runtime owns shared engine services. Individual tabs receive their own Servo WebView, frame-ready signal, load metrics, and delegate.

## WebView lifecycle

A new model tab causes the desktop layer to create a Servo WebView at `about:blank`. Navigation begins only when the controller dispatches a `Navigate` command. Closing a tab drops its adapter and closes the corresponding Servo WebView.

Only the active WebView is visible and unthrottled. Hidden tabs remain alive but receive Servo's throttled state.

## Shutdown

Every quit ends the run through one post-run sequence, `post_run::finish` (`apps/mercury-desktop/src/post_run.rs`), which runs at most once per process: the exit code; the final saves of other parts of the browser, which share one deadline (`FINAL_SAVES_BUDGET`, 3 s) and run under a 4 s watchdog; the engine's ordered shutdown under its own 5 s watchdog (the 0.5 s settle period included); then, under a 2 s watchdog, the window, the media-buffer directory and the temporary profile; and the log. A watchdog that fires removes the temporary profile and ends the process, with a 5 s backstop for its own clean-up, so the worst case is about 16 s, well under the 45 s loginwindow allows an application at log-out. Each final save has a one-line slot in `Steps` (session, history, settings, menus). `main` runs the sequence after the event loop returns. On macOS the system's terminate request runs through `macos_terminate.rs`, which adds `applicationShouldTerminate:` to winit's application delegate (through `macos_app_delegate.rs`, which other delegate methods use too): when the current Apple event is a quit whose `keyAEQuitReason` says the session is ending (log-out, restart, shut-down), and the callback is not nested inside one of Mercury's Slint callbacks (`callback_scope.rs`), the engine is not busy and the run loop is in its default mode, the sequence runs inside that callback and the answer is `NSTerminateNow` (even if the sequence panics; a run whose exit status is not 0 ends the process itself with that status). Every other terminate (a Quit menu item, the Dock's Quit) is answered `NSTerminateCancel` and quits the event loop, so `main` runs the sequence. A log-out that arrives while a menu is open, a modal panel is up, a Slint callback is running or the engine is busy is answered `NSTerminateCancel` as well (macOS then reports that Mercury cancelled the log-out) and Mercury quits through its own path. "Later" cannot be used: AppKit then waits in a modal run loop for a reply and the event loop never ends. Because the sequence may run inside AppKit's callback, no step may run Slint's or winit's event loop, wait for `invoke_from_event_loop`, or borrow Slint state.

Within the sequence, `shut_down_engine` (in `apps/mercury-desktop/src/engine.rs`) takes the engine's state, hides the window, releases the window's web-view texture, closes the WebViews, lets Servo run for a settle period, drops the runtime, then the rendering context, then the WGPU device. The quit shortcut and `--screenshot` call `slint::quit_event_loop()`. Closing the window does the same: the close-requested handler (`quit.rs`) quits and keeps the window shown, because Slint suspends a window it has hidden while the loop exits, which would tear rendering down before the engine has shut down. `main` uses `show()` and `slint::run_event_loop()` rather than `run()`, which hides the window as soon as the loop ends (on Wayland, hiding destroys the window). `SIGTERM`, `SIGINT` and `SIGHUP` start the same quit (`signals.rs`): the handler records the signal and writes to a pipe, a thread asks the event loop to quit, and after the shutdown `main` re-raises the signal with its default action, so the parent sees the process end by that signal (a shell reports 128 + its number). A second signal during the shutdown ends the process at once. Content processes ignore `SIGINT`, `SIGHUP` and `SIGTERM`, which a terminal (or a group-wide kill) sends to the whole process group: killed by them, they would die while the browser shuts Servo down in order, which then waits for them until the watchdog fires; they end when the browser closes their IPC, or within a second of the browser's death.

The order also holds when the engine is dropped without `shut_down_engine` (an early return during start-up, or unwinding): `EngineState`'s `Drop` releases the same parts in the same order through `release_in_order`, without the settle period or the watchdog. In `talaria-servo`, each holder of a handle to Servo also holds a share of the after-engine clean-up (which strips session cookies from the saved jar) in a `HandleThenCleanup`, which drops the handle first; so the clean-up runs after the last handle to Servo has gone, also when a WebView outlives the runtime.

The rendering notifier's `RenderingTeardown` must not call into the window. Slint runs it from `WinitWindowAdapter::suspend`, which holds the adapter's window cell mutably borrowed; a property change asks for a redraw, which borrows that cell again and panics unless a redraw is already pending (`i-slint-backend-winit-1.17.1/winitwindowadapter.rs:628-632, 1282-1288`). The texture imported from the old device is released by the next `RenderingSetup` or by the shutdown.

The settle period works around a race in Servo 0.8. A page's WebGL contexts are destroyed only when its content process, closing the page, sends `RemoveContext`. If the constellation exits first, the WebGL thread's message loop (`servo-webgl-0.8.0/webgl_thread.rs:319-327`) ends because its `recv()` fails, not because of `WebGLMsg::Exit`, and the thread drops contexts that are still alive, which surfman turns into a panic. Servo's shutdown does send `Exit` afterwards, but the send fails because the thread's receiver is already gone ("Could not exit WebGLThread"). By which route the receive fails (ipc-channel's router forwarding an error, or dropping the route) was not confirmed. Mercury spins Servo for `WEBGL_SETTLE_PERIOD` (500 ms) between closing the WebViews and dropping the runtime, inside one panic guard with the drop. A watchdog ends the process after 5 seconds if Servo has not shut down: it flushes the log, removes a temporary profile and calls `_exit`. The real fix is for the WebGL thread to destroy leftover contexts when its channel closes, which needs a Servo patch.

## Frame delivery

Servo calls `notify_new_frame_ready` on the WebView delegate. Mercury records the first-frame milestone and marks the tab's frame-ready signal. Servo's runtime waker schedules the native redraw, and the desktop then calls `paint()` once for that frame without issuing a duplicate wake.

Servo owns a dedicated offscreen Surfman context. On Linux, Mercury copies the completed framebuffer into Vulkan external memory and imports it into the WGPU device already owned by Slint. Slint then composes that texture as a normal image. Servo never draws into Slint's window framebuffer and cannot leak OpenGL state into the UI renderer. This separation is essential during window moves and resizes, when both renderers are active in quick succession.

The Linux bridge requires Slint's Vulkan WGPU backend. The transfer stays on the GPU and explicitly waits for the OpenGL copy to finish before Slint samples the texture, preventing partially updated frames.

### Frame bridges

The bridges live in `apps/mercury-desktop/src/rendering/`. All of them give Slint an `Rgba8Unorm` WGPU texture or a pixel buffer through the same two calls, `BrowserRenderingContext::new(size, device, queue)` and `current_frame_as_image(device)`.

| File | Platform | Role |
|---|---|---|
| `surfman.rs` | Linux, macOS | The Surfman OpenGL context, its swap chain, and Servo's `RenderingContext` trait. |
| `vulkan.rs` | Linux | Vulkan external memory, filled by an OpenGL blit. |
| `metal.rs` | macOS | IOSurface → Metal → one WGPU pass, with an OpenGL read-back fallback. |
| `software.rs` | other platforms | Servo's `SoftwareRenderingContext` with a CPU read-back. |

On macOS, Surfman renders Servo's frames with hardware OpenGL into an IOSurface. After Servo paints, Mercury waits for OpenGL to finish (`glFinish`), unbinds the surface, wraps its IOSurface as a `BGRA8Unorm` Metal texture on Slint's own Metal device, and runs one small render pass that flips the image vertically and writes the `Rgba8Unorm` texture Slint draws. No pixel crosses the CPU. The pipeline for that pass is created once per rendering context.

If Slint's WGPU device is not a Metal device, or an import fails, Mercury logs the reason once at `warn` level and from then on reads each frame back with `glReadPixels` from the same OpenGL context. Web views are not recreated. Setting `MERCURY_FORCE_FRAME_READBACK=1` selects the read-back path from the start. The active path and the OpenGL version and renderer are logged once at `info` level when the context is created. With `RUST_LOG=mercury_browser=debug`, the time spent importing each frame is summarised (count, median, 95th percentile) every few hundred frames and when the context is dropped.

If resizing Servo's surface fails, the bridge logs it once and keeps the previous size, which still matches the surface Servo draws into; Slint stretches that frame to the new viewport. The resize is not retried by itself: the desktop resizes the web view only when the viewport size or the active tab changes, so the next attempt happens with the next window resize or tab switch.

The Linux and macOS bridges follow the structure of Slint's Servo example (`examples/servo/src/webview/rendering_context/` in https://github.com/slint-ui/slint, tag `v1.17.1`, MIT). `metal.rs` is adapted from that example's `metal.rs`; see `THIRD-PARTY-NOTICES.md` and `LICENSES/MIT-slint-examples.txt`.

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

Matched subresources are intercepted and cancelled. Main-frame navigations remain allowed until Mercury has an interstitial that can explain a blocked page.

## Linux media

The Linux target enables Servo's upstream `media-gstreamer` feature. Mercury uses the `servo-media-gstreamer` 0.7.0 release unchanged; this is important because its GL frame-transfer path remains compatible with Servo's isolated content processes.

GStreamer remains a native system dependency. Cargo selects the Rust backend, but the machine must still provide the runtime plugins and codecs.

## Process isolation

The executable recognizes Servo's `--content-process <token>` bootstrap before it initializes UI, graphics, media, or worker threads. The vendored `gaol` crate also supports the system calls and shared-memory sizing needed by current Linux runtimes.

Despite that preparation, normal options explicitly set:

```rust
multiprocess: true,
sandbox: false,
```

Servo 0.7.0 content processes complete navigation and deliver visible frames, input, audio, and video in Mercury. The operating-system sandbox is a separate option: keep it disabled until Linux `gaol` can enforce its profile without destabilizing child processes. A sandbox acceptance test must cover navigation, visible pixels, audio and video, input, and clean shutdown together.
