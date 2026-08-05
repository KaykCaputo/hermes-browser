# Patched surfman 0.13.0

This directory vendors `surfman` 0.13.0 under its original MIT, Apache-2.0,
and MPL-2.0 licenses.

Hermes uses Mesa's default hardware adapter on Linux instead of implementing
the hardware preference by setting the process-global `DRI_PRIME=1` before
each GL context creation. The upstream behavior assumes that GPU index 1
exists and races with GStreamer in a multithreaded browser. Mesa and the
desktop session remain responsible for automatic device selection.

Remove the root `[patch.crates-io]` entry and this directory after an
equivalent context-local adapter selection fix lands in the Surfman release
used by Hermes.
