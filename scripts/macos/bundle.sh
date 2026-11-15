#!/usr/bin/env bash
# Adapted from Zervo, https://github.com/Goddv/Zervo, commit 86cb7126,
# scripts/bundle-macos.sh (deployment target :70-75, Info.plist :192-251,
# signing :272-327, disk image :333-434), MPL-2.0; and from Servo,
# https://github.com/servo/servo, tag v0.8.0, etc/macos_sign.py (hardened-
# runtime signing and verification), Apache-2.0 OR MIT.
#
# Assemble, sign and optionally package Mercury Browser.app.
#
#   scripts/macos/bundle.sh --gstreamer-prefix <dir> [--binary <path>] [--out <dir>]
#                           [--identity <id>] [--dmg]
#   (or MERCURY_GSTREAMER_PREFIX instead of --gstreamer-prefix; one of them is required)
#   scripts/macos/bundle.sh [--identity <id>] --print-entitlements
#
#   --binary <path>            the arm64 executable to bundle
#                              (default target/release/mercury-browser)
#   --out <dir>                where the .app (and the .dmg) are written
#                              (default dist); nothing is written anywhere else
#   --identity <id>            codesign identity; "-" signs ad hoc (default
#                              $MERCURY_SIGN_IDENTITY, else "-"). Besides ad
#                              hoc, only identities that carry a team
#                              identifier are supported (Developer ID
#                              Application, Apple Development); a self-signed
#                              certificate is refused when libraries are bundled
#   --gstreamer-prefix <dir>   GStreamer prefix handed to
#                              scripts/macos/collect-gstreamer.sh, which fills
#                              Contents/MacOS/lib (default
#                              $MERCURY_GSTREAMER_PREFIX). Required: the
#                              executable cannot start without its media
#                              libraries, so no bundle is built without them
#   --dmg                      also build mercury-browser-<version>-macos-arm64.dmg
#                              and its .sha256
#   --print-entitlements       print the entitlements file the identity would
#                              be signed with, and exit without building
#
# Relative paths are taken from the directory the script is started in. Safe
# to re-run: the bundle and the disk image are rebuilt from scratch each time.
set -euo pipefail

die() {
    printf 'bundle: error: %s\n' "$*" >&2
    exit 1
}

say() {
    printf 'bundle: %s\n' "$*" >&2
}

usage() {
    sed -n '/^# Assemble/,/^# to re-run/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

# shellcheck source=scripts/macos/lib.sh
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

absolute() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s/%s\n' "$PWD" "$1" ;;
    esac
}

[ "$(uname -s)" = "Darwin" ] || die "this builds a macOS application bundle and must run on macOS"

BINARY="target/release/mercury-browser"
OUT="dist"
IDENTITY="${MERCURY_SIGN_IDENTITY:--}"
GSTREAMER_PREFIX="${MERCURY_GSTREAMER_PREFIX:-}"
MAKE_DMG=0
PRINT_ENTITLEMENTS=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --binary | --out | --identity | --gstreamer-prefix)
            [ "$#" -ge 2 ] || die "$1 needs a value"
            case "$1" in
                --binary) BINARY="$2" ;;
                --out) OUT="$2" ;;
                # An empty identity is reported by identity_problem below.
                --identity) IDENTITY="$2" ;;
                --gstreamer-prefix) GSTREAMER_PREFIX="$2" ;;
            esac
            shift 2
            ;;
        --dmg) MAKE_DMG=1; shift ;;
        --print-entitlements) PRINT_ENTITLEMENTS=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done

[ -n "$BINARY" ] || die "--binary needs a non-empty path"
[ -n "$OUT" ] || die "--out needs a non-empty directory"
problem="$(identity_problem "$IDENTITY")"
[ -z "$problem" ] || die "$problem"

BINARY="$(absolute "$BINARY")"
OUT="$(absolute "$OUT")"
if [ -n "$GSTREAMER_PREFIX" ]; then
    GSTREAMER_PREFIX="$(absolute "$GSTREAMER_PREFIX")"
    [ -d "$GSTREAMER_PREFIX" ] || die "GStreamer prefix $GSTREAMER_PREFIX does not exist"
fi

REPO="$(cd "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO"

ENTITLEMENTS="$REPO/$(entitlements_for_identity "$IDENTITY")"
if [ "$PRINT_ENTITLEMENTS" = 1 ]; then
    printf '%s\n' "$ENTITLEMENTS"
    exit 0
fi

