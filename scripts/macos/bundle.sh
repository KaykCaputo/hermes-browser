#!/usr/bin/env bash
# Adapted from Zervo, https://github.com/Goddv/Zervo, commit 86cb7126,
# scripts/bundle-macos.sh (deployment target :70-75, Info.plist :192-251,
# signing :272-327, disk image :333-434), MPL-2.0; and from Servo,
# https://github.com/servo/servo, tag v0.8.0, etc/macos_sign.py (hardened-
# runtime signing and verification), Apache-2.0 OR MIT.
#
# Assemble, sign and optionally package Mercury Browser.app.
#
#   scripts/macos/bundle.sh [--binary <path>] [--out <dir>] [--identity <id>]
#                           [--gstreamer-prefix <dir>] [--dmg]
#
#   --binary <path>            the arm64 executable to bundle
#                              (default target/release/mercury-browser)
#   --out <dir>                where the .app (and the .dmg) are written
#                              (default dist); nothing is written anywhere else
#   --identity <id>            codesign identity; "-" signs ad hoc (default
#                              $MERCURY_SIGN_IDENTITY, else "-")
#   --gstreamer-prefix <dir>   GStreamer prefix handed to
#                              scripts/macos/collect-gstreamer.sh, which fills
#                              Contents/MacOS/lib (default
#                              $MERCURY_GSTREAMER_PREFIX; unset = no media)
#   --dmg                      also build mercury-browser-<version>-macos-arm64.dmg
#                              and its .sha256
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

while [ "$#" -gt 0 ]; do
    case "$1" in
        --binary) BINARY="${2:?--binary needs a path}"; shift 2 ;;
        --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
        --identity) IDENTITY="${2:?--identity needs a codesign identity}"; shift 2 ;;
        --gstreamer-prefix) GSTREAMER_PREFIX="${2:?--gstreamer-prefix needs a directory}"; shift 2 ;;
        --dmg) MAKE_DMG=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done

BINARY="$(absolute "$BINARY")"
OUT="$(absolute "$OUT")"
if [ -n "$GSTREAMER_PREFIX" ]; then
    GSTREAMER_PREFIX="$(absolute "$GSTREAMER_PREFIX")"
    [ -d "$GSTREAMER_PREFIX" ] || die "GStreamer prefix $GSTREAMER_PREFIX does not exist"
fi

REPO="$(cd "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO"

for tool in codesign ditto file hdiutil install_name_tool lipo otool plutil shasum; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool not found (install the Xcode command-line tools)"
done

RESOURCES_SRC="apps/mercury-desktop/resources/macos"
ENTITLEMENTS="$REPO/$RESOURCES_SRC/entitlements.plist"
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
VERSION="$(awk '
    /^\[/ { in_package = ($0 == "[workspace.package]") }
    in_package && /^version[[:space:]]*=/ {
        sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit
    }' Cargo.toml)"
[ -n "$VERSION" ] || die "no version in the [workspace.package] table of Cargo.toml"
# CFBundleVersion must be one to three dot-separated integers.
[[ "$VERSION" =~ ^[0-9]+(\.[0-9]+){0,2}$ ]] ||
    die "version '$VERSION' is not usable as CFBundleVersion (one to three numbers separated by dots)"

MIN_MACOS="$(sed -n 's/^MACOSX_DEPLOYMENT_TARGET[^"]*"\([0-9.]*\)".*/\1/p' .cargo/config.toml | head -n 1)"
[ -n "$MIN_MACOS" ] || die "no MACOSX_DEPLOYMENT_TARGET in .cargo/config.toml"

# ── The executable ────────────────────────────────────────────────────────
[ -f "$BINARY" ] || die "no executable at $BINARY (run: cargo build --release -p mercury-desktop)"
ARCHS="$(lipo -archs "$BINARY")"
[ "$ARCHS" = "arm64" ] || die "$BINARY is built for '$ARCHS'; Mercury Browser for macOS is arm64 only"

