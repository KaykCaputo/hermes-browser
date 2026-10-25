---
title: Releases
description: Build and publish the Hermes Browser Linux preview archive.
---

# Releases

Hermes publishes a Linux preview archive from an Ubuntu 22.04 GitHub-hosted runner. It is a portable test artifact, not a signed installer.

## Build a Linux package locally

```bash
make package-linux
```

The target creates an optimized executable, strips it, copies the README and license, then writes a compressed archive and SHA-256 checksum under `dist/`.

The current package name follows this shape:

```text
hermes-browser-v0.1.2-linux-x86_64.tar.gz
hermes-browser-v0.1.2-linux-x86_64.tar.gz.sha256
```

Change `PROJECT_VERSION` when testing another version:

```bash
make package-linux PROJECT_VERSION=0.2.0
```

## Continuous integration

The Linux quality job installs the same native GStreamer families used by development, verifies required plugins, runs formatting and Clippy, and executes workspace and media tests.

The release workflow builds on Ubuntu 22.04 to preserve the glibc 2.35 compatibility baseline. Windows release packaging remains deferred until the native build and GStreamer runtime can be validated together.

## Publish a tagged release

Version tags trigger GitHub Release publication:

```bash
git tag v0.1.2
git push origin v0.1.2
```

Before pushing a tag:

1. Run `make qa`.
2. Run `make docs`.
3. Test the optimized browser on Linux.
4. Test direct media playback when the release changes GStreamer or graphics code.
5. Confirm the version and expected artifact names.

Manual workflow runs keep the Linux archive as an Actions artifact without publishing a GitHub Release.

