#!/usr/bin/env bash
# Print the directory holding Servo's GStreamer plugin lists, found the way
# scripts/macos/collect-gstreamer.sh finds it.
#
#   scripts/ci/servo-plugin-lists.sh
#
# $MERCURY_SERVO_PLUGIN_LISTS when it is set; otherwise the
# gstreamer_plugin_lists directory next to the manifest of the one `servo`
# crate `cargo metadata --locked` resolves for aarch64-apple-darwin (offline
# first). Fails unless the directory has common.rs.in and macos.rs.in.
set -euo pipefail

die() {
    printf 'servo-plugin-lists: error: %s\n' "$*" >&2
    exit 1
}

case "${1:-}" in
    "") ;;
    -h | --help) sed -n '/^# Print the directory/,/^# first)/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unexpected argument: $1" ;;
esac

if [ -n "${MERCURY_SERVO_PLUGIN_LISTS:-}" ]; then
    lists="$MERCURY_SERVO_PLUGIN_LISTS"
else
    cd "$(dirname -- "${BASH_SOURCE[0]}")/../.."
    command -v python3 >/dev/null 2>&1 || die "python3 not found"
    metadata="$(mktemp)"
    trap 'rm -f -- "$metadata"' EXIT
    if ! cargo metadata --format-version 1 --locked --offline \
        --filter-platform aarch64-apple-darwin >"$metadata" 2>/dev/null; then
        cargo metadata --format-version 1 --locked \
            --filter-platform aarch64-apple-darwin >"$metadata" ||
            die "cargo metadata failed"
    fi
    manifests="$(python3 -c '
import json, sys
for package in json.load(sys.stdin)["packages"]:
    if package["name"] == "servo":
        print(package["manifest_path"])' <"$metadata")"
    [ -n "$manifests" ] && [ "$(printf '%s\n' "$manifests" | wc -l | tr -d ' ')" = 1 ] ||
        die "cargo metadata does not resolve exactly one servo crate"
    lists="$(dirname -- "$manifests")/gstreamer_plugin_lists"
fi

for list in common.rs.in macos.rs.in; do
    [ -f "$lists/$list" ] || die "$lists/$list does not exist"
done
printf '%s\n' "$lists"
