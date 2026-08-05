# Patched Servo GStreamer backend

This directory vendors `servo-media-gstreamer` 0.4.0 from the Servo project
under its MPL-2.0 license. Hermes carries focused compatibility and lifecycle
fixes:

- interpret registered codec entries ending in `*` as prefixes, matching the
  behavior already used by Servo's OHOS media backend;
- recognize the canonical `vp08.*` and `vp09.*` codec identifiers;
- log the underlying GStreamer playback error before notifying the DOM player;
- include the structured GStreamer error details in playback diagnostics;
- use the portable BGRA video-frame transfer path on Linux, because Hermes
  shares its UI OpenGL context without exposing an activatable native EGL/GLX
  media context;
- demote factories from the legacy `gstreamer-vaapi` plugin on that CPU-frame
  path, allowing GStreamer to fall back automatically when VA-API advertises a
  decoder that cannot create a usable device; this does not disable WebRender's
  GPU acceleration;
- drop an isolated unusable video sample instead of aborting the entire
  playback pipeline;
- negotiate custom Web Audio output as interleaved `f32`, infer conventional
  channel positions when caps omit them, and deinterleave each rendered
  channel without turning a malformed sample into a fatal MP4 stream error;
- disconnect the temporary setup error handler from `PlaySignalAdapter`, the
  object on which it was registered, instead of from `Play`;
- treat media actions for a context with no media instances as a no-op.

Remove the `[patch.crates-io]` entry and this directory after equivalent fixes
land in the Servo release used by Hermes.
