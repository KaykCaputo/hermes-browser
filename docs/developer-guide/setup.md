---
title: Development Setup
description: Prepare a local Mercury Browser development environment on Linux, macOS or Windows.
---

# Development setup

Linux is the primary development environment because it exercises the full Servo, Vulkan/WGPU composition, GStreamer, and packaging path.

## Clone and prepare the repository

```bash
git clone https://github.com/thepigdestroyer/mercury-browser.git
cd mercury-browser
make install-linux-deps
make setup
```

`make setup` uses the current stable Rust toolchain, installs formatting and lint components, and fetches dependencies with `--locked`.

## Check your tools

```bash
make doctor
```

The command reports the active Rust toolchain and verifies that Cargo, Rust, and rustup are available.

Servo also builds native code. On Linux, Mercury expects Clang, CMake, a C/C++ toolchain, Fontconfig, X11 development libraries, and the GStreamer development stack installed by `make install-linux-deps`.

## Build and run

```bash
make build
make run-debug
```

Pass application arguments through `ARGS` and adjust logging through `RUST_LOG`:

```bash
make run-debug ARGS="--example-option" RUST_LOG="talaria_servo=debug,servo=info"
```

Use the default optimized runtime when investigating timing or media behavior:

```bash
make run
```

`make run-release` is retained as an alias for `make run`. The development profile still optimizes
major Servo, WebRender, Vello, and ad-blocking crates while retaining useful debug information for
Mercury code, but it is intended for diagnosis rather than performance evaluation.

## Build the documentation

Create a Python virtual environment so the documentation toolchain does not affect system packages:

=== "Linux/macOS"

    ```bash
    python3 -m venv .venv-docs
    source .venv-docs/bin/activate
    python -m pip install -r docs/requirements.txt
    ```

=== "Windows (PowerShell)"

    ```powershell
    py -m venv .venv-docs
    .venv-docs\Scripts\Activate.ps1
    python -m pip install -r docs/requirements.txt
    ```

Preview the site with live reload:

```bash
make docs-serve
```

Build it with strict link and navigation checks:

```bash
make docs
```

## macOS media set-up (GStreamer)

On macOS, Servo's media backend uses the official GStreamer framework packages from
gstreamer.freedesktop.org. Mercury does not install them system-wide and needs no `sudo`:
`scripts/macos/gstreamer.sh` downloads the runtime and development packages, checks their
SHA-256 (a download that fails the check is deleted), and unpacks them into
`~/Library/Caches/mercury-browser/gstreamer/<version>/` (override with `MERCURY_GSTREAMER_CACHE`).
The pinned version and checksums are at the top of that script.

```bash
make gstreamer-macos      # fetch, unpack, check
make doctor               # also checks the GStreamer prefix on macOS
```

Disk space (measured): the two packages take 0.9 GiB, the unpacked framework 4.0 GiB, so the
cache holds **4.9 GiB** afterwards; while `unpack` runs it needs up to **9.7 GiB**, because the
expanded development package and the assembled framework exist at the same time. Downloading
took about a minute here and unpacking between 1 and 11 minutes, depending on disk load.

The Makefile's build, check, lint, test and run targets load the build environment themselves
and stop if it cannot be set up (so they never build against another GStreamer that
`pkg-config` happens to find). For a plain shell, use:

```bash
gst_env="$(scripts/macos/gstreamer.sh env)" && eval "$gst_env"   # MERCURY_GSTREAMER_PREFIX, PKG_CONFIG, PKG_CONFIG_LIBDIR, PATH
cargo build --release -p mercury-desktop
scripts/macos/collect-gstreamer.sh "$MERCURY_GSTREAMER_PREFIX" target/release/lib
```

Every cargo command that compiles Servo needs that environment, including `cargo check` from an
editor.

Notes:

