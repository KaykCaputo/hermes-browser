# Third-party notices

This file records the projects that Mercury Browser's source code is derived from or copies code
from, together with their licences. Every file that contains copied or adapted code names its
source at the top and has a matching entry here. Vendored dependencies under `third_party/` keep
their own licence files.

- Hermes Browser — https://github.com/KaykCaputo/hermes-browser — MPL-2.0 — Mercury Browser is derived from this project at commit `a41f18e`.
- Servo `servo-canvas` 0.8.0 — https://github.com/servo/servo, commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas` — MPL-2.0 — vendored in `third_party/servo-canvas` with a small fix to its GPU (vello) backend; see `third_party/servo-canvas/MERCURY.md`.
- Zervo — https://github.com/Goddv/Zervo, commit `86cb7126` — MPL-2.0 — `scripts/bundle-macos.sh` (deployment-target lookup, Info.plist keys, inside-out signing, disk image) adapted in `scripts/macos/bundle.sh` and `apps/mercury-desktop/resources/macos/Info.plist.in`; `scripts/make-icns.sh` adapted in `scripts/macos/make-icns.sh`; `assets/macos/entitlements.plist` adapted in `apps/mercury-desktop/resources/macos/entitlements.plist`.
- Servo 0.8.0 — https://github.com/servo/servo, tag `v0.8.0` — MPL-2.0 — `ports/servoshell/platform/macos/mod.rs` (the embedded `__TEXT,__info_plist` static) adapted in `apps/mercury-desktop/src/platform_macos.rs`; `ports/servoshell/platform/macos/Info.plist` adapted in `apps/mercury-desktop/resources/macos/Info.plist.in`; `support/macos/Servo.entitlements` adapted in `apps/mercury-desktop/resources/macos/entitlements.plist`.
- Servo 0.8.0 — https://github.com/servo/servo, tag `v0.8.0` — Apache-2.0 OR MIT — `etc/macos_sign.py` (hardened-runtime signing and `codesign --verify --deep --strict`) adapted in `scripts/macos/bundle.sh`.
