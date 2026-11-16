#!/usr/bin/env bash
# Run `scripts/macos/gstreamer.sh env` (or another command printing the same
# `export` lines) and hand the variables it sets to the later steps of a
# GitHub Actions job.
#
#   scripts/ci/github-env.sh [--print] <command> [arguments…]
#   scripts/ci/github-env.sh scripts/macos/gstreamer.sh env
#
#   --print   write the entries to standard output instead of $GITHUB_ENV and
#             $GITHUB_PATH (to see what would be exported)
#
# The command's output is evaluated by bash exactly as
# `eval "$(<command>)"` would be in a shell, so quoting, `$VAR` expansion and
# path lists such as `PATH=/x/bin:$PATH` mean what they mean there; nothing is
# parsed by hand. Only these variables may change, and the first four must be
# set:
#
#   MERCURY_GSTREAMER_PREFIX  the unpacked GStreamer prefix
#   PKG_CONFIG                its pkg-config
#   PKG_CONFIG_LIBDIR         its .pc directory
#   PATH                      the old PATH with directories added in front;
#                             they go to $GITHUB_PATH
#   PKG_CONFIG_PATH           may only be unset; exported as empty, which
#                             pkg-config treats the same
#
# Each value is written with GitHub's multi-line form (NAME<<DELIMITER, value,
# DELIMITER) using a random delimiter checked not to occur in the value, so
# spaces, colons, quotes and newlines arrive unchanged.
#
# Refuses, exporting nothing, if the command fails, sets or unsets anything
# else, leaves one of the four unset, or changes PATH in any way other than
# adding directories in front. DYLD_* variables are refused with an
# explanation: macOS strips them whenever a SIP-protected program such as
# /bin/bash starts, so they could not reach any later command.
set -euo pipefail

usage() {
    sed -n '/^# Run `scripts\/macos/,/^# \/bin\/bash starts/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

PRINT=0
case "${1:-}" in
    --print) PRINT=1; shift ;;
    -h | --help) usage; exit 0 ;;
esac
[ "$#" -gt 0 ] || { usage >&2; exit 2; }
if [ "$PRINT" = 0 ]; then
    [ -n "${GITHUB_ENV:-}" ] && [ -n "${GITHUB_PATH:-}" ] ||
        { printf 'github-env: GITHUB_ENV and GITHUB_PATH are not set (use --print outside Actions)\n' >&2; exit 2; }
fi
command -v python3 >/dev/null 2>&1 || { printf 'github-env: python3 not found\n' >&2; exit 1; }

BEFORE="$(mktemp)"
trap 'rm -f -- "$BEFORE"' EXIT

# DYLD_* variables, listed by bash itself: /usr/bin/python3 is SIP-protected
# and would never see them.
dyld_variables() {
    local name
    for name in $(compgen -e); do
        case "$name" in
            DYLD_*) printf '%s=%s\n' "$name" "${!name}" ;;
        esac
    done
}

# Snapshot the environment, run the command (its failure stops the script
# through set -e), then evaluate its output here.
python3 -c 'import json, os, sys; json.dump(dict(os.environ), open(sys.argv[1], "w"))' "$BEFORE"
DYLD_BEFORE="$(dyld_variables)"
OUTPUT="$("$@")"
eval "$OUTPUT"

DYLD_AFTER="$(dyld_variables)"
if [ "$DYLD_AFTER" != "$DYLD_BEFORE" ]; then
    names="$(printf '%s\n' "$DYLD_AFTER" | sed -n 's/=.*//p' | tr '\n' ' ')"
    printf 'github-env: refused: %s(macOS strips DYLD_* variables when a SIP-protected shell starts, so they cannot pass between steps; set them on the command line that needs them)\n' "$names" >&2
    exit 1
fi

python3 - "$BEFORE" "$PRINT" <<'PY'
import json
import os
import sys
import uuid

MUST_SET = ("MERCURY_GSTREAMER_PREFIX", "PKG_CONFIG", "PKG_CONFIG_LIBDIR", "PATH")
MAY_UNSET = ("PKG_CONFIG_PATH",)


def refuse(message):
    sys.exit("github-env: refused: " + message)


before = json.load(open(sys.argv[1]))
print_only = sys.argv[2] == "1"
after = dict(os.environ)
for name in ("_", "SHLVL", "PWD", "OLDPWD"):
    before.pop(name, None)
    after.pop(name, None)

removed = sorted(set(before) - set(after))
changed = sorted(name for name in after if before.get(name) != after[name])
unexpected = [n for n in changed if n not in MUST_SET] + [n for n in removed if n not in MAY_UNSET]
if unexpected:
    refuse("the command changed %s, which is not on the allow-list" % ", ".join(sorted(unexpected)))
missing = [n for n in MUST_SET if n not in changed]
if missing:
    refuse("the command did not set %s" % ", ".join(missing))

old_path = before.get("PATH", "")
new_path = after["PATH"]
if not old_path or not new_path.endswith(":" + old_path):
    refuse("PATH is not the old PATH with directories added in front")
path_prefix = new_path[: -len(old_path) - 1].split(":")
if not all(entry and "\n" not in entry for entry in path_prefix):
    refuse("PATH gains an empty directory or one containing a newline")

values = [(name, after[name]) for name in changed if name != "PATH"]
values += [(name, "") for name in removed]
env_entries = []
for name, value in values:
    delimiter = "MERCURY_ENV_" + uuid.uuid4().hex
    if delimiter in value.split("\n"):
        refuse("delimiter collision for %s" % name)
    env_entries.append("%s<<%s\n%s\n%s\n" % (name, delimiter, value, delimiter))
# The runner puts the most recently added line first, so write them
# last-to-first to keep the order the command chose.
path_entries = [entry + "\n" for entry in reversed(path_prefix)]

for name, _ in values:
    sys.stderr.write("github-env: exporting %s\n" % name)
sys.stderr.write("github-env: adding to PATH: %s\n" % ":".join(path_prefix))
if print_only:
    sys.stdout.write("# for $GITHUB_ENV\n" + "".join(env_entries))
    sys.stdout.write("# for $GITHUB_PATH\n" + "".join(path_entries))
else:
    with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as handle:
        handle.write("".join(env_entries))
    with open(os.environ["GITHUB_PATH"], "a", encoding="utf-8") as handle:
        handle.write("".join(path_entries))
PY
