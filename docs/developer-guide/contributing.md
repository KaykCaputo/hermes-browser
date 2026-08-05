---
title: Contributing
description: Prepare focused, reviewable contributions to Hermes Browser.
---

# Contributing

Good Hermes contributions make one behavior easier to understand, test, or trust. Browser work can spread across UI, platform, engine, graphics, media, and networking quickly, so a narrow change is a real advantage.

## Before writing code

- Search existing issues and recent commits.
- Reproduce the behavior with the locked dependency graph.
- Decide which layer owns the policy.
- For website failures, compare the pinned Servo `servoshell` when possible.
- For a dependency patch, read [Vendored patches](vendored-patches.md) first.

## Keep boundaries intact

Put engine-independent browser behavior in `talaria-core`. Keep Slint presentation in `hermes-ui`, desktop event translation and composition in `hermes-desktop`, and Servo-specific types in `talaria-servo`.

If a change requires Servo internals to leak into the core model, pause and look for a smaller command, event, or adapter abstraction.

## Write tests near the behavior

- State and policy changes belong in core tests.
- Servo type conversions belong in adapter tests.
- Shortcut and native event behavior belongs in desktop tests.
- Dependency regressions belong beside the vendored patch.

Use manual website testing as additional evidence, not as a replacement for a deterministic test when one is possible.

## Run the checks

```bash
make qa
make docs
```

If the change affects runtime rendering or media, also run the optimized browser and record what you tested.

## Commit style

The project uses short Conventional Commit-style subjects in English:

```text
feat(servo): add native ad and tracker blocking
fix(servo): restore Linux rendering and media support
perf(desktop): enable async loading and GPU acceleration
ci: add Linux checks and runtime dependencies
docs: add user and developer guides
```

Choose the smallest useful scope, keep the subject lowercase after the colon, and explain surprising tradeoffs in the commit body or code comments.

## Open a pull request

A useful pull request describes:

- The user-visible or developer-visible problem
- Why the chosen layer owns the fix
- Automated checks that cover it
- Manual pages, platforms, GPUs, or media files tested
- Known limitations left intentionally unresolved
- Any upstream issue or patch that should eventually replace local code

Honest limits are welcome. Hermes is an engineering preview, and clear evidence is more valuable than broad compatibility claims.
