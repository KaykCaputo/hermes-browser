# Third-party notices

This file records the projects that Mercury Browser's source code is derived from or copies code
from, together with their licences. The whole codebase derives from Hermes Browser, the upstream
base listed first; its files do not carry individual source headers. Every file that contains code
copied or adapted from any other project names that source at the top and has a matching entry
here. Vendored dependencies under `third_party/` keep their own licence files.

- Hermes Browser — https://github.com/KaykCaputo/hermes-browser — MPL-2.0 — Mercury Browser is derived from this project at commit `a41f18e`.
- Servo `servo-canvas` 0.8.0 — https://github.com/servo/servo, commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`, path `components/canvas` — MPL-2.0 — vendored in `third_party/servo-canvas` with a small fix to its GPU (vello) backend; see `third_party/servo-canvas/MERCURY.md`.
- Servo `python/servo/gstreamer.py` — https://github.com/servo/servo, tag `v0.8.0` (commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`), functions `package_gstreamer_dylibs` and `load_plugin_libraries_from_text_file` — Apache-2.0 OR MIT, Copyright 2013 The Servo Project Developers — adapted as `scripts/macos/collect-gstreamer.sh`.
- Servo `ports/servoshell/build.rs` — https://github.com/servo/servo, tag `v0.8.0` — MPL-2.0 — the macOS `@executable_path/lib/` rpath in `apps/mercury-desktop/build.rs`.
- Zervo `scripts/bundle-macos.sh` — https://github.com/Goddv/Zervo, commit `86cb7126` — MPL-2.0 — the `lipo` thinning step in `scripts/macos/collect-gstreamer.sh`.
- Slint Servo example — https://github.com/slint-ui/slint, tag `v1.17.1` (commit `cf62c975c311e7036d599ed8ed0b7e6a8386a934`), path `examples/servo/src/webview/rendering_context/metal.rs` — MIT, Copyright © SixtyFPS GmbH <info@slint.dev> — the macOS IOSurface → Metal frame import and its flip pass in `apps/mercury-desktop/src/rendering/metal.rs` are adapted from this file. The MIT text is in `LICENSES/MIT-slint-examples.txt`. `apps/mercury-desktop/src/rendering/surfman.rs` and `apps/mercury-desktop/src/rendering/vulkan.rs` follow the same example's `surfman_context.rs`, `gpu_rendering_context.rs` and `vulkan.rs` step for step.
- Servo `servo-paint-api` 0.8.0 — https://github.com/servo/servo, tag `v0.8.0` (commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`), path `components/shared/paint/rendering_context.rs` — MPL-2.0 — the macOS frame read-back in `apps/mercury-desktop/src/rendering/metal.rs` is adapted from `Framebuffer::read_framebuffer_to_image`; `SurfmanContext` in `apps/mercury-desktop/src/rendering/surfman.rs` follows Servo's `SurfmanRenderingContext`.
- Zervo — https://github.com/Goddv/Zervo, commit `86cb7126` — MPL-2.0 — `scripts/bundle-macos.sh` (deployment-target lookup, Info.plist keys, inside-out signing, disk image) adapted in `scripts/macos/bundle.sh` and `apps/mercury-desktop/resources/macos/Info.plist.in`; `scripts/make-icns.sh` adapted in `scripts/macos/make-icns.sh`; `assets/macos/entitlements.plist` adapted in `apps/mercury-desktop/resources/macos/entitlements.plist`.
- Servo 0.8.0 — https://github.com/servo/servo, tag `v0.8.0` — MPL-2.0 — `ports/servoshell/platform/macos/mod.rs` (the embedded `__TEXT,__info_plist` static) adapted in `apps/mercury-desktop/src/platform_macos.rs`; `ports/servoshell/platform/macos/Info.plist` adapted in `apps/mercury-desktop/resources/macos/Info.plist.in`; `support/macos/Servo.entitlements` adapted in `apps/mercury-desktop/resources/macos/entitlements.plist`.
- Servo 0.8.0 — https://github.com/servo/servo, tag `v0.8.0` — Apache-2.0 OR MIT — `etc/macos_sign.py` (hardened-runtime signing and `codesign --verify --deep --strict`) adapted in `scripts/macos/bundle.sh`.
- Servo servoshell `desktop/keyutils.rs` — https://github.com/servo/servo, tag `v0.8.0`, path `ports/servoshell/desktop/keyutils.rs` — MPL-2.0 — adapted in `apps/mercury-desktop/src/keyutils.rs` (winit → W3C key, code and location tables).
- Zephium — https://github.com/zephium-browser/Zephium — MPL-2.0 — commit `9026a1fffad74d6e201ef97fe8c02c1b01403cd2`. `crates/mercury-import/src/netscape.rs` adapts the Netscape bookmark writer from `crates/zephium-core/src/export.rs` (lines 28-92); `crates/mercury-import/src/{chromium.rs,firefox.rs,snapshot.rs}` adapt the browser-profile importers from `crates/zephium-import/src/{chromium.rs,firefox.rs,snapshot.rs}`.
- Zervo — https://github.com/Goddv/Zervo, commit `86cb71267066f3b1f402abbd5c26faaf24ddf165`, path `src/downloads.rs` — MPL-2.0 — the file-name sanitiser, URL-segment naming, unique naming and byte formatter (with their tests) are adapted in `crates/mercury-downloads/src/naming.rs` and `crates/mercury-downloads/tests/naming.rs`.
- Verso — https://github.com/versotile-org/verso, commit `eb719bdd6c7b4a22b640d33ea55c6d440a5a850d`, paths `src/utils/content_disposition_parser.rs` and `src/download.rs` — Apache-2.0 OR MIT (used under MIT; Copyright (c) 2020-2023 Ngo Iok Ui & Tauri Programme within The Commons Conservancy; the full MIT licence text and permission notice are in `LICENSES/MIT-verso.txt`) — the `Content-Disposition` parser is adapted in `crates/mercury-downloads/src/content_disposition.rs`, and the streaming write loop in `crates/mercury-downloads/src/transfer.rs`.
- Mozilla application-services — https://github.com/mozilla/application-services, commit `5f95eb6bafa36c4a86ff3109dacbf9026029833f`, path `components/places/src/frecency.rs` — MPL-2.0 — the frecency calculation is ported in `crates/mercury-store/src/frecency.rs`.
- Zephium — https://github.com/zephium-browser/Zephium, commit `9026a1fffad74d6e201ef97fe8c02c1b01403cd2`, paths `crates/zephium-store/src/hub/history.rs:617-640` (FTS5 query sanitiser) and `crates/zephium-store/src/migrations.rs:1944-1966` (FTS5 mirror and triggers) — MPL-2.0 — adapted in `crates/mercury-store/src/search.rs` and `crates/mercury-store/src/schema.rs`.
- Zervo — https://github.com/Goddv/Zervo, commit `86cb7126`, path `zervo-core/src/store.rs` (`write_private`, lines 80-123, and the ideas of its tests, lines 125-232) — MPL-2.0 — adapted in `crates/mercury-settings/src/atomic.rs` (exclusive, uniquely named temporary file; directory sync after the rename; see the file header). The settings patterns (a default for every field, first-run detection from a missing file, a migration test) and the starting list of search engines follow `src/settings.rs` at the same commit; no code from it is copied.
- Servo's servoshell 0.8.0 — https://github.com/servo/servo, tag `v0.8.0` (commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`), paths `ports/servoshell/panic_hook.rs`, `ports/servoshell/crash_handler.rs` and `ports/servoshell/backtrace.rs` — MPL-2.0 — adapted in `apps/mercury-desktop/src/panic_hook.rs` and `apps/mercury-desktop/src/crash_handler.rs`.
- Servo's servoshell 0.8.0 — https://github.com/servo/servo, tag `v0.8.0` (commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537`), path `ports/servoshell/prefs.rs` lines 29-53 (`EXPERIMENTAL_PREFS`, the list of experimental web-platform preferences) — MPL-2.0 — copied into `crates/talaria-servo/src/runtime.rs`.
- Zervo — https://github.com/Goddv/Zervo, commit `86cb71267066f3b1f402abbd5c26faaf24ddf165` — MPL-2.0 — adapted from its files of the same names: `rust-toolchain.toml`; `deny.toml`; `.github/workflows/ci.yml` (from `.github/workflows/check.yml` and `.github/workflows/macos.yml`); `.github/workflows/release.yml` (macOS signing and publishing, from `.github/workflows/macos.yml` and `.github/workflows/release.yml`); `scripts/ci/check-macos-bundle.sh` (from the "Check the bundle" step of `.github/workflows/macos.yml`).

