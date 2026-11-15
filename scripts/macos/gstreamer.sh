#!/bin/bash
# Obtain the official GStreamer framework packages for macOS without sudo.
#
#   scripts/macos/gstreamer.sh fetch    download and verify the two .pkg files
#   scripts/macos/gstreamer.sh unpack   expand them into a private framework prefix
#   scripts/macos/gstreamer.sh env      print the exports a build shell needs
#                                       (MERCURY_GSTREAMER_PREFIX, PKG_CONFIG*, PATH)
#   scripts/macos/gstreamer.sh check    verify pkg-config resolves from that prefix
#   scripts/macos/gstreamer.sh prefix   print the prefix (…/GStreamer.framework/Versions/1.0)
#
# Nothing is installed system-wide: the packages are expanded with
# `pkgutil --expand-full` into a cache directory (default
# ~/Library/Caches/mercury-browser/gstreamer/<version>, override with
# MERCURY_GSTREAMER_CACHE) and assembled into a GStreamer.framework there.
# Usage: eval "$(scripts/macos/gstreamer.sh env)"

set -euo pipefail

GSTREAMER_VERSION="1.28.9"
GSTREAMER_RUNTIME_PKG="gstreamer-1.0-${GSTREAMER_VERSION}-universal.pkg"
GSTREAMER_RUNTIME_SHA256="05d27c8e9b4374bbcf1788cf1604bb4b1a4bdcbdc13bc37b67d3a225c8aa86a6"
GSTREAMER_DEVEL_PKG="gstreamer-1.0-devel-${GSTREAMER_VERSION}-universal.pkg"
GSTREAMER_DEVEL_SHA256="54e27ccc9af096f7f12cff32b6111b7f1b867d968cb8888e80719ad46a97c777"
# MERCURY_GSTREAMER_URL_BASE overrides where the packages are downloaded from (a mirror,
# or a local file:// directory); the checksums above are checked either way.
GSTREAMER_URL_BASE="${MERCURY_GSTREAMER_URL_BASE:-https://gstreamer.freedesktop.org/data/pkg/osx/${GSTREAMER_VERSION}}"

# The modules the gstreamer-*-sys and glib-*-sys crates ask pkg-config for.
REQUIRED_MODULES="glib-2.0 gobject-2.0 gio-2.0 gstreamer-1.0 gstreamer-base-1.0 gstreamer-app-1.0
gstreamer-audio-1.0 gstreamer-video-1.0 gstreamer-play-1.0 gstreamer-sdp-1.0 gstreamer-webrtc-1.0"

SYSTEM_FRAMEWORK="/Library/Frameworks/GStreamer.framework"

cache_dir="${MERCURY_GSTREAMER_CACHE:-${HOME}/Library/Caches/mercury-browser/gstreamer/${GSTREAMER_VERSION}}"
framework_dir="${cache_dir}/GStreamer.framework"
prefix="${framework_dir}/Versions/1.0"
unpacked_marker="${framework_dir}/.mercury-unpacked-${GSTREAMER_VERSION}"

say() {
    printf '%s\n' "$*" >&2
}

die() {
    say "error: $*"
    exit 1
}

require_macos() {
    [ "$(uname -s)" = "Darwin" ] || die "this script is for macOS only"
}

sha256_of() {
    /usr/bin/shasum -a 256 "$1" | cut -d ' ' -f 1
}

# fetch_one <file name> <expected sha256>
fetch_one() {
    local name="$1" expected="$2" file="${cache_dir}/$1" actual
    if [ -f "$file" ]; then
        actual="$(sha256_of "$file")"
        if [ "$actual" = "$expected" ]; then
            say "ok: ${name} is already in ${cache_dir} (sha256 verified)"
            return 0
        fi
        die "${file} has sha256 ${actual}, expected ${expected}; delete it and run fetch again"
    fi
    say "downloading ${GSTREAMER_URL_BASE}/${name}"
    /usr/bin/curl --location --fail --retry 5 --retry-all-errors --continue-at - \
        --output "${file}.part" "${GSTREAMER_URL_BASE}/${name}"
    actual="$(sha256_of "${file}.part")"
    if [ "$actual" != "$expected" ]; then
        rm -f "${file}.part"
        die "${name}: sha256 ${actual} does not match the pinned ${expected}; the download was deleted"
    fi
    mv -f "${file}.part" "$file"
    say "ok: ${name} downloaded and verified"
}

cmd_fetch() {
    require_macos
    mkdir -p "$cache_dir"
    fetch_one "$GSTREAMER_RUNTIME_PKG" "$GSTREAMER_RUNTIME_SHA256"
    fetch_one "$GSTREAMER_DEVEL_PKG" "$GSTREAMER_DEVEL_SHA256"
}

# expand_into <pkg file> <framework staging dir>
# Every component package installs either into GStreamer.framework/ or into
# GStreamer.framework/Versions/1.0 (its PackageInfo install-location); copy its
# payload to the same place under the staging directory.
expand_into() {
    local pkg="$1" staging="$2" expanded component location
    expanded="${cache_dir}/expanded-$(basename "$pkg" .pkg)"
    rm -rf "$expanded"
    say "expanding $(basename "$pkg")"
    /usr/sbin/pkgutil --expand-full "$pkg" "$expanded"
    for component in "$expanded"/*.pkg; do
        [ -d "${component}/Payload" ] || continue
        # The first install-location only; sed reads the file itself, so stopping early (q) cuts no pipe.
        location="$(sed -n '/install-location="/{s/.*install-location="\([^"]*\)".*/\1/p;q;}' "${component}/PackageInfo")"
        case "$location" in
            "${SYSTEM_FRAMEWORK}" | "${SYSTEM_FRAMEWORK}/")
                /usr/bin/ditto "${component}/Payload" "$staging" ;;
            "${SYSTEM_FRAMEWORK}/Versions/1.0" | "${SYSTEM_FRAMEWORK}/Versions/1.0/")
                /usr/bin/ditto "${component}/Payload" "${staging}/Versions/1.0" ;;
            *)
                rm -rf "$expanded"
                die "$(basename "$component") installs to unexpected location '${location}'" ;;
        esac
    done
    rm -rf "$expanded"
}

