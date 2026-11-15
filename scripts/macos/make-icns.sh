#!/usr/bin/env bash
# Adapted from Zervo, https://github.com/Goddv/Zervo, commit 86cb7126,
# scripts/make-icns.sh, MPL-2.0. Changed: the iconset is rendered from the
# project's SVG logo (scripts/macos/render-icon.swift) instead of scaled from a
# committed PNG, and it is kept in the repository.
#
# Regenerate the application icon from the project logo:
#
#   scripts/macos/make-icns.sh
#
# Writes apps/mercury-desktop/resources/macos/AppIcon.iconset/ (ten PNGs) and
# AppIcon.icns next to it. Both are committed, so packaging never needs to run
# this; run it again only after the logo changes. Needs macOS 13 or later with
# the Xcode command-line tools (swiftc, iconutil); nothing else.
set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/../.."

die() {
    printf 'make-icns: %s\n' "$*" >&2
    exit 1
}

for tool in swiftc iconutil sips; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool not found (install the Xcode command-line tools)"
done

SOURCE="apps/mercury-desktop/resources/mercury-browser.svg"
RESOURCES="apps/mercury-desktop/resources/macos"
ICONSET="$RESOURCES/AppIcon.iconset"
OUTPUT="$RESOURCES/AppIcon.icns"

[ -f "$SOURCE" ] || die "missing $SOURCE"

WORK="$(mktemp -d)"
trap 'rm -rf -- "$WORK"' EXIT

# Compiled once rather than run through the `swift` interpreter ten times.
swiftc -O -o "$WORK/render-icon" scripts/macos/render-icon.swift

rm -rf -- "$ICONSET"
mkdir -p "$ICONSET"

# iconutil expects exactly these names; an @2x entry is the next size up. Each
# size is drawn from the vector logo, not scaled down from the largest one.
for spec in "16:icon_16x16" "32:icon_16x16@2x" "32:icon_32x32" "64:icon_32x32@2x" \
            "128:icon_128x128" "256:icon_128x128@2x" "256:icon_256x256" \
            "512:icon_256x256@2x" "512:icon_512x512" "1024:icon_512x512@2x"; do
    size="${spec%%:*}"
    name="${spec#*:}"
    "$WORK/render-icon" "$SOURCE" "$size" "$ICONSET/$name.png"
    # A wrong size would still make a valid .icns, so check every file.
    width="$(sips -g pixelWidth "$ICONSET/$name.png" | awk '/pixelWidth/ { print $2 }')"
    height="$(sips -g pixelHeight "$ICONSET/$name.png" | awk '/pixelHeight/ { print $2 }')"
    [ "$width" = "$size" ] && [ "$height" = "$size" ] ||
        die "$name.png is ${width}x${height}, expected ${size}x${size}"
done

iconutil -c icns "$ICONSET" -o "$OUTPUT"
printf 'make-icns: wrote %s and %s\n' "$ICONSET" "$OUTPUT"
