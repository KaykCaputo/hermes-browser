# Patched servo-canvas 0.8.0

This directory vendors `servo-canvas` 0.8.0, Servo's 2D-canvas paint thread and
its drawing backends, under its original MPL-2.0 license. It is the crate
exactly as published on crates.io (checksum
`318945d504987838d404bb7dce122e0ad17afeaf02ebd3d3a3167b739b73db7d`), built from
https://github.com/servo/servo at commit
`4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas`.

The crate was published without a licence file. Its `Cargo.toml` declares
`license = "MPL-2.0"` and every source file carries the MPL-2.0 header; the
repository's MPL-2.0 `LICENSE` file applies to this directory.

Mercury enables Servo's `vello` feature, the GPU 2D-canvas backend, and draws
2D canvases on the GPU by default (`--disable-gpu-canvas` selects the CPU
backend, `vello_cpu`). The first commit that added this directory (`60ee59f`)
holds the crate as published, so

    git diff 60ee59f -- third_party/servo-canvas ':(exclude)third_party/servo-canvas/MERCURY.md'

shows every change, and only the changes listed below. Each is marked with a
`Mercury:` comment in the source. Line numbers marked "upstream" refer to the
published files. "Parity case" means a case of
`tests/pages/canvas-parity.html` (run it in both modes and compare, see
`docs/developer-guide/vendored-patches.md`).

## Changes

### 1. Build with wgpu 30

- **What:** `VelloDrawTarget::map_read` handles the `Result` that
  `BufferSlice::get_mapped_range()` returns in wgpu 30: on `Err` it logs at
  `warn` and passes `None` to the callback, as the function already did when
  `map_async` fails; the buffer is still unmapped afterwards.
- **Why:** as published the `vello` backend does not compile; vello 0.11
  requires wgpu 30.
- **Upstream:** `vello_backend.rs:755`.
- **Proof:** the crate compiles; `tests/pages/canvas-readback.html` passes.
- **Drop when:** a Servo release compiles with the `vello` feature.

### 2. Fall back to the CPU backend when the GPU canvas cannot start

- **What:** `VelloDrawTarget::try_new` returns `None` instead of panicking when
  no adapter or device is found or vello's renderer cannot be created (an
  `Err`, or a wgpu validation error captured with an error scope);
  `Canvas::new` then creates that canvas with the CPU backend
  (`CanvasData::try_new`). One `warn` per process —
  `GPU 2D canvas unavailable (<reason>); drawing 2D canvases on the CPU` — and
  later canvases go straight to the CPU (`GPU_CANVAS_UNAVAILABLE`). The
  device's uncaptured-error handler is installed before the renderer is
  created, so a wgpu error there is logged rather than reaching wgpu's
  default handler, which panics. The trait's `new` for this backend still
  exists but nothing calls it.
- **Why:** the panics killed Servo's single Canvas thread, leaving the whole
  session without 2D canvases (likely on Linux without Vulkan).
- **Upstream:** `vello_backend.rs:222` (`context.device(None)).unwrap()`),
  `:235` (`Renderer::new(..).unwrap()`), `:236` (handler installed after);
  `canvas_paint_thread.rs:350` (backend selection).
- **Proof:** forced "no device" and "renderer creation fails" runs (Task 4d
  report, rounds 1–3): exit 0, one warning, pixel-identical to
  `--disable-gpu-canvas`.
- **Drop when:** Servo's GPU canvas falls back by itself.

### 3. Do not panic when a render fails

- **What:** a failed `Renderer::render_to_texture` is logged once per process
  (`VELLO RENDER ERROR …`); the frame is skipped, the scene is kept for the
  next render, and the texture, buffer and displayed frame keep the last
  successfully rendered image (transparent black if none).
- **Why:** the `unwrap` killed the Canvas thread.
- **Upstream:** `vello_backend.rs:703`.
- **Proof:** forced render-failure runs (report, rounds 1–3).
- **Drop when:** Servo handles render errors.

### 4. Keep a canvas usable after a failed read-back

- **What:** when `map_async` fails, `map_read` unmaps the buffer before
  returning. wgpu 30 records the buffer as mapped as soon as `map_async` is
  called (`wgpu-30.0.2/src/api/buffer.rs`, `BufferSlice::map_async`), so
  without this every later read-back on that canvas failed. When wgpu-core
  never mapped the buffer, `unmap()` reports a validation error to the
  uncaptured-error handler, which logs `VELLO WGPU ERROR`; it does not panic.
  On this path and on change 1's error path, `getImageData` sees transparent
  black and the canvas's displayed frame is zero-filled as well.
