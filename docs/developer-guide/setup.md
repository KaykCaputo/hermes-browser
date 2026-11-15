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
SHA-256, and unpacks them into `~/Library/Caches/mercury-browser/gstreamer/<version>/`
(override with `MERCURY_GSTREAMER_CACHE`). The pinned version and checksums are at the top of
that script.

```bash
make gstreamer-macos      # fetch (about 970 MB), unpack (about 4 GB), check
```

The Makefile's build, lint, test and run targets load the build environment themselves. For a
plain shell, use:

```bash
eval "$(scripts/macos/gstreamer.sh env)"   # PKG_CONFIG, PKG_CONFIG_LIBDIR, PATH, MERCURY_GSTREAMER_ROOT
cargo build --release -p mercury-desktop
scripts/macos/collect-gstreamer.sh "$MERCURY_GSTREAMER_ROOT" target/release/lib
```

Notes:

- The executable links GStreamer directly and finds it through the rpath
  `@executable_path/lib/`, so `target/<profile>/lib` must exist before it starts.
  `collect-gstreamer.sh` fills it: the libraries the executable links, the plugins listed in the
  resolved `servo` crate's `gstreamer_plugin_lists/`, and their dependencies, thinned to the
  executable's architecture. It skips the copy when the directory is up to date. `make run`,
  `make run-debug`, `make build` and `make build-release` call it.
- The first launch after a fresh copy is slow (about 10 seconds before media is ready) while
  macOS checks each new library; later launches load the plugins in about 0.3 seconds.
- Test binaries have no rpath to the libraries. Run them with
  `DYLD_FALLBACK_LIBRARY_PATH="$MERCURY_GSTREAMER_ROOT/lib"` on the `cargo test` command line
  itself, as `make test` does. Setting it in a parent shell script or in `make` does not work,
  because macOS removes `DYLD_*` variables when it starts SIP-protected programs such as `make`
  and `/bin/sh`.
- `tests/pages/media.html` checks playback without making a sound; see `tests/media/README.md`.

## Windows prerequisites

Use 64-bit Windows 10 or 11 with:

- The `x86_64-pc-windows-msvc` Rust toolchain
- Visual Studio 2022 Build Tools with MSVC v143
- Windows SDK 10.0.19041.0 or newer
- C++ ATL for the v143 x64 toolchain

Current Windows builds do not bundle native GStreamer, so they do not exercise the complete Linux media path.