## Bundled libraries (macOS application)

`Mercury Browser.app` for macOS carries GStreamer 1.28.9 and the libraries it needs as separate
dynamic libraries in `Contents/MacOS/lib`. They are not part of Mercury Browser's source code
and keep their own licences; the table below lists them.

- **Origin.** They are copied by `scripts/macos/collect-gstreamer.sh` from the official GStreamer
  1.28.9 macOS packages (https://gstreamer.freedesktop.org/data/pkg/osx/1.28.9/,
  `gstreamer-1.0-1.28.9-universal.pkg` and `gstreamer-1.0-devel-1.28.9-universal.pkg`, SHA-256 in
  `scripts/macos/gstreamer.sh`). **They are not modified, except** that each is thinned from a
  universal (x86_64 + arm64) file to its arm64 code with `lipo -thin`, and signed again with the
  application's signature by `scripts/macos/bundle.sh`. The machine code and install names are
  unchanged.
- **Dynamic linking and replacement.** Nothing from them is linked statically into
  `mercury-browser`. The executable loads `libgstreamer`, `libgstplay` and the other libraries it
  names through the rpath `@executable_path/lib/`, and Servo loads the GStreamer plugins at run
  time from the same directory. Any of them can be replaced by a compatible arm64 build with the
  same file name: copy it into `Contents/MacOS/lib`. Because the application is signed, changing
  a file invalidates its signature, so the application then has to be signed again, for example
  ad hoc:
  `codesign --force --sign - "<path to the library>"`, then
  `codesign --force --sign - --options runtime --entitlements apps/mercury-desktop/resources/macos/entitlements-adhoc.plist "Mercury Browser.app/Contents/MacOS/mercury-browser"`
  and the same for `"Mercury Browser.app"`. The ad-hoc entitlements turn library validation off,
  so libraries signed by someone else load. A Developer ID build has library validation on and
  loads only libraries signed by its own team; to use a replaced library with it, sign the whole
  application ad hoc as above (it then loses its Developer ID signature and notarisation).
