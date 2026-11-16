#!/usr/bin/env bash
# Checks the parts of scripts/macos/bundle.sh that can be tested without a
# build or a signing identity: the identity checks, which entitlement file each
# identity gets and what the files grant, and the helpers in lib.sh that parse
# otool and codesign output.
#
#   scripts/macos/test-bundle.sh      (or: make test-bundle-macos)
#
# Prints one line per check and exits non-zero if any fails. Writes nothing.
set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/../.."
# shellcheck source=scripts/macos/lib.sh
. scripts/macos/lib.sh

RESOURCES="$PWD/apps/mercury-desktop/resources/macos"
STRICT="$RESOURCES/entitlements.plist"
ADHOC="$RESOURCES/entitlements-adhoc.plist"
JIT="com.apple.security.cs.allow-jit=<true/> com.apple.security.cs.allow-unsigned-executable-memory=<true/>"
NO_VALIDATION="com.apple.security.cs.disable-library-validation=<true/>"
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

# Runs a command and prints "ok" or "fails" by its exit status.
status() {
    if "$@"; then printf 'ok'; else printf 'fails'; fi
}

# bundle.sh --print-entitlements with the given arguments, with
# MERCURY_SIGN_IDENTITY unset unless the caller sets it; prints the output and
# the exit status.
print_entitlements() {
    local output code=0
    output="$(scripts/macos/bundle.sh "$@" --print-entitlements 2>&1)" || code=$?
    printf '%s (exit %s)' "$output" "$code"
}

unset MERCURY_SIGN_IDENTITY

# ── Which entitlements each identity gets ─────────────────────────────────
check "ad-hoc identity uses the ad-hoc entitlements" "$ADHOC (exit 0)" "$(print_entitlements --identity -)"
check "default identity is ad hoc" "$ADHOC (exit 0)" "$(print_entitlements)"
check "a Developer ID uses the strict entitlements" "$STRICT (exit 0)" \
    "$(print_entitlements --identity 'Developer ID Application: Example (ABCDE12345)')"
check "an Apple Development identity uses the strict entitlements" "$STRICT (exit 0)" \
    "$(print_entitlements --identity 'Apple Development: someone@example.com (ABCDE12345)')"
check "MERCURY_SIGN_IDENTITY selects the strict entitlements" "$STRICT (exit 0)" \
    "$(MERCURY_SIGN_IDENTITY='Developer ID Application: Example (ABCDE12345)' print_entitlements)"
check "--identity overrides MERCURY_SIGN_IDENTITY" "$ADHOC (exit 0)" \
    "$(MERCURY_SIGN_IDENTITY='Developer ID Application: Example (ABCDE12345)' print_entitlements --identity -)"
check "an empty MERCURY_SIGN_IDENTITY means ad hoc" "$ADHOC (exit 0)" \
    "$(MERCURY_SIGN_IDENTITY='' print_entitlements)"

# ── Odd identities are refused, not signed with ───────────────────────────
check "an empty --identity is refused" \
    "bundle: error: the signing identity is empty; use - for ad hoc (exit 1)" \
    "$(print_entitlements --identity '')"
check "' - ' is refused" \
    "bundle: error: signing identity ' - ' has leading or trailing spaces (exit 1)" \
    "$(print_entitlements --identity ' - ')"
check "-x is refused" \
    "bundle: error: signing identity '-x' begins with '-'; use - for ad hoc, or a certificate name or SHA-1 hash (exit 1)" \
    "$(print_entitlements --identity -x)"
check "--identity without a value is refused" "bundle: error: --identity needs a value (exit 1)" \
    "$(code=0; output="$(scripts/macos/bundle.sh --identity 2>&1)" || code=$?; printf '%s (exit %s)' "$output" "$code")"

# ── What the entitlement files grant ──────────────────────────────────────
check "strict entitlements grant only the JIT keys" "$JIT " "$(plist_grants "$STRICT" | tr '\n' ' ')"
check "ad-hoc entitlements add only library validation" "$JIT $NO_VALIDATION " \
    "$(plist_grants "$ADHOC" | tr '\n' ' ')"

# ── Entitlements read back after signing ──────────────────────────────────
# The single-line form `codesign -d --entitlements - --xml` prints.
readback() {
    printf '<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict>'
    local key
    for key in "$@"; do printf '<key>%s</key><true/>' "$key"; done
    printf '</dict></plist>\n'
}
ADHOC_READBACK="$(readback com.apple.security.cs.allow-jit com.apple.security.cs.allow-unsigned-executable-memory com.apple.security.cs.disable-library-validation)"
STRICT_READBACK="$(readback com.apple.security.cs.allow-jit com.apple.security.cs.allow-unsigned-executable-memory)"
check "read-back ad-hoc entitlements match the ad-hoc file" ok \
    "$(status entitlements_match "$ADHOC" <<<"$ADHOC_READBACK")"
