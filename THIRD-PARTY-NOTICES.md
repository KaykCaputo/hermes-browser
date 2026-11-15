# Third-party notices

This file records the projects that Mercury Browser's source code is derived from or copies code
from, together with their licences. Every file that contains copied or adapted code names its
source at the top and has a matching entry here. Vendored dependencies under `third_party/` keep
their own licence files.

- Hermes Browser — https://github.com/KaykCaputo/hermes-browser — MPL-2.0 — Mercury Browser is derived from this project at commit `a41f18e`.
- Servo `servo-canvas` 0.8.0 — https://github.com/servo/servo, commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas` — MPL-2.0 — vendored in `third_party/servo-canvas` with a small fix to its GPU (vello) backend; see `third_party/servo-canvas/MERCURY.md`.
- Slint Servo example — https://github.com/slint-ui/slint, tag `v1.17.1` (commit `cf62c975c311e7036d599ed8ed0b7e6a8386a934`), path `examples/servo/src/webview/rendering_context/metal.rs` — MIT, Copyright © SixtyFPS GmbH <info@slint.dev> — the macOS IOSurface → Metal frame import and its flip pass in `apps/mercury-desktop/src/rendering/metal.rs` are adapted from this file. The MIT text is in `LICENSES/MIT-slint-examples.txt`. `apps/mercury-desktop/src/rendering/surfman.rs` and `apps/mercury-desktop/src/rendering/vulkan.rs` follow the same example's `surfman_context.rs`, `gpu_rendering_context.rs` and `vulkan.rs` step for step.
- Servo `servo-paint-api` 0.8.0 — https://github.com/servo/servo, path `components/shared/paint/rendering_context.rs` — MPL-2.0 — the macOS frame read-back in `apps/mercury-desktop/src/rendering/metal.rs` is adapted from `Framebuffer::read_framebuffer_to_image`; `SurfmanContext` in `apps/mercury-desktop/src/rendering/surfman.rs` follows Servo's `SurfmanRenderingContext`.
