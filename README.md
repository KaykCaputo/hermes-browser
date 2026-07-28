<p align="center">
  <img src="apps/hermes-desktop/resources/hermes-browser.svg" width="128" alt="Hermes Browser winged sandal logo">
</p>

<h1 align="center">Hermes Browser</h1>

<p align="center">
  An experimental desktop browser built with Rust, Servo, and Slint.
</p>

> [!WARNING]
> Hermes Browser is an early engineering prototype. It is not yet suitable for daily use or for handling sensitive data. Privacy protections such as Talaria Guard are planned, but are not implemented yet.

## About

Hermes Browser explores how a lightweight, privacy-oriented desktop browser can be built around the [Servo Web Engine](https://servo.org/) while keeping the browser shell independent from engine internals.

The product is named after Hermes, the Greek messenger associated with speed, travel, and communication. Its browser core is called **Talaria**, after Hermes' winged sandals.

The project is also a practical Rust learning environment. The code favors explicit state, small modules, typed errors, event-driven communication, and testable boundaries.

## Current capabilities

- Servo-powered HTML, CSS, and JavaScript rendering
- Asynchronous networking and script loading
- GPU-accelerated WebRender, Canvas 2D, and browser interface
- Native ad and tracker blocking powered by Brave's adblock-rust engine
- Slint desktop interface
- Responsive browser viewport
- Multiple tabs with one Servo WebView per tab
- Background-tab throttling
- Address navigation and DuckDuckGo search
- Back, forward, and reload navigation
- Mouse, wheel, keyboard, and IME input
- Page titles, favicons, and loading state
- Native light-weight new tab page
- Linux (X11 and Wayland) and Windows build configuration

Keyboard shortcuts currently include:

| Shortcut | Action |
| --- | --- |
| `Ctrl+L` | Focus the address bar |
| `Ctrl+T` | Open a new tab |
| `Ctrl+W` | Close the active tab |
| `Ctrl+Tab` | Select the next tab |
| `Ctrl+Shift+Tab` | Select the previous tab |
| `Ctrl+R` or `F5` | Reload the page |
| `Alt+Left` | Go back |
| `Alt+Right` | Go forward |

## Architecture

```text
Hermes UI (Slint)
        |
        | commands and presentation state
        v
Talaria Core
        |
        | WebView contract and browser events
        v
Talaria Servo
        |
        v
Servo Web Engine
```

The workspace currently contains:

```text
apps/
  hermes-desktop/   Desktop executable and application composition

crates/
  hermes-ui/        Slint components and presentation types
  talaria-core/     Tabs, browser state, commands, events, and policies
  talaria-servo/    The only crate coupled directly to Servo
```

`talaria-core` can be tested with fake WebViews and does not need to start Servo. UI code does not depend on Servo's internal types.

## Requirements

Use a current stable Rust toolchain. The project uses the Rust 2024 edition.

### Linux

The Linux build uses Slint's Winit backend with X11 and Wayland support. The development helper prepares the X11 toolchain and native libraries with:

```bash
make install-linux-deps
```

This helper currently supports Debian- and Ubuntu-based distributions. Other distributions need equivalent window-system, Fontconfig, compiler, and `pkg-config` packages.

### Windows

Build on 64-bit Windows 10 or 11 with:

- Rust using the `x86_64-pc-windows-msvc` toolchain
- Visual Studio 2022 Build Tools with MSVC v143
- Windows 10 or 11 SDK version 10.0.19041.0 or newer
- C++ ATL for the v143 x64 toolchain

See Servo's [Windows build requirements](https://book.servo.org/building/windows.html) when preparing a development machine.

## Getting started

```bash
git clone <repository-url>
cd hermes-browser
make setup
make run
```

`make setup` installs the Rust formatting and linting components and downloads locked dependencies. Run `make help` to list every available command.

Common commands:

```bash
make check          # Type-check the workspace
make test           # Run all tests
make qa             # Formatting, Clippy, and tests
make build          # Debug build
make build-release  # Optimized build
make run            # Run a debug build
make run-release    # Run an optimized build
```

Logs can be customized without editing the Makefile:

```bash
make run RUST_LOG="talaria_servo=debug,servo=warn"
```

## Release packages

Create a Linux x86-64 archive and SHA-256 checksum locally:

```bash
make package-linux
```

The files are written to `dist/`.

Linux and Windows release packages are also built on native GitHub-hosted runners. Push a version tag to create a GitHub Release:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The workflow can also be started manually from the repository's **Actions** page. A manual run keeps both packages as workflow artifacts without creating a GitHub Release.

The release workflow produces:

- `hermes-browser-linux-x86_64.tar.gz`
- `hermes-browser-linux-x86_64.tar.gz.sha256`
- `hermes-browser-windows-x86_64.zip`
- `hermes-browser-windows-x86_64.zip.sha256`

The Linux executable is dynamically linked and expects compatible system libraries. Release artifacts are currently intended for testing, not as universal installers.

## Planned work

- Talaria Guard advertisement and tracker blocking
- Tab suspension and memory-pressure discarding
- Session, history, bookmark, and settings persistence
- Downloads, permissions, and site controls
- Media and daily-use Web API validation
- Native Linux and Windows installers
- Broader compatibility testing and Servo upstream contributions

## Quality policy

Before a change is considered ready:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

The same checks are available through `make qa`.

## Known limitations

- Web compatibility is limited by Servo's current implementation.
- Complex pages can expose engine-level crashes or rendering defects.
- Only one browser window is supported.
- Background tabs are throttled but are not yet suspended or discarded.
- Ad and tracker blocking, persistence, downloads, and extensions are not implemented.
- Linux packages currently target x86-64 and depend on compatible system libraries.

When a page fails in the engine, reproduce it in Servo's `servoshell` before maintaining a Hermes-specific engine patch.

## License

No open-source license has been selected yet. Until a license file is added, the source code remains under its authors' default copyright.
