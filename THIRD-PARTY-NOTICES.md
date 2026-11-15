# Third-party notices

This file records the projects that Mercury Browser's source code is derived from or copies code
from, together with their licences. Every file that contains copied or adapted code names its
source at the top and has a matching entry here. Vendored dependencies under `third_party/` keep
their own licence files.

- Hermes Browser — https://github.com/KaykCaputo/hermes-browser — MPL-2.0 — Mercury Browser is derived from this project at commit `a41f18e`.
- Servo `servo-canvas` 0.8.0 — https://github.com/servo/servo, commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas` — MPL-2.0 — vendored in `third_party/servo-canvas` with a small fix to its GPU (vello) backend; see `third_party/servo-canvas/MERCURY.md`.
- Zervo — https://github.com/Goddv/Zervo, commit `86cb71267066f3b1f402abbd5c26faaf24ddf165` — MPL-2.0 — adapted from its files of the same names: `rust-toolchain.toml`; `deny.toml`; `.github/workflows/ci.yml` (from `.github/workflows/check.yml` and `.github/workflows/macos.yml`); `.github/workflows/release.yml` (macOS signing and publishing, from `.github/workflows/macos.yml` and `.github/workflows/release.yml`); `scripts/ci/check-macos-bundle.sh` (from the "Check the bundle" step of `.github/workflows/macos.yml`).
