#!/bin/bash
# The GStreamer plugins a macOS build of Mercury Browser ships: one definition,
# read by scripts/macos/collect-gstreamer.sh (what to copy) and by the bundle
# checks (what must be there).
#
#   scripts/macos/gstreamer-plugins.sh             effective plugin files, one per line
#                                                  (libgstcoreelements.dylib …)
#   scripts/macos/gstreamer-plugins.sh --excluded  excluded plugin files, one per line
#                                                  (possibly none)
#
# The plugins are the ones Servo loads: the lists in the resolved `servo` crate's
# gstreamer_plugin_lists/{common,macos}.rs.in (or in $MERCURY_SERVO_PLUGIN_LISTS),
# minus the exclusions.
#
# Exclusions: MERCURY_GSTREAMER_EXCLUDE_PLUGINS, space-separated plugin names
# without the "gst" prefix (for example "ogg"). Unset means DEFAULT_EXCLUDED
# below; the empty string means exclude nothing. A name that is not on Servo's
# list is an error.
#
# On any error the script prints a message to standard error, nothing to
# standard output, and exits with status 1 (2 for a usage error).
#
# The list parsing is adapted from Servo's `load_plugin_libraries_from_text_file`:
#   https://github.com/servo/servo, tag v0.8.0, python/servo/gstreamer.py,
#   Copyright 2013 The Servo Project Developers, Apache-2.0 OR MIT.

set -euo pipefail
set -f

# ── The default exclusion: the one place to change ────────────────────────
# ogg: oggdemux since GStreamer commit c4bfa733 (2025) can start playback at the
# end of a file after its duration probe; Ogg stays unsupported until that is fixed.
DEFAULT_EXCLUDED="ogg"

die() {
    printf 'gstreamer-plugins: error: %s\n' "$*" >&2
    exit 1
}

MODE=effective
case "$#:${1:-}" in
    0:) ;;
    1:--excluded) MODE=excluded ;;
    *)
        printf 'usage: %s [--excluded]\n' "$0" >&2
        exit 2
        ;;
esac

REPOSITORY="$(cd "$(dirname -- "$0")/../.." && pwd -P)"

servo_manifest() {
    local metadata error_file
    error_file="$(mktemp -t gstreamer-plugins)"
    if ! metadata="$(cd "$REPOSITORY" && cargo metadata --format-version 1 --locked --offline \
        --filter-platform aarch64-apple-darwin 2>"$error_file")"; then
        printf 'gstreamer-plugins: offline cargo metadata failed (%s); retrying with network access\n' \
            "$(tail -n 1 "$error_file")" >&2
        if ! metadata="$(cd "$REPOSITORY" && cargo metadata --format-version 1 --locked \
            --filter-platform aarch64-apple-darwin 2>"$error_file")"; then
            printf 'gstreamer-plugins: cargo metadata failed: %s\n' "$(tail -n 3 "$error_file")" >&2
            rm -f "$error_file"
            return 1
        fi
    fi
    rm -f "$error_file"
    if command -v jq >/dev/null 2>&1; then
        jq -r '.packages[] | select(.name == "servo") | .manifest_path' <<<"$metadata"
    else
        python3 -c '
import json, sys
for package in json.load(sys.stdin)["packages"]:
    if package["name"] == "servo":
        print(package["manifest_path"])' <<<"$metadata"
    fi
}

if [ -n "${MERCURY_SERVO_PLUGIN_LISTS:-}" ]; then
    PLUGIN_LISTS="$MERCURY_SERVO_PLUGIN_LISTS"
else
    MANIFEST="$(servo_manifest)" || die "cannot locate the resolved servo crate"
    [ -n "$MANIFEST" ] && [ "$(wc -l <<<"$MANIFEST" | tr -d ' ')" = 1 ] \
        || die "expected exactly one resolved 'servo' crate, found: ${MANIFEST:-none}"
    PLUGIN_LISTS="$(dirname -- "$MANIFEST")/gstreamer_plugin_lists"
fi

# Each list is a Rust array of string literals with `//` comment lines.
ALL=()
for list in common.rs.in macos.rs.in; do
    [ -f "$PLUGIN_LISTS/$list" ] || die "missing $PLUGIN_LISTS/$list"
    # grep exits 1 when no line is left (a list of comments only), 2 when it cannot read.
    status=0
    text="$(grep -v '^[[:space:]]*//' "$PLUGIN_LISTS/$list")" || status=$?
    [ "$status" -le 1 ] || die "cannot read $PLUGIN_LISTS/$list"
    while IFS= read -r line; do
        while [[ "$line" =~ \"([^\"]*)\" ]]; do
            ALL+=("${BASH_REMATCH[1]}")
            line="${line#*\""${BASH_REMATCH[1]}"\"}"
        done
    done <<<"$text"
done
[ "${#ALL[@]}" -gt 0 ] || die "no plugins listed in $PLUGIN_LISTS"

if [ "${MERCURY_GSTREAMER_EXCLUDE_PLUGINS+set}" = set ]; then
    SOURCE="MERCURY_GSTREAMER_EXCLUDE_PLUGINS"
    EXCLUDED_TEXT="$MERCURY_GSTREAMER_EXCLUDE_PLUGINS"
else
    SOURCE="the default exclusion"
    EXCLUDED_TEXT="$DEFAULT_EXCLUDED"
fi
EXCLUDED=()
read -r -a EXCLUDED <<<"$EXCLUDED_TEXT"

listed() {
    local wanted="$1" plugin
    for plugin in "${ALL[@]}"; do
        [ "$plugin" = "$wanted" ] && return 0
    done
    return 1
}

excluded() {
    local wanted="$1" name
    for name in ${EXCLUDED[@]+"${EXCLUDED[@]}"}; do
        [ "gst$name" = "$wanted" ] && return 0
    done
    return 1
}

for name in ${EXCLUDED[@]+"${EXCLUDED[@]}"}; do
    listed "gst$name" || die "$SOURCE names '$name', which is not in Servo's plugin list ($PLUGIN_LISTS)"
done

# Build the whole answer first, so that nothing is printed if anything failed.
OUTPUT=""
for plugin in "${ALL[@]}"; do
    if excluded "$plugin"; then
        if [ "$MODE" = excluded ]; then OUTPUT+="lib$plugin.dylib"$'\n'; fi
    elif [ "$MODE" = effective ]; then
        OUTPUT+="lib$plugin.dylib"$'\n'
    fi
done
printf '%s' "$OUTPUT"
