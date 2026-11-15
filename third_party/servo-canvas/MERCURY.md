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

Mercury enables Servo's `vello` feature, the GPU 2D-canvas backend. The first
commit that added this directory (`60ee59f`) holds the crate as published, so
`git diff 60ee59f -- third_party/servo-canvas` shows every change below.
Changes 2–9 are marked with `Mercury:` comments in the source.

## 1. Build with wgpu 30 (`vello_backend.rs`, `VelloDrawTarget::map_read`)

As published, the backend does not compile: vello 0.11 requires wgpu 30, where
`BufferSlice::get_mapped_range()` returns `Result<BufferView, MapRangeError>`
instead of a `BufferView`. When `get_mapped_range()` fails, the error is logged
at `warn` and the callback receives `None` — the same handling the function
already applies when `map_async` fails. The buffer is mapped at this point, so
the function still reaches its `unmap()`. The successful path is unchanged.

Drop it when a Servo release compiles with the `vello` feature.

## 2. Fall back to the CPU backend when the GPU canvas cannot start

Files: `vello_backend.rs` (`VelloDrawTarget::try_new` and `start`, the
`GPU_CANVAS_UNAVAILABLE` flag), `canvas_data.rs` (`CanvasData::try_new`),
`canvas_paint_thread.rs` (`Canvas::new`).

As published, `VelloDrawTarget::new` unwraps when no GPU adapter or device is
available and when vello's renderer cannot be created. The panic kills Servo's
single "Canvas" thread, after which the session has no 2D canvases at all.
Now the construction returns an error instead, and `Canvas::new` creates that
canvas with the CPU backend (`vello_cpu`, the one `--disable-gpu-canvas`
selects). The reason is logged once per process at `warn`
(`GPU 2D canvas unavailable (<reason>); drawing 2D canvases on the CPU`), and
later canvases go straight to the CPU backend without retrying the GPU.

Renderer creation also counts as failed when it raises a wgpu validation
error (captured with an error scope), and the device's uncaptured-error handler
is installed before the renderer is created rather than after, so a wgpu error
during pipeline creation is logged instead of reaching wgpu's default handler,
which panics.

`GenericDrawTarget::new` must still exist for the GPU backend; nothing in the
crate calls it for that backend any more.

Drop it when Servo's GPU canvas falls back by itself.

## 3. Do not panic when a render fails (`vello_backend.rs`, `render_to_texture`)

As published, a failed `Renderer::render_to_texture` unwraps and kills the
Canvas thread. Now it is logged once per process at `warn`
(`VELLO RENDER ERROR …`) and the canvas stays alive: the frame is skipped, the
scene is kept for the next render, and the texture, buffer and displayed frame
keep the last successfully rendered image (transparent black if none). A later
render that succeeds draws the kept scene.

Drop it when Servo handles render errors.

## 4. Keep a canvas usable after a failed read-back (`vello_backend.rs`, `map_read`)

wgpu 30 records a buffer as mapped as soon as `map_async` is called
(`wgpu-30.0.2/src/api/buffer.rs`, `BufferSlice::map_async`), before the result
is known. As published, the failure path returns without `unmap()`, so every
later read-back on that canvas fails at once. Now that path calls `unmap()`
first. When wgpu-core never mapped the buffer, `unmap()` reports a validation
error to the device's uncaptured-error handler, which logs it
(`VELLO WGPU ERROR`); it does not panic.

On this error path, and on the `get_mapped_range()` error path of change 1,
the callback receives `None`: `getImageData` and friends see transparent black,
and the canvas's displayed frame is zero-filled as well.

Drop it when Servo unmaps on this path.

## 5. Keep earlier drawing when a canvas is drawn again (`vello_backend.rs`, `render_to_texture`)

The GPU backend records drawing commands into a vello scene and renders the
scene only when pixels are needed (a read-back, `toDataURL`, or the frame
shown on screen). After rendering into the canvas's texture it empties the
scene. The next drawing command starts a new scene whose first item is the
canvas's previous frame: `ensure_drawing` draws `render_image`, an image that
`override_image` points at the canvas's own texture.

