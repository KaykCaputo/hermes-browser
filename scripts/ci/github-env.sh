#!/usr/bin/env bash
# Run a command that prints shell `export` lines, and hand every variable it
# sets or changes to the later steps of a GitHub Actions job.
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
# parsed by hand. Every variable whose value then differs from before is
# written with GitHub's multi-line form (NAME<<DELIMITER, value, DELIMITER)
# using a random delimiter checked not to occur in the value, so values with
# spaces, colons, quotes or newlines arrive unchanged. Directories prepended
# to PATH go to $GITHUB_PATH, GitHub's documented way to extend it.
# DYLD_* variables are reported in a warning and not exported (see below).
#
# Fails, rather than exporting part of the environment, if the command fails,
# prints nothing that sets a variable, unsets a variable, or sets a name GitHub
# refuses (GITHUB_*, RUNNER_*, NODE_OPTIONS).
set -euo pipefail

usage() {
    sed -n '/^# Run a command that prints/,/^# refuses/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
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

# DYLD_* variables, listed by bash itself: macOS removes them from the
# environment of every SIP-protected program, which includes /usr/bin/python3
# and /bin/bash, so python3 below may never see them.
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

# Not exported: a step's `bash` on a macOS runner is /bin/bash, which is
# SIP-protected and would drop them before any command saw them. A step that
# needs one must evaluate the command's output itself.
DYLD_AFTER="$(dyld_variables)"
if [ "$DYLD_AFTER" != "$DYLD_BEFORE" ]; then
    names="$(printf '%s\n' "$DYLD_AFTER" | sed -n 's/=.*//p' | tr '\n' ' ')"
    message="not exported: ${names}(macOS strips DYLD_* variables when a SIP-protected shell starts; evaluate '$*' inside the step that needs them)"
    if [ "$PRINT" = 0 ]; then
        printf '::warning title=github-env::%s\n' "$message"
    else
        printf 'github-env: warning: %s\n' "$message" >&2
    fi
fi

python3 - "$BEFORE" "$PRINT" <<'PY'
import json
import os
import sys
import uuid

before = json.load(open(sys.argv[1]))
print_only = sys.argv[2] == "1"
after = dict(os.environ)
for name in ("_", "SHLVL", "PWD", "OLDPWD"):
    before.pop(name, None)
    after.pop(name, None)
# Reported by the shell above; never exported.
before = {k: v for k, v in before.items() if not k.startswith("DYLD_")}
after = {k: v for k, v in after.items() if not k.startswith("DYLD_")}

removed = sorted(set(before) - set(after))
if removed:
    sys.exit("github-env: the command unset %s; GitHub cannot unset variables" % ", ".join(removed))
changed = sorted(name for name in after if before.get(name) != after[name])
if not changed:
    sys.exit("github-env: the command's output set no variables")
refused = [n for n in changed if n.startswith(("GITHUB_", "RUNNER_")) or n == "NODE_OPTIONS"]
if refused:
    sys.exit("github-env: GitHub does not let a step set %s" % ", ".join(refused))

env_entries = []
path_entries = []
for name in changed:
    value = after[name]
    if name == "PATH" and "PATH" in before:
        old = before["PATH"]
        prefix = value[: -len(old) - 1].split(":") if value.endswith(":" + old) else []
        if prefix and all(entry and "\n" not in entry for entry in prefix):
            # The runner puts the most recently added line first, so write
            # them last-to-first to keep the order the command chose.
            path_entries = [entry + "\n" for entry in reversed(prefix)]
            continue
    delimiter = "MERCURY_ENV_" + uuid.uuid4().hex
    if delimiter in value.split("\n"):
        sys.exit("github-env: delimiter collision for %s" % name)
    env_entries.append("%s<<%s\n%s\n%s\n" % (name, delimiter, value, delimiter))

for name in changed:
    sys.stderr.write("github-env: exporting %s\n" % name)
if print_only:
    sys.stdout.write("# for $GITHUB_ENV\n" + "".join(env_entries))
    sys.stdout.write("# for $GITHUB_PATH\n" + "".join(path_entries))
else:
    with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as handle:
        handle.write("".join(env_entries))
    with open(os.environ["GITHUB_PATH"], "a", encoding="utf-8") as handle:
        handle.write("".join(path_entries))
PY
