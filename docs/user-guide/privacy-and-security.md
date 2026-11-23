---
title: Privacy and Security
description: Learn what Hermes blocks, what it stores, and why the current preview is not sandboxed.
---

# Privacy and security

Hermes has useful privacy foundations, but it does not yet have the process isolation expected from a production browser. Both facts matter.

## Content blocking

Hermes compiles a snapshot of Brave's default network filter lists into the browser. Requests matching known advertising and tracking rules are cancelled before their resources load.

Top-level page navigations are not blocked. Hermes does not yet have an interstitial that could explain a blocked main document, so cancelling one would produce an unexplained blank tab.

There are currently no per-site controls, exception lists, cosmetic filtering controls, or visible blocked-request counter.

## Local data

On Linux, Hermes stores bookmarks and open tab addresses in `$XDG_STATE_HOME/hermes-browser/browser.json` (by default `~/.local/state/hermes-browser/browser.json`). Servo uses the same profile directory for site data, including cookies, HSTS and its HTTP authentication cache. These files contain personal browsing data. Close Hermes before removing its profile directory to clear it.

Launch with `--temporary-profile` to skip opening or saving the normal profile. Native libraries can still use system caches. When another process holds the profile lock, new instances use an in-memory profile.

## Sandboxing status

!!! danger "Do not use Hermes for sensitive browsing"
    Web content runs in separate Servo content processes, but those child processes currently have no operating-system sandbox restrictions. Process isolation improves fault containment, but it is not the hardened security boundary expected in a mature browser.

Hermes enables Servo multiprocess mode: navigation, visible rendering, input, audio, and video cross the process boundary. The separate Linux `gaol` sandbox mode is still unreliable, so Hermes keeps `sandbox` disabled until it can enforce restrictions without breaking content processes.

Use Hermes with local test pages, known public sites, and non-sensitive data. Do not enter passwords, payment information, private documents, or account recovery details.

## Reporting a security issue

Avoid publishing exploit details in a general bug report. Contact the maintainer through the repository profile first so the issue can be handled privately and routed upstream when it belongs to Servo or a dependency.
