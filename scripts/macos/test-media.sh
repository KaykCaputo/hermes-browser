#!/usr/bin/env bash
# Checks the macOS media scripts: scripts/macos/gstreamer-plugins.sh (the plugin
# definition), scripts/macos/collect-gstreamer.sh (the library collector),
# scripts/macos/gstreamer-notices.sh, scripts/macos/gstreamer.sh, and that the
# Makefile stops when the GStreamer environment cannot be set up.
#
#   scripts/macos/test-media.sh      (or: make test-media-macos)
#
# The collector checks need the unpacked GStreamer (make gstreamer-macos) and a
# built target/release/mercury-browser; without them they are reported as SKIP.
# Prints one line per check and exits non-zero if any fails. Writes only to a
# temporary directory, which it removes.
set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/../.."
REPO="$PWD"
TMP="$(mktemp -d -t mercury-test-media)"
trap 'rm -rf -- "$TMP"' EXIT
failures=0

check() {
    local name="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        printf 'PASS %s\n' "$name"
    else
        printf 'FAIL %s: expected [%s], got [%s]\n' "$name" "$expected" "$actual"
        failures=$((failures + 1))
    fi
}

skip() {
    printf 'SKIP %s\n' "$1"
}

# Runs a command; prints "<exit status>|<standard output>" and keeps standard
# error in $TMP/stderr.
run() {
    local output code=0
    output="$("$@" 2>"$TMP/stderr")" || code=$?
    printf '%s|%s' "$code" "$output"
}

stderr_has() {
    if grep -F -- "$1" "$TMP/stderr" >/dev/null; then printf 'yes'; else printf 'no'; fi
}

line_count() {
    if [ -z "$1" ]; then printf '0'; else printf '%s' "$(wc -l <<<"$1" | tr -d ' ')"; fi
}

# ── gstreamer-plugins.sh ──────────────────────────────────────────────────
PLUGINS=scripts/macos/gstreamer-plugins.sh
unset MERCURY_GSTREAMER_EXCLUDE_PLUGINS MERCURY_SERVO_PLUGIN_LISTS

result="$(run bash "$PLUGINS")"
check "the default plugin list resolves" "0" "${result%%|*}"
default_list="${result#*|}"
check "the default list has Servo's 38 plugins minus ogg" "37" "$(line_count "$default_list")"
check "the default list names files like libgstcoreelements.dylib" "yes" \
    "$(if grep -x 'libgstcoreelements.dylib' <<<"$default_list" >/dev/null; then echo yes; else echo no; fi)"
check "ogg is excluded by default" "0|libgstogg.dylib" "$(run bash "$PLUGINS" --excluded)"

# The remaining cases read a copy of Servo's lists, so they do not run cargo.
manifest_dir="$(cargo metadata --format-version 1 --locked --offline --filter-platform aarch64-apple-darwin 2>/dev/null |
    jq -r '.packages[] | select(.name == "servo") | .manifest_path' | xargs dirname)"
mkdir -p "$TMP/lists"
cp "$manifest_dir/gstreamer_plugin_lists/common.rs.in" "$manifest_dir/gstreamer_plugin_lists/macos.rs.in" "$TMP/lists/"
export MERCURY_SERVO_PLUGIN_LISTS="$TMP/lists"

check "MERCURY_SERVO_PLUGIN_LISTS gives the same list" "0|$default_list" "$(run bash "$PLUGINS")"
check "an explicit exclusion list" "0|libgstogg.dylib
libgstlibav.dylib" "$(MERCURY_GSTREAMER_EXCLUDE_PLUGINS='libav ogg' run bash "$PLUGINS" --excluded)"
check "an explicit list removes both from the effective list" "36" \
    "$(line_count "$(MERCURY_GSTREAMER_EXCLUDE_PLUGINS='libav ogg' bash "$PLUGINS")")"
check "the empty string excludes nothing" "0|" "$(MERCURY_GSTREAMER_EXCLUDE_PLUGINS='' run bash "$PLUGINS" --excluded)"
check "the empty string keeps all 38" "38" "$(line_count "$(MERCURY_GSTREAMER_EXCLUDE_PLUGINS='' bash "$PLUGINS")")"
check "a typo is an error with nothing on stdout" "1|" "$(MERCURY_GSTREAMER_EXCLUDE_PLUGINS='ogx' run bash "$PLUGINS")"
check "the typo is named" "yes" "$(stderr_has "names 'ogx', which is not in Servo's plugin list")"
check "missing lists are an error with nothing on stdout" "1|" \
    "$(MERCURY_SERVO_PLUGIN_LISTS="$TMP/no-such-dir" run bash "$PLUGINS")"
check "--excluded with missing lists prints nothing either" "1|" \
    "$(MERCURY_SERVO_PLUGIN_LISTS="$TMP/no-such-dir" run bash "$PLUGINS" --excluded)"
check "an unknown option is a usage error" "2|" "$(run bash "$PLUGINS" --bogus)"