vello 0.11 caches images in its image atlas and copies an overridden texture
into the atlas only when the image is new or has been marked dirty
(`Renderer::mark_override_image_dirty`, whose documentation says that
otherwise "stale image data from the atlas may be used"). Servo's backend was
written against a vello without that cache and never marks the image. So the
atlas kept the first copy of the texture — blank — and every render after the
first composited new drawing over a blank previous frame: a page that draws,
reads back or shows a frame, and then draws more lost everything drawn before.

The fix marks `render_image` dirty after each successful render. This is the
use the API is meant for, not a workaround: it costs one GPU texture-to-texture
copy of the canvas into the atlas at the next render that draws on top of the
previous frame — it does not re-render the old scene. On a 1000×1000 canvas
drawn and read back 100 times the mean step was 2.2–2.8 ms with the fix and
2.1–3.1 ms without it (under heavy machine load), i.e. no measurable cost.

Drop it when Servo's backend marks the image itself.

## 6. Share one GPU device and renderer between canvases (`vello_backend.rs`, `SHARED_GPU`, `start`, `create_device_and_renderer`)

As published, every 2D canvas creates its own wgpu instance, device and vello
renderer, synchronously on the Canvas thread while the constellation (and so
every tab) waits. vello's renderer keeps its GPU buffers between renders
(`vello_encoding` `BufferSizes`: tiles, lines, segments and segment counts of
2^21 entries each, an 8 MiB-entry `ptcl`, a 1 MiB-entry blend spill), about
175 MB per renderer, so fifty 200×100 canvases took about 9 GB of process
memory on macOS and two hundred about 34 GB. Now the first GPU canvas on the
Canvas thread creates the device, queue and renderer and keeps them in a
thread-local; later canvases reuse them through `new_with_renderer`, the path
the crate already used for a resized canvas. Measured on this change: fifty
canvases about 0.46 GB and 0.07 s to create (the CPU backend: 0.28 GB,
0.06 s); two hundred about 0.55 GB. The shared renderer (about 175 MB) is kept
until the process exits, even after the last GPU canvas is gone.

Failure isolation changes with sharing: a lost device now affects every GPU
canvas, not one. A device-lost callback marks the device lost; the next canvas
created then logs `GPU 2D canvas unavailable (GPU device lost); …`, drops the
shared device and renderer, and is drawn on the CPU, as are all later canvases.
Canvases already on the lost device stay blank. wgpu calls the callback
synchronously when it sees a device error (`wgpu-core` `Device::lose`); if the
loss is first seen while waiting for a read-back, wgpu panics instead (see
"Known limitations").

One exception: every canvas's previous frame is drawn from its renderer's
image atlas, which is at most 8192×8192. Two canvases larger than 4096 on
either side cannot both live in one atlas, and when drawn in turns the
earlier drawing of one of them was lost (`tests/pages/canvas-parity.html`,
cases `two-large-alternating` and `two-large-after-resize`). Such a canvas,
whether created at that size or resized to it, therefore gets a renderer of
its own on the shared device (`renderer_for`), at about 175 MB each.

Drop it when Servo shares the device itself.

## 7. Apply non-rectangular clips (`vello_backend.rs`, `render_to_texture`)

The backend keeps a clip as an open vello clip layer until the page restores
it, and re-opens the active clips after every render. vello, however, applies
a clip layer's shape only when the layer is closed; a layer that is still open
when the scene is rendered clips to the shape's bounding box only. So any
read-back or displayed frame taken while a clip was active showed drawing
clipped to the bounding box: a circular clip came out square
(`tests/pages/canvas-parity.html`, case `clip-path`). Rectangular clips were
unaffected. Now the render runs inside `ignore_clips`, which closes the open
clip layers and re-opens them afterwards.

Drop it when Servo closes the clip layers itself.

## 8. Honour the fill rule of a clip (`vello_backend.rs`, `push_clip`, `clips`)

