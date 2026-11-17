#!/bin/bash
# Runs one shell command while holding one of Mercury's build slots, so that only a few builds run
# on the development Mac at once and none starts without enough free disk space.
#
# The OpenCode plugin .opencode/plugins/mercury-disk-guard.ts sends every build command here
# (cargo build/test/clippy/check/run, make lint/test/build-release/...). The command arrives in
# MERCURY_GUARDED_COMMAND and runs unchanged under MERCURY_GUARDED_SHELL; its exit status is ours.
# See .opencode/skills/mercury-build-hygiene/SKILL.md.
set -uo pipefail

state_dir="${MERCURY_GUARD_STATE:?MERCURY_GUARD_STATE is not set}"
command="${MERCURY_GUARDED_COMMAND:?MERCURY_GUARDED_COMMAND is not set}"
run_shell="${MERCURY_GUARDED_SHELL:-/bin/zsh}"
worktrees="${MERCURY_GUARDED_WORKTREE:-}"
slots="${MERCURY_BUILD_SLOTS:-3}"
min_free_gb="${MERCURY_MIN_FREE_GB:-40}"
wait_minutes="${MERCURY_SLOT_WAIT_MINUTES:-360}"

slot_root="$state_dir/slots"
mkdir -p "$slot_root"

say() { printf 'mercury-disk-guard: %s\n' "$*" >&2; }

free_gb() { df -Pk "$HOME" | awk 'NR == 2 { print int($4 / 1048576) }'; }

# A slot is a directory: mkdir is atomic, so two builds can never take the same one. Its owner file
# names the process and the worktrees; a slot whose process has gone (killed, crashed) is reclaimed.
reclaim_dead_slots() {
    local dir pid age
    for dir in "$slot_root"/*/; do
        [ -d "$dir" ] || continue
        if [ -f "$dir/owner" ]; then
            pid="$(head -n 1 "$dir/owner")"
            if [ -n "$pid" ] && ! kill -0 "$pid" 2>/dev/null; then
                rm -rf "$dir"
            fi
        else
            age=$(($(date +%s) - $(stat -f %m "$dir")))
            [ "$age" -gt 120 ] && rm -rf "$dir"
        fi
    done
}

holders() {
    local dir
    for dir in "$slot_root"/*/; do
        [ -f "$dir/owner" ] && sed -n 2p "$dir/owner"
    done | tr '\n' ' '
}

claim() {
    local i dir
    for i in $(seq 1 "$slots"); do
        dir="$slot_root/$i"
        if mkdir "$dir" 2>/dev/null; then
            printf '%s\n%s\n%s\n' "$$" "${worktrees:-?}" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" >"$dir/owner"
            printf '%s\n' "$dir"
            return 0
        fi
    done
    return 1
}

started=$(date +%s)
last_note=0
slot=""
while :; do
    reclaim_dead_slots
    free="$(free_gb)"
    if [ "$free" -ge "$min_free_gb" ] && slot="$(claim)"; then
        break
    fi
    now=$(date +%s)
    if [ $((now - started)) -ge $((wait_minutes * 60)) ]; then
        say "gave up after ${wait_minutes} min: free ${free} GB (need ${min_free_gb}), builds running in: $(holders)"
        exit 75
    fi
    if [ $((now - last_note)) -ge 300 ]; then
        if [ "$free" -lt "$min_free_gb" ]; then
            say "waiting: only ${free} GB free (a build needs ${min_free_gb} GB); old build output is being removed"
        else
            say "waiting for a build slot (${slots} in use: $(holders))"
        fi
        last_note=$now
    fi
    sleep 15
done

release() {
    local wt
    rm -rf "$slot"
    # Record when each worktree last finished a build: the plugin removes build output that has
    # not been used for a while.
    for wt in $worktrees; do
        [ -d "$wt/target" ] && touch "$wt/target/.mercury-last-build"
    done
}
trap release EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

# Share the cores between the builds running now (at least two jobs each). A command that sets
# CARGO_BUILD_JOBS itself still wins, since it runs after this.
if [ -z "${CARGO_BUILD_JOBS:-}" ]; then
    cores="$(sysctl -n hw.ncpu 2>/dev/null || echo 8)"
    running="$(find "$slot_root" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
    jobs=$((cores / (running > 0 ? running : 1)))
    export CARGO_BUILD_JOBS=$((jobs > 2 ? jobs : 2))
fi

"$run_shell" -c "$command"
