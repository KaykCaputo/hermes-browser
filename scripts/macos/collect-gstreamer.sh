#!/bin/bash
# Copy the GStreamer libraries and plugins that Servo loads at run time into a
# flat directory next to the executable (Servo looks in `<exe dir>/lib` on macOS).
#
#   scripts/macos/collect-gstreamer.sh <gstreamer prefix> <destination lib dir> [executable]
#
# <gstreamer prefix> is a GStreamer.framework/Versions/1.0 directory (see
# scripts/macos/gstreamer.sh). The executable defaults to
# `<destination lib dir>/../mercury-browser`.
#
# Starting from the executable's own non-system dependencies plus the plugin files
# named in the resolved `servo` crate's gstreamer_plugin_lists/{common,macos}.rs.in,
# every non-system dependency is followed with `otool -L` and copied flat into the
# destination. When the executable has a single architecture, each copied library is
# thinned to it with `lipo`. A marker file records what was collected, so an
# up-to-date directory is not copied again.
#
# Adapted from Servo's `package_gstreamer_dylibs` and
# `load_plugin_libraries_from_text_file`:
#   https://github.com/servo/servo, tag v0.8.0 (4e136d8ef31b7c4748fbfac606ca463f3efd7537),
#   python/servo/gstreamer.py, Copyright 2013 The Servo Project Developers,
#   Apache-2.0 OR MIT.
# The thinning step is adapted from Zervo:
#   https://github.com/Goddv/Zervo, commit 86cb7126, scripts/bundle-macos.sh, MPL-2.0.

set -euo pipefail

say() {
    printf '%s\n' "$*" >&2
}

die() {
    say "error: $*"
    exit 1
}

[ "$#" -ge 2 ] && [ "$#" -le 3 ] \
    || die "usage: $0 <gstreamer prefix> <destination lib dir> [executable]"

prefix="$1"
destination="${2%/}"
executable="${3:-$(dirname "$destination")/mercury-browser}"
repository="$(cd "$(dirname "$0")/../.." && pwd -P)"

[ -f "${prefix}/lib/libgstreamer-1.0.0.dylib" ] \
    || die "${prefix} is not a GStreamer prefix (no lib/libgstreamer-1.0.0.dylib)"
[ -f "$executable" ] || die "executable ${executable} does not exist; build it first"
prefix="$(cd "$prefix" && pwd -P)"
prefix_lib="${prefix}/lib"

gstreamer_version="$(sed -n 's/^Version: *//p' "${prefix_lib}/pkgconfig/gstreamer-1.0.pc")"
[ -n "$gstreamer_version" ] || die "cannot read the GStreamer version from ${prefix_lib}/pkgconfig"

# The plugin lists ship inside the `servo` crate; use the copy Cargo resolved.
servo_manifest() {
    local metadata
    metadata="$(cd "$repository" && cargo metadata --format-version 1 --locked --offline \
        --filter-platform aarch64-apple-darwin 2>/dev/null)" \
        || metadata="$(cd "$repository" && cargo metadata --format-version 1 --locked)"
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$metadata" \
            | jq -r '.packages[] | select(.name == "servo") | .manifest_path'
    else
        printf '%s' "$metadata" | python3 -c '
import json, sys
for package in json.load(sys.stdin)["packages"]:
    if package["name"] == "servo":
        print(package["manifest_path"])'
    fi
}

if [ -n "${MERCURY_SERVO_PLUGIN_LISTS:-}" ]; then
    plugin_lists="$MERCURY_SERVO_PLUGIN_LISTS"
else
    manifest="$(servo_manifest)"
    [ -n "$manifest" ] && [ "$(printf '%s\n' "$manifest" | wc -l)" -eq 1 ] \
        || die "could not find exactly one resolved 'servo' crate with cargo metadata"
    plugin_lists="$(dirname "$manifest")/gstreamer_plugin_lists"
fi

# Each list is a Rust array of string literals with `//` comment lines.
plugin_names() {
    local list
    for list in common.rs.in macos.rs.in; do
        [ -f "${plugin_lists}/${list}" ] || die "missing ${plugin_lists}/${list}"
        grep -v '^[[:space:]]*//' "${plugin_lists}/${list}" | grep -o '"[^"]*"' | tr -d '"'
    done
}
plugins="$(plugin_names)"
[ -n "$plugins" ] || die "no plugins listed in ${plugin_lists}"