# The executable links GStreamer and stops in the dynamic loader without it, so a
# bundle without its media libraries must not exist.
COLLECTOR="scripts/macos/collect-gstreamer.sh"
[ -n "$GSTREAMER_PREFIX" ] ||
    die "no GStreamer prefix: pass --gstreamer-prefix or set MERCURY_GSTREAMER_PREFIX (see scripts/macos/gstreamer.sh; make app-macos does this)"
[ -f "$GSTREAMER_PREFIX/lib/libgstreamer-1.0.0.dylib" ] ||
    die "$GSTREAMER_PREFIX is not a GStreamer prefix (no lib/libgstreamer-1.0.0.dylib)"
[ -f "$COLLECTOR" ] || die "$COLLECTOR is missing"

for tool in codesign ditto file hdiutil install_name_tool lipo otool plutil shasum; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool not found (install the Xcode command-line tools)"
done

RESOURCES_SRC="apps/mercury-desktop/resources/macos"
APP_NAME="Mercury Browser"
EXECUTABLE="mercury-browser"
IDENTIFIER="io.github.thepigdestroyer.mercury-browser"
APP="$OUT/$APP_NAME.app"
BIN_DIR="$APP/Contents/MacOS"
RES_DIR="$APP/Contents/Resources"
BIN="$BIN_DIR/$EXECUTABLE"

# ── Version and minimum macOS ─────────────────────────────────────────────
# Both come from where the build takes them, so they cannot drift: the
# workspace version from Cargo.toml, and the deployment target the compiler was
# given from .cargo/config.toml (either `= "13.0"` or `= { value = "13.0", … }`).
VERSION="$(workspace_version <Cargo.toml)"
[ -n "$VERSION" ] || die "no version in the [workspace.package] table of Cargo.toml"
# CFBundleVersion must be one to three dot-separated integers.
[[ "$VERSION" =~ ^[0-9]+(\.[0-9]+){0,2}$ ]] ||
    die "version '$VERSION' is not usable as CFBundleVersion (one to three numbers separated by dots)"

MIN_MACOS="$(config_deployment_target <.cargo/config.toml)"
[ -n "$MIN_MACOS" ] || die "no MACOSX_DEPLOYMENT_TARGET in .cargo/config.toml"

# ── The executable ────────────────────────────────────────────────────────
[ -f "$BINARY" ] || die "no executable at $BINARY (run: cargo build --release -p mercury-desktop)"
ARCHS="$(lipo -archs "$BINARY")"
[ "$ARCHS" = "arm64" ] || die "$BINARY is built for '$ARCHS'; Mercury Browser for macOS is arm64 only"

# The binary records the deployment target it was linked for. If the
# environment overrode .cargo/config.toml, the bundle would claim a different
# minimum than the code needs.
# (Output is captured first and parsed from a here-string; see lib.sh.)
LOAD_COMMANDS="$(otool -l "$BINARY")" || die "otool cannot read $BINARY"
BINARY_MINOS="$(listing_minos <<<"$LOAD_COMMANDS")"
[ "$BINARY_MINOS" = "$MIN_MACOS" ] ||
    die "$BINARY was linked for macOS '$BINARY_MINOS' but .cargo/config.toml says $MIN_MACOS; rebuild without overriding MACOSX_DEPLOYMENT_TARGET"

say "assembling $APP ($VERSION, macOS $MIN_MACOS or later)"
mkdir -p "$OUT"
rm -rf -- "$APP"

# From here until it verifies, the bundle is incomplete; a failure removes it
# so that nothing unsigned or half-built is left looking like a result.
STAGE="$OUT/.dmg-staging"
APP_COMPLETE=0
cleanup() {
    rm -rf -- "$STAGE"
    [ "$APP_COMPLETE" = 1 ] || rm -rf -- "$APP"
}
trap cleanup EXIT

mkdir -p "$BIN_DIR" "$RES_DIR"

cp "$BINARY" "$BIN"
chmod 755 "$BIN"

sed -e "s/@VERSION@/$VERSION/g" -e "s/@MINIMUM_SYSTEM_VERSION@/$MIN_MACOS/g" \
    "$RESOURCES_SRC/Info.plist.in" >"$APP/Contents/Info.plist"
if grep -q '@[A-Z_]*@' "$APP/Contents/Info.plist"; then
    die "Info.plist still has an unfilled placeholder"
fi
plutil -lint "$APP/Contents/Info.plist" >/dev/null || die "Info.plist does not validate"
printf 'APPL????' >"$APP/Contents/PkgInfo"

