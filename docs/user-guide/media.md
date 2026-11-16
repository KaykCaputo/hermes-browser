---
title: Media Playback
description: Understand GStreamer playback on Linux and macOS, codec requirements, and current video limitations in Mercury Browser.
---

# Media playback

On Linux, Mercury connects Servo to the native GStreamer backend. That gives HTML audio and video, Web Audio, media capture foundations, and WebRTC a real system media pipeline.

## What should work

Direct media resources, ordinary `<audio>` or `<video>` elements, and YouTube playback can work when your installed GStreamer plugins provide the required codecs. Common MP4 and WebM files remain useful tests because they isolate the native media pipeline from a website's application code.

Mercury uses Servo's upstream GStreamer GL frame-transfer path. The desktop renderer shares the graphics resources needed to present those frames while web content remains in isolated Servo processes.

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

## macOS

On macOS, Mercury ships GStreamer's own libraries in a `lib` folder inside the application
(`Mercury Browser.app/Contents/MacOS/lib`). Nothing needs to be installed. Mercury keeps this
GStreamer to itself: it loads plugins only from its own `lib` folder, keeps its plugin registry in
its own cache folder (`~/Library/Caches/io.github.thepigdestroyer.mercury-browser/gstreamer`), and
ignores GStreamer plugins, registries and GIO modules that other software installed for your
user account (`GST_PLUGIN_PATH`, `~/.local/share/gstreamer-1.0/plugins`, `~/.cache/gstreamer-1.0`,
`GIO_EXTRA_MODULES`).

**What media playback writes to disk, and when it is removed.** Everything is under Mercury's
cache folder, `~/Library/Caches/io.github.thepigdestroyer.mercury-browser/`, and nothing is
written to `~/.cache`, `~/.local` or `~/.config`:

| Where | What | Removed |
|---|---|---|
| `gstreamer/registry-<version>-<architecture>.bin` | GStreamer's list of the bundled plugins (no media). | Kept; rebuilt when Mercury's GStreamer changes. |
| `media-buffers/<process id>-<time>/` | One private folder (readable only by you) for each running Mercury. While media plays, GStreamer keeps a copy of each stream it downloads in its `cache` subfolder, so that playback can seek without downloading again. These copies are of what you played. | When Mercury quits normally, the whole folder. If Mercury was killed or crashed, at its next start (a folder still locked by a running Mercury is left alone, so several copies of Mercury can run at once). |

If the media folder cannot be created, media still plays, without those download copies.

What has been checked on Apple Silicon (GStreamer 1.28.9):

| Format | Status |
|---|---|
| MP4 with H.264 video and AAC audio | Plays. Video is decoded in hardware (VideoToolbox). |
| WebM with VP9 video and Opus audio | Plays. VP9 is decoded in hardware (VideoToolbox) where the Mac supports it. |
| MP3, WAV | Play. |
| Ogg (Opus, Vorbis, Theora) | **Not supported for now.** |
| Web Audio | Works, including `decodeAudioData` for MP3 and WAV. |

Ogg is left out of the macOS build for now because GStreamer 1.24.13 and later (the change arrived
in a 1.24 patch release) can end Ogg playback about a second after it starts: a defect in
GStreamer's Ogg demuxer, not in the files. Pages are
told Ogg is not supported, so sites that offer another format use it instead.

Sites that ask `canPlayType('video/webm; codecs="vp9, opus"')` are told "no": Servo's list of
GStreamer plugins does not include the Opus parser that this check looks for. WebM files with Opus
audio still play when a page uses them directly.

The first start after installing or updating Mercury can take about ten seconds before media
is ready, while macOS checks the bundled libraries. Later starts are fast.

## YouTube and streaming sites

Mercury's Servo 0.7.0 integration can play YouTube video and audio on Linux. Streaming sites change frequently, however, and their compatibility can depend on Servo's web-platform support, the selected stream format, and the codecs installed on the machine.

If a streaming site fails, first test a direct MP4 or WebM file. That comparison separates a website compatibility problem from a GStreamer, decoder, or audio-output problem.

## Diagnose playback

Start Mercury with more detailed media and Servo logs:

```bash
make run RUST_LOG="talaria_servo=debug,servo=info,servo_media=debug"
```

Then look for the first GStreamer error rather than later playback fallout. The [troubleshooting guide](troubleshooting.md#media-does-not-play) has a short checklist.
