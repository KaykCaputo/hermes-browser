---
name: Mercury build hygiene
description: Required before building, testing, running the browser or creating files in the Mercury Browser repository or its worktrees. How builds run on the development Mac, that build output is deleted after every round, and that all work is committed and pushed to GitHub before a turn ends. A disk guard plugin enforces it.
---

# Mercury build hygiene

The development Mac keeps about 5–6 GB of Mercury between rounds of work. One Servo build is 15–20 GB,
so build output exists only while you work on a round and is deleted afterwards. Your code is safe only
once it is on GitHub.

The plugin `.opencode/plugins/mercury-disk-guard.ts` enforces this for every session in this repository,
whatever you do. Follow the rules below so that it never deletes something you still need.

## What the guard does automatically

- **Every shell command** gets `CARGO_INCREMENTAL=0` and no debug info (`CARGO_PROFILE_*_DEBUG=0`).
  Builds are smaller and start cold; that is expected.
- **`CARGO_FLAGS=--release`** is set unless you set it yourself, so `make lint`, `make test` and
  `make build-release` share one release build: a round compiles Servo once, not twice. Binaries are in
  `target/release/`. Use the same profile for any `cargo` command you run by hand (`--release`).
- **Every build command** (`cargo build|test|clippy|check|run|doc`, `make lint|test|build|build-release|check|app-macos|run`,
  `scripts/macos/bundle.sh`) runs through `scripts/dev/build-slot.sh`. It waits until one of the three
  build slots is free and at least 40 GB of disk is free, shares the cores between the builds running
  (`CARGO_BUILD_JOBS`, at least 2), then runs your command unchanged and returns
  its exit status. While it waits it prints `mercury-disk-guard: waiting …`. Exit status 75 means it gave
  up after six hours.
- **Running a script file** (`sh x.sh`, `./x.sh`) counts as a build when the script contains a build
  command, so it takes a slot too.
- **The pinned checking tools** (`make ci-tools`: cargo-deny, shellcheck, actionlint) are shared by
  all worktrees (`CI_TOOLS_DIR` points to `~/Library/Caches/mercury-browser/ci-tools`). Run
  `make ci-tools` once if `make deny` or `make lint-scripts` says they are missing.
- **Copying build output** (`cp`/`rsync`/`ditto` of a `…/target`) is refused.
- **`target/` and `dist/` of a worktree are deleted:**
  - seconds after a successful `git push` from that worktree;
  - when your session goes idle (your turn ends);
  - by a sweep every five minutes once nothing has built there for 30 minutes.

  A build that is still running, or a program started from that folder, is never touched.
- **Mercury's cache** (`~/Library/Caches/mercury-browser`) and leftover temporary profiles are deleted
  whenever no Mercury is running. The exception is the unpacked GStreamer in `…/mercury-browser/gstreamer/`,
  which every build needs (about 1 GB); only its installer packages are deleted once unpacked. **Scratch files** in `$TMPDIR/opencode/lanes/` are deleted after 24 hours.
- Everything it does is logged in `~/mercury-browser/.superpowers/disk-guard/guard.log`.

## Rules for you

1. **Push before your turn ends.** Commit your work on your lane branch and run
   `git push origin lane/<name>` before you reply to the lead, pause, or wait for a review. Unfinished work
   goes into a commit too, with a subject starting `wip:`. Never end a turn with uncommitted changes.
   Worktrees of idle lanes can be removed, and a disk clean-up can happen at any time.
2. **Do a round in this order:** edit → narrow tests (`cargo test -p <crate> <filter>`) → the full `make`
   gates → runs and screenshots with the built binary → commit → push → reply. Pushing deletes the build,
   so finish everything that needs `target/` first.
3. **Batch your builds.** Each one may wait for a slot and starts cold, so it can take 20–60 minutes,
   plus up to a few hours of waiting when other lanes are building.
   - Run builds in the shell tool's background mode (no timeout), and end your turn while it waits. You
     are woken when it finishes.
   - Run the full gates once per round, not after every edit.
   - Don't set `CARGO_BUILD_JOBS`; the slot script sets it from the number of builds running.
4. **Build only inside your own worktree** under `~/mercury-worktrees/<lane>`.
   - Never set `CARGO_TARGET_DIR` to another place, and never copy or move a `target/`.
   - Never build in the main clone `~/mercury-browser`; only the lead's `integration` worktree builds merges.
5. **Keep evidence in your report, not in files.** Write down the numbers and what each screenshot showed.
   Scratch goes only in `$TMPDIR/opencode/lanes/<lane>/`, and you delete what you no longer need.
   Large files (videos, dumps, copies of profiles) do not belong on this Mac.
6. **Leave nothing running.** Stop test servers and browser runs by PID before you push. A process
   started from `target/` keeps that folder from being deleted.
7. **Don't fight the guard.** Do not rename build folders, hide builds from it or run builds outside it.
   If it refuses or waits too long, tell the lead.

Check usage with `du -sh ~/mercury-worktrees/* 2>/dev/null` and `df -h ~`.