is_system_library() {
    case "$1" in
        /System/Library/* | /usr/lib/*) return 0 ;;
        *) return 1 ;;
    esac
}

architectures="$(lipo -archs "$executable")"
case "$architectures" in
    *" "*) thin_to="" ; otool_arch=() ;;
    *) thin_to="$architectures" ; otool_arch=(-arch "$architectures") ;;
esac

# Non-system dependencies of a Mach-O file, without its own install name.
dependencies_of() {
    local file="$1" own_id
    own_id="$(otool "${otool_arch[@]}" -D "$file" | sed -n '2p')"
    otool "${otool_arch[@]}" -L "$file" \
        | sed -n 's/^[[:space:]]\{1,\}\([^ ]*\) (compatibility.*/\1/p' \
        | while IFS= read -r dependency; do
              [ "$dependency" = "$own_id" ] && continue
              is_system_library "$dependency" && continue
              printf '%s\n' "$dependency"
          done | sort -u
}

# Resolve an otool dependency line to a file in the prefix.
resolve() {
    local dependency="$1" relative directory
    case "$dependency" in
        @rpath/*)
            relative="${dependency#@rpath/}"
            for directory in "$prefix_lib" "${prefix_lib}/.." "${prefix_lib}/gstreamer-1.0"; do
                if [ -f "${directory}/${relative}" ]; then
                    (cd "$(dirname "${directory}/${relative}")" && printf '%s/%s\n' "$(pwd -P)" "$(basename "$relative")")
                    return 0
                fi
            done
            die "cannot resolve ${dependency} in ${prefix}" ;;
        "${prefix}"/*)
            printf '%s\n' "$dependency" ;;
        *)
            die "unexpected non-system dependency ${dependency}; only the GStreamer framework's @rpath libraries can be collected" ;;
    esac
}

executable_dependencies="$(dependencies_of "$executable")"
marker_name=".gstreamer-${gstreamer_version}"
marker_contents="$(
    printf 'gstreamer %s\narchitectures %s\n' "$gstreamer_version" "$architectures"
    printf 'plugin %s\n' $plugins
    printf 'dependency %s\n' $executable_dependencies
)"

if [ -f "${destination}/${marker_name}" ] \
    && [ "$(cat "${destination}/${marker_name}")" = "$marker_contents" ]; then
    say "ok: GStreamer libraries in ${destination} are up to date"
    exit 0
fi

staging="${destination}.partial"
rm -rf "$staging"
mkdir -p "$staging"

pending="$(
    for dependency in $executable_dependencies; do printf '%s\n' "$dependency"; done
    for plugin in $plugins; do printf '%s\n' "${prefix_lib}/gstreamer-1.0/lib${plugin}.dylib"; done
)"
seen=""
copied=0
while [ -n "$pending" ]; do
    next=""
    for dependency in $pending; do
        source_file="$(resolve "$dependency")"
        [ -f "$source_file" ] || die "${source_file} (needed as ${dependency}) does not exist"
        name="$(basename "$source_file")"
        if printf '%s\n' "$seen" | grep -Fqx "$name"; then
            continue
        fi
        seen="$(printf '%s\n%s' "$seen" "$name")"
        if [ -n "$thin_to" ] && lipo "$source_file" -verify_arch "$thin_to" 2>/dev/null \
            && [ "$(lipo -archs "$source_file")" != "$thin_to" ]; then
            lipo -thin "$thin_to" "$source_file" -output "${staging}/${name}"
        else
            cp "$source_file" "${staging}/${name}"
        fi
        chmod u+w "${staging}/${name}"
        copied=$((copied + 1))
        next="$(printf '%s\n%s' "$next" "$(dependencies_of "$source_file")")"
    done
    pending="$(printf '%s\n' "$next" | sed '/^$/d' | sort -u)"
done

printf '%s\n' "$marker_contents" > "${staging}/${marker_name}"
rm -rf "$destination"
mv "$staging" "$destination"
say "ok: collected ${copied} GStreamer libraries into ${destination} ($(du -sh "$destination" | cut -f 1))"
