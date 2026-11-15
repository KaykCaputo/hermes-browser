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
Changes 2 and 3 are marked with `Mercury:` comments in the source.

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

## Removing this directory

Remove the `[patch.crates-io]` entry and this directory once a Servo release
compiles with the `vello` feature and contains equivalents of changes 2 and 3, or
once Mercury no longer needs them.