cp "$RESOURCES_SRC/AppIcon.icns" "$RES_DIR/AppIcon.icns"
cp LICENSE THIRD-PARTY-NOTICES.md "$RES_DIR/"
ditto LICENSES "$RES_DIR/LICENSES"

# ── Media libraries ───────────────────────────────────────────────────────
# Servo looks for its GStreamer libraries and plugins in <executable dir>/lib.
# The collector copies them from the prefix; inside a bundle it writes nothing
# but the libraries (no marker), since Contents/MacOS may hold only code.
say "collecting media libraries from $GSTREAMER_PREFIX"
bash "$COLLECTOR" "$GSTREAMER_PREFIX" "$BIN_DIR/lib" "$BIN" || die "collecting the media libraries failed"

# Every plugin of the shared definition is there, and no excluded one.
PLUGIN_FILES="$(bash scripts/macos/gstreamer-plugins.sh)" || die "cannot list the media plugins"
EXCLUDED_FILES="$(bash scripts/macos/gstreamer-plugins.sh --excluded)" || die "cannot list the excluded plugins"
while IFS= read -r plugin; do
    [ -z "$plugin" ] || [ -f "$BIN_DIR/lib/$plugin" ] || die "media plugin $plugin is missing from $BIN_DIR/lib"
done <<<"$PLUGIN_FILES"
while IFS= read -r plugin; do
    [ -z "$plugin" ] || [ ! -e "$BIN_DIR/lib/$plugin" ] || die "excluded plugin $plugin is in $BIN_DIR/lib"
done <<<"$EXCLUDED_FILES"

# The licence files of every bundled project, as the GStreamer packages ship them.
PROJECTS="$(bash scripts/macos/gstreamer-notices.sh --projects "$GSTREAMER_PREFIX" "$BIN_DIR/lib")" ||
    die "cannot list the projects of the bundled libraries"
while IFS= read -r project; do
    if [ -d "$GSTREAMER_PREFIX/share/licenses/$project" ]; then
        # Not "licenses": macOS file systems are case-insensitive, and LICENSES/ is there.
        mkdir -p "$RES_DIR/gstreamer-licenses"
        ditto "$GSTREAMER_PREFIX/share/licenses/$project" "$RES_DIR/gstreamer-licenses/$project"
    fi
done <<<"$PROJECTS"

# The executable finds the bundled libraries through this rpath. Builds that
# already link it in are left alone; editing the load commands invalidates the
# linker's signature, which the signing step below replaces anyway.
LOAD_COMMANDS="$(otool -l "$BIN")" || die "otool cannot read $BIN"
if ! listing_has_rpath @executable_path/lib/ <<<"$LOAD_COMMANDS"; then
    say "adding rpath @executable_path/lib/"
    install_name_tool -add_rpath @executable_path/lib/ "$BIN"
fi

# ── Signing ───────────────────────────────────────────────────────────────
# Inside out, never with --deep (deprecated for signing since macOS 13, and it
# gives nested code the bundle's options): every Mach-O under
# Contents/MacOS/lib, then the main executable, then the bundle.
#
# Options follow `--sign <identity>`. Zervo reports codesign ignoring
# `--options` placed before `--sign`; with the codesign of macOS 27.2 both
# orders gave flags=runtime, but keeping this order costs nothing.
#
# The hardened runtime is used for every identity, ad hoc included, so local
# builds run under the same rules as a notarised release; the entitlements let
# the JavaScript JIT work under it, and the ad-hoc set also turns off library
# validation (see entitlements_for_identity in lib.sh). A bundle's signature is
# stored in its main executable, so signing the bundle re-signs that file: the bundle step must
# repeat the runtime option and the entitlements or they would be dropped.
SIGN_ARGS=(--force --sign "$IDENTITY" --options runtime)
if [ "$IDENTITY" != "-" ]; then
    # A secure timestamp is required for notarisation; ad-hoc signatures cannot have one.
    SIGN_ARGS+=(--timestamp)
fi
EXECUTABLE_SIGN_ARGS=("${SIGN_ARGS[@]}" --entitlements "$ENTITLEMENTS")

LIBRARIES=()
# Process substitution, not a pipe, so that `set -e` and `die` apply in the
# loop; -print0 and `read -d ''` so a path with a space survives. Anything that
# is not Mach-O code (a marker file, say) would make codesign refuse the bundle.
while IFS= read -r -d '' lib; do
    case "$(file -b "$lib")" in
        Mach-O*) LIBRARIES+=("$lib") ;;
        *) die "${lib#"$APP/"} is not code; only libraries belong in Contents/MacOS/lib" ;;
    esac
