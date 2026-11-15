# Third-party notices

This file records the projects that Mercury Browser's source code is derived from or copies code
from, together with their licences. The whole codebase derives from Hermes Browser, the upstream
base listed first; its files do not carry individual source headers. Every file that contains code
copied or adapted from any other project names that source at the top and has a matching entry
here. Vendored dependencies under `third_party/` keep their own licence files.

- Hermes Browser — https://github.com/KaykCaputo/hermes-browser — MPL-2.0 — Mercury Browser is derived from this project at commit `a41f18e`.
- Servo `servo-canvas` 0.8.0 — https://github.com/servo/servo, commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas` — MPL-2.0 — vendored in `third_party/servo-canvas` with a small fix to its GPU (vello) backend; see `third_party/servo-canvas/MERCURY.md`.
- Servo's servoshell 0.8.0 — https://github.com/servo/servo, tag `v0.8.0` (commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`), paths `ports/servoshell/panic_hook.rs`, `ports/servoshell/crash_handler.rs` and `ports/servoshell/backtrace.rs` — MPL-2.0 — adapted in `apps/mercury-desktop/src/panic_hook.rs` and `apps/mercury-desktop/src/crash_handler.rs`.
