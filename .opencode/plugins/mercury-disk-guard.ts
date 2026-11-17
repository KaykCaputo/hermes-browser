// Mercury disk guard: keeps Mercury's build output off the development Mac between rounds of work.
//
// Loaded automatically for OpenCode sessions located in this repository (.opencode/plugins/). It
// enforces what .opencode/skills/mercury-build-hygiene/SKILL.md describes, whatever an agent does:
//
// - every shell command gets smaller-build settings (no debug info, no incremental files);
// - every build command runs through scripts/dev/build-slot.sh: at most two builds at once, and
//   none while less than 40 GB is free;
// - copying a build folder (`cp ... target`) is refused;
// - a worktree's build output (`target/`, `dist/`) is removed as soon as the agent working in it
//   pushes, when that agent's turn ends, and by a sweep every five minutes once it has
//   been unused for 30 minutes (this also catches agents stopped by an outage);
// - the sweep also removes Mercury's own cache and temporary profiles when no Mercury runs, and
//   scratch files older than a day.
//
// Only build output, caches and scratch files are ever removed: never source files, never git
// data, never anything outside the paths below.

import { spawnSync } from "node:child_process"
import * as fs from "node:fs"
import * as os from "node:os"
import * as path from "node:path"

const HOME = os.homedir()
const REPO = path.join(HOME, "mercury-browser")
const WORKTREES = path.join(HOME, "mercury-worktrees")
const STATE = path.join(REPO, ".superpowers", "disk-guard")
const SLOT_SCRIPT = path.join(REPO, "scripts", "dev", "build-slot.sh")
const LOG = path.join(STATE, "guard.log")
const SCRATCH = path.join(os.tmpdir(), "opencode", "lanes")
const MERCURY_CACHE = path.join(HOME, "Library", "Caches", "mercury-browser")

const STALE_MS = 30 * 60_000
const SWEEP_MS = 5 * 60_000
const SCRATCH_MAX_AGE_MS = 24 * 60 * 60_000
const BUILD_OUTPUT = ["target", "dist"]

// Commands that compile. `make fmt-check`, `deny`, `lint-scripts` and `ci-tools` do not.
const END = String.raw`(?=$|[\s;&|)])`
const BUILD_COMMAND = new RegExp(
  String.raw`(\bcargo\s+(\+\S+\s+)?(build|b|test|t|clippy|check|c|run|r|doc|bench|nextest|rustc|install)${END})` +
    String.raw`|(\bmake\b[^;&|\n]*\b(lint|test|build|build-release|check|app-macos|package-macos|run|run-release|run-debug|docs-api)${END})` +
    String.raw`|(scripts/macos/bundle\.sh)`,
)
const COPIES_BUILD_OUTPUT = /\b(cp|rsync|ditto)\b[^;&|\n]*\/target(\/|\s|$)/
const PUSHES = /\bgit\b[^;&|\n]*\bpush\b/

function log(message: string) {
  try {
    fs.mkdirSync(STATE, { recursive: true })
    fs.appendFileSync(LOG, `${new Date().toISOString()} ${message}\n`)
  } catch {
    // logging must never break a command
  }
}

