---
title: Vendored Patches
description: Maintain and eventually remove Mercury's focused gaol, Surfman and servo-canvas compatibility patches.
---

# Vendored patches

Mercury vendors code when a pinned Servo dependency needs a small, reviewable compatibility fix that cannot be expressed through the public API. Vendoring is a temporary maintenance cost, not a general fork strategy.

## Patch inventory

| Directory | Upstream version | Why Mercury carries it |
| --- | --- | --- |
| `third_party/gaol` | 0.2.1 | Modern Linux syscall compatibility and a non-zero file-size limit for Servo shared-memory IPC |
| `third_party/surfman` | 0.14.0 | Avoid process-global `DRI_PRIME=1` selection and its race with media initialization |
| `third_party/servo-canvas` | 0.8.0 | Make Servo's GPU 2D-canvas backend (`vello` feature) build against wgpu 30, fall back to the CPU backend when it cannot start, survive failed renders and read-backs, share one GPU device, and fix GPU drawing defects (lost drawing, clips, oversized canvases, shapes with shadows) |

The gaol and Surfman patches were introduced by [Hermes Browser](https://github.com/KaykCaputo/hermes-browser), the project Mercury is derived from, and Mercury continues to carry them. Mercury added the servo-canvas patch.

The root `[patch.crates-io]` section redirects Servo's dependency graph to these directories.

## Rules for changing a patch

1. Reproduce the failure against the exact upstream version.
2. Keep the diff limited to the behavior Mercury needs.
3. Add a regression test when the behavior can be tested locally.
4. Update the directory's `MERCURY.md` or `README.md` with the reason for the patch.
5. Preserve the upstream license files and attribution.
6. Prefer a fix that can be proposed upstream without Mercury-specific concepts.

Avoid opportunistic formatting, dependency upgrades, or unrelated cleanup inside a vendored tree. They make future comparison and removal needlessly difficult.

## Validate the gaol patch

```bash
cargo test \
  --target-dir target \
  --manifest-path third_party/gaol/Cargo.toml \
  --test shared-memory
```

The separate target directory avoids leaving an untracked build tree inside the vendored crate.

Media uses Servo's upstream GStreamer backend without a local patch. It still needs a manual playback check because unit tests cannot prove that the host's plugins, decoder selection, audio sink, frame transfer, and isolated content process work together.

## The servo-canvas patch

Servo 0.8.0's GPU 2D canvas (`vello` feature, preference `dom_canvas_backend = "vello"`) does not compile as published: in wgpu 30, which vello 0.11 requires, `BufferSlice::get_mapped_range()` returns a `Result`. The vendored copy handles the error in `VelloDrawTarget::map_read` the way that function already handles a failed `map_async`.

The vendored copy also makes the GPU backend robust:

- **CPU fallback.** When a canvas cannot get a GPU adapter or device, or vello's renderer cannot be created, that canvas is drawn by the CPU backend (the one `--disable-gpu-canvas` selects). One warning, `GPU 2D canvas unavailable (<reason>); drawing 2D canvases on the CPU`, is logged per process, and later canvases go straight to the CPU backend without trying the GPU again.
- **Failed renders** are logged once (`VELLO RENDER ERROR …`) instead of panicking Servo's Canvas thread, which would leave the session without any 2D canvas.
- **Failed read-backs** unmap the buffer, so the next read-back on that canvas can succeed.
- **Shared GPU resources.** All GPU canvases share one wgpu device and one vello renderer (whose buffers take about 175 MB); a canvas larger than 2048×2048 in area gets a renderer of its own, and one created or resized beyond what vello can draw (8192 pixels on a side, or more than 256 bins of 256×256 pixels) is drawn on the CPU. If the shared device is lost — which blanks the GPU canvases of every tab — canvases created or resized afterwards are drawn on the CPU (this path has only been exercised by simulation).
- **Correctness fixes** found with `tests/pages/canvas-parity.html`: earlier drawing is kept when a canvas is drawn again, non-rectangular and `evenodd` clips are applied (including through a full-canvas `clearRect`), and shapes drawn with a shadow appear (without the shadow, which neither backend implements; in both backends).

**Costs of the GPU canvas** (measured on an M-series Mac under load, Task 4d; figures in `third_party/servo-canvas/MERCURY.md`):

- The shared renderer holds about 175 MB of GPU memory from the first GPU canvas until the process exits. Fifty 200×100 canvases: about 0.45 GB process footprint against 0.28 GB on the CPU; two hundred: about 0.56 GB against 0.36 GB.
- **Displaying a GPU canvas is a read-back.** WebRender receives canvases as CPU pixels, so every frame that changes a GPU canvas is rendered, copied back from the GPU and un-premultiplied on the Canvas thread — the same work as `getImageData`, `toDataURL` or `drawImage` from a canvas. Up to about 3 megapixels an animated canvas runs as fast on the GPU as on the CPU (52.9 against 57.1 frames per second for a full 2360×1304 window; 51.2 against 46.1 at 800×600); above that the GPU is about 35–40 % slower (3200×1800: 28.5 against 44.1).

`third_party/servo-canvas/MERCURY.md` lists each change, why it is needed and when it can be dropped. The first commit that added the directory holds the crate exactly as published, so `git diff 60ee59f -- third_party/servo-canvas` shows the whole patch.

Mercury draws 2D canvases on the GPU by default. `--disable-gpu-canvas` selects Servo's CPU backend instead. Check both modes with `tests/pages/canvas-readback.html`, whose read-back lines must all end in `PASS`:

```bash
./target/release/mercury-browser --screenshot gpu.png --window-size 1000x800 \
  "file://$PWD/tests/pages/canvas-readback.html"
./target/release/mercury-browser --disable-gpu-canvas --screenshot cpu.png --window-size 1000x800 \
  "file://$PWD/tests/pages/canvas-readback.html"
```

To confirm which backend drew, run with `RUST_LOG=vello=debug,wgpu_core=trace`. When the GPU canvas draws, a page that creates a 2D canvas logs vello's labelled GPU resources and pipelines, for example `Device::create_buffer("vello.scene")`, `vello.flatten` and `vello.fine_area`. With `--disable-gpu-canvas` no `vello.` label appears. After the CPU fallback warning, no further `vello.` resources are created (a failed renderer creation may log some pipeline labels just before the warning). A secondary signal is the device id format: vello's wgpu 30 logs `Created Device DeviceId(…)`, while Slint's wgpu 29 logs `Created Device Id(…)`.

`tests/pages/canvas-parity.html` draws one case per small canvas (paths, line styles, transforms, clips, compositing, gradients, patterns, shadows, text, `drawImage`, image data, drawing over time, serialisation, unusual sizes) and lists the pixels it reads back with the values the specification gives. It also puts the list on one line in the document title, which the window title log carries out of the browser:

```bash
RUST_LOG=warn,winit::window=debug ./target/release/mercury-browser --screenshot gpu.png \
  --window-size 1400x1600 --screenshot-delay 2500 "file://$PWD/tests/pages/canvas-parity.html" 2>&1 \
  | grep -o 'title="PARITY-BEGIN .* PARITY-END' | perl -pe 's/ ;; /\n/g'
```

Run it in both modes and compare line by line: `i` samples must be identical, `e` samples may differ by 2 per channel, `b` text boxes by 1 pixel. The differences known today are listed in `third_party/servo-canvas/MERCURY.md`.

`tests/pages/canvas-many.html` creates 50 canvases (`?n=200` for more), reads a pixel back from each and prints the timings; use it to compare the two backends' cost of creating canvases.

## Remove a patch

When an equivalent fix reaches the Servo dependency graph used by Mercury:

1. Update the pinned dependency version.
2. Remove the relevant `[patch.crates-io]` entry.
3. Regenerate `Cargo.lock` intentionally.
4. Run the original regression test and full QA.
5. Remove the vendored directory in the same commit.

The removal commit should explain which upstream release absorbed the patch.
