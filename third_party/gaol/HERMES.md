# Hermes gaol patch

This directory vendors `gaol` 0.2.1 under its MIT/Apache-2.0 license because
Servo 0.4.0 uses its Linux namespace and seccomp implementation for content
processes.

Hermes extends the legacy seccomp allowlist only where required by current Rust,
glibc, SpiderMonkey, and Servo. It also leaves `RLIMIT_FSIZE` unchanged so
Servo's anonymous `memfd` IPC regions can be sized with `ftruncate`; the
original zero-byte limit killed content processes with `SIGXFSZ`. Filesystem
access remains constrained by gaol's mount namespace and chroot profile;
network access remains absent from content processes.

Remove the `[patch.crates-io]` entry and this directory after equivalent sandbox
updates land in the Servo release used by Hermes.
