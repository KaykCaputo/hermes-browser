# Third-party notices

This file records the projects that Mercury Browser's source code is derived from or copies code
from, together with their licences. Every file that contains copied or adapted code names its
source at the top and has a matching entry here. Vendored dependencies under `third_party/` keep
their own licence files.

- Hermes Browser — https://github.com/KaykCaputo/hermes-browser — MPL-2.0 — Mercury Browser is derived from this project at commit `a41f18e`.
- Servo `servo-canvas` 0.8.0 — https://github.com/servo/servo, commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas` — MPL-2.0 — vendored in `third_party/servo-canvas` with a small fix to its GPU (vello) backend; see `third_party/servo-canvas/MERCURY.md`.
- Servo `python/servo/gstreamer.py` — https://github.com/servo/servo, tag `v0.8.0` (commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`), functions `package_gstreamer_dylibs` and `load_plugin_libraries_from_text_file` — Apache-2.0 OR MIT, Copyright 2013 The Servo Project Developers — adapted as `scripts/macos/collect-gstreamer.sh`.
- Servo `ports/servoshell/build.rs` — https://github.com/servo/servo, tag `v0.8.0` — MPL-2.0 — the macOS `@executable_path/lib/` rpath in `apps/mercury-desktop/build.rs`.
- Zervo `scripts/bundle-macos.sh` — https://github.com/Goddv/Zervo, commit `86cb7126` — MPL-2.0 — the `lipo` thinning step in `scripts/macos/collect-gstreamer.sh`.
- GStreamer 1.28.9 macOS framework packages — https://gstreamer.freedesktop.org/data/pkg/osx/1.28.9/ — GStreamer, GLib, FFmpeg (LGPL build) and libnice are LGPL; other bundled libraries (OpenSSL, libvpx, libogg, libvorbis, libtheora, opus, zlib, bzip2, libffi, pcre2, libpng, libjpeg-turbo, graphene, orc, MoltenVK and others) carry their own licences, whose texts are in the development package under `share/licenses/`. Not part of the source tree: `scripts/macos/collect-gstreamer.sh` copies the unmodified dynamic libraries next to the macOS executable.
