# Third-party notices

This file records the projects that Mercury Browser's source code is derived from or copies code
from, together with their licences. Every file that contains copied or adapted code names its
source at the top and has a matching entry here. Vendored dependencies under `third_party/` keep
their own licence files.

- Hermes Browser — https://github.com/KaykCaputo/hermes-browser — MPL-2.0 — Mercury Browser is derived from this project at commit `a41f18e`.
- Servo `servo-canvas` 0.8.0 — https://github.com/servo/servo, commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas` — MPL-2.0 — vendored in `third_party/servo-canvas` with a small fix to its GPU (vello) backend; see `third_party/servo-canvas/MERCURY.md`.
- Mozilla application-services — https://github.com/mozilla/application-services, commit `5f95eb6bafa36c4a86ff3109dacbf9026029833f`, path `components/places/src/frecency.rs` — MPL-2.0 — the frecency calculation is ported in `crates/mercury-store/src/frecency.rs`.
- Zephium — https://github.com/zephium-browser/Zephium, commit `9026a1fffad74d6e201ef97fe8c02c1b01403cd2`, paths `crates/zephium-store/src/hub/history.rs:617-640` (FTS5 query sanitiser) and `crates/zephium-store/src/migrations.rs:1944-1966` (FTS5 mirror and triggers) — MPL-2.0 — adapted in `crates/mercury-store/src/search.rs` and `crates/mercury-store/src/schema.rs`.
