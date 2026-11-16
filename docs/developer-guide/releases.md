---
title: Releases
description: Build and publish the Mercury Browser Linux preview archive and the macOS application and disk image.
---

# Releases

Mercury publishes a Linux preview archive from an Ubuntu 22.04 GitHub-hosted runner. It is a portable test artifact, not a signed installer. On macOS (Apple Silicon) the same version is packaged as `Mercury Browser.app` inside a disk image; see [Package for macOS](#package-for-macos).

## Build a Linux package locally

```bash
make package-linux
```

The target creates an optimized executable, strips it, copies the README and license, then writes a compressed archive and SHA-256 checksum under `dist/`.

The current package name follows this shape:

```text
mercury-browser-v0.2.0-linux-x86_64.tar.gz
mercury-browser-v0.2.0-linux-x86_64.tar.gz.sha256
```

Change `PROJECT_VERSION` when testing another version:

```bash
make package-linux PROJECT_VERSION=0.3.0
```

## Package for macOS

Run these on an Apple Silicon Mac with the Xcode command-line tools installed (`xcode-select --install`); full Xcode is not needed. The bundle carries its GStreamer libraries, so the unpacked GStreamer must be present first ([Development setup](setup.md#macos-media-set-up-gstreamer)):

```bash
make gstreamer-macos # once: download and unpack GStreamer (no sudo)
make app-macos       # release build, then dist/Mercury Browser.app
make package-macos   # the same, plus the disk image and its checksum
```

Both targets run `scripts/macos/bundle.sh`, which can also be called directly on an existing build:

```bash
scripts/macos/bundle.sh --gstreamer-prefix <dir> [--binary target/release/mercury-browser] \
    [--out dist] [--identity -] [--dmg]
```

`--gstreamer-prefix` (or `MERCURY_GSTREAMER_PREFIX`, which `scripts/macos/gstreamer.sh env` exports and the Makefile targets pass on) is required: the executable cannot start without its media libraries, so the script refuses to build a bundle without them.

The script writes only inside the output directory and can be re-run; it rebuilds the bundle from scratch each time and removes it again if any step fails. It produces:

```text
dist/Mercury Browser.app/Contents/
  Info.plist            from apps/mercury-desktop/resources/macos/Info.plist.in
  PkgInfo
  MacOS/mercury-browser the release executable (arm64 only)
  MacOS/lib/            the GStreamer libraries and plugins (see below); nothing else
  Resources/AppIcon.icns
  Resources/LICENSE
  Resources/THIRD-PARTY-NOTICES.md
  Resources/LICENSES/   the licence texts the notices refer to (LGPL-2.1, GPL-3.0, …)
  Resources/gstreamer-licenses/  each bundled library project's licence files, from the GStreamer packages
  _CodeSignature/
dist/mercury-browser-0.2.0-macos-arm64.dmg         with --dmg
dist/mercury-browser-0.2.0-macos-arm64.dmg.sha256
```

The version comes from `[workspace.package]` in `Cargo.toml` and `LSMinimumSystemVersion` from `MACOSX_DEPLOYMENT_TARGET` in `.cargo/config.toml`. The script refuses a binary that is not arm64 or that was linked for a different minimum macOS version than the configuration says.

### Info.plist in the executable

`apps/mercury-desktop/build.rs` fills the same `Info.plist.in` when building for macOS and `src/platform_macos.rs` places it in the executable's `__TEXT,__info_plist` section. A binary started from `target/release/` therefore has the application name and HiDPI behaviour of the bundle. Check it with:

```bash
otool -s __TEXT __info_plist target/release/mercury-browser | head -3
```

### Signing

Signing goes inside out and never uses `codesign --deep`: every Mach-O file in `Contents/MacOS/lib`, then the executable, then the bundle, all with the same identity. The hardened runtime (`--options runtime`) is on for every identity.

The default identity is `-`, an ad-hoc signature: valid for running on the machine that built it, but not trusted by Gatekeeper on other Macs. Set `--identity` or `MERCURY_SIGN_IDENTITY` to sign with a certificate from your keychain (`security find-identity -v -p codesigning` lists them). A real identity also gets a secure timestamp (`--timestamp`), and the disk image is signed too.

Besides ad hoc, only identities that carry a team identifier are supported: a Developer ID Application certificate for releases, or an Apple Development certificate for testing. A self-signed certificate has no team, so library validation would reject the bundled libraries at launch even though `codesign --verify` passes; the script refuses that combination after signing. The script also refuses identities that are empty, start with `-` (other than `-` itself) or have leading or trailing spaces, since those are almost always typing mistakes.

#### Entitlements

The identity decides which of two entitlement files in `apps/mercury-desktop/resources/macos/` is used, and the script prints which one:

| Identity | File | Grants |
|---|---|---|
| `-` (ad hoc, the default) | `entitlements-adhoc.plist` | `com.apple.security.cs.allow-jit`, `com.apple.security.cs.allow-unsigned-executable-memory`, `com.apple.security.cs.disable-library-validation` |
| anything else (Developer ID, Apple Development) | `entitlements.plist` | `com.apple.security.cs.allow-jit`, `com.apple.security.cs.allow-unsigned-executable-memory` |

The two JIT entitlements are needed by SpiderMonkey to run JavaScript under the hardened runtime; without them no page loads.

`disable-library-validation` exists only because of ad-hoc signing. The hardened runtime normally lets a process load only libraries signed by Apple or by its own team, and an ad-hoc signature has no team identifier, so an ad-hoc build could never load the libraries it bundles in `Contents/MacOS/lib`: it would stop at launch with "mapping process and mapped file (non-platform) have different Team IDs". The ad-hoc file turns that check off.

The trade-off is real: with library validation off, any process that can write into the application bundle can make Mercury load a library of its own. That is acceptable for a build you made and run yourself. Until releases are signed with a Developer ID, the builds published for download are ad hoc too, so they also run with library validation off. A Developer ID build closes the gap: every bundled library is signed with the same team identifier as the executable, validation stays on, and `entitlements-adhoc.plist` must never be used with a Developer ID.

#### Checks after signing

`codesign --verify` checks neither the hardened runtime nor the entitlements, so after verifying the bundle the script reads both back from the executable and from the bundle. It stops if the `runtime` flag is missing or if the granted entitlements differ from the chosen file.

#### Testing the script

`make test-bundle-macos` (or `scripts/macos/test-bundle.sh`) runs without a build or a signing identity. It checks which entitlement file each identity gets (`scripts/macos/bundle.sh --identity <id> --print-entitlements` prints it without building), the refused identities, what each file grants, and the helpers in `scripts/macos/lib.sh` that parse `otool` and `codesign` output. That includes a load-command listing far larger than a pipe buffer, which a parser that stops reading early would turn into a silent exit.

Check a bundle with:

```bash
codesign --verify --deep --strict --verbose=2 "dist/Mercury Browser.app"
codesign -dv "dist/Mercury Browser.app"                    # flags=…(runtime)
codesign -d --entitlements - "dist/Mercury Browser.app"
spctl --assess --type execute -vv "dist/Mercury Browser.app" # "rejected" for ad hoc
"dist/Mercury Browser.app/Contents/MacOS/mercury-browser" --screenshot /path/to/js.png \
    "file://$PWD/tests/pages/js.html"                        # every line ends in PASS
```

### Media libraries

Servo loads its GStreamer libraries from `Contents/MacOS/lib`, found through the executable's `@executable_path/lib/` rpath (the script adds it when the build did not). Before signing, the script runs `scripts/macos/collect-gstreamer.sh <prefix> "Contents/MacOS/lib" <executable>`, which copies the libraries the executable links, the plugins `scripts/macos/gstreamer-plugins.sh` names (Servo's list minus the excluded plugins; Ogg is excluded for now) and their dependencies, thinned to arm64. Inside a bundle the collector writes no marker file: a signed bundle may hold only code in `Contents/MacOS`. The script then checks that every plugin from `gstreamer-plugins.sh` is present and no excluded one is, and copies `LICENSES/` and the bundled projects' licence files into `Contents/Resources`.

Only libraries and loadable plugins belong in `Contents/MacOS/lib`. They are signed without entitlements, so an executable placed there (a plugin scanner, for example) would run with library validation on and, in an ad-hoc build, fail to load the libraries next to it; the script stops if it finds one, and it stops too on any file there that is not code, which `codesign` would refuse. The collected libraries are signed with the same identity as the app. Ad-hoc builds can load them because of the ad-hoc entitlement set described under [Entitlements](#entitlements).

### Developer ID and notarisation

To publish a build that opens without warnings, a release manager with an Apple Developer ID Application certificate in the keychain signs with it:

```bash
make package-macos MERCURY_SIGN_IDENTITY="Developer ID Application: NAME (TEAMID)"
```

The bundle then carries the Developer ID signature, the hardened runtime, the entitlements and a secure timestamp, which is what notarisation requires. Submit the disk image and staple the ticket to it (credentials are stored once with `xcrun notarytool store-credentials mercury-notary`):

```bash
xcrun notarytool submit dist/mercury-browser-0.2.0-macos-arm64.dmg --keychain-profile mercury-notary --wait
xcrun stapler staple dist/mercury-browser-0.2.0-macos-arm64.dmg
```

Stapling changes the disk image, so write the checksum again afterwards:

```bash
(cd dist && shasum -a 256 mercury-browser-0.2.0-macos-arm64.dmg > mercury-browser-0.2.0-macos-arm64.dmg.sha256)
```

Notarisation has not been run for Mercury yet; current macOS builds are ad hoc.

### Application icon

`AppIcon.icns` and the PNGs in `apps/mercury-desktop/resources/macos/AppIcon.iconset/` are generated from `apps/mercury-desktop/resources/mercury-browser.svg` and committed, so packaging does not need to render them. After changing the logo, regenerate both with:

```bash
scripts/macos/make-icns.sh
```

It compiles `scripts/macos/render-icon.swift`, which draws the logo onto a rounded square following Apple's macOS icon grid at each of the ten iconset sizes, then runs `iconutil`.

## Continuous integration

The Linux quality job installs the same native GStreamer families used by development, verifies required plugins, runs formatting and Clippy, and executes workspace and media tests.

The release workflow builds on Ubuntu 22.04 to preserve the glibc 2.35 compatibility baseline. Windows release packaging remains deferred until the native build and GStreamer runtime can be validated together.

## Publish a tagged release

Version tags trigger GitHub Release publication:

```bash
git tag v0.2.0
git push origin v0.2.0
```

Before pushing a tag:

1. Run `make qa`.
2. Run `make docs`.
3. Test the optimized browser on Linux.
4. Test direct media playback when the release changes GStreamer or graphics code.
5. Confirm the version and expected artifact names.

Manual workflow runs keep the Linux archive as an Actions artifact without publishing a GitHub Release.