- **Upstream:** `vello_backend.rs:753`.
- **Proof:** forced `map_async` failure with and without the fix (report,
  round 1): 1 failure versus 12.
- **Drop when:** Servo unmaps on this path.

### 5. Keep earlier drawing when a canvas is drawn again

- **What:** after each successful render, `render_to_texture` calls
  `Renderer::mark_override_image_dirty(&render_image.image)`.
- **Why:** the backend empties its scene after a render and starts the next
  one by drawing the previous frame (`render_image`, overridden to point at
  the canvas texture). vello 0.11 caches overridden images in its atlas and
  re-copies them only when marked dirty; never marked, the atlas kept the
  first, blank copy, so drawing done before any render (read-back or shown
  frame) vanished when the page drew again. The fix costs one GPU copy of the
  canvas into the atlas per render that draws on the previous frame — not a
  re-render: a 1000×1000 canvas drawn and read back 100 times took
  2.2–2.8 ms per step with the fix and 2.1–3.1 ms without.
- **Upstream:** `vello_backend.rs:704` (after the render), with
  `ensure_drawing` at `:186-197`.
- **Proof:** parity case `incremental`; `recover.html` (report, round 2).
- **Drop when:** Servo marks the image itself.

### 6. Share one GPU device and renderer between canvases

- **What:** the first GPU canvas on the Canvas thread creates a device, queue
  and vello renderer (`create_device_and_renderer`) and keeps them in the
  thread-local `SHARED_GPU`; later canvases reuse them through the existing
  `new_with_renderer`, each with its own texture and read-back buffer.
  Exception (`with_renderer_for`): a canvas whose area exceeds 2048×2048 — a
  sixteenth of the image atlas, which is at most 8192×8192 and holds every
  canvas's previous frame next to the images drawn into it — gets a renderer
  of its own on the shared device, so that it cannot crowd other canvases out
  of a shared atlas; a canvas that already has one keeps it when resized. If
  that renderer cannot be created, that canvas alone is drawn on the CPU.
  A device-lost callback sets `GPU_DEVICE_LOST`; the next canvas created or
  resized then drops the shared device and renderer, logs
  `GPU 2D canvas unavailable (GPU device lost); …` once and is drawn on the
  CPU, as are all later ones. After a loss, the read-back failure is logged
  once and wgpu's validation errors are no longer logged.
- **Why:** as published every canvas created its own wgpu instance, device
  and renderer, synchronously while the constellation (every tab) waited.
  vello's renderer keeps about 175 MB of GPU buffers between renders
  (`vello_encoding` `BufferSizes`), so fifty 200×100 canvases took about 9 GB
  of process memory and 1.5 s to create, two hundred about 34 GB.
- **Upstream:** `vello_backend.rs:204-240` (`new`), `:236-238` (the
  uncaptured-error handler, now quiet after a loss), `:752` (the map-error
  warning, now once per process).
- **The rule, measured** (`tests/pages/canvas-atlas.html`, `canvas-anim.html`;
  Task 4d report, round 4): with a renderer per canvas larger than 4096 on
  either side, five and ten animated 5120×600 canvases took 1.46 and 2.57 GB
  (CPU backend 0.56 and 0.78 GB), and shared 4096×4096 canvases still lost
  drawing when four were drawn in turns with a full-size `putImageData` and
  40 small `drawImage` sources, as did two 4096×4096 plus two 8192×1024
  canvases. With the area rule the thin canvases take 0.77 and 1.02 GB and
  every atlas run passes (up to sixteen 2048×2048 canvases sharing, and four
  4096×4096 canvases with their own renderers). The rule is a measured
  compromise, not a proof: enough large images drawn into one shared canvas
  in one frame can still exceed its atlas.
- **Proof:** `tests/pages/canvas-many.html` (fifty canvases: about 0.45 GB and
  under 0.1 s to create; two hundred: about 0.55 GB); parity cases
  `two-large-alternating`, `two-large-after-resize`; the atlas runs above.
- **Failure isolation:** there is one Canvas thread per process, serving
  every tab, so a lost device now blanks the GPU canvases of **every tab**,
  not one canvas. Besides a driver or GPU reset, wgpu loses the device on an
  out-of-memory error when work is submitted, so one page's oversized canvas
  can trigger it. Canvases created or resized afterwards fall back to the
  CPU. **The device-lost
  path has been exercised only by simulation** (destroying the device and
  setting the flag by hand): `device.destroy()` alone does not reach the
  callback because nothing polls the destroyed device. A real hardware loss
  reaches it through wgpu-core's `Device::lose`, which calls the callback
  synchronously (read from the source, not observed).