cmd_unpack() {
    require_macos
    if [ -f "$unpacked_marker" ]; then
        say "ok: ${prefix} is already unpacked"
        return 0
    fi
    local pkg
    for pkg in "$GSTREAMER_RUNTIME_PKG" "$GSTREAMER_DEVEL_PKG"; do
        [ -f "${cache_dir}/${pkg}" ] || die "${cache_dir}/${pkg} is missing; run: make gstreamer-macos (or $0 fetch)"
    done
    [ "$(sha256_of "${cache_dir}/${GSTREAMER_RUNTIME_PKG}")" = "$GSTREAMER_RUNTIME_SHA256" ] \
        || die "${GSTREAMER_RUNTIME_PKG} fails its checksum; run fetch again"
    [ "$(sha256_of "${cache_dir}/${GSTREAMER_DEVEL_PKG}")" = "$GSTREAMER_DEVEL_SHA256" ] \
        || die "${GSTREAMER_DEVEL_PKG} fails its checksum; run fetch again"

    local staging="${framework_dir}.partial"
    rm -rf "$staging" "$framework_dir"
    mkdir -p "${staging}/Versions/1.0"
    expand_into "${cache_dir}/${GSTREAMER_RUNTIME_PKG}" "$staging"
    expand_into "${cache_dir}/${GSTREAMER_DEVEL_PKG}" "$staging"

    local directory
    for directory in bin lib include lib/pkgconfig lib/gstreamer-1.0; do
        [ -d "${staging}/Versions/1.0/${directory}" ] || die "unpacked prefix has no ${directory}/"
    done
    mv "$staging" "$framework_dir"
    printf '%s\n' "$GSTREAMER_VERSION" > "$unpacked_marker"
    say "ok: unpacked to ${prefix} ($(du -sh "$framework_dir" | cut -f 1))"
}

cmd_env() {
    require_macos
    [ -f "$unpacked_marker" ] || die "no unpacked GStreamer in ${prefix}; run: make gstreamer-macos (or $0 fetch, then $0 unpack)"
    # PKG_CONFIG_LIBDIR replaces pkg-config's default search path, so only this
    # prefix's .pc files are seen (not Homebrew's, not /Library/Frameworks).
    printf 'export MERCURY_GSTREAMER_PREFIX=%q\n' "$prefix"
    printf 'export PKG_CONFIG=%q\n' "${prefix}/bin/pkg-config"
    printf 'export PKG_CONFIG_LIBDIR=%q\n' "${prefix}/lib/pkgconfig"
    printf 'unset PKG_CONFIG_PATH\n'
    # The $PATH here is meant literally: it is expanded by the shell that evals this output.
    # shellcheck disable=SC2016
    printf 'export PATH=%q:"$PATH"\n' "${prefix}/bin"
}

cmd_check() {
    require_macos
    [ -f "$unpacked_marker" ] || die "no unpacked GStreamer in ${prefix}; run: make gstreamer-macos (or $0 fetch, then $0 unpack)"
    if [ -d "$SYSTEM_FRAMEWORK" ]; then
        say "note: ${SYSTEM_FRAMEWORK} is installed; it is ignored in favour of ${prefix}"
    fi
    local pkg_config="${prefix}/bin/pkg-config" version resolved_prefix expected_prefix module
    [ -x "$pkg_config" ] || die "${pkg_config} is missing"
    version="$(env -u PKG_CONFIG_PATH PKG_CONFIG_LIBDIR="${prefix}/lib/pkgconfig" \
        "$pkg_config" --modversion gstreamer-1.0)"
    [ "$version" = "$GSTREAMER_VERSION" ] \
        || die "pkg-config reports gstreamer-1.0 ${version}, expected ${GSTREAMER_VERSION}"
    resolved_prefix="$(env -u PKG_CONFIG_PATH PKG_CONFIG_LIBDIR="${prefix}/lib/pkgconfig" \
        "$pkg_config" --variable=prefix gstreamer-1.0)"
    expected_prefix="$(cd "$prefix" && pwd -P)"
    [ "$(cd "$resolved_prefix" && pwd -P)" = "$expected_prefix" ] \
        || die "gstreamer-1.0 resolves to ${resolved_prefix}, not ${prefix}"
    for module in $REQUIRED_MODULES; do
        env -u PKG_CONFIG_PATH PKG_CONFIG_LIBDIR="${prefix}/lib/pkgconfig" \
            "$pkg_config" --exists "$module" || die "pkg-config cannot resolve ${module}"
        say "ok: ${module} $(env -u PKG_CONFIG_PATH PKG_CONFIG_LIBDIR="${prefix}/lib/pkgconfig" \
            "$pkg_config" --modversion "$module")"
    done
    say "ok: gstreamer-1.0 ${version} from ${prefix}"
}

case "${1:-}" in
    fetch) cmd_fetch ;;
    unpack) cmd_unpack ;;
    env) cmd_env ;;
    check) cmd_check ;;
    prefix) printf '%s\n' "$prefix" ;;
    *)
        say "usage: $0 fetch|unpack|env|check|prefix"
        exit 2
        ;;
esac