- The executable links GStreamer directly and finds it through the rpath
  `@executable_path/lib/`, so `target/<profile>/lib` must exist before it starts; without it the
  program stops in the dynamic loader. `collect-gstreamer.sh` fills it: the libraries the
  executable links, the plugins `scripts/macos/gstreamer-plugins.sh` names (Servo's list from the
  resolved `servo` crate, minus the excluded plugins), and their dependencies, thinned to the
  executable's architecture. A marker records what was collected; an intact, up-to-date directory
  is not copied again, a damaged one is rebuilt, and a directory without a marker is never
  replaced. `make run`, `make run-debug`, `make build`, `make build-release` and `make test` call it.
- **Ogg is left out** (`DEFAULT_EXCLUDED` in `scripts/macos/gstreamer-plugins.sh`; see the user
  guide). `MERCURY_GSTREAMER_EXCLUDE_PLUGINS` overrides the list for one run (`""` = none). If the
  set of collected files changes, the collector stops until the library table in
  `THIRD-PARTY-NOTICES.md` is regenerated: rerun it with `MERCURY_GSTREAMER_UPDATE_NOTICES=1` and
  commit the table.
- The first launch after a fresh copy is slow (2 to 10 seconds before media is ready) while macOS
  checks each new library; later launches load the plugins in about 0.3 seconds. Users see the
  same delay on their first launch after installing or updating Mercury. Several worktrees can
  share one collected directory through a symlink (`ln -s <shared lib> target/release/lib`);
  it stays warm across them.
- Test binaries have no rpath to the libraries. `make test` builds the executable, collects
  `target/<profile>/lib` and runs the tests with `DYLD_FALLBACK_LIBRARY_PATH` pointing at that
  directory, on the `cargo test` command line itself: macOS removes `DYLD_*` variables when it
  starts SIP-protected programs such as `make` and `/bin/sh`, so setting it in a parent shell
  script or in `make` does not work. Use `make test CARGO_FLAGS=--release` (likewise `check` and
  `lint`) for release-mode runs.
- **Where GStreamer and GLib write.** At the top of `main()`,
  `apps/mercury-desktop/src/media_isolation.rs` points GStreamer's registry and plugin paths at
  the application's own files, and `apps/mercury-desktop/src/media_buffers.rs` creates a private,
  locked run directory under `<dirs::cache_dir()>/io.github.thepigdestroyer.mercury-browser/media-buffers/`
  and points `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_CONFIG_HOME` and `XDG_RUNTIME_DIR` into it,
  so GStreamer's download buffers (copies of played media) land there. It is removed at a normal
  quit, and leftovers of killed runs at the next start. `scripts/macos/test-media.sh` plays the
  media page with a scratch `HOME` and fails if anything appears outside that directory.
- **Expected start-up warning while Ogg is excluded.** Every launch logs
  `WARN servo::servo::media_platform] Error initializing GStreamer: ErrorLoadingPlugins(["libgstogg.dylib"]). Trying fallback initialization.`
  Servo loads its plugins by name and treats the missing Ogg plugin as an error; it then starts
  the backend through its fallback, with the other plugins it had already loaded. Media works.
  A real failure looks different: more than `libgstogg.dylib` in that list (a damaged or
  incomplete `lib`), `dyld: Library not loaded: @rpath/libgst…` before anything else (no `lib`
  at all), or `canPlayType('video/mp4; codecs="avc1.42E01E"')` answering `''` in a page.
- `make test-media-macos` runs `scripts/macos/test-media.sh`, the tests for these scripts.
- `tests/pages/media.html` checks playback without making a sound; serve it with
  `scripts/serve-tests.py` (a loopback server that honours `Range`); see `tests/media/README.md`.

## Windows prerequisites

Use 64-bit Windows 10 or 11 with:

- The `x86_64-pc-windows-msvc` Rust toolchain
- Visual Studio 2022 Build Tools with MSVC v143
- Windows SDK 10.0.19041.0 or newer
- C++ ATL for the v143 x64 toolchain

Current Windows builds do not bundle native GStreamer, so they do not exercise the complete Linux media path.
