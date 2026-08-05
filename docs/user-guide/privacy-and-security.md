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

The current preview does not implement persistent sessions, history, bookmarks, settings, extensions, or private profiles. Do not interpret the absence of those features as a complete privacy mode: Servo and native media libraries may still use normal process and system resources during a session.

## Sandboxing status

!!! danger "Do not use Hermes for sensitive browsing"
    Web content currently runs inside the main browser process without an operating-system sandbox. A malicious page that exploits the engine would not face the process boundary expected in a mature browser.

Servo 0.4.0 can start content processes, and Hermes retains the required bootstrap and Linux `gaol` compatibility work. In embedded multiprocess mode, pages load and can update their titles, but their cross-process Paint output does not reach the visible rendering context. Hermes therefore keeps both `multiprocess` and `sandbox` disabled until that path can display content correctly.

Use Hermes with local test pages, known public sites, and non-sensitive data. Do not enter passwords, payment information, private documents, or account recovery details.

## Reporting a security issue

Avoid publishing exploit details in a general bug report. Contact the maintainer through the repository profile first so the issue can be handled privately and routed upstream when it belongs to Servo or a dependency.

