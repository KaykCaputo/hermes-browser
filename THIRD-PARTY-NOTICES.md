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
- **Corresponding source.** GStreamer 1.28.9:
  https://gstreamer.freedesktop.org/src/gstreamer/gstreamer-1.28.9.tar.xz,
  https://gstreamer.freedesktop.org/src/gst-plugins-base/gst-plugins-base-1.28.9.tar.xz,
  https://gstreamer.freedesktop.org/src/gst-plugins-good/gst-plugins-good-1.28.9.tar.xz,
  https://gstreamer.freedesktop.org/src/gst-plugins-bad/gst-plugins-bad-1.28.9.tar.xz and
  https://gstreamer.freedesktop.org/src/gst-libav/gst-libav-1.28.9.tar.xz. Every other library
  was built by GStreamer's build system, cerbero, at commit
  `efda998b6e8032a8ed785a70000a63d337898f82` ("Build 1.28.9 release",
  https://gitlab.freedesktop.org/gstreamer/cerbero/-/tree/efda998b6e8032a8ed785a70000a63d337898f82);
  its `recipes/<project>.recipe` names each project's source URL, checksum and patches. The
  versions below are those the packages record in `share/versions.txt`. FFmpeg, for example, is
  https://ffmpeg.org/releases/ffmpeg-7.1.tar.xz (SHA-256
  `40973d44970dbc83ef302b0609f2e74982be2d85916dd2ee7472d30678a7abe6`) with the patches in
  cerbero's `recipes/ffmpeg/`. The bundled build records `-Dnonfree=disabled -Dversion3=disabled`
  and reports its licence as "LGPL version 2.1 or later".
- **Licence texts.** The LGPL version 2.1 is in `LICENSES/LGPL-2.1.txt` (the LGPL 2.1 permits use
  under any later version, which covers the "LGPL-2.0-or-later" projects too). Each project's
  own licence files, as the packages ship them under `share/licenses/<project>/`, are copied into
  `Contents/Resources/licenses/<project>/` of the application.

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
