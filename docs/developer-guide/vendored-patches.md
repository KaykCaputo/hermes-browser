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
| `third_party/servo-canvas` | 0.8.0 | Make Servo's GPU 2D-canvas backend (`vello` feature) build against wgpu 30 |

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

Servo 0.8.0's GPU 2D canvas (`vello` feature, preference `dom_canvas_backend = "vello"`) does not compile as published: in wgpu 30, which vello 0.11 requires, `BufferSlice::get_mapped_range()` returns a `Result`. The vendored copy handles the error in `VelloDrawTarget::map_read` the way that function already handles a failed `map_async`; `third_party/servo-canvas/MERCURY.md` describes the change. The first commit that added the directory holds the crate exactly as published, so `git diff` against it shows the whole patch.

Mercury draws 2D canvases on the GPU by default. `--disable-gpu-canvas` selects Servo's CPU backend instead. Check both modes with `tests/pages/canvas-readback.html`, whose read-back lines must all end in `PASS`:

```bash
./target/release/mercury-browser --screenshot gpu.png --window-size 1000x800 \
  "file://$PWD/tests/pages/canvas-readback.html"
./target/release/mercury-browser --disable-gpu-canvas --screenshot cpu.png --window-size 1000x800 \
  "file://$PWD/tests/pages/canvas-readback.html"
```

To confirm which backend drew, run with `RUST_LOG=vello=debug,wgpu_core=trace`. With the GPU canvas, a page that creates a 2D canvas logs a second `Created Device DeviceId(…)` (wgpu 30; Slint's wgpu 29 logs `Created Device Id(…)`) and many `vello` lines. With `--disable-gpu-canvas` only Slint's device appears and nothing mentions `vello`.

## Remove a patch

When an equivalent fix reaches the Servo dependency graph used by Mercury:

1. Update the pinned dependency version.
2. Remove the relevant `[patch.crates-io]` entry.
3. Regenerate `Cargo.lock` intentionally.
4. Run the original regression test and full QA.
5. Remove the vendored directory in the same commit.

The removal commit should explain which upstream release absorbed the patch.
