# Third-party notices

This file records the projects that Mercury Browser's source code is derived from or copies code
from, together with their licences. Every file that contains copied or adapted code names its
source at the top and has a matching entry here. Vendored dependencies under `third_party/` keep
their own licence files.

- Hermes Browser — https://github.com/KaykCaputo/hermes-browser — MPL-2.0 — Mercury Browser is derived from this project at commit `a41f18e`.
- Servo `servo-canvas` 0.8.0 — https://github.com/servo/servo, commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas` — MPL-2.0 — vendored in `third_party/servo-canvas` with a small fix to its GPU (vello) backend; see `third_party/servo-canvas/MERCURY.md`.
- Zervo — https://github.com/Goddv/Zervo, commit `86cb71267066f3b1f402abbd5c26faaf24ddf165`, path `src/downloads.rs` — MPL-2.0 — the file-name sanitiser, URL-segment naming, unique naming and byte formatter (with their tests) are adapted in `crates/mercury-downloads/src/naming.rs` and `crates/mercury-downloads/tests/naming.rs`.
- Verso — https://github.com/versotile-org/verso, commit `eb719bdd6c7b4a22b640d33ea55c6d440a5a850d`, paths `src/utils/content_disposition_parser.rs` and `src/download.rs` — Apache-2.0 OR MIT (used under MIT; Copyright (c) 2020-2023 Ngo Iok Ui & Tauri Programme within The Commons Conservancy; the full MIT licence text and permission notice are in `LICENSES/MIT-verso.txt`) — the `Content-Disposition` parser is adapted in `crates/mercury-downloads/src/content_disposition.rs`, and the streaming write loop in `crates/mercury-downloads/src/transfer.rs`.
