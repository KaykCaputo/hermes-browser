---
title: Media Playback
description: Understand Linux GStreamer playback, codec requirements, and current video limitations in Hermes Browser.
---

# Media playback

On Linux, Hermes connects Servo to the native GStreamer backend. That gives HTML audio and video, Web Audio, media capture foundations, and WebRTC a real system media pipeline.

## What should work

Direct media resources and ordinary `<audio>` or `<video>` elements can work when your installed GStreamer plugins provide the required codec. Common MP4 and WebM files are the most useful test cases.

The browser intentionally uses a portable CPU-frame transfer path for Linux video. WebRender remains GPU accelerated; only the handoff from GStreamer avoids assuming that the media backend can activate Hermes' native OpenGL context.

## Install the common codecs

On Debian or Ubuntu, the project setup installs the expected plugin families:

```bash
make install-linux-deps
```

To inspect a particular decoder, use `gst-inspect-1.0`. For example:

```bash
gst-inspect-1.0 avdec_h264
gst-inspect-1.0 vp9dec
```

## Why YouTube does not work yet

YouTube's current player does not select its progressive fallback in the Servo revision pinned by Hermes. Its adaptive playback path depends on Media Source Extensions, including `MediaSource` and `SourceBuffer`, which Servo 0.4.0 does not implement.

Installing more codecs cannot add a missing browser API. Use a direct MP4 or WebM file when testing the media pipeline itself.

## Diagnose playback

Start Hermes with more detailed media and Servo logs:

```bash
make run RUST_LOG="talaria_servo=debug,servo=info,servo_media=debug"
```

Then look for the first GStreamer error rather than later playback fallout. The [troubleshooting guide](troubleshooting.md#media-does-not-play) has a short checklist.

