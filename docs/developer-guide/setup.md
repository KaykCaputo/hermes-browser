---
title: Development Setup
description: Prepare a local Hermes Browser development environment on Linux or Windows.
---

# Development setup

Linux is the primary development environment because it exercises the full Servo, Vulkan/WGPU composition, GStreamer, and packaging path.

## Clone and prepare the repository

```bash
git clone https://github.com/KaykCaputo/hermes-browser.git
cd hermes-browser
make install-linux-deps
make setup
```

`make setup` uses the current stable Rust toolchain, installs formatting and lint components, and fetches dependencies with `--locked`.

## Check your tools

```bash
make doctor
```

The command reports the active Rust toolchain and verifies that Cargo, Rust, and rustup are available.

Servo also builds native code. On Linux, Hermes expects Clang, CMake, a C/C++ toolchain, Fontconfig, X11 development libraries, and the GStreamer development stack installed by `make install-linux-deps`.

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
Hermes code, but it is intended for diagnosis rather than performance evaluation.

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

## Windows prerequisites

Use 64-bit Windows 10 or 11 with:

- The `x86_64-pc-windows-msvc` Rust toolchain
- Visual Studio 2022 Build Tools with MSVC v143
- Windows SDK 10.0.19041.0 or newer
- C++ ATL for the v143 x64 toolchain

Current Windows builds do not bundle native GStreamer, so they do not exercise the complete Linux media path.
