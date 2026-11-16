#!/usr/bin/env bash
# Download the pinned CI tools, verify their SHA-256, and unpack them.
#
#   scripts/ci/install-tools.sh [--dir <dir>] [shellcheck] [actionlint] [cargo-deny]
#
#   --dir <dir>   where to put them (default target/ci-tools); the programs
#                 end up in <dir>/bin, which the caller adds to PATH
#   (no names)    all three
#
# Only the official release archives of the versions below are fetched, from
# GitHub release pages, and each is checked against the checksum recorded here
# before anything is unpacked; a mismatch stops the script. Nothing is
# installed system-wide and nothing is run as root. Works on Linux x86-64 (CI)
# and macOS arm64. Re-running skips a tool already present at its version.
#
# To move a version, change it below together with both checksums, taken from
# the project's published checksum file (actionlint's checksums.txt,
# cargo-deny's .sha256 files) or, for shellcheck, which publishes none, from
# the asset digest GitHub shows on the release page.
set -euo pipefail

SHELLCHECK_VERSION="0.11.0"
ACTIONLINT_VERSION="1.7.12"
CARGO_DENY_VERSION="0.20.2"

usage() {
    sed -n '/^# Download the pinned/,/^# the asset digest/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

die() {
    printf 'install-tools: error: %s\n' "$*" >&2
    exit 1
}

say() {
    printf 'install-tools: %s\n' "$*" >&2
}

DIR="target/ci-tools"
TOOLS=()
while [ "$#" -gt 0 ]; do
    case "$1" in
        --dir) DIR="${2:?--dir needs a directory}"; shift 2 ;;
        -h | --help) usage; exit 0 ;;
        shellcheck | actionlint | cargo-deny) TOOLS+=("$1"); shift ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done
[ "${#TOOLS[@]}" -gt 0 ] || TOOLS=(shellcheck actionlint cargo-deny)

case "$(uname -s)/$(uname -m)" in
    Linux/x86_64) PLATFORM=linux-x86_64 ;;
    Darwin/arm64) PLATFORM=macos-arm64 ;;
    *) die "no pinned tools for $(uname -s)/$(uname -m); Linux x86-64 and macOS arm64 are supported" ;;
esac

if command -v sha256sum >/dev/null 2>&1; then
    SHA256=(sha256sum)
elif command -v shasum >/dev/null 2>&1; then
    SHA256=(shasum -a 256)
else
    die "neither sha256sum nor shasum is available"
fi
command -v curl >/dev/null 2>&1 || die "curl not found"
command -v tar >/dev/null 2>&1 || die "tar not found"

# url, sha256 and the archive member holding the program, for one tool here.
describe() {
    local base
    case "$1/$PLATFORM" in
        shellcheck/linux-x86_64)
            base="shellcheck-v$SHELLCHECK_VERSION.linux.x86_64"
            URL="https://github.com/koalaman/shellcheck/releases/download/v$SHELLCHECK_VERSION/$base.tar.xz"
            SUM=8c3be12b05d5c177a04c29e3c78ce89ac86f1595681cab149b65b97c4e227198
            MEMBER="shellcheck-v$SHELLCHECK_VERSION/shellcheck"
            ;;
        shellcheck/macos-arm64)
            base="shellcheck-v$SHELLCHECK_VERSION.darwin.aarch64"
            URL="https://github.com/koalaman/shellcheck/releases/download/v$SHELLCHECK_VERSION/$base.tar.xz"
            SUM=56affdd8de5527894dca6dc3d7e0a99a873b0f004d7aabc30ae407d3f48b0a79
            MEMBER="shellcheck-v$SHELLCHECK_VERSION/shellcheck"
            ;;
        actionlint/linux-x86_64)
            URL="https://github.com/rhysd/actionlint/releases/download/v$ACTIONLINT_VERSION/actionlint_${ACTIONLINT_VERSION}_linux_amd64.tar.gz"
            SUM=8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8
            MEMBER="actionlint"
            ;;
        actionlint/macos-arm64)
            URL="https://github.com/rhysd/actionlint/releases/download/v$ACTIONLINT_VERSION/actionlint_${ACTIONLINT_VERSION}_darwin_arm64.tar.gz"
            SUM=aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f
            MEMBER="actionlint"
            ;;
        cargo-deny/linux-x86_64)
            base="cargo-deny-$CARGO_DENY_VERSION-x86_64-unknown-linux-musl"
            URL="https://github.com/EmbarkStudios/cargo-deny/releases/download/$CARGO_DENY_VERSION/$base.tar.gz"
            SUM=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f
            MEMBER="$base/cargo-deny"
            ;;
        cargo-deny/macos-arm64)
            base="cargo-deny-$CARGO_DENY_VERSION-aarch64-apple-darwin"
            URL="https://github.com/EmbarkStudios/cargo-deny/releases/download/$CARGO_DENY_VERSION/$base.tar.gz"
            SUM=fe67d82a10d8597a3549364cb733a3f9cc1bfff9031b7ae46384a9f2a72090c3
            MEMBER="$base/cargo-deny"
            ;;
        *) die "no pin for $1 on $PLATFORM" ;;
    esac
}

pinned_version() {
    case "$1" in
        shellcheck) printf '%s\n' "$SHELLCHECK_VERSION" ;;
        actionlint) printf '%s\n' "$ACTIONLINT_VERSION" ;;
        cargo-deny) printf '%s\n' "$CARGO_DENY_VERSION" ;;
    esac
}

# The version a program reports, in the same form as the pins above.
reported_version() {
    case "$1" in
        shellcheck) "$2" --version 2>/dev/null | sed -n 's/^version: //p' ;;
        actionlint) "$2" -version 2>/dev/null | sed -n 1p ;;
        cargo-deny) "$2" --version 2>/dev/null | sed -n 's/^cargo-deny //p' ;;
    esac
}

mkdir -p "$DIR/bin" "$DIR/downloads"
for tool in "${TOOLS[@]}"; do
    version="$(pinned_version "$tool")"
    program="$DIR/bin/$tool"
    if [ -x "$program" ] && [ "$(reported_version "$tool" "$program")" = "$version" ]; then
        say "$tool $version already in $DIR/bin"
        continue
    fi
    describe "$tool"
    archive="$DIR/downloads/${URL##*/}"
    say "downloading $tool $version"
    curl --fail --silent --show-error --location --retry 3 --output "$archive.part" "$URL"
    if ! printf '%s  %s\n' "$SUM" "$archive.part" | "${SHA256[@]}" -c - >/dev/null; then
        rm -f -- "$archive.part"
        die "checksum mismatch for ${URL##*/}; expected $SUM"
    fi
    mv -f -- "$archive.part" "$archive"
    rm -rf -- "$DIR/unpack"
    mkdir -p "$DIR/unpack"
    tar -xf "$archive" -C "$DIR/unpack" "$MEMBER"
    mv -f -- "$DIR/unpack/$MEMBER" "$program"
    rm -rf -- "$DIR/unpack"
    chmod 755 "$program"
    [ "$(reported_version "$tool" "$program")" = "$version" ] ||
        die "$program does not report version $version"
    say "$tool $version verified and installed in $DIR/bin"
done
