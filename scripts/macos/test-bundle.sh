#!/usr/bin/env bash
# Checks the parts of scripts/macos/bundle.sh that can be tested without a
# build or a signing identity: which entitlement file each identity gets, and
# what the two files grant.
#
#   scripts/macos/test-bundle.sh
#
# Prints one line per check and exits non-zero if any fails. Writes nothing.
set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/../.."

RESOURCES="$PWD/apps/mercury-desktop/resources/macos"
STRICT="$RESOURCES/entitlements.plist"
ADHOC="$RESOURCES/entitlements-adhoc.plist"
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

chosen() {
    env -u MERCURY_SIGN_IDENTITY scripts/macos/bundle.sh --print-entitlements "$@" 2>&1 || true
}

# The keys a file grants, sorted, one per line (comments dropped by plutil).
keys() {
    plutil -convert xml1 -o - "$1" | sed -n 's:.*<key>\(.*\)</key>.*:\1:p' | sort | tr '\n' ' '
}

check "ad-hoc identity uses the ad-hoc entitlements" "$ADHOC" "$(chosen --identity -)"
check "default identity is ad hoc" "$ADHOC" "$(chosen)"
check "a Developer ID uses the strict entitlements" "$STRICT" \
    "$(chosen --identity 'Developer ID Application: Example (ABCDE12345)')"
check "MERCURY_SIGN_IDENTITY selects the strict entitlements" "$STRICT" \
    "$(MERCURY_SIGN_IDENTITY='Developer ID Application: Example (ABCDE12345)' \
        scripts/macos/bundle.sh --print-entitlements 2>&1 || true)"
check "--identity overrides MERCURY_SIGN_IDENTITY" "$ADHOC" \
    "$(MERCURY_SIGN_IDENTITY='Developer ID Application: Example (ABCDE12345)' \
        scripts/macos/bundle.sh --identity - --print-entitlements 2>&1 || true)"

check "strict entitlements grant only the JIT keys" \
    "com.apple.security.cs.allow-jit com.apple.security.cs.allow-unsigned-executable-memory " \
    "$(keys "$STRICT")"
check "ad-hoc entitlements add only library validation" \
    "com.apple.security.cs.allow-jit com.apple.security.cs.allow-unsigned-executable-memory com.apple.security.cs.disable-library-validation " \
    "$(keys "$ADHOC")"
for file in "$STRICT" "$ADHOC"; do
    [ -f "$file" ] || { check "$(basename -- "$file") exists" yes no; continue; }
    check "$(basename -- "$file") grants every key as true" "" \
        "$(plutil -convert xml1 -o - "$file" | grep -A1 '<key>' | grep -v -e '<key>' -e '<true/>' -e '^--$' || true)"
done

[ "$failures" -eq 0 ] || { printf '%d check(s) failed\n' "$failures"; exit 1; }
printf 'all checks passed\n'