- **Upstream (resize):** `canvas_paint_thread.rs:629` (`recreate`); the
  resize path goes through `CanvasData::try_recreate` and
  `VelloDrawTarget::try_similar`, so the trait's `create_similar_draw_target`
  is unchanged.
- **Drop when:** Servo shares the device itself.

### 7. Apply non-rectangular clips

- **What:** `render_to_texture` renders inside `ignore_clips`, which closes
  the open clip layers for the render and re-opens them afterwards; and
  `clear_rect` takes its whole-canvas shortcut (reset the scene, forget the
  clips) only when no clip is active.
- **Why:** the backend keeps an active clip as an open vello layer; vello
  applies a clip shape only when its layer is closed, and an open one clips
  to the shape's bounding box. A circular clip came out square. The
  `clear_rect` shortcut judges clips by their bounding boxes, which was
  consistent only while clips were bounding boxes: with real clip shapes, a
  full-canvas `clearRect` inside a rounded-rectangle or even-odd clip cleared
  pixels outside it and left everything drawn afterwards unclipped until
  `restore()`.
- **Upstream:** `vello_backend.rs:689-703` (render), `:253-257`
  (`clear_rect`), `:170-184` (`is_viewport_cleared`).
- **Proof:** parity cases `clip-path`, `clip-path-closed`,
  `clip-rounded-clearrect`, `clip-evenodd-ring-clearrect` (the last two fail
  on the GPU with the old shortcut, and fail on the CPU backend).
- **Drop when:** Servo closes the clip layers itself.

### 8. Honour the fill rule of a clip

- **What:** `push_clip` converts its `fill_rule` (the crate's existing
  `FillRule` → `peniko::Fill` conversion), uses it for the clip layer, and
  keeps it with the stored clip (`clips: Vec<(Path, peniko::Fill)>`) so the
  layer is re-opened with the same rule.
- **Why:** `push_clip` ignored `fill_rule`, so `clip("evenodd")` clipped to
  the whole outer shape.
- **Upstream:** `vello_backend.rs:488-494`, `:54`, `:166`, `:710`.
- **Proof:** parity case `clip-evenodd` (the CPU backend still has this
  defect; not changed).
- **Drop when:** Servo passes the fill rule itself.

### 9. Canvases beyond the GPU limits use the CPU backend

