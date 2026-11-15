#!/usr/bin/env bash
# Adapted from Zervo, https://github.com/Goddv/Zervo, commit 86cb7126,
# .github/workflows/macos.yml lines 183-222 ("Check the bundle"), MPL-2.0.
#
# Check an assembled Mercury Browser.app: the things that are easy to get
# silently wrong and impossible to notice from a green build.
#
#   scripts/ci/check-macos-bundle.sh [--signature adhoc|developer-id] <app>
#
#   --signature adhoc          require an ad-hoc signature (what CI builds)
#   --signature developer-id   require a Developer ID signature with a secure
#                              timestamp (a signed release)
#   (no --signature)           accept either and check the entitlements that
#                              go with the signature found
#
# Checks, each printed as PASS or FAIL:
#   - Info.plist passes `plutil -lint` and names an executable that exists;
#   - every Mach-O file in the bundle is arm64 only;
#   - every Mach-O file records exactly one minimum macOS version; the
#     executable's equals LSMinimumSystemVersion, and no library's is newer
#     (a library built for an older macOS, such as GStreamer's 11.0, is fine);
#   - `codesign --verify --deep --strict` passes and the hardened runtime is on;
#   - the signature is the kind asked for, and the entitlements are exactly the
#     set scripts/macos/bundle.sh uses for it (the ad-hoc set for ad hoc);
#   - scripts/macos/test-bundle.sh passes.
#
# Runs every check even after a failure, then exits 1 if any failed. Reads the
# bundle; writes nothing except a temporary file it removes. macOS only.
set -euo pipefail

usage() {
    sed -n '/^# Check an assembled/,/^# bundle; writes/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

die() {
    printf 'check-macos-bundle: error: %s\n' "$*" >&2
    exit 2
}

WANT_SIGNATURE=""
APP=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --signature)
            WANT_SIGNATURE="${2:?--signature needs adhoc or developer-id}"
            shift 2
            ;;
        -h | --help) usage; exit 0 ;;
        -*) die "unknown option: $1 (try --help)" ;;
        *)
            [ -z "$APP" ] || die "more than one bundle given"
            APP="$1"
            shift
            ;;
    esac
done
case "$WANT_SIGNATURE" in
    "" | adhoc | developer-id) ;;
    *) die "--signature must be adhoc or developer-id, not '$WANT_SIGNATURE'" ;;
esac
[ -n "$APP" ] || die "no bundle given (try --help)"
[ "$(uname -s)" = "Darwin" ] || die "this checks a macOS application bundle and must run on macOS"
[ -d "$APP/Contents" ] || die "$APP is not an application bundle (no Contents directory)"
for tool in codesign file lipo otool plutil; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool not found (install the Xcode command-line tools)"
done

REPO="$(cd "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
failures=0

pass() {
    printf 'PASS %s\n' "$*"
}

fail() {
    printf 'FAIL %s\n' "$*"
    failures=$((failures + 1))
}

# 0 when dotted version $1 <= $2 (numeric, up to three components).
version_le() {
    local a1 a2 a3 b1 b2 b3
    IFS=. read -r a1 a2 a3 <<<"$1"
    IFS=. read -r b1 b2 b3 <<<"$2"
    a1=${a1:-0} a2=${a2:-0} a3=${a3:-0} b1=${b1:-0} b2=${b2:-0} b3=${b3:-0}
    [ "$a1" -lt "$b1" ] && return 0
    [ "$a1" -gt "$b1" ] && return 1
    [ "$a2" -lt "$b2" ] && return 0
    [ "$a2" -gt "$b2" ] && return 1
    [ "$a3" -le "$b3" ]
}

# Every minimum-macOS record in a Mach-O file, one per line: LC_BUILD_VERSION's
# `minos`, or LC_VERSION_MIN_MACOSX's `version` for files built by older tools.
minimum_versions() {
    otool -l "$1" | awk '
        $1 == "cmd" { cmd = $2 }
        cmd == "LC_BUILD_VERSION" && $1 == "minos" { print $2 }
        cmd == "LC_VERSION_MIN_MACOSX" && $1 == "version" { print $2 }'
}

# The entitlements in a plist file as sorted `key=value` lines (values are
# `true`, `false` or `other`), so two files can be compared exactly.
entitlement_lines() {
    plutil -convert xml1 -o - "$1" | awk '
        /<key>/ { sub(/.*<key>/, ""); sub(/<\/key>.*/, ""); key = $0; next }
        key != "" && /<true\/>/ { print key "=true"; key = ""; next }
        key != "" && /<false\/>/ { print key "=false"; key = ""; next }
        key != "" && NF { print key "=other"; key = "" }' | sort
}

TMP="$(mktemp)"
trap 'rm -f -- "$TMP"' EXIT

printf 'checking %s\n' "$APP"

# ── Info.plist and the executable ─────────────────────────────────────────
PLIST="$APP/Contents/Info.plist"
EXECUTABLE=""
MIN_MACOS=""
if [ -f "$PLIST" ] && plutil -lint "$PLIST" >/dev/null; then
    pass "Info.plist passes plutil -lint"
    EXECUTABLE="$(plutil -extract CFBundleExecutable raw -o - "$PLIST" 2>/dev/null || true)"
    MIN_MACOS="$(plutil -extract LSMinimumSystemVersion raw -o - "$PLIST" 2>/dev/null || true)"
else
    fail "Info.plist is missing or does not pass plutil -lint"
fi
BIN="$APP/Contents/MacOS/$EXECUTABLE"
if [ -n "$EXECUTABLE" ] && [ -f "$BIN" ]; then
    pass "CFBundleExecutable '$EXECUTABLE' exists"
