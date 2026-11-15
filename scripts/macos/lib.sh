# shellcheck shell=bash
# Helpers shared by scripts/macos/bundle.sh and scripts/macos/test-bundle.sh.
# Sourced, not run. The parsing helpers read the text they parse from standard
# input and print a result, so they can be tested without a build.
#
# Every parser reads its whole input. A reader that stops early (`awk … exit`,
# `head`, `grep -q`) makes the program writing into the pipe fail with a broken
# pipe once its output passes the pipe buffer (about 64 KB), and under
# `set -o pipefail` that failure ends the calling script with no message.
# Callers capture a tool's output in a variable first and feed it to the parser
# with a here-string, so the tool's own exit status is checked separately.

# The entitlements file for a signing identity. An ad-hoc signature ("-") has
# no team identifier, so the hardened runtime's library validation could never
# accept the ad-hoc libraries in Contents/MacOS/lib; the ad-hoc set therefore
# turns it off. Any real identity gets the strict set, with validation on.
entitlements_for_identity() {
    if [ "$1" = "-" ]; then
        printf '%s\n' "apps/mercury-desktop/resources/macos/entitlements-adhoc.plist"
    else
        printf '%s\n' "apps/mercury-desktop/resources/macos/entitlements.plist"
    fi
}

# Prints why an identity is unusable, or nothing if it is "-" or plausibly a
# certificate name or SHA-1 hash. Odd values are refused rather than passed to
# codesign: " - " and "-x" are almost certainly typos for "-", and they would
# otherwise be treated as real identities and get the strict entitlements.
identity_problem() {
    case "$1" in
        -) ;;
        "") printf '%s\n' "the signing identity is empty; use - for ad hoc" ;;
        -*) printf '%s\n' "signing identity '$1' begins with '-'; use - for ad hoc, or a certificate name or SHA-1 hash" ;;
        [[:space:]]* | *[[:space:]]) printf '%s\n' "signing identity '$1' has leading or trailing spaces" ;;
    esac
}

# Cargo.toml on stdin: the version in [workspace.package].
workspace_version() {
    awk '
        /^\[/ { in_package = ($0 == "[workspace.package]") }
        in_package && !found && /^version[[:space:]]*=/ {
            value = $0; sub(/^[^"]*"/, "", value); sub(/".*$/, "", value)
            print value; found = 1
        }'
}

# .cargo/config.toml on stdin: MACOSX_DEPLOYMENT_TARGET, written either as
# `= "13.0"` or as `= { value = "13.0", force = true }`.
config_deployment_target() {
    awk -F'"' '
        !found && /^MACOSX_DEPLOYMENT_TARGET[[:space:]]*=/ && $2 ~ /^[0-9.]+$/ {
            print $2; found = 1
        }'
}

# `otool -l` output on stdin: the minos of the first LC_BUILD_VERSION.
listing_minos() {
    awk '
        $2 == "LC_BUILD_VERSION" { in_build = 1 }
        in_build && !found && $1 == "minos" { print $2; found = 1 }'
}

# `otool -l` output on stdin; succeeds if the executable has rpath $1.
listing_has_rpath() {
    awk -v wanted="$1" '
        $1 == "cmd" { in_rpath = ($2 == "LC_RPATH") }
        in_rpath && $1 == "path" && $2 == wanted { found = 1 }
        END { exit !found }'
}

# `otool -hv` output on stdin: the Mach-O file types (EXECUTE, DYLIB, BUNDLE,
# …), one per line, without repeats. A universal file lists one per slice.
macho_filetypes() {
    awk '$1 ~ /^MH_(MAGIC|CIGAM)/ { print $5 }' | sort -u
}

# Prints why a file in Contents/MacOS/lib does not belong there, or nothing.
# $1 is the path, used in the message; `otool -hv` output for it is on stdin.
# Only libraries (DYLIB) and loadable plugins (BUNDLE) belong there: they are
# signed without entitlements, so an executable placed there would run with
# library validation on and, in an ad-hoc build, fail to load the libraries
# next to it.
library_problem() {
    local types type
    types="$(macho_filetypes)"
    [ -n "$types" ] || { printf '%s\n' "$1: not a Mach-O file otool can read"; return 0; }
    while IFS= read -r type; do
        case "$type" in
            DYLIB | BUNDLE) ;;
            EXECUTE)
                printf '%s\n' "$1 is an executable; only libraries belong in Contents/MacOS/lib (an executable there would be signed without entitlements)"
                return 0
                ;;
            *)
                printf '%s\n' "$1 is a Mach-O $type, not a library; only libraries belong in Contents/MacOS/lib"
                return 0
                ;;
        esac
    done <<<"$types"
}

# `codesign -dv` output on stdin; succeeds if the hardened runtime flag is set.
codesign_has_runtime() {
    awk '
        /flags=0x[0-9a-fA-F]+\(/ {
            list = $0; sub(/.*flags=0x[0-9a-fA-F]+\(/, "", list); sub(/\).*/, "", list)
            if (("," list ",") ~ /,runtime,/) found = 1
        }
        END { exit !found }'
}

# `codesign -dv` output on stdin: the TeamIdentifier value ("not set" for ad hoc).
codesign_team() {
    awk -F= '$1 == "TeamIdentifier" && !found { print $2; found = 1 }'
}

# Prints why a signed bundle would not start, or nothing. Arguments: identity,
# number of libraries in Contents/MacOS/lib, TeamIdentifier after signing.
# With the strict entitlements library validation is on, and it accepts only
# libraries from the executable's own team; an identity without a team (a
# self-signed certificate) passes `codesign --verify` and then fails at launch.
team_problem() {
    if [ "$1" != "-" ] && [ "$2" -gt 0 ] && { [ -z "$3" ] || [ "$3" = "not set" ]; }; then
        printf '%s\n' "identity '$1' has no team identifier, so the bundled libraries would fail library validation at launch; use a Developer ID or Apple Development certificate, or - for ad hoc"
    fi
}

# A property list (a file, or - for stdin) as sorted key=value lines, one per
# top-level key, e.g. "com.apple.security.cs.allow-jit=<true/>". Fails if the
# input is not a property list (including empty input).
plist_grants() {
    local xml
    xml="$(plutil -convert xml1 -o - -- "$1" 2>/dev/null)" || return 1
    awk '
        /<key>/ { key = $0; sub(/.*<key>/, "", key); sub(/<\/key>.*/, "", key); next }
        key != "" { value = $0; gsub(/[[:space:]]/, "", value); print key "=" value; key = "" }' <<<"$xml" | sort
}

# Succeeds if the entitlements on stdin (as printed by
# `codesign -d --entitlements - --xml`) grant exactly what file $1 grants.
entitlements_match() {
    local expected actual
    expected="$(plist_grants "$1")" || return 1
    actual="$(plist_grants -)" || return 1
    [ "$expected" = "$actual" ]
}
