# Third-party notices

This file records the projects that Mercury Browser's source code is derived from or copies code
from, together with their licences. Every file that contains copied or adapted code names its
source at the top and has a matching entry here. Vendored dependencies under `third_party/` keep
their own licence files.

- Hermes Browser — https://github.com/KaykCaputo/hermes-browser — MPL-2.0 — Mercury Browser is derived from this project at commit `a41f18e`.
- Servo `servo-canvas` 0.8.0 — https://github.com/servo/servo, commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas` — MPL-2.0 — vendored in `third_party/servo-canvas` with a small fix to its GPU (vello) backend; see `third_party/servo-canvas/MERCURY.md`.
- Zephium — https://github.com/zephium-browser/Zephium — MPL-2.0 — commit `9026a1fffad74d6e201ef97fe8c02c1b01403cd2`. `crates/mercury-import/src/netscape.rs` adapts the Netscape bookmark writer from `crates/zephium-core/src/export.rs` (lines 28-92); `crates/mercury-import/src/{chromium.rs,firefox.rs,snapshot.rs}` adapt the browser-profile importers from `crates/zephium-import/src/{chromium.rs,firefox.rs,snapshot.rs}`.
