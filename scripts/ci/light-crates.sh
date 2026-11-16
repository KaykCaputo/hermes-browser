#!/usr/bin/env bash
# Print the cargo `-p` arguments for every workspace member that can be built
# without the browser engine or the UI toolkit: members whose full dependency
# closure (normal, build and dev dependencies, for every target platform)
# contains neither the `servo` crate nor `slint`.
#
#   scripts/ci/light-crates.sh            # -p talaria-core -p …   (one line)
#   scripts/ci/light-crates.sh --explain  # one line per member, light or not, and why
#
# The list is derived from `cargo metadata --locked` every time, so a new
# library crate is picked up without editing anything here. Compiles nothing.
# Exits non-zero if no member qualifies, since a fast job with nothing to check
# would pass for the wrong reason.
set -euo pipefail

usage() {
    sed -n '/^# Print the cargo/,/^# would pass/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

EXPLAIN=0
case "${1:-}" in
    "") ;;
    --explain) EXPLAIN=1 ;;
    -h | --help) usage; exit 0 ;;
    *) printf 'light-crates: unknown option: %s\n' "$1" >&2; exit 2 ;;
esac

cd "$(dirname -- "${BASH_SOURCE[0]}")/../.."

command -v "${CARGO:-cargo}" >/dev/null 2>&1 || { printf 'light-crates: cargo not found\n' >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { printf 'light-crates: python3 not found\n' >&2; exit 1; }

# The metadata (several megabytes, too large for the environment or the
# command line) goes through a temporary file to standard input, so that a
# cargo failure stops here with cargo's own message; the program is passed
# with -c.
PROGRAM="$(
    cat <<'PY'
import json
import sys

HEAVY = ("servo", "slint")

metadata = json.load(sys.stdin)
explain = sys.argv[1] == "1"
names = {package["id"]: package["name"] for package in metadata["packages"]}
edges = {
    node["id"]: [dependency["pkg"] for dependency in node["deps"]]
    for node in metadata["resolve"]["nodes"]
}


def heavy_in_closure(root):
    found = set()
    seen = {root}
    stack = [root]
    while stack:
        for child in edges.get(stack.pop(), []):
            if child not in seen:
                seen.add(child)
                stack.append(child)
                if names[child] in HEAVY:
                    found.add(names[child])
    return sorted(found)


light = []
for member in sorted(metadata["workspace_members"], key=lambda id_: names[id_]):
    heavy = heavy_in_closure(member)
    if explain:
        verdict = "heavy (depends on " + " and ".join(heavy) + ")" if heavy else "light"
        print("%-24s %s" % (names[member], verdict))
    if not heavy:
        light.append(names[member])

if not light:
    sys.stderr.write("light-crates: no workspace member is free of %s\n" % " and ".join(HEAVY))
    sys.exit(1)
if not explain:
    print(" ".join("-p " + name for name in light))
PY
)"

METADATA_FILE="$(mktemp)"
trap 'rm -f -- "$METADATA_FILE"' EXIT
"${CARGO:-cargo}" metadata --locked --format-version 1 >"$METADATA_FILE"
python3 -c "$PROGRAM" "$EXPLAIN" <"$METADATA_FILE"
