#!/usr/bin/env bash
# Fetch Servo's GStreamer plugin lists from the `servo` crate pinned in
# Cargo.lock, without cargo.
#
#   scripts/ci/fetch-servo-plugin-lists.sh <destination directory>
#
# Reads the one `servo` package and its checksum from Cargo.lock, downloads
# that version's .crate from static.crates.io, refuses it unless its SHA-256
# equals the checksum, and writes gstreamer_plugin_lists/common.rs.in and
# macos.rs.in into the destination (created if needed). Each file is
# extracted to standard output and written to a name this script chooses, so
# the archive cannot decide where anything lands. Prints the destination.
#
# Used where cargo must not run (the release signing job), to set
# MERCURY_SERVO_PLUGIN_LISTS for scripts/macos/gstreamer-plugins.sh.
set -euo pipefail

die() {
    printf 'fetch-servo-plugin-lists: error: %s\n' "$*" >&2
    exit 1
}

case "${1:-}" in
    "") die "usage: $0 <destination directory>" ;;
    -h | --help) sed -n '/^# Fetch Servo/,/^# MERCURY_SERVO_PLUGIN_LISTS for/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
esac
[ "$#" = 1 ] || die "usage: $0 <destination directory>"
destination="$1"

lock="$(cd "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)/Cargo.lock"
[ -f "$lock" ] || die "$lock does not exist"

# name, version, source and checksum of every package called exactly "servo".
entries="$(awk '
    /^\[\[package\]\]/ { name = ""; version = ""; source = ""; checksum = "" }
    /^name = /     { name = $3 }
    /^version = /  { version = $3 }
    /^source = /   { source = $3 }
    /^checksum = / { checksum = $3; if (name == "\"servo\"") print version, source, checksum }
' "$lock" | tr -d '"')"
[ -n "$entries" ] && [ "$(printf '%s\n' "$entries" | wc -l | tr -d ' ')" = 1 ] ||
    die "Cargo.lock does not have exactly one servo package with a checksum"
read -r version source checksum <<<"$entries"
[ "$source" = "registry+https://github.com/rust-lang/crates.io-index" ] ||
    die "servo comes from '$source', not crates.io"
case "$version" in
    *[!0-9A-Za-z.+-]* | "") die "unexpected servo version '$version'" ;;
esac
case "$checksum" in
    *[!0-9a-f]* | "") die "unexpected checksum '$checksum'" ;;
esac

if command -v sha256sum >/dev/null 2>&1; then
    SHA256=(sha256sum)
else
    SHA256=(shasum -a 256)
fi

work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT
crate="$work/servo-$version.crate"
curl --fail --silent --show-error --location --retry 3 --output "$crate" \
    "https://static.crates.io/crates/servo/servo-$version.crate"
printf '%s  %s\n' "$checksum" "$crate" | "${SHA256[@]}" -c - >/dev/null ||
    die "servo-$version.crate does not match the checksum in Cargo.lock ($checksum)"

mkdir -p "$destination"
for list in common.rs.in macos.rs.in; do
    tar -xzOf "$crate" "servo-$version/gstreamer_plugin_lists/$list" >"$destination/$list" ||
        die "servo-$version.crate has no gstreamer_plugin_lists/$list"
    [ -s "$destination/$list" ] || die "gstreamer_plugin_lists/$list is empty"
done
printf '%s\n' "$destination"
