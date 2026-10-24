---
title: Install Hermes
description: Build and run Hermes Browser on Linux, with the required Rust, graphics, and GStreamer dependencies.
---

# Install Hermes

The fastest supported route is a source build on Debian or Ubuntu. The repository includes a command that installs the native packages needed by Servo, Slint, graphics, and GStreamer.

## Before you begin

You need:

- A 64-bit Linux system
- Git
- A current stable Rust toolchain managed by `rustup`
- `sudo` access to install native packages
- A working X11 or Wayland desktop session

Packaged Linux builds target glibc 2.35 or newer.

## Build from source

```bash
git clone https://github.com/KaykCaputo/hermes-browser.git
cd hermes-browser
make install-linux-deps
make setup
make run
```

The first build compiles Servo and its native dependencies. It can take considerably longer than later builds and needs several gigabytes of free disk space.

## What the setup commands do

`make install-linux-deps`
: Installs the compiler, Fontconfig, X11 support, input libraries, GStreamer development headers, and common GStreamer plugin sets on apt-based distributions.

`make setup`
: Checks Rust tooling, installs `rustfmt` and Clippy, then downloads the locked Cargo dependency graph.

`make run`
: Builds and starts the optimized desktop application with useful default logging.

## Run an optimized build

`make run` already uses the optimized profile so that complex pages and media behave like the
packaged application. `make run-release` remains an equivalent alias:

```bash
make run-release
```

Developers who need complete debug information can use `make run-debug`.

## Other Linux distributions

`make install-linux-deps` supports apt-based systems. On another distribution, install equivalent packages for:

- A C and C++ build toolchain, CMake, Clang, and `pkg-config`
- Fontconfig
- X11, XCB, Xcursor, and XKB Common X11
- GStreamer core, app, audio, video, WebRTC, and development headers
- GStreamer Base, Good, Bad, Ugly, libav, and PulseAudio plugins

Package names vary between distributions. Once `pkg-config` can find the required GStreamer modules, continue with `make setup` and `make run`.

## Verify GStreamer

If the browser starts but media does not play, check the runtime before debugging Hermes:

```bash
gst-inspect-1.0 playbin3
gst-inspect-1.0 autoaudiosink
gst-inspect-1.0 avdec_h264
```

All three commands should describe a plugin instead of reporting that it is missing.

## Windows status

Hermes has a native Windows build configuration, but current release archives do not bundle GStreamer and its plugins. Windows builds therefore use Servo's fallback media backend. Windows is useful for development, but Linux remains the complete supported path for the preview.