check "read-back strict entitlements match the strict file" ok \
    "$(status entitlements_match "$STRICT" <<<"$STRICT_READBACK")"
check "ad-hoc entitlements on a strict build are a mismatch" fails \
    "$(status entitlements_match "$STRICT" <<<"$ADHOC_READBACK")"
check "strict entitlements on an ad-hoc build are a mismatch" fails \
    "$(status entitlements_match "$ADHOC" <<<"$STRICT_READBACK")"
check "no entitlements at all are a mismatch" fails "$(status entitlements_match "$ADHOC" <<<"")"
check "a key granted as false is a mismatch" fails \
    "$(status entitlements_match "$STRICT" <<<"${STRICT_READBACK/<true\/><\/dict>/<false/></dict>}")"

# ── codesign -dv: runtime flag and team ───────────────────────────────────
check "runtime flag found (ad hoc)" ok \
    "$(status codesign_has_runtime <<<'CodeDirectory v=20500 size=342378 flags=0x10002(adhoc,runtime) hashes=10688+7 location=embedded')"
check "runtime flag found (Developer ID)" ok \
    "$(status codesign_has_runtime <<<'CodeDirectory v=20500 size=342378 flags=0x10000(runtime) hashes=10688+7 location=embedded')"
check "runtime flag missing" fails \
    "$(status codesign_has_runtime <<<'CodeDirectory v=20400 size=252 flags=0x2(adhoc) hashes=2+3 location=embedded')"
check "linker-signed is not runtime" fails \
    "$(status codesign_has_runtime <<<'CodeDirectory v=20400 size=252 flags=0x20002(adhoc,linker-signed) hashes=2+3 location=embedded')"
check "team of an ad-hoc signature" "not set" "$(codesign_team <<<$'Signature=adhoc\nTeamIdentifier=not set')"
check "team of a Developer ID signature" "ABCDE12345" \
    "$(codesign_team <<<$'Authority=Developer ID Application: Example (ABCDE12345)\nTeamIdentifier=ABCDE12345')"
check "ad hoc with libraries is fine" "" "$(team_problem - 3 'not set')"
check "a teamless identity with libraries is refused" \
    "identity 'Self Signed' has no team identifier, so the bundled libraries would fail library validation at launch; use a Developer ID or Apple Development certificate, or - for ad hoc" \
    "$(team_problem 'Self Signed' 3 'not set')"
check "a teamless identity without libraries is allowed" "" "$(team_problem 'Self Signed' 0 'not set')"
check "a Developer ID with libraries is fine" "" "$(team_problem 'Developer ID Application: Example (ABCDE12345)' 3 ABCDE12345)"

# ── What may sit in Contents/MacOS/lib ────────────────────────────────────
header() { # otool -hv output for one or more slices of the given file types
    local type
    for type in "$@"; do
        printf 'Mach header\n      magic  cputype cpusubtype  caps    filetype ncmds sizeofcmds      flags\n'
        printf 'MH_MAGIC_64    ARM64        ALL  0x00 %11s    15       1064   NOUNDEFS DYLDLINK TWOLEVEL\n' "$type"
    done
}
check "a library belongs in lib" "" "$(library_problem lib/libx.dylib <<<"$(header DYLIB)")"
check "a plugin bundle belongs in lib" "" "$(library_problem lib/x.so <<<"$(header BUNDLE)")"
check "a universal library belongs in lib" "" "$(library_problem lib/libx.dylib <<<"$(header DYLIB DYLIB)")"
check "an executable is refused" \
    "lib/scanner is an executable; only libraries belong in Contents/MacOS/lib (an executable there would be signed without entitlements)" \
    "$(library_problem lib/scanner <<<"$(header EXECUTE)")"
check "a universal file with an executable slice is refused" \
    "lib/odd is an executable; only libraries belong in Contents/MacOS/lib (an executable there would be signed without entitlements)" \
    "$(library_problem lib/odd <<<"$(header DYLIB EXECUTE)")"
check "an object file is refused" \
    "lib/x.o is a Mach-O OBJECT, not a library; only libraries belong in Contents/MacOS/lib" \
    "$(library_problem lib/x.o <<<"$(header OBJECT)")"
check "unreadable output is refused" "lib/x: not a Mach-O file otool can read" "$(library_problem lib/x <<<"")"