done < <(find "$BIN_DIR/lib" ! -type d -print0)

# Only libraries and loadable plugins belong in Contents/MacOS/lib. They are
# signed below without entitlements, so an executable there (a plugin scanner,
# say) would run with library validation on and, in an ad-hoc build, fail to
# load the libraries beside it. Stop rather than ship that.
for lib in ${LIBRARIES[@]+"${LIBRARIES[@]}"}; do
    header="$(otool -hv "$lib")" || die "otool cannot read $lib"
    problem="$(library_problem "${lib#"$APP/"}" <<<"$header")"
    [ -z "$problem" ] || die "$problem"
done

say "signing with identity '$IDENTITY'"
if [ "$IDENTITY" = "-" ]; then
    say "entitlements: ad-hoc set (JIT, library validation off): ${ENTITLEMENTS#"$REPO/"}"
else
    say "entitlements: strict set (JIT, library validation on): ${ENTITLEMENTS#"$REPO/"}"
fi
# Every library gets the app's identity, so with a Developer ID they share its
# team identifier and pass library validation.
for lib in ${LIBRARIES[@]+"${LIBRARIES[@]}"}; do
    codesign "${SIGN_ARGS[@]}" "$lib" || die "codesign failed on $lib"
done
[ "${#LIBRARIES[@]}" -eq 0 ] || say "signed ${#LIBRARIES[@]} libraries in Contents/MacOS/lib"
codesign "${EXECUTABLE_SIGN_ARGS[@]}" --identifier "$IDENTIFIER" "$BIN" ||
    die "codesign failed on $BIN"
codesign "${EXECUTABLE_SIGN_ARGS[@]}" "$APP" || die "codesign failed on $APP"

codesign --verify --deep --strict --verbose=2 "$APP" || die "$APP does not verify"

# `codesign --verify` checks neither the hardened runtime nor the
# entitlements, and a signing step that silently drops both still verifies.
# Read them back from the executable and from the bundle and compare.
for signed in "$BIN" "$APP"; do
    info="$(codesign -dv "$signed" 2>&1)" || die "cannot read the signature of $signed"
    codesign_has_runtime <<<"$info" || die "$signed is not signed with the hardened runtime"
    granted="$(codesign -d --entitlements - --xml "$signed" 2>/dev/null)" ||
        die "cannot read the entitlements of $signed"
    entitlements_match "$ENTITLEMENTS" <<<"$granted" ||
        die "the entitlements of $signed do not match ${ENTITLEMENTS#"$REPO/"}: expected [$(plist_grants "$ENTITLEMENTS" | tr '\n' ' ')], found [$(plist_grants - <<<"$granted" | tr '\n' ' ')]"
done
TEAM="$(codesign_team <<<"$info")"
problem="$(team_problem "$IDENTITY" "${#LIBRARIES[@]}" "$TEAM")"
[ -z "$problem" ] || die "$problem"
say "verified: hardened runtime and entitlements on the executable and the bundle (team: ${TEAM:-unknown})"
APP_COMPLETE=1
say "built $APP"

# ── Disk image ────────────────────────────────────────────────────────────
if [ "$MAKE_DMG" = 1 ]; then
    DMG_NAME="mercury-browser-$VERSION-macos-arm64.dmg"
    DMG="$OUT/$DMG_NAME"
    rm -rf -- "$STAGE"
    mkdir -p "$STAGE"

    # ditto keeps the signature, extended attributes and symlinks intact.
    ditto "$APP" "$STAGE/$APP_NAME.app"
    ln -s /Applications "$STAGE/Applications"

    say "creating $DMG"
    rm -f -- "$DMG" "$DMG.sha256"
    # ULFO (LZFSE) is smaller and faster to make than UDZO and mounts on any
    # macOS since 10.11, well below the minimum.
    hdiutil create -quiet -volname "$APP_NAME $VERSION" -srcfolder "$STAGE" \
        -format ULFO -ov "$DMG"
    rm -rf -- "$STAGE"

    if [ "$IDENTITY" != "-" ]; then
        codesign --force --sign "$IDENTITY" --timestamp "$DMG" || die "codesign failed on $DMG"
    fi

    (cd "$OUT" && shasum -a 256 "$DMG_NAME" >"$DMG_NAME.sha256")
    say "built $DMG"
fi
