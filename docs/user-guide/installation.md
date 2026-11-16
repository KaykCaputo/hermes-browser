---
title: Install Mercury
description: Install Mercury Browser on macOS from the disk image, or build and run it on Linux with the required Rust, graphics, and GStreamer dependencies.
---

# Install Mercury

## macOS (Apple Silicon)

Mercury for macOS runs on Macs with Apple silicon (M1 or later) and macOS 13 Ventura or later. It is distributed as a disk image named like `mercury-browser-0.2.0-macos-arm64.dmg`.

### Install from the disk image

1. Optionally, check the download against the `.sha256` file published next to it:

    ```bash
    shasum -a 256 -c mercury-browser-0.2.0-macos-arm64.dmg.sha256
    ```

2. Double-click the `.dmg` to open it.
3. Drag **Mercury Browser** onto the **Applications** folder shown next to it.
4. Eject the disk image.

### The first launch: allowing Mercury past Gatekeeper

Current Mercury builds are not signed with an Apple Developer ID or notarised by Apple; they carry only an "ad-hoc" signature. macOS marks everything downloaded from the internet, and Gatekeeper blocks a marked app that Apple has not checked. The first time you open Mercury Browser, macOS refuses and offers only **Done** or **Move to Trash**. Choose **Done**, then allow it once:

1. Open **System Settings** and choose **Privacy & Security**.
2. Scroll down to the **Security** section. It shows a line saying that "Mercury Browser" was blocked to protect your Mac.
3. Click **Open Anyway** and confirm with your password or Touch ID.
4. Open Mercury Browser again and click **Open** in the dialog that follows.

macOS remembers the choice; later launches open normally. The button only appears for about an hour after the blocked attempt, so try opening the app again if it is missing.

If macOS instead reports that the app "is damaged and can't be opened", it has blocked it without offering **Open Anyway**. The app is not damaged. If you trust where you downloaded it from, remove the download mark in Terminal and open it again:

```bash
xattr -dr com.apple.quarantine "/Applications/Mercury Browser.app"
```

Only do either of these for a copy you downloaded from the project's own release page.

### Uninstall

Quit Mercury Browser and move `/Applications/Mercury Browser.app` to the Trash. Mercury's media cache, if you want it gone too, is `~/Library/Caches/io.github.thepigdestroyer.mercury-browser`.

### Build the app yourself

With the Xcode command-line tools and Rust installed, run `make gstreamer-macos` once (it downloads GStreamer, about 0.9 GB, and unpacks it into your user cache without `sudo`; see [Development setup](../developer-guide/setup.md#macos-media-set-up-gstreamer)). Then `make app-macos` builds `dist/Mercury Browser.app` from source, with audio and video, and `make package-macos` also makes the disk image. An app you build yourself is not marked as downloaded, so Gatekeeper does not block it. See [Releases](../developer-guide/releases.md#package-for-macos).

## Linux

The fastest supported route is a source build on Debian or Ubuntu. The repository includes a command that installs the native packages needed by Servo, Slint, graphics, and GStreamer.

### Before you begin

You need:

- A 64-bit Linux system
- Git
- A current stable Rust toolchain managed by `rustup`
- `sudo` access to install native packages
- A working X11 or Wayland desktop session

Packaged Linux builds target glibc 2.35 or newer.

### Build from source

```bash
git clone https://github.com/thepigdestroyer/mercury-browser.git
cd mercury-browser
make install-linux-deps
make setup
make run
```

The first build compiles Servo and its native dependencies. It can take considerably longer than later builds and needs several gigabytes of free disk space.

### What the setup commands do

`make install-linux-deps`
: Installs the compiler, Fontconfig, X11 support, input libraries, GStreamer development headers, and common GStreamer plugin sets on apt-based distributions.

`make setup`
: Checks Rust tooling, installs `rustfmt` and Clippy, then downloads the locked Cargo dependency graph.

`make run`
: Builds and starts the optimized desktop application with useful default logging.

### Run an optimized build

`make run` already uses the optimized profile so that complex pages and media behave like the
packaged application. `make run-release` remains an equivalent alias:

```bash
make run-release
```

Developers who need complete debug information can use `make run-debug`.

### Other Linux distributions

`make install-linux-deps` supports apt-based systems. On another distribution, install equivalent packages for:

- A C and C++ build toolchain, CMake, Clang, and `pkg-config`
- Fontconfig
- X11, XCB, Xcursor, and XKB Common X11
- GStreamer core, app, audio, video, WebRTC, and development headers
- GStreamer Base, Good, Bad, Ugly, libav, and PulseAudio plugins

Package names vary between distributions. Once `pkg-config` can find the required GStreamer modules, continue with `make setup` and `make run`.

### Verify GStreamer

If the browser starts but media does not play, check the runtime before debugging Mercury:

```bash
gst-inspect-1.0 playbin3
gst-inspect-1.0 autoaudiosink
gst-inspect-1.0 avdec_h264
```

All three commands should describe a plugin instead of reporting that it is missing.

## Windows status

Mercury has a native Windows source-build configuration, but the project does not currently publish Windows release artifacts. Native GStreamer and its plugins are not bundled, so Windows builds use Servo's fallback media backend. Linux remains the complete supported path for the preview.

