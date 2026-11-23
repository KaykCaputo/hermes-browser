---
title: Vendored Patches
description: Maintain and eventually remove Hermes' focused gaol and Surfman compatibility patches.
---

# Vendored patches

Hermes vendors code when a pinned Servo dependency needs a small, reviewable compatibility fix that cannot be expressed through the public API. Vendoring is a temporary maintenance cost, not a general fork strategy.

## Patch inventory

| Directory | Upstream version | Why Hermes carries it |
| --- | --- | --- |
| `third_party/servo-canvas` | 0.8.0 | Read mapped GPU buffers with the WGPU 30 API |
| `third_party/servo-net` | 0.8.0 | Accept larger HTTP/2 response headers, request the compatible YouTube player, and skip unused inspector metadata |
| `third_party/gaol` | 0.2.1 | Modern Linux syscall compatibility and a non-zero file-size limit for Servo shared-memory IPC |
| `third_party/surfman` | 0.14.0 | Avoid process-global `DRI_PRIME=1` selection and its race with media initialization |

The root `[patch.crates-io]` section redirects Servo's dependency graph to these directories.

## Rules for changing a patch

1. Reproduce the failure against the exact upstream version.
2. Keep the diff limited to the behavior Hermes needs.
3. Add a regression test when the behavior can be tested locally.
4. Update the directory's `HERMES.md` or `README.md` with the reason for the patch.
5. Preserve the upstream license files and attribution.
6. Prefer a fix that can be proposed upstream without Hermes-specific concepts.

Avoid opportunistic formatting, dependency upgrades, or unrelated cleanup inside a vendored tree. They make future comparison and removal needlessly difficult.

## Validate the gaol patch

```bash
cargo test \
  --target-dir target \
  --manifest-path third_party/gaol/Cargo.toml \
  --test shared-memory
```

The separate target directory avoids leaving an untracked build tree inside the vendored crate.

Media uses Servo's upstream GStreamer backend without a local patch. The release smoke test checks the packaged executable with visible video frames and captured audio. Live streaming sites also need periodic manual checks.

## Remove a patch

When an equivalent fix reaches the Servo dependency graph used by Hermes:

1. Update the pinned dependency version.
2. Remove the relevant `[patch.crates-io]` entry.
3. Regenerate `Cargo.lock` intentionally.
4. Run the original regression test and full QA.
5. Remove the vendored directory in the same commit.

The removal commit should explain which upstream release absorbed the patch.
