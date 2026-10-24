---
title: Media Playback
description: Understand Linux GStreamer playback, codec requirements, and current video limitations in Hermes Browser.
---

# Media playback

On Linux, Hermes connects Servo to the native GStreamer backend. That gives HTML audio and video, Web Audio, media capture foundations, and WebRTC a real system media pipeline.

## What should work

Direct media resources, ordinary `<audio>` or `<video>` elements, and YouTube playback can work when your installed GStreamer plugins provide the required codecs. Common MP4 and WebM files remain useful tests because they isolate the native media pipeline from a website's application code.

Hermes uses Servo's upstream GStreamer GL frame-transfer path. The desktop renderer shares the graphics resources needed to present those frames while web content remains in isolated Servo processes.

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

## YouTube and streaming sites

Hermes' Servo 0.7.0 integration can play YouTube video and audio on Linux. Streaming sites change frequently, however, and their compatibility can depend on Servo's web-platform support, the selected stream format, and the codecs installed on the machine.

If a streaming site fails, first test a direct MP4 or WebM file. That comparison separates a website compatibility problem from a GStreamer, decoder, or audio-output problem.

## Diagnose playback

Start Hermes with more detailed media and Servo logs:

```bash
make run RUST_LOG="talaria_servo=debug,servo=info,servo_media=debug"
```

Then look for the first GStreamer error rather than later playback fallout. The [troubleshooting guide](troubleshooting.md#media-does-not-play) has a short checklist.