# ── Large otool -l output through a pipe ──────────────────────────────────
# About 600 KB of load commands, far past the 64 KB pipe buffer, with
# LC_BUILD_VERSION near the top and the rpath at the very end. A parser that
# stops reading early breaks the pipe and, under pipefail, fails the pipeline.
BIG_LISTING="$(awk 'BEGIN {
    print "mercury-browser:"
    print "Load command 0"; print "      cmd LC_BUILD_VERSION"; print "  cmdsize 32"
    print " platform 1"; print "    minos 13.0"; print "      sdk 27.2"
    for (i = 1; i <= 6000; i++) {
        print "Load command " i; print "          cmd LC_LOAD_DYLIB"; print "      cmdsize 72"
        printf "         name @rpath/libgstexample%05d-1.0.0.dylib (offset 24)\n", i
    }
    print "Load command 6001"; print "          cmd LC_RPATH"; print "      cmdsize 40"
    print "         path @executable_path/lib/ (offset 12)"
}')"
check "the generated listing is larger than a pipe buffer" yes \
    "$([ "${#BIG_LISTING}" -gt 500000 ] && echo yes || echo "no (${#BIG_LISTING} bytes)")"
early_exit_reader() { awk '$1 == "minos" { print $2; exit }' >/dev/null; }
# Each pipes the big listing into a reader; they run under the script's pipefail.
big_into_early_exit_reader() { printf '%s\n' "$BIG_LISTING" | early_exit_reader; }
big_into_has_rpath() { printf '%s\n' "$BIG_LISTING" | listing_has_rpath "$1"; }
check "control: an early-exit reader fails on it under pipefail" fails \
    "$(status big_into_early_exit_reader 2>/dev/null)"
check "listing_minos reads it through a pipe" "13.0 ok" \
    "$(set -o pipefail; result="$(printf '%s\n' "$BIG_LISTING" | listing_minos)" && echo "$result ok" || echo "$result fails ($?)")"
check "listing_has_rpath finds the last rpath through a pipe" ok \
    "$(status big_into_has_rpath @executable_path/lib/)"
check "listing_has_rpath reports a missing rpath" fails \
    "$(status big_into_has_rpath @loader_path/)"
check "an LC_LOAD_DYLIB name is not an rpath" fails \
    "$(status big_into_has_rpath @rpath/libgstexample00001-1.0.0.dylib)"

# ── Version and deployment target ─────────────────────────────────────────
check "deployment target, plain form" 13.0 "$(config_deployment_target <<<'MACOSX_DEPLOYMENT_TARGET = "13.0"')"
check "deployment target, table form" 13.0 \
    "$(config_deployment_target <<<'MACOSX_DEPLOYMENT_TARGET = { value = "13.0", force = true }')"
check "deployment target from .cargo/config.toml is a version" yes \
    "$([[ "$(config_deployment_target <.cargo/config.toml)" =~ ^[0-9]+(\.[0-9]+)*$ ]] && echo yes || echo no)"
check "workspace version" 1.2.3 \
    "$(workspace_version <<<$'[package]\nversion = "9.9.9"\n[workspace.package]\nversion = "1.2.3"\nedition = "2024"')"

# ── No bundle without its media libraries ─────────────────────────────────
# Each run stops before writing anything; the --out directory must not appear.
NO_OUT="$(mktemp -u -t mercury-test-bundle)"
bundle_status() {
    local output code=0
    output="$(scripts/macos/bundle.sh --out "$NO_OUT" "$@" 2>&1)" || code=$?
    printf '%s (exit %s)' "$output" "$code"
}
check "bundle.sh refuses to build without a GStreamer prefix" \
    "bundle: error: no GStreamer prefix: pass --gstreamer-prefix or set MERCURY_GSTREAMER_PREFIX (see scripts/macos/gstreamer.sh; make app-macos does this) (exit 1)" \
    "$(unset MERCURY_GSTREAMER_PREFIX; bundle_status)"
# Captured first and matched as a string: no pipe into an early-exiting reader.
contains_text() { case "$1" in *"$2"*) echo yes ;; *) echo no ;; esac; }
check "an empty MERCURY_GSTREAMER_PREFIX is refused too" yes \
    "$(contains_text "$(MERCURY_GSTREAMER_PREFIX='' bundle_status)" 'no GStreamer prefix')"
check "a prefix that does not exist is refused" yes \
    "$(contains_text "$(bundle_status --gstreamer-prefix "$NO_OUT-prefix")" 'does not exist (exit 1)')"
check "a directory that is not a GStreamer prefix is refused" yes \
    "$(contains_text "$(bundle_status --gstreamer-prefix "$PWD/scripts")" 'is not a GStreamer prefix')"
check "the refusals wrote nothing" no "$([ -e "$NO_OUT" ] && echo yes || echo no)"

[ "$failures" -eq 0 ] || { printf '%d check(s) failed\n' "$failures"; exit 1; }
printf 'all checks passed\n'