`push_clip` ignored its `fill_rule` argument and always clipped with the
non-zero rule, so `clip("evenodd")` clipped to the whole outer shape
(`tests/pages/canvas-parity.html`, case `clip-evenodd`). Now the rule is
converted with the crate's existing `FillRule` → `peniko::Fill` conversion,
used for the clip layer and stored with the clip path, so that the layer is
re-opened with the same rule after a render. (The CPU backend also clips
`evenodd` paths with the non-zero rule; not investigated, not changed.)

Drop it when Servo passes the fill rule itself.

## 9. Draw canvases larger than the GPU texture limit on the CPU (`vello_backend.rs`, `try_new`)

vello creates its device with wgpu's default limits, whose largest 2D texture
is 8192 pixels on a side. A larger canvas (for example 10000×50) got an
invalid texture: every draw and read-back failed with wgpu validation errors
and the canvas stayed blank (`tests/pages/canvas-parity.html`, case
`wide-10000-incremental`). Now such a canvas is created with the CPU backend,
and a GPU canvas resized beyond the limit moves to the CPU backend
(`Canvas::recreate` in `canvas_paint_thread.rs`, `CanvasData::recreate_as`,
which keeps the canvas's WebRender image key; cases `wide-10000-after-resize`
and `resize-past-limit-and-back`). A canvas that moved stays on the CPU when
it is made small again.

Drop it when Servo checks the size itself.

## 10. Draw shapes that have a shadow, without the shadow (`canvas_data.rs`, `draws_shadow`)

`CanvasData::fill_rect`, `stroke_rect` and `draw_image` (upstream
`canvas_data.rs:109`, `:188`, `:230`) send a shape with a visible shadow
through `draw_with_shadow` (`:377`), which draws the shape only into a
temporary target and passes that to `draw_surface_with_shadow` — a stub that
only logs `no support for drawing shadows` in both backends
(`vello_backend.rs:375`, `vello_cpu_backend.rs:294`). So the shape itself was
never drawn: a button, card or chart bar with a shadow vanished. Paths and
text ignore shadows and were drawn. Now `draws_shadow` returns false, so the
three calls take their normal path and the shape is drawn without its
shadow; one warning per process says shadows are not supported. Parity cases
`shadow-offset`, `shadow-blur`, `shadow-stroke-rect`, `shadow-drawimage`
(shape present, shadow absent as a known gap), `shadow-transparent-color`
and `shadow-zero-offset-blur` (no shadow due, exact).

Drop it when a backend implements `draw_surface_with_shadow`.

## Known limitations (accepted)

- A device lost while a read-back waits on the GPU panics inside
  `vello::util::block_on_wgpu` (`device.poll(..).unwrap()`) or wgpu's own
  `Device::poll`, which treats a lost device as fatal. That kills the Canvas
  thread and so every 2D canvas for the rest of the session.
- The CPU backend reads `rgba(0, 102, 204, 0.5)` back as `[0,101,203,128]`
  (premultiplied 8-bit storage, truncated when un-premultiplied); the GPU
  backend reads `[0,102,203,128]`. Both are within the ±1 tolerance of
  `tests/pages/canvas-readback.html`, the CPU one at its edge.

## Known differences between the backends (`tests/pages/canvas-parity.html`)

Left as they are; see the Task 4d report for details.

- GPU: gradients are evaluated about half a pixel off the pixel centre
  (`gradient-radial`: 255 and 127 where the specification gives 251 and
  130); inside vello, cosmetic.
- CPU: `clearRect` ignores the clip (`clip-then-clear`), `putImageData`
  blends instead of replacing (`putimagedata-over-drawing`), `evenodd` clips
  use the non-zero rule (`clip-evenodd`).
- Both: shapes drawn with a shadow are not drawn at all — neither backend
  implements `draw_surface_with_shadow` (`shadow-offset`, `shadow-blur`).
  Conic gradients and `OffscreenCanvas` are not available in this Servo
  configuration, and an image loaded from a `toBlob` blob URL fails to decode
  in both modes (an engine issue, not a canvas one).

## Removing this directory

Remove the `[patch.crates-io]` entry and this directory once a Servo release
compiles with the `vello` feature and contains equivalents of changes 2–9, or
once Mercury no longer needs them.