- **What:** `VelloDrawTarget::usable_for` refuses a size larger than wgpu's
  default texture limit (8192, the limit vello's device is created with) or
  covering more than 256 bins of 256×256 pixels. A canvas created like that
  is drawn on the CPU (`try_new`); a GPU canvas resized like that is replaced
  by a CPU-backend one (`Canvas::recreate`, `CanvasData::try_recreate` and
  `recreate_as`, keeping the WebRender image key) and stays on the CPU if
  made small again.
- **Why:** beyond the texture limit a canvas got an invalid texture and
  stayed blank, with a wgpu validation error on every draw. Beyond 256 bins
  vello 0.11 renders only part of the canvas (it warns "Trying to paint too
  large image", linebender/vello#680): a 4352×4096, 5000×4000 or 8192×2304
  canvas lost the content of its far bins.
- **Upstream:** `vello_backend.rs:204` (creation), `canvas_paint_thread.rs:629`
  (`recreate`).
- **Proof:** parity cases `wide-10000-incremental`, `wide-10000-after-resize`,
  `resize-past-limit-and-back`, `beyond-256-bins`.
- **Drop when:** Servo checks the size itself. Re-check both limits when
  vello changes the device limits it requests or its bin limit.

### 10. Draw shapes that have a shadow, without the shadow

- **What:** `draws_shadow` (in `canvas_data.rs`) replaces the three
  `need_to_draw_shadow()` checks and returns false, so `fillRect`,
  `strokeRect` and `drawImage` take their normal path; one `warn` per process
  says shadows are not supported.
- **Why:** with a visible shadow those calls went through
  `draw_with_shadow`, which draws the shape only into a temporary target and
  hands it to `draw_surface_with_shadow` — a stub in both backends that only
  logs `no support for drawing shadows`. The shape itself was never drawn:
  a card, button or chart bar given a shadow vanished. Paths and text ignore
  shadows and were unaffected. A transparent shadow colour, or zero blur and
  offsets, already meant no shadow and still does.
- **Upstream:** `canvas_data.rs:109`, `:188`, `:230` (the checks), `:377`
  (`draw_with_shadow`); `vello_backend.rs:375`, `vello_cpu_backend.rs:294`
  (the stubs).
- **Proof:** parity cases `shadow-*` (shape present in both backends; the
  shadow itself is a known gap).
- **What a real shadow would take:** the GPU scene API offers only
  `Scene::draw_blurred_rounded_rect`, enough for the shadow of a filled
  rectangle; any other shape would need an offscreen render plus a blur,
  which vello 0.11 does not provide. The CPU backend (`vello_cpu` 0.2) has
  filter layers including a drop shadow, so shadows there are feasible
  separately.
- **Drop when:** a backend implements `draw_surface_with_shadow`.

## Known limitations (accepted)

- A device lost while a read-back waits on the GPU panics inside
  `vello::util::block_on_wgpu` (`device.poll(..).unwrap()`) or wgpu's own
  `Device::poll`, which treats a lost device as fatal. That kills the Canvas
  thread and so every 2D canvas for the rest of the session.
- **Every displayed frame of a GPU canvas is a read-back.** WebRender is
  given the canvas as CPU pixels, so for each frame that changes the canvas
  the Canvas thread renders the scene, copies the texture into the atlas and
  into a buffer, waits for the buffer to map, and un-premultiplies the whole
  canvas into shared memory — the same path as `getImageData`, `toDataURL`
  and `drawImage` from a canvas. The cost grows with the canvas's pixel
  count. Measured (Task 4d report, round 4; animated canvas redrawn every
  frame with `clearRect`, 300 shapes and text, no `getImageData`; machine
  load in brackets):

  | Canvas (device pixels) | GPU fps | CPU fps |
  | --- | --- | --- |
  | 400×300, 60 s | 58.9 (9–12) | 59.8 (6–9) |
  | 800×600, 10 min | 51.2 (6–14) | 46.1 (6–82) |
  | 1600×1200, 60 s | 58.2 (5) | 56.9 (5–10) |
  | 2360×1304 (full default window at 2×), 10 min | 52.9 (6–23) | 57.1 (4–18) |
  | 3200×1800, 60 s | 28.5 (8–10) | 44.1 (10–20) |
  | 4096×2304, 60 s | 24.9 (21–33) | 39.7 (35–45) |

  Up to about 3 megapixels the two backends are within the noise of each
  other; above that the GPU canvas is about 35–40 % slower. Incremental
  drawing (a few strokes per frame on a 2360×1304 canvas, one pixel read every
  5 s, 5 min): 45.7 fps on the GPU, 52.5 on the CPU, nothing lost in either.
  Explicit read-backs cost the same way: an 800×600 `getImageData` every
  frame took a median 6.7 ms on the GPU against 4.5 ms on the CPU.
- **Memory.** The shared renderer keeps about 175 MB of GPU memory from the
  first GPU canvas until the process exits (fifty small canvases: about
  0.45 GB process footprint against 0.28 GB on the CPU; a 2360×1304 animated
  canvas: about 0.6 GB against 0.38 GB). Over the 10-minute runs the GPU
  process grew by 30–50 MB (597 → 646 MB, 471 → 502 MB), the CPU process by
  under 40 MB with more scatter; no unbounded growth was seen.
- The CPU backend reads `rgba(0, 102, 204, 0.5)` back as `[0,101,203,128]`
  (premultiplied 8-bit storage, truncated when un-premultiplied); the GPU
  backend reads `[0,102,203,128]`. Both are within the ±1 tolerance of
  `tests/pages/canvas-readback.html`, the CPU one at its edge.

## Known differences between the backends

From `tests/pages/canvas-parity.html` and real pages (Task 4d report):

- GPU: gradients are evaluated about half a pixel off the pixel centre
  (`gradient-radial`: 255 and 127 where the specification gives 251 and 130),
  and text is sometimes placed one device pixel lower than on the CPU
  (`text-baseline-hanging`; chart axis labels). Inside vello; cosmetic.
- CPU: `clearRect` ignores the clip and a full-canvas `clearRect` drops it
  (`clip-then-clear`, `clip-rounded-clearrect`,
  `clip-evenodd-ring-clearrect`), `putImageData` blends instead of replacing
  (`putimagedata-over-drawing`), `evenodd` clips use the non-zero rule
  (`clip-evenodd`).
- Both: shadows are not drawn (change 10 keeps the shapes). Conic gradients,
  `OffscreenCanvas`, `ctx.filter`, `isPointInStroke` and the `FontFace` API
  are not available in this Servo configuration; an image loaded from a
  `toBlob` blob URL does not decode; text drawn by pdf.js comes out as
  missing-glyph boxes, drawn on the canvas (they differ in anti-aliasing
  between the two backends, which DOM text would not).

## Removing this directory

Remove the `[patch.crates-io]` entry and this directory once a Servo release
compiles with the `vello` feature and contains equivalents of changes 2–10,
or once Mercury no longer needs them.