else
    fail "CFBundleExecutable '$EXECUTABLE' is not a file in Contents/MacOS"
    BIN=""
fi
if [ -n "$MIN_MACOS" ]; then
    pass "LSMinimumSystemVersion is $MIN_MACOS"
else
    fail "Info.plist has no LSMinimumSystemVersion"
fi

# ── Every Mach-O: architecture and minimum macOS ──────────────────────────
machos=0
while IFS= read -r -d '' path; do
    case "$(file -b "$path")" in
        Mach-O*) ;;
        *) continue ;;
    esac
    machos=$((machos + 1))
    name="${path#"$APP"/}"
    archs="$(lipo -archs "$path" 2>/dev/null || true)"
    if [ "$archs" = "arm64" ]; then
        pass "$name is arm64 only"
    else
        fail "$name is built for '$archs', not arm64 only"
    fi
    minimum_versions "$path" >"$TMP"
    count="$(wc -l <"$TMP" | tr -d ' ')"
    minos="$(head -n 1 "$TMP")"
    if [ "$count" != 1 ]; then
        fail "$name records $count minimum macOS versions ($(tr '\n' ' ' <"$TMP")), not exactly one"
    elif [ -z "$MIN_MACOS" ]; then
        fail "$name targets macOS $minos, but there is no LSMinimumSystemVersion to compare with"
    elif [ "$path" = "$BIN" ]; then
        if [ "$minos" = "$MIN_MACOS" ]; then
            pass "$name targets macOS $minos, as LSMinimumSystemVersion says"
        else
            fail "$name targets macOS $minos but LSMinimumSystemVersion is $MIN_MACOS"
        fi
    elif version_le "$minos" "$MIN_MACOS"; then
        pass "$name targets macOS $minos, not newer than $MIN_MACOS"
    else
        fail "$name needs macOS $minos, newer than LSMinimumSystemVersion $MIN_MACOS"
    fi
done < <(find "$APP/Contents" -type f -print0)
if [ "$machos" -gt 0 ]; then
    pass "found $machos Mach-O file(s)"
else
    fail "no Mach-O files in the bundle"
fi

# ── Signature ─────────────────────────────────────────────────────────────
if codesign --verify --deep --strict --verbose=2 "$APP" >"$TMP" 2>&1; then
    pass "codesign --verify --deep --strict"
else
    fail "codesign --verify --deep --strict: $(tr '\n' ' ' <"$TMP")"
fi

codesign -dv --verbose=4 "$APP" >"$TMP" 2>&1 || true
if grep -q '^Signature=adhoc' "$TMP"; then
    SIGNATURE="adhoc"
    IDENTITY="-"
elif grep -q '^Authority=Developer ID Application: ' "$TMP"; then
    SIGNATURE="developer-id"
    IDENTITY="$(sed -n 's/^Authority=\(Developer ID Application: .*\)$/\1/p' "$TMP" | head -n 1)"
else
    SIGNATURE="other"
    IDENTITY=""
fi
if [ -z "$WANT_SIGNATURE" ] && [ "$SIGNATURE" != other ]; then
    pass "signature is $SIGNATURE"
elif [ "$SIGNATURE" = "$WANT_SIGNATURE" ]; then
    pass "signature is $SIGNATURE, as required"
else
    fail "signature is $SIGNATURE, expected ${WANT_SIGNATURE:-adhoc or developer-id}"
fi
if grep -Eq '^CodeDirectory .*flags=0x[0-9a-f]+\([^)]*runtime' "$TMP"; then
    pass "hardened runtime is on"
else
    fail "hardened runtime is off: $(grep '^CodeDirectory' "$TMP" || echo 'no CodeDirectory line')"
fi
if [ "$SIGNATURE" = developer-id ]; then
    if grep -q '^Timestamp=' "$TMP" && ! grep -q '^TeamIdentifier=not set' "$TMP"; then
        pass "Developer ID signature has a secure timestamp and a team identifier"
    else
        fail "Developer ID signature lacks a secure timestamp or a team identifier"
    fi
fi

# ── Entitlements: exactly the set bundle.sh chooses for this signature ────
if [ -n "$IDENTITY" ]; then
    EXPECTED="$("$REPO/scripts/macos/bundle.sh" --identity "$IDENTITY" --print-entitlements)"
    if codesign -d --entitlements - --xml "$APP" >"$TMP" 2>/dev/null && [ -s "$TMP" ] &&
        plutil -lint "$TMP" >/dev/null; then
        actual="$(entitlement_lines "$TMP")"
        expected="$(entitlement_lines "$EXPECTED")"
        if [ "$actual" = "$expected" ]; then
            pass "entitlements are exactly ${EXPECTED#"$REPO"/}: $(printf '%s' "$actual" | tr '\n' ' ')"
        else
            fail "entitlements [$(printf '%s' "$actual" | tr '\n' ' ')] differ from ${EXPECTED#"$REPO"/} [$(printf '%s' "$expected" | tr '\n' ' ')]"
        fi
    else
        fail "the bundle carries no readable entitlements"
    fi
else
    fail "entitlements not checked: the signature is neither ad hoc nor Developer ID"
fi

# ── The bundle script's own checks ────────────────────────────────────────
if "$REPO/scripts/macos/test-bundle.sh" >"$TMP" 2>&1; then
    pass "scripts/macos/test-bundle.sh"
else
    fail "scripts/macos/test-bundle.sh:"
    sed 's/^/    /' "$TMP"
fi

if [ "$failures" -gt 0 ]; then
    printf '%d check(s) failed for %s\n' "$failures" "$APP"
    exit 1
fi
printf 'all checks passed for %s\n' "$APP"