function shellQuote(value: string) {
  return `'${value.replaceAll("'", `'\\''`)}'`
}

/** The worktree (or the main clone) a path lies in, if any. */
function owningWorktree(dir: string | undefined): string | undefined {
  if (!dir) return undefined
  const resolved = path.resolve(dir.replace(/^~(?=\/|$)/, HOME))
  if (resolved === REPO || resolved.startsWith(REPO + path.sep)) return REPO
  if (resolved.startsWith(WORKTREES + path.sep)) {
    const name = resolved.slice(WORKTREES.length + 1).split(path.sep)[0]
    if (name) return path.join(WORKTREES, name)
  }
  return undefined
}

/** Every worktree a command names, plus the one it runs in. */
function worktreesOf(command: string, cwd: string | undefined): string[] {
  const found = new Set<string>()
  const named = /(?:~|\$HOME|\/Users\/[^/\s'"]+)\/(mercury-worktrees\/[A-Za-z0-9._-]+|mercury-browser)(?=[/\s'";&|)]|$)/g
  for (const match of command.matchAll(named)) {
    const wt = owningWorktree(path.join(HOME, match[1]))
    if (wt) found.add(wt)
  }
  const here = owningWorktree(cwd)
  if (here) found.add(here)
  return [...found]
}

function slotHolders(): Set<string> {
  const holders = new Set<string>()
  const slots = path.join(STATE, "slots")
  let entries: string[] = []
  try {
    entries = fs.readdirSync(slots)
  } catch {
    return holders
  }
  for (const entry of entries) {
    try {
      const [pid, worktrees] = fs.readFileSync(path.join(slots, entry, "owner"), "utf8").split("\n")
      process.kill(Number(pid), 0)
      for (const wt of (worktrees ?? "").split(" ")) if (wt) holders.add(wt)
    } catch {
      // a slot being claimed, or one whose process has gone: build-slot.sh reclaims it
    }
  }
  return holders
}

function processesMention(text: string): boolean {
  const result = spawnSync("/usr/bin/pgrep", ["-f", text], { encoding: "utf8" })
  return result.status === 0 && result.stdout.trim().length > 0
}

function mercuryRunning(): boolean {
  const result = spawnSync("/usr/bin/pgrep", ["-x", "mercury-browser"], { encoding: "utf8" })
  return result.status === 0
}

function sizeOf(target: string): string {
  const result = spawnSync("/usr/bin/du", ["-sh", target], { encoding: "utf8" })
  return result.status === 0 ? result.stdout.split("\t")[0] : "?"
}

/** Removes a worktree's build output unless a build or a program from it is running. */
function cleanWorktree(wt: string, reason: string, holders = slotHolders()) {
  if (wt !== REPO && path.dirname(wt) !== WORKTREES) return
  if (holders.has(wt)) {
    log(`keep ${wt}: a build is running (${reason})`)
    return
  }
  for (const name of BUILD_OUTPUT) {
    const target = path.join(wt, name)
    if (!fs.existsSync(target)) continue
    if (processesMention(target)) {
      log(`keep ${target}: a program from it is running (${reason})`)
      continue
    }
    const size = sizeOf(target)
    fs.rmSync(target, { recursive: true, force: true })
    log(`removed ${target} (${size}) after ${reason}`)
  }
}

function lastUsed(wt: string): number {
  let newest = 0
  for (const name of BUILD_OUTPUT) {
    for (const file of [path.join(wt, name, ".mercury-last-build"), path.join(wt, name)]) {
      try {
        newest = Math.max(newest, fs.statSync(file).mtimeMs)
      } catch {
        // missing
      }
    }
  }
  return newest
}

function knownWorktrees(): string[] {
  const all = [REPO]
  try {
    for (const name of fs.readdirSync(WORKTREES)) all.push(path.join(WORKTREES, name))
  } catch {
    // no worktrees at the moment
  }
  return all
}

function removeOlderThan(dir: string, maxAgeMs: number) {
  let entries: string[] = []
  try {
    entries = fs.readdirSync(dir)
  } catch {
    return
  }
  for (const entry of entries) {
    const full = path.join(dir, entry)
    try {
      if (Date.now() - fs.statSync(full).mtimeMs > maxAgeMs) {
        fs.rmSync(full, { recursive: true, force: true })
        log(`removed old scratch ${full}`)
      }
    } catch {
      // vanished meanwhile
    }
  }
}

function sweep() {
  const holders = slotHolders()
  for (const wt of knownWorktrees()) {
    const used = lastUsed(wt)
    if (used > 0 && Date.now() - used > STALE_MS) cleanWorktree(wt, "30 min without a build", holders)
  }
  if (!mercuryRunning()) {
    if (fs.existsSync(MERCURY_CACHE)) {
      fs.rmSync(MERCURY_CACHE, { recursive: true, force: true })
      log(`removed ${MERCURY_CACHE}`)
    }
    try {
      for (const entry of fs.readdirSync(os.tmpdir())) {
        if (entry.startsWith("mercury-browser-profile-")) {
          fs.rmSync(path.join(os.tmpdir(), entry), { recursive: true, force: true })
          log(`removed temporary profile ${entry}`)
        }
      }
    } catch {
      // temporary directory unreadable
    }
  }
  removeOlderThan(SCRATCH, SCRATCH_MAX_AGE_MS)
}

const sessionWorktrees = new Map<string, Set<string>>()

function remember(sessionID: string | undefined, worktrees: string[]) {
  if (!sessionID || worktrees.length === 0) return
  const set = sessionWorktrees.get(sessionID) ?? new Set<string>()
  for (const wt of worktrees) set.add(wt)
  sessionWorktrees.set(sessionID, set)
}

export default {
  id: "mercury.disk-guard",
  async setup(ctx: any) {
    fs.mkdirSync(STATE, { recursive: true })
    log(`loaded for ${ctx?.location?.directory ?? "?"}`)

    await ctx.shell.hook("create.before", (event: any) => {
      event.env = event.env ?? {}
      event.env.CARGO_INCREMENTAL = "0"
      event.env.CARGO_PROFILE_DEV_DEBUG = "0"
      event.env.CARGO_PROFILE_TEST_DEBUG = "0"
      event.env.CARGO_PROFILE_RELEASE_DEBUG = "0"
      event.env.MERCURY_DISK_GUARD = "1"
      const command: string = event.command ?? ""
      if (command.includes(SLOT_SCRIPT)) return
      if (COPIES_BUILD_OUTPUT.test(command)) {
        log(`refused copy of build output: ${command.slice(0, 200)}`)
        event.command =
          "echo 'mercury-disk-guard: copying build output is not allowed; build output is removed after every round (see the mercury-build-hygiene skill)' >&2; exit 1"
        return
      }
      if (!BUILD_COMMAND.test(command)) return
      const worktrees = worktreesOf(command, event.cwd)
      event.env.MERCURY_GUARDED_COMMAND = command
      event.env.MERCURY_GUARDED_SHELL = event.shell || "/bin/zsh"
      event.env.MERCURY_GUARDED_WORKTREE = worktrees.join(" ")
      event.env.MERCURY_GUARD_STATE = STATE
      event.command = `/bin/bash ${shellQuote(SLOT_SCRIPT)}`
      log(`build in ${worktrees.join(" ") || event.cwd}: ${command.slice(0, 160).replaceAll("\n", " ")}`)
    })

    await ctx.tool.hook("execute.before", (event: any) => {
      const tool = event?.tool ?? event?.name ?? event?.toolID
      if (tool !== "shell") return
      const input = event.input ?? {}
      remember(event.sessionID ?? event.context?.sessionID, worktreesOf(input.command ?? "", input.workdir))
    })

    await ctx.tool.hook("execute.after", (event: any) => {
      const tool = event?.tool ?? event?.name ?? event?.toolID
      if (tool !== "shell" || event.status !== "completed") return
      const input = event.input ?? {}
      const command: string = input.command ?? ""
      if (!PUSHES.test(command)) return
      const output = JSON.stringify(event.result ?? "")
      if (/\[rejected\]|fatal:|error: failed to push/.test(output)) return
      for (const wt of worktreesOf(command, input.workdir)) cleanWorktree(wt, "git push")
    })

    const stop = new AbortController()
    void (async () => {
      try {
        // A turn ends with one of these (OpenCode does not send `session.idle` to plugins).
        const turnEnded = ["session.execution.succeeded", "session.execution.failed", "session.execution.interrupted"]
        for await (const event of ctx.event.subscribe({ types: turnEnded, signal: stop.signal })) {
          const sessionID = event?.data?.sessionID
          const worktrees = sessionWorktrees.get(sessionID)
          if (!worktrees) continue
          sessionWorktrees.delete(sessionID)
          for (const wt of worktrees) cleanWorktree(wt, `the turn of session ${sessionID} ended`)
        }
      } catch (error) {
        if (!stop.signal.aborted) log(`event stream ended: ${String(error)}`)
      }
    })()

    const timer = setInterval(() => {
      try {
        sweep()
      } catch (error) {
        log(`sweep failed: ${String(error)}`)
      }
    }, SWEEP_MS)
    sweep()

    return () => {
      clearInterval(timer)
      stop.abort()
    }
  },
}
