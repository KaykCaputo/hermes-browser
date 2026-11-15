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
# that scripts/macos/gstreamer-plugins.sh names (Servo's list minus the excluded
# plugins; see that script), every non-system dependency is followed with
# `otool -L` and copied flat into the destination, thinned to the executable's
# architecture with `lipo`. A marker file records what was collected (inputs, file
# count and a digest of the contents), so an intact, up-to-date directory is not
# copied again and a damaged one is rebuilt. A directory without a marker is never
# deleted. Finally the bundled-library table in THIRD-PARTY-NOTICES.md is checked
# against the collected set (scripts/macos/gstreamer-notices.sh).
#
# Inside an application bundle (a destination ending in .app/Contents/MacOS/lib)
# no marker is written: a signed bundle may hold only code in Contents/MacOS, and
# the bundle is rebuilt from scratch each time, so the destination must not exist.
#
# Environment:
#   MERCURY_GSTREAMER_EXCLUDE_PLUGINS, MERCURY_SERVO_PLUGIN_LISTS
#                                      read by scripts/macos/gstreamer-plugins.sh
#   MERCURY_GSTREAMER_UPDATE_NOTICES   1 = rewrite the THIRD-PARTY-NOTICES.md table
#                                      instead of checking it
#
# Adapted from Servo's `package_gstreamer_dylibs` and
# `load_plugin_libraries_from_text_file`:
#   https://github.com/servo/servo, tag v0.8.0 (4e136d8ef31b7c4748fbfac606ca463f3efd7537),
#   python/servo/gstreamer.py, Copyright 2013 The Servo Project Developers,
#   Apache-2.0 OR MIT.
# The thinning step is adapted from Zervo:
#   https://github.com/Goddv/Zervo, commit 86cb7126, scripts/bundle-macos.sh, MPL-2.0.

set -euo pipefail
# Paths are kept in arrays and quoted; globbing is off so that a `*` or `?` in a
# path can never expand.
set -f

say() {
    printf 'collect-gstreamer: %s\n' "$*" >&2
}

die() {
    say "error: $*"
    exit 1
}

[ "$#" -ge 2 ] && [ "$#" -le 3 ] \
    || die "usage: $0 <gstreamer prefix> <destination lib dir> [executable]"

PREFIX="$1"
DESTINATION="${2%/}"
EXECUTABLE="${3:-$(dirname -- "$DESTINATION")/mercury-browser}"
REPOSITORY="$(cd "$(dirname -- "$0")/../.." && pwd -P)"

[ -n "$DESTINATION" ] || die "the destination must not be empty"
[ -f "$PREFIX/lib/libgstreamer-1.0.0.dylib" ] \
    || die "$PREFIX is not a GStreamer prefix (no lib/libgstreamer-1.0.0.dylib)"
[ -f "$EXECUTABLE" ] || die "executable $EXECUTABLE does not exist; build it first"
PREFIX="$(cd -- "$PREFIX" && pwd -P)"
PREFIX_LIB="$PREFIX/lib"

PC_FILE="$PREFIX_LIB/pkgconfig/gstreamer-1.0.pc"
[ -f "$PC_FILE" ] || die "$PC_FILE is missing (the development package is needed)"
GSTREAMER_VERSION="$(sed -n 's/^Version: *//p' "$PC_FILE")"
[ -n "$GSTREAMER_VERSION" ] || die "cannot read the GStreamer version from $PC_FILE"

# ── Which plugins: one definition, shared with the bundle checks ──────────
PLUGIN_SCRIPT="$REPOSITORY/scripts/macos/gstreamer-plugins.sh"
plugin_files="$(bash "$PLUGIN_SCRIPT")" || die "cannot determine the plugin list ($PLUGIN_SCRIPT)"
excluded_files="$(bash "$PLUGIN_SCRIPT" --excluded)" \
    || die "cannot determine the excluded plugins ($PLUGIN_SCRIPT --excluded)"
PLUGINS=()
while IFS= read -r file; do
    [ -n "$file" ] || continue
    plugin="${file#lib}"
    PLUGINS+=("${plugin%.dylib}")
done <<<"$plugin_files"
[ "${#PLUGINS[@]}" -gt 0 ] || die "$PLUGIN_SCRIPT listed no plugins"
EXCLUDED_FILES=()
while IFS= read -r file; do
    if [ -n "$file" ]; then EXCLUDED_FILES+=("$file"); fi
done <<<"$excluded_files"

contains() { # needle, haystack...
    local needle="$1" item
    shift
    for item in "$@"; do
        [ "$item" = "$needle" ] && return 0
    done
    return 1
}

for plugin in "${PLUGINS[@]}"; do
    [ -f "$PREFIX_LIB/gstreamer-1.0/lib$plugin.dylib" ] \
        || die "plugin $plugin is missing from $PREFIX_LIB/gstreamer-1.0"
done