# ── gstreamer.sh ──────────────────────────────────────────────────────────
check "env fails without an unpacked cache, printing nothing" "1|" \
    "$(MERCURY_GSTREAMER_CACHE="$TMP/no-cache" run scripts/macos/gstreamer.sh env)"

mkdir -p "$TMP/mirror" "$TMP/cache"
printf 'not a package\n' >"$TMP/mirror/gstreamer-1.0-1.28.9-universal.pkg"
result="$(MERCURY_GSTREAMER_CACHE="$TMP/cache" MERCURY_GSTREAMER_URL_BASE="file://$TMP/mirror" \
    run scripts/macos/gstreamer.sh fetch)"
check "a download with the wrong checksum fails" "1" "${result%%|*}"
check "and is deleted, with a message" "no|yes" \
    "$(if [ -e "$TMP/cache/gstreamer-1.0-1.28.9-universal.pkg.part" ]; then echo yes; else echo no; fi)|$(stderr_has "the download was deleted")"

PREFIX=""
if scripts/macos/gstreamer.sh check >/dev/null 2>&1; then
    PREFIX="$(scripts/macos/gstreamer.sh prefix)"
    result="$(run scripts/macos/gstreamer.sh env)"
    check "env prints exactly the five lines CI exports" \
        "export MERCURY_GSTREAMER_PREFIX|export PKG_CONFIG|export PKG_CONFIG_LIBDIR|unset PKG_CONFIG_PATH|export PATH" \
        "$(sed -E 's/^(export|unset) ([A-Z_]+).*/\1 \2/' <<<"${result#*|}" | paste -sd '|' -)"
else
    skip "env output (no unpacked GStreamer; run make gstreamer-macos)"
fi

# ── Makefile: a failed environment step stops the recipe ──────────────────
for target in check lint test; do
    result="$(MERCURY_GSTREAMER_CACHE="$TMP/no-cache" run make -s "$target" CARGO=true)"
    check "make $target stops when gstreamer.sh env fails" "yes" "$([ "${result%%|*}" != 0 ] && echo yes || echo no)"
done
if [ -n "$PREFIX" ]; then
    check "make check runs when the environment is available" "0" \
        "$(result="$(run make -s check CARGO=true)"; printf '%s' "${result%%|*}")"
fi

# ── gstreamer-notices.sh ──────────────────────────────────────────────────
if [ -n "$PREFIX" ]; then
    mkdir -p "$TMP/unknown-lib"
    : >"$TMP/unknown-lib/libunknown.1.dylib"
    check "an unmapped library is an error" "1" \
        "$(result="$(run bash scripts/macos/gstreamer-notices.sh --check "$PREFIX" "$TMP/unknown-lib")"; printf '%s' "${result%%|*}")"
    check "and is named" "yes" "$(stderr_has "libunknown.1.dylib is not mapped")"
fi

# ── collect-gstreamer.sh ──────────────────────────────────────────────────
EXE="$REPO/target/release/mercury-browser"
if [ -z "$PREFIX" ] || [ ! -f "$EXE" ]; then
    skip "collector (needs make gstreamer-macos and target/release/mercury-browser)"
else
    collect() { run bash scripts/macos/collect-gstreamer.sh "$@"; }
    files_in() { find "$1" -type f | wc -l | tr -d ' '; }

    # A prefix and a destination whose paths contain a space and a `*`.
    odd="$TMP/odd *dir"
    mkdir -p "$odd/out dir"
    ln -s "$PREFIX" "$odd/pre fix*"
    cp "$EXE" "$odd/out dir/mercury-browser"
    lib="$odd/out dir/lib"
    result="$(collect "$odd/pre fix*" "$lib")"
    check "collects through paths with a space and *" "0" "${result%%|*}"
    check "87 libraries and one marker" "88" "$(files_in "$lib")"
    check "the excluded plugin is absent" "no" "$([ -e "$lib/libgstogg.dylib" ] && echo yes || echo no)"
    check "the marker records the exclusion" "excluded libgstogg.dylib" \
        "$(grep '^excluded' "$lib/.gstreamer-1.28.9")"
    check "the marker records the file count" "files 87" "$(grep '^files' "$lib/.gstreamer-1.28.9")"

    result="$(collect "$odd/pre fix*" "$lib")"
    check "a second run finds it up to date" "0|yes" "${result%%|*}|$(stderr_has "is up to date")"

    printf 'x' >>"$lib/libgstvolume.dylib"
    result="$(collect "$odd/pre fix*" "$lib")"
    check "a damaged library is noticed and the directory rebuilt" "0|yes" "${result%%|*}|$(stderr_has "out of date or damaged")"
    lipo -thin arm64 "$PREFIX/lib/gstreamer-1.0/libgstvolume.dylib" -output "$TMP/volume-arm64.dylib"
    check "the rebuilt library matches the prefix again" "same" \
        "$(if cmp -s "$lib/libgstvolume.dylib" "$TMP/volume-arm64.dylib"; then echo same; else echo different; fi)"

    cp "$PREFIX/lib/gstreamer-1.0/libgstogg.dylib" "$lib/"
    result="$(collect "$odd/pre fix*" "$lib")"
    check "a stale excluded plugin is removed by the rebuild" "0|no" \
        "${result%%|*}|$([ -e "$lib/libgstogg.dylib" ] && echo yes || echo no)"

    mkdir -p "$TMP/foreign/lib"
    : >"$TMP/foreign/lib/keep-me"
    cp "$EXE" "$TMP/foreign/mercury-browser"
    result="$(collect "$PREFIX" "$TMP/foreign/lib")"
    check "a directory without a marker is not replaced" "1|yes|yes" \
        "${result%%|*}|$(stderr_has "refusing to replace it")|$([ -e "$TMP/foreign/lib/keep-me" ] && echo yes || echo no)"

    app="$TMP/Test.app/Contents/MacOS"
    mkdir -p "$app"
    cp "$EXE" "$app/mercury-browser"
    result="$(collect "$PREFIX" "$app/lib")"
    check "a bundle destination is collected" "0" "${result%%|*}"
    check "and holds only the 87 libraries, no marker" "87|0" \
        "$(files_in "$app/lib")|$(find "$app/lib" -type f ! -name '*.dylib' | wc -l | tr -d ' ')"
    result="$(collect "$PREFIX" "$app/lib")"
    check "a bundle destination is never collected twice" "1|yes" "${result%%|*}|$(stderr_has "already exists")"

    # A prefix whose files are symlinks to the real one, with one file replaced.
    fake_prefix() {
        local dir="$1" entry
        rm -rf "$dir"
        mkdir -p "$dir/lib/gstreamer-1.0" "$dir/lib/pkgconfig" "$dir/share"
        cp "$PREFIX/lib/pkgconfig/gstreamer-1.0.pc" "$dir/lib/pkgconfig/"
        cp "$PREFIX/share/versions.txt" "$dir/share/"
        for entry in "$PREFIX"/lib/*.dylib; do ln -s "$entry" "$dir/lib/"; done
        for entry in "$PREFIX"/lib/gstreamer-1.0/*.dylib; do ln -s "$entry" "$dir/lib/gstreamer-1.0/"; done
    }
    mkdir -p "$TMP/fake-out"
    cp "$EXE" "$TMP/fake-out/mercury-browser"

    fake_prefix "$TMP/no-arm64"
    rm "$TMP/no-arm64/lib/gstreamer-1.0/libgstvolume.dylib"
    lipo -thin x86_64 "$PREFIX/lib/gstreamer-1.0/libgstvolume.dylib" -output "$TMP/no-arm64/lib/gstreamer-1.0/libgstvolume.dylib"
    result="$(collect "$TMP/no-arm64" "$TMP/fake-out/lib")"
    check "a library without arm64 code stops the collector" "1|yes" "${result%%|*}|$(stderr_has "has no arm64 code")"

    fake_prefix "$TMP/broken"
    rm "$TMP/broken/lib/gstreamer-1.0/libgstvolume.dylib"
    head -c 100 "$PREFIX/lib/gstreamer-1.0/libgstvolume.dylib" >"$TMP/broken/lib/gstreamer-1.0/libgstvolume.dylib"
    result="$(collect "$TMP/broken" "$TMP/fake-out/lib")"
    check "an unreadable library stops the collector" "1" "${result%%|*}"

    fake_prefix "$TMP/missing"
    rm "$TMP/missing/lib/gstreamer-1.0/libgstvolume.dylib"
    result="$(collect "$TMP/missing" "$TMP/fake-out/lib")"
    check "a missing plugin stops the collector" "1|yes" "${result%%|*}|$(stderr_has "plugin gstvolume is missing")"
    check "no partial result is left behind" "no|no" \
        "$([ -e "$TMP/fake-out/lib" ] && echo yes || echo no)|$([ -e "$TMP/fake-out/lib.partial" ] && echo yes || echo no)"

    notices_before="$(shasum -a 256 THIRD-PARTY-NOTICES.md)"
    mkdir -p "$TMP/with-ogg"
    cp "$EXE" "$TMP/with-ogg/mercury-browser"
    result="$(MERCURY_GSTREAMER_EXCLUDE_PLUGINS='' collect "$PREFIX" "$TMP/with-ogg/lib")"
    check "a collection the notices do not list stops the collector" "1|yes" \
        "${result%%|*}|$(stderr_has "THIRD-PARTY-NOTICES.md does not list what was collected")"
    check "and leaves THIRD-PARTY-NOTICES.md alone" "$notices_before" "$(shasum -a 256 THIRD-PARTY-NOTICES.md)"
fi

if [ "$failures" -gt 0 ]; then
    printf '%s check(s) failed\n' "$failures"
    exit 1
fi
printf 'all checks passed\n'
