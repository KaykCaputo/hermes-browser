# Patched servo-canvas 0.8.0

This directory vendors `servo-canvas` 0.8.0, Servo's 2D-canvas paint thread and
its drawing backends, under its original MPL-2.0 license. It is the crate
exactly as published on crates.io (checksum
`318945d504987838d404bb7dce122e0ad17afeaf02ebd3d3a3167b739b73db7d`), built from
https://github.com/servo/servo at commit
`4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas`.

Mercury enables Servo's `vello` feature, the GPU 2D-canvas backend. As
published, that backend does not compile: vello 0.11 requires wgpu 30, where
`BufferSlice::get_mapped_range()` returns `Result<BufferView, MapRangeError>`
instead of a `BufferView`.

Mercury's one change, in `VelloDrawTarget::map_read` in `vello_backend.rs`:
when `get_mapped_range()` fails, the error is logged at `warn` and the
callback receives `None` — the same handling the function already applies
when `map_async` fails, which makes `getImageData` and friends see a
transparent-black image. Unlike that earlier failure, the buffer is mapped at
this point, so the function still reaches its `unmap()`. The successful path
is unchanged.

Remove the `[patch.crates-io]` entry and this directory once a Servo release
compiles with the `vello` feature.