- **Corresponding source.** Every bundled library was built by GStreamer's build system,
  cerbero, at commit `efda998b6e8032a8ed785a70000a63d337898f82` ("Build 1.28.9 release",
  https://gitlab.freedesktop.org/gstreamer/cerbero/-/tree/efda998b6e8032a8ed785a70000a63d337898f82);
  its `recipes/<project>.recipe` names each project's source, checksum and patches, and the
  patches themselves are in `recipes/<project>/` at that commit. The versions in the table below
  are those the packages record in `share/versions.txt`. For the LGPL projects:
  - GStreamer 1.28.9, built from these releases without patches:
    https://gstreamer.freedesktop.org/src/gstreamer/gstreamer-1.28.9.tar.xz,
    https://gstreamer.freedesktop.org/src/gst-plugins-base/gst-plugins-base-1.28.9.tar.xz,
    https://gstreamer.freedesktop.org/src/gst-plugins-good/gst-plugins-good-1.28.9.tar.xz,
    https://gstreamer.freedesktop.org/src/gst-plugins-bad/gst-plugins-bad-1.28.9.tar.xz and
    https://gstreamer.freedesktop.org/src/gst-libav/gst-libav-1.28.9.tar.xz.
  - GLib 2.82.4: https://download.gnome.org/sources/glib/2.82/glib-2.82.4.tar.xz (the recipe's SHA-256
    `37dd0877fe964cd15e9a2710b044a1830fb1bd93652a6d0cb6b8b2dff187c709`), **with 14 patches** from
    cerbero's `recipes/glib/` (GIO module paths, platform build fixes).
  - FFmpeg 7.1: https://ffmpeg.org/releases/ffmpeg-7.1.tar.xz (the recipe's SHA-256
    `40973d44970dbc83ef302b0609f2e74982be2d85916dd2ee7472d30678a7abe6`), **with the patches** in
    cerbero's `recipes/ffmpeg/` (among them a Meson build). The bundled build records
    `-Dnonfree=disabled -Dversion3=disabled` and reports its licence as "LGPL version 2.1 or later".
  - libnice 0.1.24 (LGPL-2.1-or-later or MPL-1.1):
    https://libnice.freedesktop.org/releases/libnice-0.1.24.tar.gz (the recipe's SHA-256
    `cfb5e8e778534f2f5b3c6f4958a1eb057c6b95c537c0f100817a537cf5d64fcc`), without patches.
  - proxy-libintl 0.5: https://github.com/frida/proxy-libintl/archive/refs/tags/0.5.tar.gz
    (the recipe's SHA-256 `f7a1cbd7579baaf575c66f9d99fb6295e9b0684a28b095967cfda17857595303`), with one patch
    from cerbero's `recipes/proxy-libintl/`.

  The other bundled projects are under permissive licences (see the table) with no source
  obligation.
- **Licence texts.** The LGPL version 2.1 is in `LICENSES/LGPL-2.1.txt` (the LGPL 2.1 permits use
  under any later version, which covers the "LGPL-2.0-or-later" projects too). Each project's
  own licence files, as the packages ship them under `share/licenses/<project>/`, are copied into
  `Contents/Resources/gstreamer-licenses/<project>/` of the application.

Bundled libraries by project (generated from the collected set; the collector fails if this
table does not match what it collected):

<!-- gstreamer-libraries:begin (generated by scripts/macos/gstreamer-notices.sh; do not edit) -->
| Project (cerbero recipe) | Version | Licence | Bundled files |
|---|---|---|---|
| bzip2 | 1.0.8 | bzip2-1.0.6 | `libbz2.1.dylib` |
| ffmpeg | 7.1 | LGPL-2.1-or-later | `libavcodec.61.dylib`, `libavfilter.10.dylib`, `libavformat.61.dylib`, `libavutil.59.dylib`, `libswresample.5.dylib`, `libswscale.8.dylib` |
| glib | 2.82.4 | LGPL-2.0-or-later | `libgio-2.0.0.dylib`, `libglib-2.0.0.dylib`, `libgmodule-2.0.0.dylib`, `libgobject-2.0.0.dylib` |
| graphene | 1.10.8 | MIT | `libgraphene-1.0.0.dylib` |
| gst-libav-1.0 | 1.28.9 | LGPL-2.1-or-later | `libgstlibav.dylib` |
| gst-plugins-bad-1.0 | 1.28.9 | LGPL-2.0-or-later (usrsctp in libgstsctp: BSD-3-Clause) | `libgstapplemedia.dylib`, `libgstaudiobuffersplit.dylib`, `libgstcodecparsers-1.0.0.dylib`, `libgstdtls.dylib`, `libgstid3tag.dylib`, `libgstplay-1.0.0.dylib`, `libgstproxy.dylib`, `libgstsctp-1.0.0.dylib`, `libgstvideoparsersbad.dylib`, `libgstvulkan-1.0.0.dylib`, `libgstwebrtc-1.0.0.dylib`, `libgstwebrtc.dylib`, `libgstwebrtcnice-1.0.0.dylib` |
| gst-plugins-base-1.0 | 1.28.9 | LGPL-2.0-or-later | `libgstapp-1.0.0.dylib`, `libgstapp.dylib`, `libgstaudio-1.0.0.dylib`, `libgstaudioconvert.dylib`, `libgstaudioresample.dylib`, `libgstfft-1.0.0.dylib`, `libgstgio.dylib`, `libgstgl-1.0.0.dylib`, `libgstopengl.dylib`, `libgstopus.dylib`, `libgstpbutils-1.0.0.dylib`, `libgstplayback.dylib`, `libgstriff-1.0.0.dylib`, `libgstrtp-1.0.0.dylib`, `libgstsdp-1.0.0.dylib`, `libgsttag-1.0.0.dylib`, `libgsttheora.dylib`, `libgsttypefindfunctions.dylib`, `libgstvideo-1.0.0.dylib`, `libgstvideoconvertscale.dylib`, `libgstvolume.dylib`, `libgstvorbis.dylib` |
| gst-plugins-good-1.0 | 1.28.9 | LGPL-2.1-or-later | `libgstaudiofx.dylib`, `libgstaudioparsers.dylib`, `libgstautodetect.dylib`, `libgstdeinterlace.dylib`, `libgstid3demux.dylib`, `libgstinterleave.dylib`, `libgstisomp4.dylib`, `libgstmatroska.dylib`, `libgstosxaudio.dylib`, `libgstosxvideo.dylib`, `libgstrtp.dylib`, `libgstrtpmanager.dylib`, `libgstvideofilter.dylib`, `libgstvpx.dylib`, `libgstwavparse.dylib` |
| gstreamer-1.0 | 1.28.9 | LGPL-2.0-or-later | `libgstbase-1.0.0.dylib`, `libgstcontroller-1.0.0.dylib`, `libgstcoreelements.dylib`, `libgstnet-1.0.0.dylib`, `libgstreamer-1.0.0.dylib` |
| libffi | 3.2.9999.5 | MIT | `libffi.7.dylib` |
| libjpeg-turbo | 3.1.0 | IJG AND BSD-3-Clause AND Zlib | `libjpeg.8.3.2.dylib` |
| libnice | 0.1.24 | LGPL-2.1-or-later OR MPL-1.1 | `libgstnice.dylib`, `libnice.10.dylib` |
| libogg | 1.3.5 | BSD-3-Clause | `libogg.0.dylib` |
| libpng | 1.6.56 | libpng-2.0 | `libpng16.16.56.0.dylib` |
| libtheora | 1.1.1 | BSD-3-Clause | `libtheoradec.1.dylib`, `libtheoraenc.1.dylib` |
| libvorbis | 1.3.7 | BSD-3-Clause | `libvorbis.0.dylib`, `libvorbisenc.2.dylib` |
| libvpx | v1.14.1 | BSD-3-Clause (with the WebM patent grant) | `libvpx.9.dylib` |
| moltenvk | 1.3.283.0 | Apache-2.0 | `libMoltenVK.dylib` |
| openssl | 3.5.0 | Apache-2.0 | `libcrypto.3.dylib`, `libssl.3.dylib` |
| opus | 1.5.2 | BSD-3-Clause | `libopus.0.dylib` |
| orc | 0.4.42 | BSD-2-Clause | `liborc-0.4.0.dylib` |
| pcre2 | 10.42 | BSD-3-Clause WITH PCRE2-exception | `libpcre2-8.0.dylib` |
| proxy-libintl | 0.5 | LGPL-2.0-or-later | `libintl.8.dylib` |
| zlib | 1.3.1 | Zlib | `libz.1.dylib` |
<!-- gstreamer-libraries:end -->

## Bundled data

The filter lists in `crates/talaria-servo/resources/adblock/` are data files read by the content
blocker, redistributed unmodified from the Brave mirror snapshot commit named in
`crates/talaria-servo/resources/adblock/README.md`; they are not part of Mercury Browser's source
code and keep their own licences.

- EasyList — The EasyList authors, https://easylist.to/ (source repository https://github.com/easylist/easylist) — local file `easylist.txt` — dual licensed under the GNU General Public License version 3 or (at your option) any later version, and Creative Commons Attribution-ShareAlike 3.0 Unported or (at your option) any later version; "The EasyList authors (https://easylist.to/)" should be attributed as the source. Stated at https://easylist.to/pages/licence.html, the page named by the list's `! Licence:` header. Texts: `LICENSES/GPL-3.0.txt`, `LICENSES/CC-BY-SA-3.0.txt`.
- EasyPrivacy — The EasyList authors, https://easylist.to/ (source repository https://github.com/easylist/easylist) — local file `easyprivacy.txt` — same licence and attribution as EasyList, stated at https://easylist.to/pages/licence.html, the page named by the list's `! Licence:` header. Texts: `LICENSES/GPL-3.0.txt`, `LICENSES/CC-BY-SA-3.0.txt`.
- uBlock filters — uBlock Origin uAssets, https://github.com/uBlockOrigin/uAssets (`filters/filters.txt`) — local file `ublock-filters.txt` — GNU General Public License version 3, the text in the repository's `LICENSE` file (read at commit `e140ffc75aee8c1af136f579126389d6defae5ed`), which the list's `! License:` header names. Text: `LICENSES/GPL-3.0.txt`.
- Brave Firstparty specific filters — Brave, https://github.com/brave/adblock-lists (`brave-lists/brave-firstparty.txt`) — local file `brave-firstparty.txt` — MPL-2.0, the repository's `LICENSE` file (read at commit `a190fe07fe27b697dc796d54a54b58dfd75e7b1c`); the list itself has no licence line. Text: `LICENSE`.
- Brave Specific — Brave, https://github.com/brave/adblock-lists (`brave-lists/brave-specific.txt`) — local file `brave-specific.txt` — MPL-2.0, the repository's `LICENSE` file (read at commit `a190fe07fe27b697dc796d54a54b58dfd75e7b1c`); the list itself has no licence line. Text: `LICENSE`.
- Brave unbreak list (its only line is `! Brave desktop filters`) — Brave, https://github.com/brave/adblock-lists (`brave-lists/brave-unbreak.txt`) — local file `brave-unbreak.txt` — MPL-2.0, the repository's `LICENSE` file (read at commit `a190fe07fe27b697dc796d54a54b58dfd75e7b1c`); the list itself has no licence line. Text: `LICENSE`.