# Fails if an excluded plugin is in the directory (a stale copy must not ride along).
require_no_excluded() {
    local file
    for file in ${EXCLUDED_FILES[@]+"${EXCLUDED_FILES[@]}"}; do
        [ ! -e "$1/$file" ] || die "$1/$file is an excluded plugin and must not be shipped"
    done
}

# ── Mach-O helpers ────────────────────────────────────────────────────────
is_system_library() {
    case "$1" in
        /System/Library/* | /usr/lib/*) return 0 ;;
        *) return 1 ;;
    esac
}

ARCH="$(lipo -archs "$EXECUTABLE")" || die "lipo cannot read $EXECUTABLE"
case "$ARCH" in
    *" "*) die "$EXECUTABLE is universal ($ARCH); build for one architecture" ;;
esac

# Fails unless the file has a slice for $ARCH.
require_arch() {
    local archs
    archs="$(lipo -archs "$1")" || die "lipo cannot read $1"
    case " $archs " in
        *" $ARCH "*) ;;
        *) die "$1 has no $ARCH code (it has: $archs)" ;;
    esac
}

# Sets DEPS to the non-system dependencies of $1 (its own install name excluded).
DEPS=()
dependencies_of() {
    local file="$1" id_listing own listing line dependency
    require_arch "$file"
    id_listing="$(otool -arch "$ARCH" -D "$file")" || die "otool -D failed on $file"
    own="$(sed -n '2p' <<<"$id_listing")"
    listing="$(otool -arch "$ARCH" -L "$file")" || die "otool -L failed on $file"
    DEPS=()
    while IFS= read -r line; do
        case "$line" in
            $'\t'*) ;;
            *) continue ;;
        esac
        dependency="${line#$'\t'}"
        dependency="${dependency% (compatibility version*}"
        [ "$dependency" = "$own" ] && continue
        is_system_library "$dependency" && continue
        DEPS+=("$dependency")
    done <<<"$listing"
}

# Sets RESOLVED to the file in the prefix that a dependency line names.
RESOLVED=""
resolve() {
    local dependency="$1" relative directory parent
    case "$dependency" in
        @rpath/*)
            relative="${dependency#@rpath/}"
            for directory in "$PREFIX_LIB" "$PREFIX_LIB/.." "$PREFIX_LIB/gstreamer-1.0"; do
                if [ -f "$directory/$relative" ]; then
                    parent="$(cd -- "$(dirname -- "$directory/$relative")" && pwd -P)" \
                        || die "cannot enter the directory of $directory/$relative"
                    RESOLVED="$parent/$(basename -- "$relative")"
                    return 0
                fi
            done
            die "cannot resolve $dependency in $PREFIX"
            ;;
        "$PREFIX"/*)
            RESOLVED="$dependency"
            ;;
        *)
            die "unexpected non-system dependency $dependency; only the GStreamer framework's libraries can be collected"
            ;;
    esac
}

# ── Marker ────────────────────────────────────────────────────────────────
dependencies_of "$EXECUTABLE"
EXECUTABLE_DEPS=(${DEPS[@]+"${DEPS[@]}"})
MARKER=".gstreamer-$GSTREAMER_VERSION"

marker_inputs() {
    printf 'gstreamer %s\n' "$GSTREAMER_VERSION"
    printf 'architecture %s\n' "$ARCH"
    printf 'plugin %s\n' "${PLUGINS[@]}"
    if [ "${#EXCLUDED_FILES[@]}" -gt 0 ]; then printf 'excluded %s\n' "${EXCLUDED_FILES[@]}"; fi
    if [ "${#EXECUTABLE_DEPS[@]}" -gt 0 ]; then printf 'dependency %s\n' "${EXECUTABLE_DEPS[@]}"; fi
}

# Sets SUMMARY to the number of files and a digest of their names and contents (marker
# excluded). Runs in this shell, not a command substitution, so that `set -e` and `die`
# apply to every step.
SUMMARY=""
contents_summary() {
    local directory="$1" names name hash listing="" count=0 digest
    names="$(cd -- "$directory" && find . -type f ! -name '.gstreamer-*')" || die "cannot list $directory"
    names="$(LC_ALL=C sort <<<"$names")"
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        hash="$(shasum -a 256 -- "$directory/$name")" || die "cannot read $directory/$name"
        listing+="${hash%% *}  $name"$'\n'
        count=$((count + 1))
    done <<<"$names"
    digest="$(shasum -a 256 <<<"$listing")" || die "shasum failed"
    SUMMARY="files $count"$'\n'"digest ${digest%% *}"
}

# The table of bundled libraries in THIRD-PARTY-NOTICES.md must list what ships: a bundle
# whose libraries differ from it is refused. In the build tree (a developer trying an
# exclusion, say) a difference is only reported.
check_notices() {
    local generator="$REPOSITORY/scripts/macos/gstreamer-notices.sh"
    if [ "${MERCURY_GSTREAMER_UPDATE_NOTICES:-0}" = 1 ]; then
        bash "$generator" --write "$PREFIX" "$1" || die "could not rewrite the table in THIRD-PARTY-NOTICES.md"
    elif ! bash "$generator" --check "$PREFIX" "$1"; then
        if [ "$BUNDLE_MODE" = 1 ]; then
            die "THIRD-PARTY-NOTICES.md does not list the libraries of this bundle; rerun with MERCURY_GSTREAMER_UPDATE_NOTICES=1 to regenerate the table, and commit it"
        fi
        say "warning: THIRD-PARTY-NOTICES.md does not list what was collected into $DESTINATION; a bundle with these libraries would be refused until the table is regenerated (MERCURY_GSTREAMER_UPDATE_NOTICES=1)"
    fi
}

INPUTS="$(marker_inputs)"
BUNDLE_MODE=0
case "$DESTINATION" in
    *.app/Contents/MacOS/lib) BUNDLE_MODE=1 ;;
esac
if [ "$BUNDLE_MODE" = 1 ]; then
    if [ -e "$DESTINATION" ] || [ -L "$DESTINATION" ]; then
        die "$DESTINATION already exists; an application bundle's lib directory is collected once, into a fresh bundle"
    fi
else
    # A symbolic link (one collected directory shared by several worktrees) is followed:
    # the directory it points to is checked and, if needed, rebuilt in place, and the link
    # stays. Every worktree that shares it then uses the rebuilt directory.
    if [ -L "$DESTINATION" ]; then
        [ -d "$DESTINATION" ] || die "$DESTINATION is a symbolic link to something that is not a directory"
        LINK="$DESTINATION"
        DESTINATION="$(cd -- "$DESTINATION" && pwd -P)" || die "cannot resolve $LINK"
        say "$LINK links to $DESTINATION; collecting there"
    fi
    if [ -e "$DESTINATION" ]; then
        [ -d "$DESTINATION" ] || die "$DESTINATION exists and is not a directory"
        markers="$(find "$DESTINATION/" -maxdepth 1 -name '.gstreamer-*' -type f)"
        [ -n "$markers" ] \
            || die "$DESTINATION exists but was not made by this script (no .gstreamer-* marker); refusing to replace it"
        if [ -f "$DESTINATION/$MARKER" ]; then
            recorded="$(cat "$DESTINATION/$MARKER")"
            contents_summary "$DESTINATION"
            if [ "$recorded" = "$INPUTS"$'\n'"$SUMMARY" ]; then
                require_no_excluded "$DESTINATION"
                check_notices "$DESTINATION"
                say "ok: $DESTINATION is up to date"
                exit 0
            fi
        fi
        say "$DESTINATION is out of date or damaged; rebuilding it"
    fi
fi

# A fresh, uniquely named staging directory beside the destination: nothing that already
# exists is ever deleted as "staging".
mkdir -p -- "$(dirname -- "$DESTINATION")"
STAGING="$(mktemp -d "$DESTINATION.partial.XXXXXX")" || die "cannot create a staging directory beside $DESTINATION"
trap 'rm -rf -- "$STAGING"' EXIT

# ── Dependency walk ───────────────────────────────────────────────────────
PENDING=(${EXECUTABLE_DEPS[@]+"${EXECUTABLE_DEPS[@]}"})
for plugin in "${PLUGINS[@]}"; do
    PENDING+=("$PREFIX_LIB/gstreamer-1.0/lib$plugin.dylib")
done
SEEN=()
COPIED=0
while [ "${#PENDING[@]}" -gt 0 ]; do
    NEXT=()
    for dependency in "${PENDING[@]}"; do
        resolve "$dependency"
        source_file="$RESOLVED"
        [ -f "$source_file" ] || die "$source_file (needed as $dependency) does not exist"
        name="$(basename -- "$source_file")"
        if [ "${#SEEN[@]}" -gt 0 ] && contains "$name" "${SEEN[@]}"; then
            continue
        fi
        SEEN+=("$name")
        require_arch "$source_file"
        archs="$(lipo -archs "$source_file")" || die "lipo cannot read $source_file"
        if [ "$archs" = "$ARCH" ]; then
            cp -- "$source_file" "$STAGING/$name"
        else
            lipo -thin "$ARCH" "$source_file" -output "$STAGING/$name" \
                || die "lipo could not thin $source_file to $ARCH"
        fi
        chmod u+w "$STAGING/$name"
        COPIED=$((COPIED + 1))
        dependencies_of "$source_file"
        NEXT+=(${DEPS[@]+"${DEPS[@]}"})
    done
    PENDING=(${NEXT[@]+"${NEXT[@]}"})
done

require_no_excluded "$STAGING"
check_notices "$STAGING"
if [ "$BUNDLE_MODE" = 0 ]; then
    contents_summary "$STAGING"
    printf '%s\n%s\n' "$INPUTS" "$SUMMARY" >"$STAGING/$MARKER"
fi
chmod 755 "$STAGING"
rm -rf -- "$DESTINATION"
mv -- "$STAGING" "$DESTINATION"
trap - EXIT
summary="$(du -sh -- "$DESTINATION" | cut -f 1)"
say "ok: collected $COPIED GStreamer libraries into $DESTINATION ($summary)"
