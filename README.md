<p align="center">
  <img src="apps/hermes-desktop/resources/hermes-browser.svg" width="112" alt="Hermes Browser winged sandal logo">
</p>

<h1 align="center">Hermes Browser</h1>

<p align="center">
  <strong>A focused, independent browser powered by Rust and Servo.</strong>
</p>

<p align="center">
  Fast by design. Private by default. Built without the weight of a traditional browser stack.
</p>

<p align="center">
  <img src="docs/screenshots/hermes-new-tab.png" width="100%" alt="Hermes Browser new tab page">
</p>

> [!IMPORTANT]
> Hermes is an early technical preview. It is ready for experimentation and development, but not yet for daily use or sensitive browsing.

> [!WARNING]
> Web content currently runs in the main browser process without operating-system sandboxing. Servo 0.4.0 can start isolated content processes, but its embedded cross-process Paint path does not display their rendered output yet. Hermes keeps that path disabled until it can render pages reliably.

## A lighter path to the web

Hermes is a desktop browser that combines the memory safety of Rust, the parallel architecture of the [Servo Web Engine](https://servo.org/), and a lightweight native interface built with [Slint](https://slint.dev/).

The project is designed around three principles:

| Performance | Privacy | Independence |
| --- | --- | --- |
| Asynchronous loading, GPU rendering, and aggressive background-tab throttling. | Native network-level ad and tracker blocking using Brave filter snapshots. | A browser shell with clean boundaries around Servo instead of deep engine coupling. |

Hermes takes its name from the Greek messenger associated with speed and travel. Its browser core is called **Talaria**, after Hermes' winged sandals.

## See Hermes in action

<p align="center">
  <img src="docs/screenshots/hermes-servo.png" width="100%" alt="Servo website rendered in Hermes Browser with multiple tabs">
</p>

<p align="center">
  <em>Real web content rendered by Servo inside the Hermes tab and navigation experience.</em>
</p>

## Highlights

- **Servo-powered browsing** — modern web content through an embeddable Rust engine
- **GPU-accelerated experience** — WebRender, Vello Canvas 2D, and a native OpenGL interface
- **Native Linux multimedia** — HTML audio, video, Web Audio, and WebRTC through GStreamer
- **Native content blocking** — ads and trackers are intercepted before network resources are loaded
- **Responsive tabs** — one dedicated Servo WebView per tab with background throttling
- **Fast navigation** — unified address and search input with DuckDuckGo search
- **Desktop input** — mouse, wheel, keyboard, shortcuts, and IME composition
- **Live page identity** — titles, favicons, navigation history, and loading feedback
- **Cross-platform foundation** — Linux support through X11 and Wayland, plus Windows build configuration

## Technology

```text
Hermes UI · Slint
        │
        │  commands and presentation state
        ▼
Talaria Core · Rust
        │
        │  engine-independent WebView contract
        ▼
Talaria Servo
        │
        ▼
Servo Web Engine
```

| Component | Responsibility |
| --- | --- |
| `apps/hermes-desktop` | Application lifecycle, desktop window, and runtime composition |
| `crates/hermes-ui` | Native Slint interface and presentation types |
| `crates/talaria-core` | Tabs, browser state, commands, events, and navigation policies |
| `crates/talaria-servo` | Servo integration, OpenGL rendering, Linux multimedia, metrics, and content blocking |

The browser core can be tested with fake WebViews without starting Servo. UI code remains independent from Servo's internal types, keeping the product easier to evolve and the engine easier to upgrade.

### Runtime compatibility patches

Hermes currently carries focused patches for three dependencies used by Servo 0.4.0:

- `gaol`, to support current Linux system calls and allow Servo's shared-memory IPC regions to be sized correctly;
- `servo-media-gstreamer`, for codec detection, portable Linux video frames, Web Audio, and media lifecycle fixes;
- `surfman`, to avoid process-global GPU selection through `DRI_PRIME`.

Each vendored directory documents why the patch exists and when it can be removed. These patches prepare the isolated content-process path, but do not enable it while cross-process rendering remains broken.

## Quick start

Hermes uses the current stable Rust toolchain and the Rust 2024 edition.

```bash
git clone https://github.com/KaykCaputo/hermes-browser.git
cd hermes-browser
make install-linux-deps
make setup
make run
```

`make install-linux-deps` currently supports Debian- and Ubuntu-based Linux distributions. Other distributions need equivalent compiler, Fontconfig, X11, Wayland, GStreamer development libraries, and the Base, Good, Bad, Ugly, and libav GStreamer plugin sets.

On Linux, Hermes lets Mesa and DRM select the system's default GPU automatically and ignores an inherited `DRI_PRIME` override. No per-GPU launch configuration is required. Multimedia support depends on the GStreamer plugins installed by `make install-linux-deps`.

For an optimized runtime build:

```bash
make run-release
```

### Windows

Build on 64-bit Windows 10 or 11 with:

- the `x86_64-pc-windows-msvc` Rust toolchain;
- Visual Studio 2022 Build Tools with MSVC v143;
- Windows 10 or 11 SDK 10.0.19041.0 or newer;
- C++ ATL for the v143 x64 toolchain.

See the [Servo Windows build requirements](https://book.servo.org/building/windows.html) for environment preparation details.

## Everyday commands

```bash
make check          # Type-check every workspace target and feature
make test           # Run the complete test suite
make qa             # Formatting, Clippy, and tests
make build          # Build the debug workspace
make build-release  # Build the optimized workspace
make run            # Run the development build
make run-release    # Run the optimized build
make package-linux  # Create a Linux archive and checksum
```

Runtime logging can be customized without changing project files:

```bash
make run RUST_LOG="talaria_servo=debug,servo=warn"
```

## Keyboard shortcuts

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

## Release builds

Linux and Windows packages are built on native GitHub-hosted runners. Version tags create a GitHub Release automatically:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The release workflow produces:

- `hermes-browser-linux-x86_64.tar.gz` with a SHA-256 checksum;
- `hermes-browser-windows-x86_64.zip` with a SHA-256 checksum.

The Linux executable is built on Ubuntu 22.04 and targets glibc 2.35 or newer.
The workflow verifies this compatibility baseline before publishing the package.

Manual workflow runs keep both packages as downloadable Actions artifacts without publishing a release. Current packages are intended for testing and are not universal installers.

## Roadmap

- Package the native GStreamer runtime with Windows releases
- Add permission prompts, downloads, native form controls, and site information
- Persist sessions, history, bookmarks, cookies, and settings
- Introduce multiprocess execution, sandboxing, and crash recovery once Servo's embedded cross-process Paint path is usable
- Add per-site content-blocking controls and cosmetic filtering
- Suspend and discard tabs under memory pressure
- Expand automated compatibility testing and upstream Servo contributions
- Ship signed native installers and an update channel

## Project status

The current version is an engineering preview with known limitations:

- Web compatibility follows Servo's current implementation maturity.
- Linux builds use Servo's production GStreamer backend for multimedia playback. Codec availability still depends on the installed GStreamer plugins.
- Direct MP4/WebM playback is supported on Linux. YouTube remains incompatible: its current web player does not select its progressive fallback in this Servo revision, while its adaptive path requires Media Source Extensions (`MediaSource`/`SourceBuffer`), which the pinned Servo revision does not implement.
- Windows builds still use Servo's fallback media backend until the release archive includes the native GStreamer runtime and plugins.
- Web content currently runs in-process without operating-system sandboxing. Servo 0.4.0 can load pages in isolated content processes, but its embedded cross-process Paint path does not yet display their rendered content, so Hermes keeps that experimental path disabled.
- Persistence, downloads, permission prompts, extensions, and private profiles are not implemented yet.
- Background tabs are throttled but are not yet suspended or discarded.
- Linux packages currently target x86-64, glibc 2.35 or newer, and compatible system libraries.

When a page exposes an engine-level failure, reproduce it in Servo's `servoshell` before maintaining a Hermes-specific engine patch.

## Contributing

Hermes is a practical space for exploring browser engineering in Rust. Focused bug reports, compatibility reproductions, tests, documentation improvements, and well-scoped pull requests are welcome.

Before submitting a change, run:

```bash
make qa
```

## License

Hermes Browser's original source code and project assets are available under the [Mozilla Public License 2.0](LICENSE).

The MPL-2.0 allows commercial use and combination with code under other licenses while requiring modifications to MPL-covered files to remain available under the same license. Third-party dependencies and bundled filter lists remain subject to their respective licenses and notices.
