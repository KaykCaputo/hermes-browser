---
title: Releases
description: Build and publish Hermes Browser Linux and Windows preview archives.
---

# Releases

Hermes publishes preview archives from native GitHub-hosted Linux and Windows runners. These are portable test artifacts, not signed installers.

## Build a Linux package locally

```bash
make package-linux
```

The target creates an optimized executable, strips it, copies the README and license, then writes a compressed archive and SHA-256 checksum under `dist/`.

The current package name follows this shape:

```text
hermes-browser-v0.1.0-linux-x86_64.tar.gz
hermes-browser-v0.1.0-linux-x86_64.tar.gz.sha256
```

Change `PROJECT_VERSION` when testing another version:

```bash
make package-linux PROJECT_VERSION=0.2.0
```

## Continuous integration

The Linux quality job installs the same native GStreamer families used by development, verifies required plugins, runs formatting and Clippy, and executes workspace and media tests.

The release workflow builds on Ubuntu 22.04 to preserve the glibc 2.35 compatibility baseline. Windows uses its native runner and currently ships without the GStreamer runtime.

## Publish a tagged release

Version tags trigger GitHub Release publication:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Before pushing a tag:

1. Run `make qa`.
2. Run `make docs`.
3. Test the optimized browser on Linux.
4. Test direct media playback when the release changes GStreamer or graphics code.
5. Confirm the version and expected artifact names.

Manual workflow runs keep the archives as Actions artifacts without publishing a GitHub Release.