# The binary records the deployment target it was linked for. If the
# environment overrode .cargo/config.toml, the bundle would claim a different
# minimum than the code needs.
BINARY_MINOS="$(otool -l "$BINARY" | awk '/LC_BUILD_VERSION/ { found = 1 } found && $1 == "minos" { print $2; exit }')"
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

# ── Media libraries (hook) ────────────────────────────────────────────────
# Servo looks for its GStreamer libraries and plugins in <executable dir>/lib.
# The collector is provided separately; it copies them from a GStreamer prefix:
#   scripts/macos/collect-gstreamer.sh <prefix> <destination lib dir>
COLLECTOR="scripts/macos/collect-gstreamer.sh"
if [ -f "$COLLECTOR" ] && [ -n "$GSTREAMER_PREFIX" ]; then
    say "collecting media libraries from $GSTREAMER_PREFIX"
    bash "$COLLECTOR" "$GSTREAMER_PREFIX" "$BIN_DIR/lib"
elif [ -f "$COLLECTOR" ]; then
    say "media libraries were not bundled: no GStreamer prefix configured (--gstreamer-prefix or MERCURY_GSTREAMER_PREFIX)"
else
    say "media libraries were not bundled: $COLLECTOR is not present"
fi

# The executable finds the bundled libraries through this rpath. Builds that
# already link it in are left alone; editing the load commands invalidates the
# linker's signature, which the signing step below replaces anyway.
if ! otool -l "$BIN" | awk '$1 == "path" { print $2 }' | grep -qx '@executable_path/lib/'; then
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
# The hardened runtime is used even for an ad-hoc signature so that local
# builds run under the same rules as a notarised release; the entitlements let
# the JavaScript JIT work under it. A bundle's signature is stored in its main
# executable, so signing the bundle re-signs that file: the bundle step must
# repeat the runtime option and the entitlements or they would be dropped.
SIGN_ARGS=(--force --sign "$IDENTITY" --options runtime)
if [ "$IDENTITY" != "-" ]; then
    # A secure timestamp is required for notarisation; ad-hoc signatures cannot have one.
    SIGN_ARGS+=(--timestamp)
fi
EXECUTABLE_SIGN_ARGS=("${SIGN_ARGS[@]}" --entitlements "$ENTITLEMENTS")

LIBRARIES=()
if [ -d "$BIN_DIR/lib" ]; then
    # Process substitution, not a pipe, so that `set -e` and `die` apply in
    # the loop; -print0 and `read -d ''` so a path with a space survives.
    while IFS= read -r -d '' lib; do
        case "$(file -b "$lib")" in
            Mach-O*) LIBRARIES+=("$lib") ;;
        esac
    done < <(find "$BIN_DIR/lib" -type f -print0)
fi

# The hardened runtime turns on library validation: the process may load only
# libraries signed by Apple or by the same team. Ad-hoc signatures have no
# team, so an ad-hoc, hardened executable cannot load an ad-hoc library from
# Contents/MacOS/lib ("mapping process and mapped file (non-platform) have
# different Team IDs") and would fail at launch. Refuse rather than produce
# that bundle; sign with a Developer ID to bundle libraries.
if [ "$IDENTITY" = "-" ] && [ "${#LIBRARIES[@]}" -gt 0 ]; then
    die "the ${#LIBRARIES[@]} Mach-O files in Contents/MacOS/lib cannot be loaded by an ad-hoc signed executable under the hardened runtime; sign with a Developer ID (--identity) or build without them"
fi

say "signing with identity '$IDENTITY'"
for lib in ${LIBRARIES[@]+"${LIBRARIES[@]}"}; do
    codesign "${SIGN_ARGS[@]}" "$lib" || die "codesign failed on $lib"
done
[ "${#LIBRARIES[@]}" -eq 0 ] || say "signed ${#LIBRARIES[@]} libraries in Contents/MacOS/lib"
codesign "${EXECUTABLE_SIGN_ARGS[@]}" --identifier "$IDENTIFIER" "$BIN" ||
    die "codesign failed on $BIN"
codesign "${EXECUTABLE_SIGN_ARGS[@]}" "$APP" || die "codesign failed on $APP"

codesign --verify --deep --strict --verbose=2 "$APP" || die "$APP does not verify"
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
